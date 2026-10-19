// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! mcode-core — MCode_Desktop 的 Rust 内核
//!
//! 能力对齐 `mcode` CLI（Node/TS 版）的完整能力面：
//! compact-harness（context/history/compaction/scheduler）、工具体系、
//! 四权限模式、会话持久化、slash 指令全集、skills 渐进式披露、插件加载。
//!
//! 数据目录与 CLI 完全互通（`~/.mcode/`），config.json 是唯一事实源。

pub mod agent;
pub mod api;
pub mod commands;
pub mod config;
pub mod hooks;
pub mod mcp;
pub mod permission;
pub mod plugins;
pub mod sessions;
pub mod skills;
pub mod tools;
pub mod util;

pub use config::{Config, Mode, Provider};
pub use permission::Decision;
