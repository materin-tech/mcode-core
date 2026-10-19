// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! Agent harness — compact-harness 模式（对齐 MCode.md 1.2/4.3 与 CLI agent/loop.ts）：
//! context（系统提示词组装）/ history（消息管理）/ compaction（阈值压缩）/ 主循环编排。
//!
//! 分层：`ModelBackend`（api 层实现，单轮流式）→ `agent::loop`（编排：重试/权限/工具/回喂）。

pub mod completion;
pub mod context;
pub mod history;
pub mod runner;
pub mod subagent;

use anyhow::Result;
use serde::{Deserialize, Serialize};

// ---------- 消息模型（UI 渲染 + 会话持久化用，富结构） ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        media_type: String,
        data: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        call_id: String,
        output: String,
        success: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
    pub ts: chrono::DateTime<chrono::Utc>,
}

impl Message {
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            content: vec![ContentBlock::Text { text: text.into() }],
            ts: chrono::Utc::now(),
        }
    }

    /// 多模态用户消息：图片附件仅当模型带 multimodal 标记时进入上下文，
    /// 否则拒绝并降级为文本提示（对齐 CLI 近期修复的语义）
    pub fn user_with_images(
        text: impl Into<String>,
        images: Vec<(String, String)>, // (name, data_url)
        model_multimodal: bool,
    ) -> Self {
        let mut content = vec![ContentBlock::Text { text: text.into() }];
        for (name, data) in images {
            if model_multimodal {
                content.push(ContentBlock::Image {
                    media_type: "image/png".into(),
                    data,
                });
            } else {
                content.push(ContentBlock::Text {
                    text: format!("[图片已忽略：{name}，当前模型不支持多模态]"),
                });
            }
        }
        Self {
            role: Role::User,
            content,
            ts: chrono::Utc::now(),
        }
    }
}

/// 提炼纯文本（渲染 / 持久化摘要用）
pub fn text_of(m: &Message) -> String {
    m.content
        .iter()
        .filter_map(|c| match c {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------- 模型后端抽象（单轮流式，对齐 CLI api/client.ts streamResponse） ----------

/// 传给后端的纯文本消息（history 只存 user/assistant 文本对，对齐 CLI ChatMessage）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiMessage {
    pub role: crate::agent::Role,
    pub text: String,
}

/// 图片附件（multimodal 模型的 input_image part）
#[derive(Debug, Clone)]
pub struct Attachment {
    pub path: String,
    pub data_url: String,
}

/// 模型产出的工具调用（response.output_item.done → function_call）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

/// 单轮流式回合的结果：回复文本 + 工具调用集合 + 用量
#[derive(Debug, Clone, Default)]
pub struct StreamTurn {
    pub reply: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<crate::util::TokenUsage>,
    pub interrupted: bool,
}

/// 后端流事件（loop 负责聚合成 AgentEvent）
#[derive(Debug, Clone)]
pub enum StreamEvent {
    Reasoning(String),
    TextDelta(String),
}

/// 模型回调抽象：api 层实现它，loop 只做编排（可测试）
#[async_trait::async_trait]
#[allow(clippy::too_many_arguments)] // 签名锁定 CLI 对齐口径（system/history/input/attachments/effort…），不强行收拢成参数对象
pub trait ModelBackend: Send + Sync {
    /// 单轮流式调用：产出文本/思考增量，返回完整回复与工具调用。
    /// 无请求级超时（对齐 CLI：只有用户打断能终止长生成）。
    async fn stream_turn(
        &self,
        system: &str,
        history: &[ApiMessage],
        user_input: &str,
        attachments: &[Attachment],
        tools: &[crate::tools::ToolDefinition],
        effort: &crate::config::ThinkingEffort,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<StreamTurn>;
}

// ---------- 上下文预算（自动 compact：达模型窗口 80% 触发，对齐 Claude Code） ----------

/// 自动压缩触发比例：上下文超过模型窗口的 80% 即压缩
pub const AUTO_COMPACT_RATIO: f64 = 0.8;

/// UI 上下文预算展示上限（固定兜底值；模型窗口可经 context_window_for_model 细化）
pub const CONTEXT_LIMIT_TOKENS: usize = 160_000;

/// 按模型名推断上下文窗口（估算表，token；未知模型回退 128k）
pub fn context_window_for_model(model: &str) -> usize {
    let m = model.to_ascii_lowercase();
    if m.contains("claude") {
        200_000
    } else if m.contains("gemini") {
        1_000_000
    } else if m.contains("gpt-5") || m.contains("o1") || m.contains("o3") {
        200_000
    } else if m.contains("kimi") || m.contains("moonshot") {
        256_000
    } else {
        // gpt / qwen / deepseek / glm 及未知模型
        128_000
    }
}

/// 自动压缩阈值：模型上下文窗口 × 80%
pub fn auto_compact_threshold(model: &str) -> usize {
    (context_window_for_model(model) as f64 * AUTO_COMPACT_RATIO) as usize
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Reasoning {
        text: String,
        scope: Option<String>,
    },
    TextDelta(String),
    Retrying {
        attempt: usize,
        max: usize,
        reason: String,
    },
    ToolStart {
        name: String,
        args: serde_json::Value,
        scope: Option<String>,
    },
    ToolResult {
        name: String,
        success: bool,
        output: String,
        display: Option<crate::tools::DisplayBlock>,
        scope: Option<String>,
    },
    Usage(crate::util::TokenUsage),
    Compacted {
        note: String,
    },
    Done {
        reply: String,
    },
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multimodal_gate_drops_images_without_flag() {
        let with =
            Message::user_with_images("看图", vec![("a.png".into(), "data:...".into())], true);
        assert!(matches!(with.content[1], ContentBlock::Image { .. }));

        let without =
            Message::user_with_images("看图", vec![("a.png".into(), "data:...".into())], false);
        assert!(text_of(&without).contains("图片已忽略"));
    }

    #[test]
    fn context_window_matches_model_family() {
        assert_eq!(context_window_for_model("claude-sonnet-5-5"), 200_000);
        assert_eq!(context_window_for_model("gemini-2.5-pro"), 1_000_000);
        assert_eq!(context_window_for_model("gpt-5"), 200_000);
        assert_eq!(context_window_for_model("o3-mini"), 200_000);
        assert_eq!(context_window_for_model("kimi-k2"), 256_000);
        assert_eq!(context_window_for_model("deepseek-chat"), 128_000);
        assert_eq!(context_window_for_model("qwen-max"), 128_000);
        assert_eq!(context_window_for_model("unknown-model"), 128_000);
        // 大小写不敏感
        assert_eq!(context_window_for_model("Claude-Opus"), 200_000);
    }

    #[test]
    fn auto_compact_threshold_is_80_percent_of_window() {
        assert_eq!(auto_compact_threshold("claude-sonnet-5-5"), 160_000);
        assert_eq!(auto_compact_threshold("deepseek-chat"), 102_400);
        assert_eq!(auto_compact_threshold("unknown"), 102_400);
    }
}
