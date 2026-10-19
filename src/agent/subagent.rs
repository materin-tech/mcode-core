// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! Subagent 体系 — 对齐 CLI agent/subagent.ts：
//! task 工具派发独立上下文的子 agent，事件带 scope 冒泡回 UI。

use super::history::HistoryStore;
use super::runner::{AgentDeps, TurnOpts, run_agent_turn};
use crate::tools::{Tool, ToolContext, ToolDefinition, ToolResult, clamp_output};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentDefinition {
    pub name: String,
    /// 主模型据此决定何时派发
    pub description: String,
    pub system_prompt: String,
    /// 工具白名单；缺省继承全部
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    #[serde(default)]
    pub max_turns: Option<usize>,
}

const READ_ONLY_TOOLS: &[&str] = &["read_file", "glob", "grep"];

/// 内置 agent 注册表（对齐 CLI BUILTIN_AGENTS）
pub fn builtin_agents() -> Vec<AgentDefinition> {
    vec![
        AgentDefinition {
            name: "general-purpose".into(),
            description: "Multi-step research, search, and task execution with full tool access.".into(),
            system_prompt: "You are a general-purpose subagent. Complete the given task autonomously using tools, then reply with a concise final report. Always answer in the language of the task.".into(),
            tools: None,
            max_turns: Some(15),
        },
        AgentDefinition {
            name: "code-explorer".into(),
            description: "Read-only codebase exploration and code search. Use for broad lookups.".into(),
            system_prompt: "You are a read-only code exploration subagent. Use read_file/glob/grep to investigate, then reply with a structured findings report.".into(),
            tools: Some(READ_ONLY_TOOLS.iter().map(|s| s.to_string()).collect()),
            max_turns: Some(12),
        },
        AgentDefinition {
            name: "planner".into(),
            description: "Implementation planning: break a task into ordered steps with risks.".into(),
            system_prompt: "You are a planning subagent. Investigate the codebase read-only, then produce an ordered implementation plan with steps, risks, and acceptance criteria.".into(),
            tools: Some(READ_ONLY_TOOLS.iter().map(|s| s.to_string()).collect()),
            max_turns: Some(10),
        },
        AgentDefinition {
            name: "code-reviewer".into(),
            description: "Code quality review of recently changed or specified files.".into(),
            system_prompt: "You are a code review subagent. Read the specified files and reply with findings ranked by severity (critical/high/medium/low), each with file:line references.".into(),
            tools: Some(READ_ONLY_TOOLS.iter().map(|s| s.to_string()).collect()),
            max_turns: Some(10),
        },
    ]
}

/// 用户级 agent（~/.mcode/agents/*.md + 物化后的内置 ecc agents，frontmatter + 正文为 system prompt）
pub fn load_user_agents() -> Vec<AgentDefinition> {
    let mut out = vec![];
    for dir in [
        crate::util::mcode_dir().join("assets/ecc-core/agents"), // 物化后的内置层
        crate::util::mcode_dir().join("agents"),                 // 用户级（覆盖同名内置）
    ] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(def) = parse_agent_md(&text) {
                out.push(def);
            }
        }
    }
    out
}

/// Claude Code 工具名 → MCode 工具名映射（内置 ECC agents 用 CC 命名，对齐 CLI 的映射口径）
fn map_tool_names(tools: Option<Vec<String>>) -> Option<Vec<String>> {
    const MAP: &[(&str, &str)] = &[
        ("read", "read_file"),
        ("write", "write_file"),
        ("edit", "edit_file"),
        ("multiedit", "edit_file"),
        ("notebook", "edit_file"),
        ("notebookedit", "edit_file"),
        ("bash", "bash"),
        ("glob", "glob"),
        ("grep", "grep"),
        ("webfetch", "webfetch"),
        ("websearch", "websearch"),
        ("task", "task"),
        ("skill", "skill"),
    ];
    tools.map(|list| {
        list.into_iter()
            .map(|t| {
                MAP.iter()
                    .find(|(cc, _)| cc.eq_ignore_ascii_case(&t))
                    .map(|(_, mc)| mc.to_string())
                    .unwrap_or(t)
            })
            .filter(|t| {
                const KNOWN: &[&str] = &[
                    "read_file",
                    "write_file",
                    "edit_file",
                    "bash",
                    "glob",
                    "grep",
                    "webfetch",
                    "websearch",
                    "skill",
                    "task",
                ];
                KNOWN.contains(&t.as_str())
            })
            .collect()
    })
}

