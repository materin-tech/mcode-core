// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! 内置工具 — 对齐 MCode.md 3.1：read_file / write_file / edit_file / bash / glob / grep

use super::{
    Tool, ToolContext, ToolDefinition, ToolResult, clamp_output, contain_to_workspace, unified_diff,
};
use anyhow::{Result, bail};
use serde_json::{Value, json};

const MAX_FILE_BYTES: u64 = 512 * 1024; // 大文件截断保护

fn str_arg(input: &Value, key: &str) -> Result<String> {
    Ok(input
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("缺少参数 {key}"))?
        .to_string())
}

fn file_ref(path: &str, lines: String) -> ToolResult {
    ToolResult {
        success: true,
        output: clamp_output(lines.clone()),
        display: Some(super::DisplayBlock::FileRef {
            path: path.into(),
            lines,
        }),
    }
}

fn opt_u64(input: &Value, key: &str) -> Option<u64> {
    input.get(key).and_then(|v| v.as_u64())
}

/// 内置工具全集（注册顺序对齐 CLI registry.ts）
pub fn all_builtins() -> Vec<std::sync::Arc<dyn Tool>> {
    vec![
        std::sync::Arc::new(ReadFile),
        std::sync::Arc::new(WriteFile),
        std::sync::Arc::new(EditFile),
        std::sync::Arc::new(Bash),
        std::sync::Arc::new(Glob),
        std::sync::Arc::new(Grep),
    ]
}

// ---------- read_file ----------

pub struct ReadFile;

#[async_trait::async_trait]
impl Tool for ReadFile {
    fn def(&self) -> ToolDefinition {
        ToolDefinition {
            name: "read_file".into(),
            description: "读取工作区内文件内容（大文件自动截断）".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "工作区相对路径" },
                    "offset": { "type": "integer", "description": "起始行（可选）" },
                    "limit": { "type": "integer", "description": "行数上限（可选）" }
                },
                "required": ["path"]
            }),
            is_read_only: true,
        }
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let rel = str_arg(&input, "path")?;
        if is_image_path(&rel) {
            bail!(
                "图片文件无法作为文本读取；请让用户用 @{} 方式附上该图片",
                rel
            );
        }
        let path = contain_to_workspace(&ctx.cwd, &rel)?;
        let bytes = std::fs::read(&path)?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            bail!("文件过大（>512KB），请指定 offset/limit 分段读取");
        }
        let text = String::from_utf8_lossy(&bytes);
        let start = opt_u64(&input, "offset").unwrap_or(0) as usize;
        let limit = opt_u64(&input, "limit").map(|v| v as usize);
        let numbered: String = text
            .lines()
            .skip(start)
            .take(limit.unwrap_or(2000))
            .enumerate()
            .map(|(i, l)| format!("{:>5} | {}", start + i + 1, l))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(file_ref(&rel, numbered))
    }
}

fn is_image_path(p: &str) -> bool {
    matches!(
        p.rsplit('.').next().unwrap_or_default(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
    )
}

// ---------- write_file ----------

pub struct WriteFile;

#[async_trait::async_trait]
impl Tool for WriteFile {
    fn def(&self) -> ToolDefinition {
        ToolDefinition {
            name: "write_file".into(),
            description: "写入（或创建）工作区内文件".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }),
            is_read_only: false,
        }
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let rel = str_arg(&input, "path")?;
        let content = str_arg(&input, "content")?;
        let path = contain_to_workspace(&ctx.cwd, &rel)?;
        let old = std::fs::read(&path)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, &content)?;
        let lines = unified_diff(&old, &content, &rel);
        Ok(ToolResult {
            success: true,
            output: format!("已写入 {rel}（{} 字节）", content.len()),
            display: Some(super::DisplayBlock::Diff {
                path: rel.clone(),
                lines,
            }),
        })
    }
}

// ---------- edit_file ----------

pub struct EditFile;

#[async_trait::async_trait]
impl Tool for EditFile {
    fn def(&self) -> ToolDefinition {
        ToolDefinition {
            name: "edit_file".into(),
            description: "精确替换工作区内文件内容片段（old_string 唯一匹配）".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old_string": { "type": "string" },
                    "new_string": { "type": "string" }
                },
                "required": ["path", "old_string", "new_string"]
            }),
            is_read_only: false,
        }
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let rel = str_arg(&input, "path")?;
        let old_s = str_arg(&input, "old_string")?;
        let new_s = str_arg(&input, "new_string")?;
        let path = contain_to_workspace(&ctx.cwd, &rel)?;
        let content = String::from_utf8(std::fs::read(&path)?)?;
        let count = content.matches(&old_s).count();
        if count == 0 {
            bail!("old_string 未命中");
        }
        if count > 1 {
            bail!("old_string 匹配到 {count} 处，需唯一匹配");
        }
        let updated = content.replacen(&old_s, &new_s, 1);
        std::fs::write(&path, &updated)?;
        let lines = unified_diff(&content, &updated, &rel);
        Ok(ToolResult {
            success: true,
            output: format!("已修改 {rel}"),
            display: Some(super::DisplayBlock::Diff {
                path: rel.clone(),
                lines,
            }),
        })
    }
}

