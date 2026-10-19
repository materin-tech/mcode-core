// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! hooks 引擎 — 工具执行生命周期钩子（PreToolUse / PostToolUse / Stop）。
//! 规则来源：~/.mcode/desktop.json + <project>/.mcode/desktop.json（项目覆盖同名匹配，全部生效）。
//! 约定：PreToolUse 退出码 2 = 拒绝（stdout 作为拒绝理由）；其余退出码仅记录不阻断；
//! hook 命令经 stdin 收到 JSON 载荷 {event, tool, args}，10s 超时保护。

use crate::config::{DesktopConfig, HookEvent};
use anyhow::Result;
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

const HOOK_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct HookOutcome {
    /// PreToolUse 拒绝理由（退出码 2）
    pub deny_reason: Option<String>,
    /// hook stdout（供 UI 展示 / 模型参考）
    pub output: String,
}

pub struct HookEngine {
    hooks: Vec<crate::config::HookConfig>,
}

impl HookEngine {
    /// 测试/无 hooks 场景
    pub fn empty() -> Self {
        Self { hooks: vec![] }
    }

    /// 全局 + 项目 hooks 合并加载（项目级排在后面、同名事件全执行）
    pub fn load(project_dir: &Path) -> Self {
        let mut hooks = vec![];
        hooks.extend(Self::load_file(&crate::util::desktop_config_path()));
        hooks.extend(Self::load_file(
            &project_dir.join(".mcode").join("desktop.json"),
        ));
        Self { hooks }
    }

    fn load_file(path: &Path) -> Vec<crate::config::HookConfig> {
        let cfg: DesktopConfig = DesktopConfig::load_from(path);
        cfg.hooks
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    pub fn rules(&self) -> &[crate::config::HookConfig] {
        &self.hooks
    }

    fn matches(&self, event: HookEvent, tool: &str) -> Vec<&crate::config::HookConfig> {
        self.hooks
            .iter()
            .filter(|h| h.event == event && (h.matcher == "*" || tool.contains(&h.matcher)))
            .collect()
    }

    /// 执行一个 hook 命令：JSON 经 stdin 传入，捕获 stdout/stderr + 退出码
    async fn run_command(&self, command: &str, payload: &Value) -> Result<HookOutcome> {
        use tokio::io::AsyncWriteExt;
        let shell = if cfg!(windows) { "cmd" } else { "sh" };
        let shell_args: Vec<&str> = if cfg!(windows) {
            vec!["/C"]
        } else {
            vec!["-c"]
        };
        let mut child = tokio::process::Command::new(shell)
            .args(shell_args)
            .arg(command)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(payload.to_string().as_bytes()).await;
            drop(stdin); // 关闭 stdin 让脚本 read 到 EOF
        }
        let out = tokio::time::timeout(HOOK_TIMEOUT, child.wait_with_output()).await;
        match out {
            Ok(Ok(o)) => {
                let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
                let code = o.status.code().unwrap_or(-1);
                Ok(HookOutcome {
                    deny_reason: (code == 2).then(|| {
                        if stdout.is_empty() {
                            format!("hook 拒绝（{command}）")
                        } else {
                            stdout.clone()
                        }
                    }),
                    output: if stdout.is_empty() { stderr } else { stdout },
                })
            }
            Ok(Err(e)) => Ok(HookOutcome {
                deny_reason: None,
                output: format!("hook 执行失败: {e}"),
            }),
            Err(_) => Ok(HookOutcome {
                deny_reason: None,
                output: format!("hook 超时（{}s）", HOOK_TIMEOUT.as_secs()),
            }),
        }
    }

    /// PreToolUse：退出码 2 → 拒绝执行
    pub async fn run_pre_tool_use(&self, tool: &str, args: &Value) -> HookOutcome {
        let mut deny = None;
        let mut outputs = vec![];
        for h in self.matches(HookEvent::PreToolUse, tool) {
            let payload = json!({"event": "pre_tool_use", "tool": tool, "args": args});
            if let Ok(o) = self.run_command(&h.command, &payload).await {
                if o.deny_reason.is_some() {
                    deny = o.deny_reason;
                }
                if !o.output.is_empty() {
                    outputs.push(o.output);
                }
            }
        }
        HookOutcome {
            deny_reason: deny,
            output: outputs.join("\n"),
        }
    }

    /// PostToolUse：fire-and-forget（失败不影响主流程）
    pub async fn run_post_tool_use(
        &self,
        tool: &str,
        success: bool,
        output_summary: &str,
    ) -> String {
        let mut outputs = vec![];
        for h in self.matches(HookEvent::PostToolUse, tool) {
            let payload = json!({
                "event": "post_tool_use", "tool": tool,
                "success": success,
                "output": output_summary.chars().take(2000).collect::<String>(),
            });
            if let Ok(o) = self.run_command(&h.command, &payload).await
                && !o.output.is_empty()
            {
                outputs.push(o.output);
            }
        }
        outputs.join("\n")
    }

    /// Stop：回合结束
    pub async fn run_stop(&self, reply_summary: &str) {
        for h in self.matches(HookEvent::Stop, "*") {
            let payload = json!({
                "event": "stop",
                "tool": "",
                "reply": reply_summary.chars().take(2000).collect::<String>(),
            });
            let _ = self.run_command(&h.command, &payload).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HookConfig;

    #[tokio::test]
    async fn pre_tool_use_deny_on_exit_2() {
        let engine = HookEngine {
            hooks: vec![HookConfig {
                event: HookEvent::PreToolUse,
                matcher: "*".into(),
                command: "test \"$HOOK_CHECK\" = x && exit 0 || exit 2".into(),
            }],
        };
        // exit 2 → 拒绝
        let o = engine
            .run_pre_tool_use("bash", &json!({"command": "rm -rf"}))
            .await;
        assert!(o.deny_reason.is_some());
        // exit 0 → 放行
        let engine2 = HookEngine {
            hooks: vec![HookConfig {
                event: HookEvent::PreToolUse,
                matcher: "bash".into(),
                command: "exit 0".into(),
            }],
        };
        let o2 = engine2.run_pre_tool_use("bash", &json!({})).await;
        assert!(o2.deny_reason.is_none());
    }

    #[tokio::test]
    async fn stdin_payload_reaches_hook() {
        let engine = HookEngine {
            hooks: vec![HookConfig {
                event: HookEvent::PreToolUse,
                matcher: "*".into(),
                // 读取 stdin 的 tool 字段；名字匹配则拒绝
                command: "grep -q readme_tool && exit 2 || exit 0".into(),
            }],
        };
        let o = engine.run_pre_tool_use("readme_tool", &json!({})).await;
        assert!(o.deny_reason.is_some());
    }

    #[test]
    fn matcher_substring_and_wildcard() {
        let engine = HookEngine {
            hooks: vec![
                HookConfig {
                    event: HookEvent::PreToolUse,
                    matcher: "*".into(),
                    command: "a".into(),
                },
                HookConfig {
                    event: HookEvent::PostToolUse,
                    matcher: "bash".into(),
                    command: "b".into(),
                },
            ],
        };
        assert_eq!(engine.matches(HookEvent::PreToolUse, "anything").len(), 1);
        assert_eq!(engine.matches(HookEvent::PostToolUse, "bash").len(), 1);
        assert_eq!(engine.matches(HookEvent::PostToolUse, "grep").len(), 0);
    }
}
