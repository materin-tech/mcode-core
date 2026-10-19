// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! 系统提示词组装（对齐 CLI agent/context.ts）+ MCODE.md 记忆加载 + goal 提示词

use crate::config::{Config, Language};
use crate::skills::Skill;

use super::subagent::AgentDefinition;

fn language_directive(lang: Language) -> &'static str {
    match lang {
        Language::ZhCn => "Always respond in Simplified Chinese.",
        Language::ZhTw => "Always respond in Traditional Chinese.",
        Language::EnUs => "Always respond in English.",
        Language::JaJp => "Always respond in Japanese.",
        Language::KoKr => "Always respond in Korean.",
    }
}

/// 组装系统提示词：身份 + 决策策略 + 规则 + 语言 + 工具清单 + skill 索引 + goal + MCODE.md + subagent 协议
pub fn build_system_prompt(opts: SystemPromptOpts) -> String {
    let lang = language_directive(opts.language);
    let mut lines: Vec<String> = vec![
        "You are MCode, an interactive coding agent running on the user's desktop.".into(),
        String::new(),
        "DECISION POLICY — read this before every reply:".into(),
        "1. Understand what the user is actually asking FIRST.".into(),
        "2. If the question does NOT require this workspace, answer directly from your own knowledge and call NO tools.".into(),
        "3. Use tools ONLY when the task genuinely needs this workspace: reading or editing real files, running commands, or searching code the user explicitly wants you to work on.".into(),
        "4. When a task does need files, read only what the question requires — never explore files \"just in case\".".into(),
        "5. NEVER end a turn by only listing your limitations. websearch and webfetch can reach the network; attempt the most promising approach, then present the result.".into(),
        "6. If you genuinely cannot proceed, do not dead-end: ask ONE concrete follow-up question or offer 2-3 concrete options so the user can pick.".into(),
        "7. LONG TASKS: work incrementally. Complete ONE small step per response, verify it with tools, give a brief progress note, then continue. Write file-by-file with write_file, run checks between steps. Dispatch independent subtasks to subagents to keep this conversation lean.".into(),
        "8. CLOSURE DISCIPLINE (non-negotiable): NEVER self-report completion. Before saying a task is done: (a) restate the acceptance criteria; (b) run an actual verification against the real artifact — build it, run the tests, curl the server; (c) show the evidence output in your reply; (d) if something cannot be verified, say exactly what and why.".into(),
        "9. If a previous step claimed completion without evidence, treat that claim as unverified and verify it now before building on it.".into(),
        String::new(),
        "Rules:".into(),
        "- Never invent file contents; verify with tools when file facts matter.".into(),
        "- Keep the workspace boundary: all file paths must stay inside the workspace.".into(),
        "- After edits, summarize what changed in your reply.".into(),
        "- When you finish a task, state the result plainly with evidence (files changed, commands run).".into(),
        format!("- {lang}"),
        String::new(),
        format!("Available tools: {}", opts.tool_names.join(", ")),
    ];
    if !opts.skills.is_empty() {
        lines.push(String::new());
        lines.push("Skill index:".into());
        for s in opts.skills {
            lines.push(format!("- {}: {}", s.entry.name, s.entry.description));
        }
        lines.push(
            "Call the skill tool with a skill name to load its full instructions when relevant."
                .into(),
        );
    }
    if let Some(goal) = opts.goal.filter(|g| !g.is_empty()) {
        lines.push(String::new());
        lines.push("ACTIVE GOAL — work directly toward this until fully achieved.".into());
        lines.push(format!("GOAL: {goal}"));
        lines.push("Decompose complex goals: write an ordered subtask list first, then execute and verify each one.".into());
    }
    if let Some(doc) = opts.project_doc.filter(|d| !d.is_empty()) {
        lines.push(String::new());
        lines.push(doc);
    }
    if !opts.agents.is_empty() {
        lines.push(String::new());
        lines.push("SUBAGENT DISPATCH PROTOCOL:".into());
        lines.push("- For complex tasks (multi-step / multi-file / cross-domain), write an ordered subtask list first, then execute and verify each one.".into());
        lines.push("- Dispatch matching subtasks to subagents via the task tool (dispatch independent subtasks in parallel).".into());
        lines.push("- Do simple tasks directly without dispatching.".into());
        lines.push(format!(
            "- Available subagents: {}",
            opts.agents
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(pua) = opts.pua_directive.filter(|p| !p.is_empty()) {
        lines.push(String::new());
        lines.push(pua);
    }
    lines.join("\n")
}

pub struct SystemPromptOpts<'a> {
    pub language: Language,
    pub tool_names: Vec<String>,
    pub skills: &'a [Skill],
    pub project_doc: Option<String>,
    pub goal: Option<String>,
    pub agents: &'a [AgentDefinition],
    pub pua_directive: Option<String>,
}

impl<'a> SystemPromptOpts<'a> {
    pub fn new(
        cfg: &Config,
        tool_names: Vec<String>,
        skills: &'a [Skill],
        agents: &'a [AgentDefinition],
    ) -> Self {
        Self {
            language: cfg.language,
            tool_names,
            skills,
            project_doc: None,
            goal: None,
            agents,
            pua_directive: None,
        }
    }
}

/// MCODE.md 记忆文档：项目向上搜索 + 全局，20k 字符合并预算（对齐 CLI mdoc.ts）
pub fn load_project_doc(cwd: &std::path::Path) -> Option<String> {
    let mut parts = vec![];
    let mut used = 0usize;
    const BUDGET: usize = 20_000;
    for p in crate::util::mcode_md_paths(cwd) {
        if used >= BUDGET {
            break;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        let take: String = text.chars().take(BUDGET - used).collect();
        used += take.len();
        parts.push(format!("--- {} ---\n{}", p.display(), take));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

/// /goal 持续指令（对齐 CLI goal.ts buildGoalPrompt）
pub fn build_goal_prompt(goal: &str) -> String {
    [
        format!("[goal] {goal}"),
        "Continue working toward the goal. Use tools to make real progress.".into(),
        "If the goal is already fully achieved, reply with exactly GOAL_COMPLETE and no tool calls.".into(),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_contains_sections() {
        let cfg = Config::default();
        let opts = SystemPromptOpts::new(&cfg, vec!["bash".into()], &[], &[]);
        let p = build_system_prompt(opts);
        assert!(p.contains("DECISION POLICY"));
        assert!(p.contains("Available tools: bash"));
        assert!(p.contains("Simplified Chinese"));
    }

    #[test]
    fn goal_prompt_shape() {
        let p = build_goal_prompt("ship it");
        assert!(p.starts_with("[goal] ship it"));
        assert!(p.contains("GOAL_COMPLETE"));
    }
}

/// goal 外层循环轮数上限（对齐 CLI cli.tsx runTurn）
pub const MAX_GOAL_ROUNDS: usize = 12;

/// goal 完成裁判：独立单发流式调用（无工具、空历史），仅显式 GOAL_COMPLETE 判词才通过。
/// fail-open：裁判出错/被打断 → 返回 false 继续干（对齐 CLI goal.ts checkGoalComplete）。
pub async fn check_goal_complete(
    backend: &dyn super::ModelBackend,
    goal: &str,
    cancel: tokio::sync::watch::Receiver<bool>,
) -> bool {
    let system = [
        "You are a strict completion judge. Given a goal and the work done so far (summarized by the conversation), decide:",
        "- If the goal is FULLY achieved: reply with exactly GOAL_COMPLETE",
        "- Otherwise: reply with exactly GOAL_CONTINUE",
        "Reply with one word only.",
    ]
    .join("\n");
    let input = format!(
        "Goal: {goal}\nJudge whether the goal is fully achieved based on the conversation history above your judge instruction is not visible to you — answer GOAL_COMPLETE only if the visible work demonstrably completes the goal."
    );
    let mut noop = |_: super::StreamEvent| {};
    match backend
        .stream_turn(
            system.as_str(),
            &[],
            &input,
            &[],
            &[],
            &crate::config::ThinkingEffort::Default,
            &mut noop,
            cancel,
        )
        .await
    {
        Ok(t) if !t.interrupted => t.reply.to_lowercase().contains("goal_complete"),
        _ => false,
    }
}

#[cfg(test)]
mod goal_tests {
    use super::*;
    use crate::agent::{ApiMessage, Attachment, StreamEvent, StreamTurn, ToolCall};

    struct JudgeBackend {
        reply: &'static str,
    }

    #[async_trait::async_trait]
    impl crate::agent::ModelBackend for JudgeBackend {
        async fn stream_turn(
            &self,
            _system: &str,
            _history: &[ApiMessage],
            _user_input: &str,
            _attachments: &[Attachment],
            _tools: &[crate::tools::ToolDefinition],
            _effort: &crate::config::ThinkingEffort,
            _on_event: &mut (dyn FnMut(StreamEvent) + Send),
            _cancel: tokio::sync::watch::Receiver<bool>,
        ) -> anyhow::Result<StreamTurn> {
            Ok(StreamTurn {
                reply: self.reply.into(),
                tool_calls: Vec::<ToolCall>::new(),
                ..Default::default()
            })
        }
    }

    #[tokio::test]
    async fn judge_accepts_only_explicit_verdict() {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let yes = JudgeBackend {
            reply: "GOAL_COMPLETE",
        };
        assert!(check_goal_complete(&yes, "ship", rx.clone()).await);
        let no = JudgeBackend {
            reply: "GOAL_CONTINUE",
        };
        assert!(!check_goal_complete(&no, "ship", rx.clone()).await);
        let junk = JudgeBackend {
            reply: "I think it is done",
        };
        assert!(!check_goal_complete(&junk, "ship", rx).await);
        let _ = tx;
    }
}