// ---------- bash ----------

pub struct Bash;

#[async_trait::async_trait]
impl Tool for Bash {
    fn def(&self) -> ToolDefinition {
        ToolDefinition {
            name: "bash".into(),
            description: "在工作目录执行 shell 命令（受权限门控，可被取消）".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "timeout_ms": { "type": "integer", "description": "默认 120000" }
                },
                "required": ["command"]
            }),
            is_read_only: false,
        }
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let command = str_arg(&input, "command")?;
        let timeout =
            std::time::Duration::from_millis(opt_u64(&input, "timeout_ms").unwrap_or(120_000));
        let mut child = tokio::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" })
            .args(if cfg!(windows) {
                ["/C", &command]
            } else {
                ["-c", &command]
            })
            .current_dir(&ctx.cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let mut stdout_pipe = child.stdout.take().unwrap();
        let mut stderr_pipe = child.stderr.take().unwrap();
        // 管道并发排空（避免缓冲区满导致死锁）
        let out_task = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut v = Vec::new();
            let _ = stdout_pipe.read_to_end(&mut v).await;
            v
        });
        let err_task = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut v = Vec::new();
            let _ = stderr_pipe.read_to_end(&mut v).await;
            v
        });
        // 子进程归属独立任务：超时/取消 → kill + 回收，真实 exit code
        let mut cancel = ctx.cancel.clone();
        let wait_task = tokio::spawn(async move {
            tokio::select! {
                st = child.wait() => (st.ok().and_then(|s| s.code()), false, false),
                _ = cancel.changed() => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    (None, false, true)
                }
                _ = tokio::time::sleep(timeout) => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    (None, true, false)
                }
            }
        });
        let (code, timed_out, cancelled) = wait_task.await.unwrap_or((None, false, false));
        let stdout = String::from_utf8_lossy(&out_task.await.unwrap_or_default()).into_owned();
        let stderr = String::from_utf8_lossy(&err_task.await.unwrap_or_default()).into_owned();

        let (success, suffix) = if cancelled {
            (false, "[cancelled]".to_string())
        } else if timed_out {
            (false, format!("[timeout: {}ms]", timeout.as_millis()))
        } else {
            match code {
                Some(0) => (true, String::new()),
                Some(n) => (false, format!("[exit {n}]")),
                None => (false, "[exit: unknown]".to_string()),
            }
        };
        let mut output = format!("{stdout}{stderr}");
        if !suffix.is_empty() {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&suffix);
        }
        Ok(ToolResult {
            success,
            output: clamp_output(output),
            display: Some(super::DisplayBlock::Output {
                stdout,
                stderr,
                code: code.unwrap_or(if timed_out { 124 } else { -1 }),
            }),
        })
    }
}

// ---------- glob ----------

pub struct Glob;

#[async_trait::async_trait]
impl Tool for Glob {
    fn def(&self) -> ToolDefinition {
        ToolDefinition {
            name: "glob".into(),
            description: "按 glob 模式列出工作区文件".into(),
            parameters: json!({
                "type": "object",
                "properties": { "pattern": { "type": "string", "example": "src/**/*.ts" } },
                "required": ["pattern"]
            }),
            is_read_only: true,
        }
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let pattern = str_arg(&input, "pattern")?;
        let matcher = glob_matcher(&pattern)?;
        let mut hits: Vec<String> = pruned_walk(&ctx.cwd)
            .filter(|e| e.file_type().is_file())
            .filter_map(|e| {
                let rel = e
                    .path()
                    .strip_prefix(&ctx.cwd)
                    .ok()?
                    .to_string_lossy()
                    .replace('\\', "/");
                if matcher(&rel) { Some(rel) } else { None }
            })
            .take(2000)
            .collect();
        hits.sort();
        let joined = hits.join("\n");
        Ok(file_ref(&pattern, joined))
    }
}

/// glob 匹配（globset crate：`*` 不跨 `/`，`**` 跨目录，`?` 单字符，`{a,b}` 支持）
pub fn glob_matcher(pattern: &str) -> Result<impl Fn(&str) -> bool + Send + Sync> {
    let glob = globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()?;
    let matcher = glob.compile_matcher();
    Ok(move |s: &str| matcher.is_match(s))
}

/// 依赖目录 / 构建产物 / 系统杂项：glob、grep 遍历时统一剪枝
/// （对齐 mcode CLI 的 node_modules/.git/dist 过滤，并补齐 target 等构建目录）
fn is_junk_name(name: &str) -> bool {
    matches!(
        name,
        "node_modules"
            | ".git"
            | "target"
            | "dist"
            | "build"
            | "__pycache__"
            | ".venv"
            | "venv"
            | ".next"
            | ".cache"
            | ".idea"
            | ".vscode"
            | ".DS_Store"
    )
}

