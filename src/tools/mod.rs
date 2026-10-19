// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! 工具体系 — 对齐 MCode.md 3.1：工作区路径收敛（防逃逸）+ diff 产出 + 注册表。

pub mod builtin;
pub mod skill_tool;
pub mod types;
pub mod web;

pub use types::*;

use anyhow::Result;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};

/// 统一 diff 行级数据（edit_file / write_file 产出，UI 直接渲染）
pub fn unified_diff(old: &str, new: &str, path: &str) -> Vec<DiffLine> {
    use similar::ChangeTag;
    let diff = similar::TextDiff::from_lines(old, new);
    let mut out = vec![];
    out.push(DiffLine {
        kind: DiffKind::Hunk,
        text: format!("@@ {path} @@"),
    });
    for change in diff.iter_all_changes() {
        let line = match change.tag() {
            ChangeTag::Equal => DiffLine {
                kind: DiffKind::Context,
                text: change.to_string(),
            },
            ChangeTag::Insert => DiffLine {
                kind: DiffKind::Add,
                text: change.to_string(),
            },
            ChangeTag::Delete => DiffLine {
                kind: DiffKind::Delete,
                text: change.to_string(),
            },
        };
        out.push(line);
    }
    out
}

pub fn diff_stats(lines: &[DiffLine]) -> (usize, usize) {
    let add = lines.iter().filter(|l| l.kind == DiffKind::Add).count();
    let del = lines.iter().filter(|l| l.kind == DiffKind::Delete).count();
    (add, del)
}

/// 工作区路径收敛（防路径逃逸，对齐 CLI guard.ts：支持 `~` 展开 + 逃逸拒绝）
pub fn contain_to_workspace(cwd: &Path, rel: &str) -> Result<PathBuf> {
    let rel = rel.trim();
    let root = cwd.canonicalize()?;
    let joined: PathBuf = if rel == "~" {
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("无法获取 HOME"))?
    } else if let Some(stripped) = rel.strip_prefix("~/") {
        dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("无法获取 HOME"))?
            .join(stripped)
    } else {
        root.join(rel) // 相对路径基于 canonical root（规避 /var → /private/var 符号链接）
    };
    if let Ok(c) = joined.canonicalize() {
        if c.starts_with(&root) {
            return Ok(c);
        }
        anyhow::bail!("路径逃逸被拒绝: {rel}");
    }
    // 目标不存在（write 新文件）：逐段校验，拒绝 `..` 与绝对路径组件
    let suffix = joined.strip_prefix(&root)?;
    let mut norm = root.clone();
    for comp in suffix.components() {
        match comp {
            Component::Normal(seg) => norm.push(seg),
            Component::CurDir => {}
            _ => anyhow::bail!("路径逃逸被拒绝: {rel}"),
        }
    }
    Ok(norm)
}

/// 工具产出钳制（对齐 CLI MAX_TOOL_OUTPUT_CHARS = 30_000）
pub const MAX_TOOL_OUTPUT_CHARS: usize = 30_000;

pub fn clamp_output(s: String) -> String {
    if s.chars().count() <= MAX_TOOL_OUTPUT_CHARS {
        return s;
    }
    let truncated: String = s.chars().take(MAX_TOOL_OUTPUT_CHARS).collect();
    format!("{truncated}\n…[输出已截断，共 {} 字符]", s.chars().count())
}

/// 事件汇：loop 把 AgentEvent 推给 UI（task 工具冒泡子 agent 事件时复用）
pub type EventSink = Arc<dyn Fn(crate::agent::AgentEvent) + Send + Sync>;

/// 空事件汇（工具单测用）
pub fn noop_sink() -> EventSink {
    Arc::new(|_| {})
}

/// 工具执行上下文
#[derive(Clone)]
pub struct ToolContext {
    pub cwd: PathBuf,
    pub cancel: tokio::sync::watch::Receiver<bool>,
    /// harness 依赖（task 工具派发 subagent 用；普通工具忽略）
    pub deps: Option<Arc<crate::agent::runner::AgentDeps>>,
    /// 事件汇（task 工具冒泡子 agent 工具事件）
    pub emit: EventSink,
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn def(&self) -> ToolDefinition;
    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolResult>;
}

