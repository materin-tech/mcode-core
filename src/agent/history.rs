// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! 会话历史 — 纯 user/assistant 文本对（对齐 CLI agent/history.ts）。
//! 工具中间轮不进 history（工具结果只经当前输入喂给模型，最终回复落一对）。

use super::{ApiMessage, Role};

#[derive(Default)]
pub struct HistoryStore {
    /// (user, assistant) 轮次对
    turns: Vec<(String, String)>,
}

impl HistoryStore {
    /// assistant 为空不落对（纯工具轮不重复记录，对齐 CLI pushTurn 语义）
    pub fn push_turn(&mut self, user: impl Into<String>, assistant: impl Into<String>) {
        let a = assistant.into();
        if a.trim().is_empty() {
            return;
        }
        self.turns.push((user.into(), a));
    }

    pub fn all(&self) -> Vec<ApiMessage> {
        let mut out = vec![];
        for (u, a) in &self.turns {
            out.push(ApiMessage {
                role: Role::User,
                text: u.clone(),
            });
            out.push(ApiMessage {
                role: Role::Assistant,
                text: a.clone(),
            });
        }
        out
    }

    pub fn message_count(&self) -> usize {
        self.turns.len() * 2
    }

    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    /// 上下文 token 粗估（4 chars ≈ 1 token）
    pub fn estimate_tokens(&self) -> usize {
        let chars: usize = self.turns.iter().map(|(u, a)| u.len() + a.len()).sum();
        chars / CHARS_PER_TOKEN
    }

    /// 压缩：摘要 + 保留最近 keep_turns 轮（对齐 CLI compact）
    pub fn compact(&mut self, summary: String, keep_turns: usize) {
        let keep = keep_turns.max(2);
        let start = self.turns.len().saturating_sub(keep);
        let recent: Vec<(String, String)> = self.turns[start..].to_vec();
        self.turns = vec![(format!("[compacted history]\n{summary}"), String::new())];
        // 摘要本身作为 user 侧消息（assistant 侧为空，避免空对被过滤）
        self.turns[0].1 = "(acknowledged)".into();
        self.turns.extend(recent);
    }

    pub fn clear(&mut self) {
        self.turns.clear();
    }
}

const CHARS_PER_TOKEN: usize = 4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_assistant_not_recorded() {
        let mut h = HistoryStore::default();
        h.push_turn("q", "");
        assert!(h.is_empty());
        h.push_turn("q", "a");
        assert_eq!(h.message_count(), 2);
    }

    #[test]
    fn compact_keeps_recent_turns() {
        let mut h = HistoryStore::default();
        for i in 0..10 {
            h.push_turn(format!("u{i}"), format!("a{i}"));
        }
        h.compact("earlier stuff".into(), 2);
        let msgs = h.all();
        assert!(msgs[0].text.contains("compacted history"));
        assert!(msgs[0].text.contains("earlier stuff"));
        // 保留最近 2 轮 → 最后两条是 u8/a8, u9/a9
        assert!(msgs.last().unwrap().text.contains("a9"));
        assert!(!msgs[0].text.contains("a9"));
    }
}
