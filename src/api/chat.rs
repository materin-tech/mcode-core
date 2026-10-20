// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! OpenAI Chat Completions 后端（`POST {baseURL}/chat/completions`，SSE 流式）。
//! 面向私有化部署：vLLM/SGLang/one-api 网关的 DeepSeek/Qwen 等只提供此协议
//! （实测某 vLLM 网关对 /responses 返回 401/400，对 /chat/completions 正常）。
//!
//! 与 Responses 后端的协议映射差异：
//! - 消息体 `messages`（system/user/assistant），工具结果沿用 history 抽象折叠为 user 文本
//! - 图片附件 `{"type":"image_url","image_url":{"url":…}}`（Responses 为 input_image）
//! - 工具调用经 `delta.tool_calls` 增量下发（index 分片，arguments 跨 chunk 拼接）
//! - 思考增量 `delta.reasoning_content`（DeepSeek/vLLM 惯例），兼容 `delta.reasoning`
//! - usage 由 `stream_options: {"include_usage": true}` 请求，任意 chunk 携带即采纳（末次为准）

use anyhow::Result;
use futures::StreamExt;
use serde_json::{Value, json};

use super::authorized_client;
use crate::agent::{ApiMessage, Attachment, ModelBackend, StreamEvent, StreamTurn, ToolCall};
use crate::config::{Provider, ThinkingEffort};

pub struct ChatCompletionsBackend {
    provider: Provider,
    model: String,
    client: reqwest::Client,
}

impl ChatCompletionsBackend {
    pub fn new(provider: Provider, model: String) -> Result<Self> {
        let client = authorized_client(&provider)?;
        Ok(Self {
            provider,
            model,
            client,
        })
    }

    fn url(&self) -> String {
        format!(
            "{}/chat/completions",
            self.provider.base_url.trim_end_matches('/')
        )
    }
}

/// 工具调用增量累积器：`delta.tool_calls` 按 index 分片，id/name 首片给出，arguments 跨片拼接
#[derive(Default)]
struct ToolCallAccumulator {
    slots: Vec<Option<PartialToolCall>>,
}

#[derive(Default, Clone)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

impl ToolCallAccumulator {
    fn apply(&mut self, tc: &Value) {
        let idx = tc["index"].as_u64().unwrap_or(0) as usize;
        if self.slots.len() <= idx {
            self.slots.resize(idx + 1, None);
        }
        let slot = self.slots[idx].get_or_insert_with(PartialToolCall::default);
        if let Some(id) = tc["id"].as_str() {
            slot.id = id.to_string();
        }
        if let Some(name) = tc["function"]["name"].as_str() {
            slot.name.push_str(name);
        }
        if let Some(args) = tc["function"]["arguments"].as_str() {
            slot.arguments.push_str(args);
        }
    }

    /// 流结束：导出完整调用清单（无 name 的空槽丢弃；call_id 缺省空串，对齐 Responses 后端口径）
    fn finish(self) -> Vec<ToolCall> {
        self.slots
            .into_iter()
            .flatten()
            .filter(|p| !p.name.is_empty())
            .map(|p| ToolCall {
                call_id: p.id,
                name: p.name,
                arguments: if p.arguments.is_empty() {
                    "{}".into()
                } else {
                    p.arguments
                },
            })
            .collect()
    }
}

/// 组装 messages：system + history（tool 结果折叠为 user 文本，对齐 Responses 后端 role_str）+
/// 当前输入（有图片附件时用多段 content，否则用纯字符串以最大化网关兼容性）
fn build_messages(
    system: &str,
    history: &[ApiMessage],
    user_input: &str,
    attachments: &[Attachment],
) -> Vec<Value> {
    let mut messages = Vec::new();
    if !system.is_empty() {
        messages.push(json!({ "role": "system", "content": system }));
    }
    for m in history {
        let role = match m.role {
            crate::agent::Role::User | crate::agent::Role::Tool => "user",
            crate::agent::Role::Assistant => "assistant",
        };
        messages.push(json!({ "role": role, "content": m.text }));
    }
    if attachments.is_empty() {
        messages.push(json!({ "role": "user", "content": user_input }));
    } else {
        let mut parts = vec![json!({ "type": "text", "text": user_input })];
        for a in attachments {
            parts.push(json!({ "type": "image_url", "image_url": { "url": a.data_url } }));
        }
        messages.push(json!({ "role": "user", "content": parts }));
    }
    messages
}