/// 工具注册表 — 收纳 + 按名检索（对齐 tools/registry.ts；支持运行时注册 MCP/task 工具）
pub struct ToolRegistry {
    tools: RwLock<Vec<Arc<dyn Tool>>>,
}

impl ToolRegistry {
    pub fn with_builtins() -> Self {
        let r = Self {
            tools: RwLock::new(vec![]),
        };
        for t in builtin::all_builtins() {
            r.register(t);
        }
        r
    }

    pub fn register(&self, t: Arc<dyn Tool>) {
        self.tools.write().unwrap().push(t);
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.read().unwrap().iter().map(|t| t.def()).collect()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools
            .read()
            .unwrap()
            .iter()
            .find(|t| t.def().name == name)
            .cloned()
    }

    pub fn names(&self) -> Vec<String> {
        self.tools
            .read()
            .unwrap()
            .iter()
            .map(|t| t.def().name.to_string())
            .collect()
    }
}

/// 轻量 JSON-schema 校验（对齐 CLI tools/validate.ts：required + 类型 + 未知参数）
pub fn validate_input(tool: &ToolDefinition, input: &Value) -> std::result::Result<(), String> {
    let Some(obj) = input.as_object() else {
        return Err(format!("{}: 输入不是对象", tool.name));
    };
    if let Some(required) = tool.parameters["required"].as_array() {
        for key in required {
            let key = key.as_str().unwrap_or_default();
            if !obj.contains_key(key) {
                return Err(format!("{}: 缺少必填参数 '{key}'", tool.name));
            }
        }
    }
    let props = &tool.parameters["properties"];
    for (key, value) in obj {
        let Some(prop) = props.get(key) else {
            return Err(format!("{}: 未知参数 '{key}'", tool.name));
        };
        let expected = prop["type"].as_str().unwrap_or_default();
        let actual = if value.is_array() {
            "array"
        } else if value.is_object() {
            "object"
        } else if value.is_string() {
            "string"
        } else if value.is_boolean() {
            "boolean"
        } else if value.is_number() {
            "number"
        } else {
            "null"
        };
        if !expected.is_empty() && expected != actual {
            return Err(format!(
                "{}: 参数 '{key}' 应为 {expected}，实为 {actual}",
                tool.name
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_escape() {
        let cwd = std::env::temp_dir().join("mcore-test-cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        assert!(contain_to_workspace(&cwd, "../../etc/passwd").is_err());
        assert!(contain_to_workspace(&cwd, "src/a.rs").is_ok());
    }

    #[test]
    fn diff_marks_add_delete() {
        let lines = unified_diff("a\nb\n", "a\nc\n", "f.rs");
        let (a, d) = diff_stats(&lines);
        assert_eq!((a, d), (1, 1));
    }

    #[test]
    fn registry_has_six_builtins() {
        let r = ToolRegistry::with_builtins();
        assert_eq!(
            r.names(),
            vec![
                "read_file",
                "write_file",
                "edit_file",
                "bash",
                "glob",
                "grep"
            ]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>()
        );
    }

    #[test]
    fn validate_required_and_unknown() {
        let def = ToolDefinition {
            name: "t".to_string(),
            description: String::new(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
            is_read_only: true,
        };
        assert!(validate_input(&def, &serde_json::json!({"path": "a"})).is_ok());
        assert_eq!(
            validate_input(&def, &serde_json::json!({})).unwrap_err(),
            "t: 缺少必填参数 'path'"
        );
        assert!(validate_input(&def, &serde_json::json!({"path": 1})).is_err());
        assert!(validate_input(&def, &serde_json::json!({"path": "a", "x": 1})).is_err());
    }

    #[test]
    fn clamp_truncates_long_output() {
        let long = "x".repeat(MAX_TOOL_OUTPUT_CHARS + 10);
        let clamped = clamp_output(long);
        assert!(clamped.contains("已截断"));
        assert_eq!(clamp_output("short".into()), "short");
    }
}
