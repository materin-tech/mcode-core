// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! MCP 客户端 — stdio transport（newline-delimited JSON-RPC 2.0）。
//! 对齐原型 extensions.html 的设计口径：MCP 工具自动转成 ToolDefinition 挂进 registry，
//! 与内置工具同一权限门控（is_read_only=false → 经 PermissionManager 裁决）。

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

const REQ_TIMEOUT: Duration = Duration::from_secs(30);
const PROTOCOL_VERSION: &str = "2024-11-05";

/// 单个 MCP server 连接
pub struct McpServer {
    pub name: String,
    child: Child,
    stdin: tokio::process::ChildStdin,
    reader: BufReader<tokio::process::ChildStdout>,
    next_id: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl McpServer {
    /// 连接：spawn 进程 → initialize 握手 → initialized 通知 → tools/list
    pub async fn connect(cfg: &crate::config::McpServerConfig) -> Result<Self> {
        let mut child = Command::new(&cfg.command)
            .args(&cfg.args)
            .envs(&cfg.env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("启动 MCP server '{}' 失败: {}", cfg.name, cfg.command))?;
        let stdin = child.stdin.take().context("MCP stdin 缺失")?;
        let stdout = child.stdout.take().context("MCP stdout 缺失")?;
        let mut srv = Self {
            name: cfg.name.clone(),
            child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: 1,
        };

        // initialize 握手
        let init = srv
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": { "name": "mcode-desktop", "version": "0.1.0" }
                }),
            )
            .await?;
        if init["result"].is_null() {
            bail!("MCP '{}' initialize 失败: {}", cfg.name, init);
        }
        srv.notify("notifications/initialized", json!({})).await?;
        Ok(srv)
    }

    async fn send_raw(&mut self, v: &Value) -> Result<()> {
        self.stdin.write_all(v.to_string().as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        Ok(())
    }

    /// 发请求并等待同 id 响应（顺序处理，忽略 notification 行）
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.send_raw(&msg).await?;
        let deadline = tokio::time::Instant::now() + REQ_TIMEOUT;
        loop {
            let mut line = String::new();
            let n = tokio::time::timeout_at(deadline, self.reader.read_line(&mut line)).await;
            match n {
                Ok(Ok(0)) => bail!("MCP '{}' 连接已关闭", self.name),
                Ok(Ok(_)) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let Ok(v) = serde_json::from_str::<Value>(trimmed) else {
                        continue;
                    };
                    // 只匹配响应（有 id 且属于本次请求）
                    if v["id"] == json!(id) {
                        if let Some(err) = v.get("error") {
                            bail!("MCP '{}.{}' 错误: {}", self.name, method, err);
                        }
                        return Ok(v);
                    }
                }
                Ok(Err(e)) => bail!("MCP '{}' 读取失败: {e}", self.name),
                Err(_) => bail!(
                    "MCP '{}.{}' 超时（{}s）",
                    self.name,
                    method,
                    REQ_TIMEOUT.as_secs()
                ),
            }
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send_raw(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await
    }

    /// 工具发现（tools/list）
    pub async fn list_tools(&mut self) -> Result<Vec<McpToolInfo>> {
        let resp = self.request("tools/list", json!({})).await?;
        let mut out = vec![];
        if let Some(tools) = resp["result"]["tools"].as_array() {
            for t in tools {
                out.push(McpToolInfo {
                    name: t["name"].as_str().unwrap_or_default().into(),
                    description: t["description"].as_str().unwrap_or_default().into(),
                    input_schema: t
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or(json!({"type":"object"})),
                });
            }
        }
        Ok(out)
    }

    /// 工具调用（tools/call）— content 文本块拼接为输出
    pub async fn call_tool(&mut self, tool: &str, args: Value) -> Result<String> {
        let resp = self
            .request("tools/call", json!({"name": tool, "arguments": args}))
            .await?;
        if resp["result"]["isError"] == json!(true) {
            bail!("MCP 工具 {tool} 返回错误: {}", resp["result"]);
        }
        let mut out = vec![];
        if let Some(content) = resp["result"]["content"].as_array() {
            for c in content {
                match c["type"].as_str() {
                    Some("text") => out.push(c["text"].as_str().unwrap_or_default().to_string()),
                    Some(other) => out.push(format!("[{other} content]")),
                    None => {}
                }
            }
        }
        Ok(out.join("\n"))
    }

    pub async fn shutdown(&mut self) {
        let _ = self.child.kill().await;
    }
}

/// MCP 工具 → Tool 适配器（挂进 ToolRegistry，统一权限门控）
pub struct McpToolAdapter {
    server: Arc<Mutex<McpServer>>,
    /// registry 内名称：mcp__<server>__<tool>
    pub full_name: String,
    server_name: String,
    info: McpToolInfo,
}

impl McpToolAdapter {
    pub fn new(server: Arc<Mutex<McpServer>>, server_name: &str, info: McpToolInfo) -> Self {
        Self {
            server,
            full_name: format!("mcp__{server_name}__{}", info.name),
            server_name: server_name.to_string(),
            info,
        }
    }
}

#[async_trait::async_trait]
impl crate::tools::Tool for McpToolAdapter {
    fn def(&self) -> crate::tools::ToolDefinition {
        crate::tools::ToolDefinition {
            name: self.full_name.clone(),
            description: format!("[MCP:{}] {}", self.server_name, self.info.description),
            parameters: self.info.input_schema.clone(),
            is_read_only: false,
        }
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &crate::tools::ToolContext,
    ) -> Result<crate::tools::ToolResult> {
        let mut srv = self.server.lock().await;
        let output = srv.call_tool(&self.info.name, input).await?;
        Ok(crate::tools::ToolResult {
            success: true,
            output: crate::tools::clamp_output(output),
            display: Some(crate::tools::DisplayBlock::FileRef {
                path: self.full_name.clone(),
                lines: String::new(),
            }),
        })
    }
}

/// 连接所有启用的 server 并把工具注册进 registry；返回 (成功, 失败原因) 清单
pub async fn attach_all(
    cfgs: &[crate::config::McpServerConfig],
    registry: &crate::tools::ToolRegistry,
) -> Vec<(String, Result<usize>)> {
    let mut report = vec![];
    for cfg in cfgs.iter().filter(|c| c.enabled) {
        let res: Result<usize> = async {
            let mut srv = McpServer::connect(cfg).await?;
            let tools = srv.list_tools().await?;
            let n = tools.len();
            let shared = Arc::new(Mutex::new(srv));
            for t in tools {
                registry.register(Arc::new(McpToolAdapter::new(shared.clone(), &cfg.name, t)));
            }
            Ok(n)
        }
        .await;
        report.push((cfg.name.clone(), res));
    }
    report
}