/// 解析 agent .md：--- 包裹 frontmatter（name/description/tools 逗号分隔），正文 = system prompt（20k 截断）
fn parse_agent_md(text: &str) -> Option<AgentDefinition> {
    let rest = text.strip_prefix("---")?;
    let end = rest.find("---")?;
    let (front, body) = rest.split_at(end);
    let body = body.trim_start_matches("---").trim();
    let body: String = body.chars().take(20_000).collect();
    let mut name = None;
    let mut desc = String::new();
    let mut tools = None;
    for line in front.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let v = v.trim().trim_matches('"').to_string();
            match k.trim() {
                "name" => name = Some(v),
                "description" => desc = v,
                "tools" => {
                    let list: Vec<String> = v
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    tools = (!list.is_empty()).then_some(list);
                }
                _ => {}
            }
        }
    }
    if body.is_empty() {
        return None;
    }
    Some(AgentDefinition {
        name: name?,
        description: desc,
        system_prompt: body,
        tools: map_tool_names(tools),
        max_turns: None,
    })
}

/// 合并注册表：内置 → 用户级同名覆盖（对齐 CLI merge order）
pub fn all_agents() -> Vec<AgentDefinition> {
    let mut merged: std::collections::BTreeMap<String, AgentDefinition> = Default::default();
    for a in builtin_agents() {
        merged.insert(a.name.clone(), a);
    }
    for a in load_user_agents() {
        merged.insert(a.name.clone(), a);
    }
    merged.into_values().collect()
}

// ---------- task 工具 ----------

/// task 工具：派发 subagent（独立 HistoryStore 上下文隔离，事件带 scope 冒泡）
pub struct TaskTool {
    pub deps: Arc<AgentDeps>,
    pub agents: Arc<Vec<AgentDefinition>>,
}

#[async_trait::async_trait]
impl Tool for TaskTool {
    fn def(&self) -> ToolDefinition {
        ToolDefinition {
            name: "task".into(),
            description: "Dispatch a subagent (independent context) to handle a subtask. Returns its final report.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "subagent_type": { "type": "string", "description": "Agent name (e.g. code-explorer, planner)." },
                    "prompt": { "type": "string", "description": "Task description for the subagent." }
                },
                "required": ["subagent_type", "prompt"]
            }),
            is_read_only: false,
        }
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let agent_name = input["subagent_type"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let prompt = input["prompt"].as_str().unwrap_or_default().to_string();
        let Some(agent) = self.agents.iter().find(|a| a.name == agent_name) else {
            let known = self
                .agents
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!("未知 agent '{agent_name}'（已知: {known}）");
        };
        let scope = Some(agent.name.clone());
        // 白名单过滤（始终排除 task 自身，禁止递归）
        let emit = ctx.emit.clone();
        let sub_name = agent_name.clone();
        let report = run_agent_turn(
            &self.deps,
            &prompt,
            TurnOpts {
                instructions: Some(agent.system_prompt.clone()),
                // 对齐 CLI 实际行为：loop.ts 的 while 用常量 MAX_TOOL_TURNS，
                // agent.maxTurns 定义了但未被引用——subagent 缺省同样跑满 25 轮
                max_turns: agent.max_turns.unwrap_or(super::runner::MAX_TOOL_TURNS),
                goal: None,
                tools_filter: agent.tools.clone(),
                attachments: vec![],
                // subagent 不继承主回合的思考深度设置
                effort: crate::config::ThinkingEffort::Default,
                history: &mut HistoryStore::default(),
                emit: Arc::new(move |mut ev| {
                    // 子回合事件标注 scope 后冒泡（对齐 CLI: name = "agentname:toolname"）。
                    // Done/Error 不冒泡——主回合以 task 工具的 ToolResult 收尾，
                    // 子回合的流错误转为 ToolResult 展示。
                    let forward = match &mut ev {
                        super::AgentEvent::ToolStart { name, scope: s, .. } => {
                            *name = format!("{sub_name}:{name}");
                            *s = scope.clone();
                            true
                        }
                        super::AgentEvent::ToolResult { name, scope: s, .. } => {
                            *name = format!("{sub_name}:{name}");
                            *s = scope.clone();
                            true
                        }
                        super::AgentEvent::Reasoning { scope: s, .. } => {
                            *s = scope.clone();
                            true
                        }
                        super::AgentEvent::Done { .. } => false,
                        super::AgentEvent::Error { message } => {
                            ev = super::AgentEvent::ToolResult {
                                name: format!("{sub_name}:error"),
                                success: false,
                                output: message.clone(),
                                display: None,
                                scope: Some(sub_name.clone()),
                            };
                            true
                        }
                        super::AgentEvent::TextDelta(_) | super::AgentEvent::Retrying { .. } => {
                            false
                        }
                        _ => true,
                    };
                    if forward {
                        emit(ev);
                    }
                }),
            },
        )
        .await;
        let final_report = if report.trim().is_empty() {
            "(no report produced)".to_string()
        } else {
            report
        };
        Ok(ToolResult {
            success: true,
            output: clamp_output(format!("[{agent_name} report]\n{final_report}")),
            display: Some(crate::tools::DisplayBlock::FileRef {
                path: format!("task:{agent_name}"),
                lines: String::new(),
            }),
        })
    }
}

// ---------- 管理原语（create / install / remove，供 CLI 与桌面端共用） ----------

