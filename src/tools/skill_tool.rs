// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! skill 工具 — 渐进式披露第二层：按名加载 SKILL.md 全文（对齐 CLI skills/tool.ts）

use super::{Tool, ToolContext, ToolDefinition, ToolResult};
use anyhow::Result;
use serde_json::{Value, json};

pub struct SkillTool;

#[async_trait::async_trait]
impl Tool for SkillTool {
    fn def(&self) -> ToolDefinition {
        ToolDefinition {
            name: "skill".into(),
            description: "按名称加载 skill 的完整指令内容（索引常驻系统提示词，正文按需展开）"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            }),
            is_read_only: true,
        }
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let name = input
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("缺少参数 name"))?
            .to_string();
        let skills = crate::skills::load_all(&ctx.cwd);
        let Some(skill) = skills.iter().find(|s| s.entry.name == name) else {
            let known: Vec<&str> = skills.iter().map(|s| s.entry.name.as_str()).collect();
            anyhow::bail!("未知 skill '{name}'（已知: {}）", known.join(", "));
        };
        let body = format!(
            "[skill: {} | 来源: {}]\n{}",
            skill.entry.name, skill.entry.source, skill.body
        );
        Ok(ToolResult {
            success: true,
            output: super::clamp_output(body),
            display: Some(super::DisplayBlock::FileRef {
                path: format!("skill:{name}"),
                lines: String::new(),
            }),
        })
    }
}