/// 组装请求体（独立成 fn 供测试：effort=Default 时不发 reasoning_effort 键；
/// stream_options 请求 usage——实测 vLLM 网关逐 chunk 携带 usage，末次为准）
fn build_request_body(
    model: &str,
    messages: &[Value],
    tools: &[Value],
    effort: &ThinkingEffort,
) -> Value {
    let mut body = json!({
        "model": model,
        "messages": messages,
        "tools": tools,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if !tools.is_empty() {
        body["tool_choice"] = json!("auto");
    }
    if let Some(level) = effort.request_value() {
        body["reasoning_effort"] = json!(level);
    }
    body
}

fn tools_json(tools: &[crate::tools::ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                },
            })
        })
        .collect()
}

/// 处理单个 SSE data 载荷：文本/思考增量直接回调并入 reply，工具调用增量入累积器，usage 末次为准
fn apply_chunk(
    v: &Value,
    acc: &mut ToolCallAccumulator,
    reply: &mut String,
    usage: &mut Option<crate::util::TokenUsage>,
    on_event: &mut (dyn FnMut(StreamEvent) + Send),
) {
    let delta = &v["choices"][0]["delta"];
    if let Some(d) = delta["content"].as_str()
        && !d.is_empty()
    {
        reply.push_str(d);
        on_event(StreamEvent::TextDelta(d.to_string()));
    }
    // DeepSeek/vLLM 惯例 reasoning_content；部分实现用 reasoning
    let reasoning = delta["reasoning_content"]
        .as_str()
        .or_else(|| delta["reasoning"].as_str());
    if let Some(d) = reasoning
        && !d.is_empty()
    {
        on_event(StreamEvent::Reasoning(d.to_string()));
    }
    if let Some(tcs) = delta["tool_calls"].as_array() {
        for tc in tcs {
            acc.apply(tc);
        }
    }
    if let Some(u) = v["usage"].as_object() {
        *usage = Some(crate::util::TokenUsage {
            input: u.get("prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
            output: u
                .get("completion_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0),
            cache_read: u
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|x| x.as_u64())
                .unwrap_or(0),
            cache_write: 0,
        });
    }
}

#[async_trait::async_trait]
impl ModelBackend for ChatCompletionsBackend {
    async fn stream_turn(
        &self,
        system: &str,
        history: &[ApiMessage],
        user_input: &str,
        attachments: &[Attachment],
        tools: &[crate::tools::ToolDefinition],
        effort: &ThinkingEffort,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<StreamTurn> {
        let messages = build_messages(system, history, user_input, attachments);
        let body = build_request_body(&self.model, &messages, &tools_json(tools), effort);

        let resp = self.client.post(self.url()).json(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("API {status}: {}", super::truncate(&text, 500));
        }

        let mut stream = resp.bytes_stream();
        let mut cancel = cancel;
        let mut buf = String::new();
        let mut turn = StreamTurn::default();
        let mut acc = ToolCallAccumulator::default();
        let mut reply = String::new();

        loop {
            // cancel 竞争（对齐 Responses 后端：stream.next() 挂起时 ESC 仍可打断）
            if *cancel.borrow() {
                turn.interrupted = true;
                return Ok(turn);
            }
            let next_chunk = tokio::select! {
                _ = cancel.changed() => continue,
                chunk = stream.next() => chunk,
            };
            let Some(chunk) = next_chunk else { break };
            let chunk = chunk?;
            buf.push_str(&String::from_utf8_lossy(&chunk));
            // SSE 按行取 data: 载荷（对齐 CLI parseSSE）
            while let Some(pos) = buf.find('\n') {
                let line: String = buf.drain(..=pos).collect();
                let line = line.trim_end();
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data.is_empty() || data == "[DONE]" {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(data) else {
                    continue;
                };
                apply_chunk(&v, &mut acc, &mut reply, &mut turn.usage, on_event);
            }
        }

        turn.reply = reply;
        turn.tool_calls = acc.finish();
        Ok(turn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(data_url: &str) -> Attachment {
        Attachment {
            path: "x.png".into(),
            data_url: data_url.into(),
        }
    }

    #[test]
    fn messages_system_history_and_plain_user() {
        let history = vec![
            ApiMessage {
                role: crate::agent::Role::User,
                text: "hi".into(),
            },
            ApiMessage {
                role: crate::agent::Role::Assistant,
                text: "hello".into(),
            },
            ApiMessage {
                role: crate::agent::Role::Tool,
                text: "result".into(), // 工具结果折叠为 user（对齐 Responses 后端 role_str）
            },
        ];
        let msgs = build_messages("sys", &history, "go", &[]);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[2]["role"], "assistant");
        assert_eq!(msgs[3]["role"], "user"); // Tool → user
        // 无附件 → 纯字符串 content（最大化网关兼容性）
        assert_eq!(msgs[4]["content"], "go");
    }

    #[test]
    fn messages_with_attachments_use_image_url_parts() {
        let msgs = build_messages("", &[], "看图", &[att("data:image/png;base64,AAA")]);
        let parts = msgs[0]["content"].as_array().unwrap();
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[1]["type"], "image_url");
        assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,AAA");
    }

    #[test]
    fn default_effort_omits_reasoning_effort_and_empty_tools_omit_choice() {
        let body = build_request_body(
            "m",
            &[json!({"role": "user", "content": "x"})],
            &[],
            &ThinkingEffort::Default,
        );
        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("tool_choice").is_none());
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn effort_and_tools_shape_matches_chat_completions() {
        let tools = vec![crate::tools::ToolDefinition {
            name: "read_file".into(),
            description: "d".into(),
            parameters: json!({"type": "object"}),
            is_read_only: true,
        }];
        let body = build_request_body("m", &[], &tools_json(&tools), &ThinkingEffort::High);
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["tool_choice"], "auto");
        let f = &body["tools"][0]["function"];
        assert_eq!(f["name"], "read_file");
        assert!(body["tools"][0]["function"]["parameters"].is_object());
        // Chat Completions 的工具在 function 包装内（无 Responses 的顶层 name）
        assert!(body["tools"][0].get("name").is_none());
    }

    #[test]
    fn tool_call_deltas_accumulate_by_index_across_chunks() {
        let mut acc = ToolCallAccumulator::default();
        let mut usage = None;
        let mut reply = String::new();
        let mut sink = |_: StreamEvent| {};
        // 同一 index 的 arguments 分三片到达（拼接为 {"a":1}）；第二个 index 独立成槽
        let chunks = vec![
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":""}}]}}]}),
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"a"}}]}}]}),
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\":1}"}}]}}]}),
            json!({"choices":[{"index":0,"delta":{"content":"读一下"}}]}),
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_2","function":{"name":"edit_file","arguments":"{}"}}]}}]}),
            json!({"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"prompt_tokens_details":{"cached_tokens":3}}}),
        ];
        for c in &chunks {
            apply_chunk(c, &mut acc, &mut reply, &mut usage, &mut sink);
        }
        assert_eq!(reply, "读一下");
        let calls = acc.finish();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].call_id, "call_1");
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments, r#"{"a":1}"#);
        assert_eq!(calls[1].name, "edit_file");
        let u = usage.unwrap();
        assert_eq!((u.input, u.output, u.cache_read), (10, 5, 3));
    }

    #[test]
    fn reasoning_delta_and_done_line_are_handled() {
        let mut acc = ToolCallAccumulator::default();
        let mut usage = None;
        let mut reply = String::new();
        let mut events: Vec<StreamEvent> = Vec::new();
        {
            let mut sink = |ev: StreamEvent| events.push(ev);
            apply_chunk(
                &json!({"choices":[{"index":0,"delta":{"reasoning_content":"想"}}]}),
                &mut acc,
                &mut reply,
                &mut usage,
                &mut sink,
            );
            // 兼容 reasoning 别名键
            apply_chunk(
                &json!({"choices":[{"index":0,"delta":{"reasoning":"考"}}]}),
                &mut acc,
                &mut reply,
                &mut usage,
                &mut sink,
            );
        }
        assert!(
            matches!(events.as_slice(), [StreamEvent::Reasoning(a), StreamEvent::Reasoning(b)] if a == "想" && b == "考")
        );
    }
}
