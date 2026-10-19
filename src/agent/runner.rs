// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! Agent 主循环 — compact-harness 编排（对齐 CLI agent/loop.ts）：
//! 压缩门控 → 流式调用（瞬时错误重试 + 指数退避）→ 工具调用（hooks → 权限 → 校验 → 执行）
//! → 结果作为下一轮输入回喂 → 直到无工具调用或轮次耗尽。

use super::context::{SystemPromptOpts, build_system_prompt};
use super::history::HistoryStore;
use super::{
    AgentEvent, ApiMessage, Attachment, ModelBackend, StreamEvent, auto_compact_threshold,
    context_window_for_model,
};
use crate::hooks::HookEngine;
use crate::permission::{Decision, PermissionManager, Verdict};
use crate::tools::{EventSink, ToolContext, ToolRegistry};
use crate::util::TokenLedger;
use anyhow::Result;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

pub const MAX_TOOL_TURNS: usize = 25;
pub const MAX_STREAM_RETRIES: usize = 10;

/// 瞬时流式错误（对齐 CLI TRANSIENT_STREAM_ERRORS + reqwest 表述）
const TRANSIENT_STREAM_ERRORS: &[&str] = &[
    "terminated",
    "econnreset",
    "epipe",
    "econnaborted",
    "network",
    "fetch failed",
    "socket",
    "premature",
    "other side closed",
    "error sending request",
    "connection reset",
    "broken pipe",
    "channel closed",
    "timed out",
];

fn is_transient(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    TRANSIENT_STREAM_ERRORS.iter().any(|k| lower.contains(k))
}

// ---------- 确认通道（权限弹窗 UI ↔ loop） ----------

pub struct ConfirmRequest {
    pub tool: String,
    pub args_summary: String,
    pub respond: tokio::sync::oneshot::Sender<Decision>,
}

pub type ConfirmSender = tokio::sync::mpsc::Sender<ConfirmRequest>;

// ---------- harness 依赖（全部共享句柄，task 工具复用同一套） ----------

pub struct AgentDeps {
    pub cfg: crate::config::Config,
    pub backend: Arc<dyn ModelBackend>,
    pub registry: Arc<ToolRegistry>,
    pub perms: Arc<tokio::sync::Mutex<PermissionManager>>,
    pub ledger: Arc<tokio::sync::Mutex<TokenLedger>>,
    pub hooks: Arc<HookEngine>,
    pub confirm: ConfirmSender,
    /// true = 请求打断当前回合
    pub cancel_tx: tokio::sync::watch::Sender<bool>,
    pub cwd: PathBuf,
    pub skills: Vec<crate::skills::Skill>,
    pub agents: Arc<Vec<super::subagent::AgentDefinition>>,
    pub project_doc: Option<String>,
}

impl AgentDeps {
    pub fn is_cancelled(&self) -> bool {
        *self.cancel_tx.borrow()
    }

    /// UI 展示 / 系统提示词组装的上下文预算（估算）
    pub fn estimate_tokens(&self, history: &HistoryStore) -> usize {
        history.estimate_tokens() + 2000 // 系统提示词粗估
    }
}

pub struct TurnOpts<'a> {
    /// subagent 覆盖系统提示词
    pub instructions: Option<String>,
    pub max_turns: usize,
    pub goal: Option<String>,
    /// 工具白名单（subagent）
    pub tools_filter: Option<Vec<String>>,
    /// 仅首轮附带的图片附件
    pub attachments: Vec<Attachment>,
    /// 思考深度（/effort；随调用链透传给后端组请求体）
    pub effort: crate::config::ThinkingEffort,
    pub history: &'a mut HistoryStore,
    pub emit: EventSink,
}

