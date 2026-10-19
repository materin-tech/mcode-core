// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! 权限门控 — 四模式对齐 CLI permission/manager.ts（MCode.md 3.4）

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// 每个非只读工具执行前弹确认
    Manual,
    /// 白名单内自动放行，其余确认
    Auto,
    /// 只读模式：仅只读工具，输出计划不落盘
    Plan,
    /// 全自动放行（默认）
    #[default]
    Passon,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Passon, Mode::Manual, Mode::Auto, Mode::Plan];

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Mode::Manual => "manual",
            Mode::Auto => "auto",
            Mode::Plan => "plan",
            Mode::Passon => "passon",
        }
    }

    pub fn hint(&self) -> &'static str {
        match self {
            Mode::Manual => "每个非只读工具执行前弹确认",
            Mode::Auto => "白名单内的工具自动放行，其余确认",
            Mode::Plan => "只读模式：仅只读工具，输出计划不落盘",
            Mode::Passon => "全自动放行（默认）",
        }
    }
}

/// 权限请求的裁决
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    AllowAlways,
    Deny,
}

/// 权限门控：Mode + 会话内记忆 + 规则表 → 单一裁决入口
pub struct PermissionManager {
    pub mode: Mode,
    /// 会话内「总是允许」沉淀：tool name -> true
    always_allow: std::collections::HashSet<String>,
    rules: Vec<crate::config::PermissionRule>,
}

impl PermissionManager {
    pub fn new(mode: Mode, rules: Vec<crate::config::PermissionRule>) -> Self {
        Self {
            mode,
            always_allow: Default::default(),
            rules,
        }
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    pub fn cycle_mode(&mut self) -> Mode {
        self.mode = self.mode.next();
        self.mode
    }

    /// 只读工具在 plan 模式下放行；passon 全放行；allow 规则优先于确认
    pub fn evaluate(&self, tool: &str, is_read_only: bool, args_summary: &str) -> Verdict {
        if is_read_only {
            return Verdict::Allow;
        }
        if self.mode == Mode::Passon {
            return Verdict::Allow;
        }
        if self.always_allow.contains(tool) {
            return Verdict::Allow;
        }
        for r in &self.rules {
            if r.tool == tool && (r.pattern == "*" || args_summary.contains(&r.pattern)) {
                return if r.allow {
                    Verdict::Allow
                } else {
                    Verdict::Deny {
                        reason: format!("规则拒绝: {}", r.pattern),
                    }
                };
            }
        }
        match self.mode {
            Mode::Plan => Verdict::Deny {
                reason: "plan 模式：只允许只读工具".into(),
            },
            Mode::Auto => Verdict::Confirm,
            _ => Verdict::Confirm,
        }
    }

    pub fn record(&mut self, tool: &str, d: &Decision) {
        if *d == Decision::AllowAlways {
            self.always_allow.insert(tool.to_string());
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Confirm,
    Deny { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_cycle_matches_shift_tab_order() {
        assert_eq!(Mode::Passon.next(), Mode::Manual);
        assert_eq!(Mode::Manual.next(), Mode::Auto);
        assert_eq!(Mode::Auto.next(), Mode::Plan);
        assert_eq!(Mode::Plan.next(), Mode::Passon);
    }

    #[test]
    fn mode_semantics() {
        let mut pm = PermissionManager::new(Mode::Passon, vec![]);
        assert_eq!(pm.evaluate("bash", false, "rm"), Verdict::Allow);

        pm.set_mode(Mode::Manual);
        assert!(matches!(pm.evaluate("bash", false, "ls"), Verdict::Confirm));
        assert_eq!(pm.evaluate("grep", true, ""), Verdict::Allow); // 只读放行

        pm.record("bash", &Decision::AllowAlways);
        assert_eq!(pm.evaluate("bash", false, "rm -rf"), Verdict::Allow);

        pm.set_mode(Mode::Plan);
        assert!(matches!(
            pm.evaluate("write_file", false, ""),
            Verdict::Deny { .. }
        ));
    }
}