fn agents_base(base_dir: Option<&std::path::Path>) -> std::path::PathBuf {
    base_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| crate::util::mcode_dir().join("agents"))
}

pub fn validate_agent_name(name: &str) -> anyhow::Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    anyhow::ensure!(ok, "非法 agent 名 `{name}`（仅限字母数字-_，≤64 字符）");
    Ok(())
}

/// 模板创建：写 `<base>/<name>.md`（frontmatter + 正文 = system prompt），已存在拒绝
pub fn create_agent(
    name: &str,
    description: &str,
    system_prompt: &str,
    tools: Option<Vec<String>>,
    base_dir: Option<&std::path::Path>,
) -> anyhow::Result<std::path::PathBuf> {
    validate_agent_name(name)?;
    anyhow::ensure!(
        !description.trim().is_empty(),
        "description 不能为空（主模型据此决定何时派发）"
    );
    anyhow::ensure!(
        !system_prompt.trim().is_empty(),
        "system_prompt（正文）不能为空"
    );
    let dir = agents_base(base_dir);
    let path = dir.join(format!("{name}.md"));
    anyhow::ensure!(!path.exists(), "agent `{name}` 已存在于 {}", path.display());
    std::fs::create_dir_all(&dir)?;
    let tools_line = tools
        .map(|t| format!("tools: {}\n", t.join(", ")))
        .unwrap_or_default();
    let md = format!(
        "---\nname: {name}\ndescription: {}\n{tools_line}---\n{}\n",
        description.trim(),
        system_prompt.trim()
    );
    std::fs::write(&path, md)?;
    Ok(path)
}

/// 本地文件安装：必须能解析出合法 frontmatter，落到 `<base>/<frontmatter-name>.md`
pub fn install_agent_from_file(
    src: &std::path::Path,
    base_dir: Option<&std::path::Path>,
) -> anyhow::Result<std::path::PathBuf> {
    let text =
        std::fs::read_to_string(src).with_context(|| format!("读取失败：{}", src.display()))?;
    let def = parse_agent_md(&text)
        .ok_or_else(|| anyhow::anyhow!("frontmatter 解析失败或正文为空：{}", src.display()))?;
    validate_agent_name(&def.name)?;
    let dir = agents_base(base_dir);
    let path = dir.join(format!("{}.md", def.name));
    anyhow::ensure!(
        !path.exists(),
        "agent `{}` 已存在于 {}",
        def.name,
        path.display()
    );
    std::fs::create_dir_all(&dir)?;
    std::fs::copy(src, &path)?;
    Ok(path)
}

/// 移除：仅限用户级 `<base>/<name>.md`；内置层不受此 API 支配。返回是否真的删除。
pub fn remove_agent(name: &str, base_dir: Option<&std::path::Path>) -> anyhow::Result<bool> {
    validate_agent_name(name)?;
    let path = agents_base(base_dir).join(format!("{name}.md"));
    if !path.is_file() {
        return Ok(false);
    }
    std::fs::remove_file(&path)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_lifecycle_primitives() {
        let base = std::env::temp_dir().join(format!("mcode-agents-{}-life", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = create_agent(
            "demo-agent",
            "demo desc",
            "You are a demo.",
            Some(vec!["read_file".into(), "glob".into()]),
            Some(&base),
        )
        .unwrap();
        assert_eq!(dir, base.join("demo-agent.md"));
        let text = std::fs::read_to_string(&dir).unwrap();
        let def = parse_agent_md(&text).unwrap();
        let (n, d, t) = (def.name.clone(), def.description.clone(), def.tools.clone());
        assert_eq!(n, "demo-agent");
        assert_eq!(d, "demo desc");
        assert_eq!(t, Some(vec!["read_file".into(), "glob".into()]));
        // create 重复拒绝
        assert!(create_agent("demo-agent", "x", "y", None, Some(&base)).is_err());
        // install：合法文件按 frontmatter 名落盘
        let src = base.join("src.md");
        std::fs::write(
            &src,
            "---\nname: inst-agent\ndescription: install test\n---\nbody here",
        )
        .unwrap();
        let got = install_agent_from_file(&src, Some(&base)).unwrap();
        assert_eq!(got, base.join("inst-agent.md"));
        // install 重复 / 不可解析拒绝
        assert!(install_agent_from_file(&src, Some(&base)).is_err());
        let bad = base.join("bad.md");
        std::fs::write(&bad, "no frontmatter here").unwrap();
        assert!(install_agent_from_file(&bad, Some(&base)).is_err());
        // remove 命中 true / 未命中 false
        assert!(remove_agent("inst-agent", Some(&base)).unwrap());
        assert!(!remove_agent("ghost", Some(&base)).unwrap());
        std::fs::write(&src, "---\nname: demo-agent\ndescription: dup").unwrap();
        assert!(install_agent_from_file(&src, Some(&base)).is_err());
    }
}