/// Agent 回合：返回最终回复文本（空串 = 打断或错误）
pub async fn run_agent_turn(deps: &Arc<AgentDeps>, user_input: &str, opts: TurnOpts<'_>) -> String {
    let emit = opts.emit.clone();

    // -- 压缩门控（对齐 Claude Code：上下文达模型窗口 80% 自动压缩） --
    let model = deps.cfg.active_model().unwrap_or_default();
    let threshold = auto_compact_threshold(&model);
    let used = deps.estimate_tokens(opts.history);
    if used >= threshold {
        let msgs = opts.history.message_count();
        let keep_turns = (msgs / 4).max(2);
        let summary = summarize_history(opts.history);
        opts.history.compact(summary, keep_turns);
        let window = context_window_for_model(&model);
        emit(AgentEvent::Compacted {
            note: format!(
                "上下文已达 {used} / {window} tokens（≥80%），已自动压缩至 ~{keep_turns} 轮"
            ),
        });
    }

    // -- 系统提示词（subagent 覆盖 or 完整组装） --
    let system = match &opts.instructions {
        Some(instr) => instr.clone(),
        None => {
            let tool_names: Vec<String> = match &opts.tools_filter {
                Some(f) => deps
                    .registry
                    .names()
                    .into_iter()
                    .filter(|n| f.contains(n))
                    .collect(),
                None => deps.registry.names(),
            };
            let pua_directive = deps
                .cfg
                .pua_always_on
                .then(|| build_pua_directive(&deps.skills));
            build_system_prompt(SystemPromptOpts {
                language: deps.cfg.language,
                tool_names,
                skills: &deps.skills,
                project_doc: deps.project_doc.clone(),
                goal: opts.goal.clone(),
                agents: &deps.agents,
                pua_directive,
            })
        }
    };

    // 工具定义（白名单过滤，task 始终排除在 subagent 白名单外）
    let tools: Vec<crate::tools::ToolDefinition> = deps
        .registry
        .definitions()
        .into_iter()
        .filter(|d| match &opts.tools_filter {
            Some(f) => f.contains(&d.name),
            None => true,
        })
        .collect();

    let mut current_input = user_input.to_string();
    let original_input = user_input.to_string();
    let mut turn_count = 0usize;
    let cancel = deps.cancel_tx.subscribe();

    let result = loop {
        if turn_count >= opts.max_turns {
            break Err(anyhow::anyhow!(MAX_TURNS_SENTINEL));
        }

        // -- 流式调用（瞬时错误重试 + 指数退避） --
        let mut turn = None;
        for attempt in 0..MAX_STREAM_RETRIES {
            if deps.is_cancelled() {
                emit(AgentEvent::Done {
                    reply: String::new(),
                });
                return String::new();
            }
            let stream_emit = emit.clone();
            let mut on_stream = move |ev: StreamEvent| match ev {
                StreamEvent::TextDelta(t) => stream_emit(AgentEvent::TextDelta(t)),
                StreamEvent::Reasoning(t) => stream_emit(AgentEvent::Reasoning {
                    text: t,
                    scope: None,
                }),
            };
            let attachments = if turn_count == 0 {
                opts.attachments.clone()
            } else {
                vec![]
            };
            match deps
                .backend
                .stream_turn(
                    &system,
                    &opts.history.all(),
                    &current_input,
                    &attachments,
                    &tools,
                    &opts.effort,
                    &mut on_stream,
                    cancel.clone(),
                )
                .await
            {
                Ok(t) => {
                    turn = Some(t);
                    break;
                }
                Err(e) => {
                    if deps.is_cancelled() {
                        emit(AgentEvent::Done {
                            reply: String::new(),
                        });
                        return String::new();
                    }
                    let msg = format!("{e:#}");
                    if !is_transient(&msg) || attempt >= MAX_STREAM_RETRIES - 1 {
                        emit(AgentEvent::Error { message: msg });
                        // Done 必须跟随 Error：UI/pump 以 Done 为回合收尾信号，
                        // 缺失会导致 Running 占位滞留（对齐 CLI 错误路径也发 done 的语义）
                        emit(AgentEvent::Done {
                            reply: String::new(),
                        });
                        return String::new();
                    }
                    emit(AgentEvent::Retrying {
                        attempt: attempt + 1,
                        max: MAX_STREAM_RETRIES,
                        reason: msg.chars().take(80).collect(),
                    });
                    tokio::time::sleep(std::time::Duration::from_millis(
                        (1000u64.saturating_mul(1 << attempt)).min(10_000),
                    ))
                    .await;
                }
            }
        }
        let Some(t) = turn else {
            emit(AgentEvent::Error {
                message: "流式重试耗尽".into(),
            });
            emit(AgentEvent::Done {
                reply: String::new(),
            });
            return String::new();
        };

        if let Some(u) = t.usage {
            deps.ledger
                .lock()
                .await
                .record(&deps.cfg.active_model().unwrap_or_default(), u.clone());
            emit(AgentEvent::Usage(u));
        }

        if t.interrupted {
            emit(AgentEvent::Done {
                reply: String::new(),
            });
            return String::new();
        }

        // -- 无工具调用 → 回合结束 --
        if t.tool_calls.is_empty() {
            opts.history
                .push_turn(original_input.clone(), t.reply.clone());
            deps.hooks.run_stop(&t.reply).await;
            emit(AgentEvent::Done {
                reply: t.reply.clone(),
            });
            break Ok(t.reply);
        }

        // -- 权限门控 + 校验 + 执行 --
        let mut outputs: Vec<String> = Vec::new();
        for call in &t.tool_calls {
            if deps.is_cancelled() {
                break;
            }
            let parsed: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
            let parsed = if parsed.is_null() {
                Value::Object(Default::default())
            } else {
                parsed
            };
            let args_summary = serde_json::to_string(&parsed)
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect::<String>();

            let (tool_def, tool) = match deps.registry.get(&call.name) {
                Some(t) => (t.def(), t),
                None => {
                    outputs.push(format!("[{}] error: unknown tool", call.name));
                    emit(AgentEvent::ToolResult {
                        name: call.name.clone(),
                        success: false,
                        output: "error: unknown tool".into(),
                        display: None,
                        scope: None,
                    });
                    continue;
                }
            };

            // hooks PreToolUse（退出码 2 → 拒绝）
            let hook_out = deps.hooks.run_pre_tool_use(&call.name, &parsed).await;
            if let Some(reason) = hook_out.deny_reason {
                outputs.push(format!("[{}] error: {}", call.name, reason));
                emit(AgentEvent::ToolResult {
                    name: call.name.clone(),
                    success: false,
                    output: format!("error: {reason}"),
                    display: None,
                    scope: None,
                });
                continue;
            }

            // 权限门控
            let verdict =
                deps.perms
                    .lock()
                    .await
                    .evaluate(&call.name, tool_def.is_read_only, &args_summary);
            let allowed = match verdict {
                Verdict::Allow => true,
                Verdict::Deny { reason } => {
                    outputs.push(format!("[{}] error: {}", call.name, reason));
                    emit(AgentEvent::ToolResult {
                        name: call.name.clone(),
                        success: false,
                        output: format!("error: {reason}"),
                        display: None,
                        scope: None,
                    });
                    false
                }
                Verdict::Confirm => {
                    // 弹窗等待用户裁决（UI 经 oneshot 回传）
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    if deps
                        .confirm
                        .send(ConfirmRequest {
                            tool: call.name.clone(),
                            args_summary: args_summary.clone(),
                            respond: tx,
                        })
                        .await
                        .is_err()
                    {
                        false
                    } else {
                        match rx.await {
                            Ok(Decision::Deny) => {
                                outputs.push(format!("[{}] error: denied by user", call.name));
                                emit(AgentEvent::ToolResult {
                                    name: call.name.clone(),
                                    success: false,
                                    output: "error: denied by user".into(),
                                    display: None,
                                    scope: None,
                                });
                                false
                            }
                            Ok(d) => {
                                deps.perms.lock().await.record(&call.name, &d);
                                true
                            }
                            Err(_) => false,
                        }
                    }
                }
            };
            if !allowed {
                continue;
            }

            // 输入校验
            if let Err(err) = crate::tools::validate_input(&tool_def, &parsed) {
                outputs.push(format!("[{}] {err}", call.name));
                emit(AgentEvent::ToolResult {
                    name: call.name.clone(),
                    success: false,
                    output: err,
                    display: None,
                    scope: None,
                });
                continue;
            }

            // 执行
            emit(AgentEvent::ToolStart {
                name: call.name.clone(),
                args: parsed.clone(),
                scope: None,
            });
            let ctx = ToolContext {
                cwd: deps.cwd.clone(),
                cancel: cancel.clone(),
                deps: Some(deps.clone()),
                emit: emit.clone(),
            };
            let result: Result<crate::tools::ToolResult> = tool.execute(parsed, &ctx).await;
            let result = match result {
                Ok(r) => r,
                Err(e) => crate::tools::ToolResult {
                    success: false,
                    output: format!("error: {e:#}"),
                    display: None,
                },
            };
            deps.hooks
                .run_post_tool_use(&call.name, result.success, &result.output)
                .await;
            outputs.push(format!("[{}] {}", call.name, result.output));
            emit(AgentEvent::ToolResult {
                name: call.name.clone(),
                success: result.success,
                output: result.output.clone(),
                display: result.display.clone(),
                scope: None,
            });
        }

        // -- 工具结果回喂（对齐 CLI：作为下一轮 user 输入） --
        let mut feed = vec!["Tool call results:".to_string()];
        feed.extend(outputs);
        feed.push(String::new());
        feed.push("Continue based on these results. When the task is complete, reply with the final answer and no tool calls.".to_string());
        current_input = feed.join("\n");
        turn_count += 1;
    };

    match result {
        Ok(reply) => reply,
        Err(e) if format!("{e:#}") == MAX_TURNS_SENTINEL => {
            let stopped = "(stopped: max tool turns reached)";
            opts.history.push_turn(original_input, stopped.to_string());
            emit(AgentEvent::Done {
                reply: stopped.to_string(),
            });
            stopped.to_string()
        }
        Err(_) => String::new(),
    }
}