/// 带剪枝的 workspace 遍历：目录命中 `is_junk_name` 整棵跳过（depth 0 的根目录除外），
/// 不可读项直接跳过（与 TS 版 glob/grep 行为一致）
fn pruned_walk(cwd: &std::path::Path) -> impl Iterator<Item = walkdir::DirEntry> {
    walkdir::WalkDir::new(cwd)
        .into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            e.file_name().to_str().is_none_or(|n| !is_junk_name(n))
        })
        .filter_map(|e| e.ok())
}

// ---------- grep ----------

pub struct Grep;

#[async_trait::async_trait]
impl Tool for Grep {
    fn def(&self) -> ToolDefinition {
        ToolDefinition {
            name: "grep".into(),
            description: "正则搜索工作区文件内容（返回 文件:行号: 内容）".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "include": { "type": "string", "description": "文件名 glob 过滤（可选）" }
                },
                "required": ["pattern"]
            }),
            is_read_only: true,
        }
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let pattern = str_arg(&input, "pattern")?;
        let re = regex::Regex::new(&pattern)?;
        let inc_matcher = match input.get("include").and_then(|v| v.as_str()) {
            Some(p) => Some(glob_matcher(p)?),
            None => None,
        };
        let mut hits: Vec<String> = Vec::new();
        for entry in pruned_walk(&ctx.cwd).filter(|e| e.file_type().is_file()) {
            let rel = match entry.path().strip_prefix(&ctx.cwd) {
                Ok(p) => p.to_string_lossy().replace('\\', "/"),
                Err(_) => continue,
            };
            if let Some(m) = &inc_matcher
                && !m(&rel)
            {
                continue;
            }
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            if bytes.len() > MAX_FILE_BYTES as usize {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    hits.push(format!("{rel}:{}: {}", i + 1, line.trim()));
                    if hits.len() >= 500 {
                        break;
                    }
                }
            }
            if hits.len() >= 500 {
                break;
            }
        }
        let joined = hits.join("\n");
        Ok(file_ref(&pattern, joined))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{ToolRegistry, noop_sink};

    fn test_ctx() -> (ToolContext, tokio::sync::watch::Sender<bool>) {
        let (tx, rx) = tokio::sync::watch::channel(false);
        (
            ToolContext {
                cwd: std::env::temp_dir().join("mcode-builtin-test"),
                cancel: rx,
                deps: None,
                emit: noop_sink(),
            },
            tx,
        )
    }

    #[tokio::test]
    async fn bash_reports_real_exit_code() {
        let (ctx, _tx) = test_ctx();
        std::fs::create_dir_all(&ctx.cwd).unwrap();
        let r = Bash
            .execute(json!({"command": "exit 3"}), &ctx)
            .await
            .unwrap();
        assert!(!r.success);
        assert!(r.output.contains("[exit 3]"));
    }

    #[tokio::test]
    async fn bash_captures_stdout() {
        let (ctx, _tx) = test_ctx();
        std::fs::create_dir_all(&ctx.cwd).unwrap();
        let r = Bash
            .execute(json!({"command": "echo hello"}), &ctx)
            .await
            .unwrap();
        assert!(r.success);
        assert_eq!(r.output.trim(), "hello");
    }

    #[tokio::test]
    async fn glob_matches_recursive() {
        let (ctx, _tx) = test_ctx();
        let dir = ctx.cwd.join("a/b");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.ts"), "1").unwrap();
        std::fs::write(ctx.cwd.join("root.ts"), "1").unwrap();
        let m = glob_matcher("**/*.ts").unwrap();
        assert!(m("a/b/x.ts"));
        assert!(m("root.ts"));
        let m2 = glob_matcher("*.ts").unwrap();
        assert!(!m2("a/b/x.ts"), "单层 * 不应跨目录");
    }

    #[tokio::test]
    async fn glob_and_grep_skip_junk_dirs() {
        let (ctx, _tx) = test_ctx();
        std::fs::create_dir_all(ctx.cwd.join("src")).unwrap();
        std::fs::create_dir_all(ctx.cwd.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(ctx.cwd.join(".git/objects")).unwrap();
        std::fs::write(ctx.cwd.join("src/a.rs"), "fn main() {}").unwrap();
        std::fs::write(ctx.cwd.join("node_modules/pkg/i.js"), "fn decoy() {}").unwrap();
        std::fs::write(ctx.cwd.join(".git/objects/o"), "fn decoy2() {}").unwrap();

        let r = Glob
            .execute(json!({"pattern": "**/*"}), &ctx)
            .await
            .unwrap();
        assert!(r.output.contains("src/a.rs"));
        assert!(
            !r.output.contains("node_modules"),
            "glob 不应进入 node_modules"
        );
        assert!(!r.output.contains(".git"), "glob 不应进入 .git");

        let r = Grep
            .execute(json!({"pattern": "decoy"}), &ctx)
            .await
            .unwrap();
        assert!(
            !r.output.contains("decoy"),
            "grep 不应命中被剪枝目录里的内容"
        );
    }

    #[test]
    fn registry_builtins() {
        let r = ToolRegistry::with_builtins();
        assert_eq!(r.names().len(), 6);
    }
}