const MAX_TURNS_SENTINEL: &str = "__max_tool_turns__";

fn summarize_history(h: &HistoryStore) -> String {
    let msgs: Vec<ApiMessage> = h.all();
    if msgs.is_empty() {
        return "(empty history)".into();
    }
    let first: String = msgs[0].text.chars().take(200).collect();
    format!(
        "Earlier conversation had {} messages, starting with: \"{first}…\"",
        msgs.len()
    )
}

/// PUA 收敛纪律指令：pua skill 正文（≤8k）优先，否则内置短指令
fn build_pua_directive(skills: &[crate::skills::Skill]) -> String {
    let header = "PUA CLOSURE DISCIPLINE — always-on:\n- Never self-report completion without verification evidence (commands run + output shown).\n- If a method failed twice, stop and switch to a fundamentally different approach.\n- Facts first: label guesses as hypotheses with a verification plan.";
    let body = skills
        .iter()
        .find(|s| s.entry.name == "pua")
        .map(|s| s.body.chars().take(8_000).collect::<String>())
        .unwrap_or_default();
    if body.is_empty() {
        header.to_string()
    } else {
        format!("{header}\n\n--- pua skill ---\n{body}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentEvent;
    use crate::agent::{StreamTurn, ToolCall};
    use crate::permission::Mode;
    use crate::tools::{ToolResult, noop_sink};
    use crate::util::TokenUsage;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ---- mock 后端：第一轮流返回工具调用，第二轮流返回纯文本 ----
    struct MockBackend {
        calls: AtomicUsize,
    }

    fn mock_tool_call() -> ToolCall {
        ToolCall {
            call_id: "c1".into(),
            name: "mock_tool".into(),
            arguments: r#"{"command":"echo hi"}"#.into(),
        }
    }

    #[async_trait::async_trait]
    impl ModelBackend for MockBackend {
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
        ) -> Result<StreamTurn> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(if n == 0 {
                StreamTurn {
                    reply: String::new(),
                    tool_calls: vec![mock_tool_call()],
                    usage: Some(TokenUsage {
                        input: 10,
                        output: 5,
                        ..Default::default()
                    }),
                    interrupted: false,
                }
            } else {
                StreamTurn {
                    reply: "done!".into(),
                    tool_calls: vec![],
                    usage: Some(TokenUsage {
                        input: 20,
                        output: 8,
                        ..Default::default()
                    }),
                    interrupted: false,
                }
            })
        }
    }

    // ---- mock 工具：记录执行 ----
    struct EchoTool;

    #[async_trait::async_trait]
    impl crate::tools::Tool for EchoTool {
        fn def(&self) -> crate::tools::ToolDefinition {
            crate::tools::ToolDefinition {
                name: "mock_tool".into(),
                description: "mock".into(),
                parameters: serde_json::json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}),
                is_read_only: false,
            }
        }
        async fn execute(&self, _input: Value, _ctx: &ToolContext) -> Result<ToolResult> {
            Ok(ToolResult {
                success: true,
                output: "hi".into(),
                display: None,
            })
        }
    }

    fn test_deps() -> (Arc<AgentDeps>, tokio::sync::mpsc::Receiver<ConfirmRequest>) {
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        std::mem::forget(cancel_rx); // 保持至少一个接收者，避免 send 失败
        let (confirm_tx, confirm_rx) = tokio::sync::mpsc::channel(8);
        let registry = Arc::new(ToolRegistry::with_builtins());
        registry.register(Arc::new(EchoTool));
        let deps = Arc::new(AgentDeps {
            cfg: crate::config::Config::default(),
            backend: Arc::new(MockBackend {
                calls: AtomicUsize::new(0),
            }),
            registry,
            perms: Arc::new(tokio::sync::Mutex::new(PermissionManager::new(
                Mode::Passon,
                vec![],
            ))),
            ledger: Arc::new(tokio::sync::Mutex::new(TokenLedger::default())),
            hooks: Arc::new(HookEngine::empty()),
            confirm: confirm_tx,
            cancel_tx,
            cwd: std::env::temp_dir(),
            skills: vec![],
            agents: Arc::new(vec![]),
            project_doc: None,
        });
        (deps, confirm_rx)
    }

    #[tokio::test]
    async fn loop_feeds_tool_results_and_finishes() {
        let (deps, mut confirm_rx) = test_deps();
        let events: Arc<std::sync::Mutex<Vec<AgentEvent>>> = Default::default();
        let ev_sink: EventSink = {
            let ev = events.clone();
            Arc::new(move |e| ev.lock().unwrap().push(e))
        };
        // passon 模式：权限直接放行，不弹确认
        let reply = run_agent_turn(
            &deps,
            "run echo",
            TurnOpts {
                instructions: None,
                max_turns: MAX_TOOL_TURNS,
                goal: None,
                tools_filter: None,
                attachments: vec![],
                effort: crate::config::ThinkingEffort::Default,
                history: &mut HistoryStore::default(),
                emit: ev_sink,
            },
        )
        .await;
        assert_eq!(reply, "done!");
        let ev = events.lock().unwrap();
        assert!(
            ev.iter()
                .any(|e| matches!(e, AgentEvent::ToolStart { name, .. } if name == "mock_tool"))
        );
        assert!(
            ev.iter()
                .any(|e| matches!(e, AgentEvent::ToolResult { success: true, .. }))
        );
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Usage(_))));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Done { .. })));
        // 无确认请求（passon 放行）
        assert!(confirm_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn manual_mode_asks_confirmation_via_channel() {
        let (deps, mut confirm_rx) = test_deps();
        deps.perms.lock().await.set_mode(Mode::Manual);
        let ev: EventSink = Arc::new(|_| {});
        // spawn：确认通道回 Allow
        let deps2 = deps.clone();
        let responder = tokio::spawn(async move {
            if let Some(req) = confirm_rx.recv().await {
                req.respond.send(Decision::Allow).unwrap();
            }
        });
        let reply = run_agent_turn(
            &deps2,
            "run echo",
            TurnOpts {
                instructions: None,
                max_turns: MAX_TOOL_TURNS,
                goal: None,
                tools_filter: None,
                attachments: vec![],
                effort: crate::config::ThinkingEffort::Default,
                history: &mut HistoryStore::default(),
                emit: ev,
            },
        )
        .await;
        assert_eq!(reply, "done!");
        responder.await.unwrap();
    }

    #[tokio::test]
    async fn deny_stops_tool_execution() {
        let (deps, mut confirm_rx) = test_deps();
        deps.perms.lock().await.set_mode(Mode::Manual);
        let events: Arc<std::sync::Mutex<Vec<AgentEvent>>> = Default::default();
        let ev_sink: EventSink = {
            let ev = events.clone();
            Arc::new(move |e| ev.lock().unwrap().push(e))
        };
        let deps2 = deps.clone();
        let responder = tokio::spawn(async move {
            if let Some(req) = confirm_rx.recv().await {
                req.respond.send(Decision::Deny).unwrap();
            }
        });
        let reply = run_agent_turn(
            &deps2,
            "run echo",
            TurnOpts {
                instructions: None,
                max_turns: MAX_TOOL_TURNS,
                goal: None,
                tools_filter: None,
                attachments: vec![],
                effort: crate::config::ThinkingEffort::Default,
                history: &mut HistoryStore::default(),
                emit: ev_sink,
            },
        )
        .await;
        assert_eq!(reply, "done!"); // 拒绝后模型继续产出最终回复
        responder.await.unwrap();
        let ev = events.lock().unwrap();
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolResult { success: false, output, .. } if output.contains("denied by user"))));
    }

    #[tokio::test]
    async fn cancel_aborts_turn() {
        let (deps, _confirm_rx) = test_deps();
        deps.cancel_tx.send(true).unwrap();
        let reply = run_agent_turn(
            &deps,
            "x",
            TurnOpts {
                instructions: None,
                max_turns: MAX_TOOL_TURNS,
                goal: None,
                tools_filter: None,
                attachments: vec![],
                effort: crate::config::ThinkingEffort::Default,
                history: &mut HistoryStore::default(),
                emit: noop_sink(),
            },
        )
        .await;
        assert_eq!(reply, "");
    }
}
