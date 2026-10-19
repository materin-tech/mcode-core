// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! 通用工具：数据目录、token 统计

pub mod file_refs;

use std::path::PathBuf;

/// 与 CLI 版完全互通的数据根目录：`~/.mcode/`
pub fn mcode_dir() -> PathBuf {
    dirs::home_dir().expect("无法获取 HOME 目录").join(".mcode")
}

pub fn config_path() -> PathBuf {
    mcode_dir().join("config.json")
}

/// 桌面扩展配置（权限规则 / MCP / hooks）— CLI 不读取，避免污染互操作面
pub fn desktop_config_path() -> PathBuf {
    mcode_dir().join("desktop.json")
}

/// 项目级 MCODE.md 记忆文档搜索 + 全局，均存在时拼接
pub fn mcode_md_paths(cwd: &std::path::Path) -> Vec<PathBuf> {
    let mut out = vec![];
    // 最近优先：cwd → 根（对齐 CLI mdoc.ts 的 nearest-first walk）
    let mut cur = Some(cwd.to_path_buf());
    while let Some(d) = cur {
        out.push(d.join("MCODE.md"));
        cur = d.parent().map(|p| p.to_path_buf());
    }
    out.push(mcode_dir().join("MCODE.md"));
    out
}

/// /init 模板（逐字对齐 TS memory/mdoc.ts 的 TEMPLATE）
pub const MCODE_MD_TEMPLATE: &str = "# MCODE.md

Project-level instructions for MCode. Loaded into the system prompt on every session.

## Build & test

<!-- e.g. npm run build / npm test -->

## Conventions

<!-- code style, naming, directory layout -->

## Notes

<!-- anything the agent should always know -->
";

/// 生成 MCODE.md（对齐 TS initMcodeDoc：已存在则不覆盖，返回 (路径, 是否新建)）
pub fn init_mcode_doc(cwd: &std::path::Path) -> std::io::Result<(PathBuf, bool)> {
    let path = cwd.join("MCODE.md");
    if path.exists() {
        return Ok((path, false));
    }
    std::fs::write(&path, MCODE_MD_TEMPLATE)?;
    Ok((path, true))
}

pub fn sessions_dir(cwd: &std::path::Path) -> PathBuf {
    let hash = format!("{:x}", md5_of(cwd.to_string_lossy().as_bytes()));
    mcode_dir().join("projects").join(hash).join("sessions")
}

fn md5_of(bytes: &[u8]) -> u64 {
    // 简易 FNV-1a：与 CLI 的 hash-of-pwd 语义对齐（目录标识，非安全用途）
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// token 用量累计（按模型分组，对齐 CLI util/token-usage.ts）
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct TokenLedger {
    /// model name -> usage
    pub by_model: std::collections::BTreeMap<String, TokenUsage>,
}

impl TokenLedger {
    pub fn record(&mut self, model: &str, u: TokenUsage) {
        let e = self.by_model.entry(model.to_string()).or_default();
        e.input += u.input;
        e.output += u.output;
        e.cache_read += u.cache_read;
        e.cache_write += u.cache_write;
    }

    pub fn total(&self) -> TokenUsage {
        self.by_model
            .values()
            .fold(TokenUsage::default(), |a, u| TokenUsage {
                input: a.input + u.input,
                output: a.output + u.output,
                cache_read: a.cache_read + u.cache_read,
                cache_write: a.cache_write + u.cache_write,
            })
    }
}

/// 打包内资产的运行时定位：macOS .app Resources → 可执行文件旁 assets/ → 仓库源码路径（cargo run 兜底）
pub fn assets_source_dir() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        // .app/Contents/MacOS/exe → .app/Contents/Resources/assets
        if let Some(res) = exe
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("Resources/assets"))
            && res.is_dir()
        {
            return Some(res);
        }
        if let Some(adj) = exe.parent().map(|p| p.join("assets"))
            && adj.is_dir()
        {
            return Some(adj);
        }
    }
    // cargo run / 开发态：仓库根 assets
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    if repo.is_dir() {
        return Some(repo);
    }
    None
}

/// CLI 同款落地机制：内置资产一次性物化到 ~/.mcode/assets（已存在则跳过，删除后重新落地可强制更新）
pub fn materialize_bundled_assets() -> usize {
    let Some(src) = assets_source_dir() else {
        return 0;
    };
    let dst_root = mcode_dir().join("assets");
    let mut n = 0;
    for tier in ["ecc-core", "pua-core"] {
        let from = src.join(tier);
        let to = dst_root.join(tier);
        if !from.is_dir() || to.is_dir() {
            continue;
        }
        if copy_tree(&from, &to).is_ok() {
            n += 1;
        }
    }
    n
}

/// 递归复制目录（物化与 skill 安装共用；文件操作工具另有带钳制/防逃逸的实现）
pub fn copy_tree(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let t = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &t)?;
        } else {
            std::fs::copy(entry.path(), &t)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_groups_by_model() {
        let mut l = TokenLedger::default();
        l.record(
            "glm-4.7",
            TokenUsage {
                input: 100,
                output: 10,
                ..Default::default()
            },
        );
        l.record(
            "glm-4.7",
            TokenUsage {
                input: 50,
                output: 5,
                ..Default::default()
            },
        );
        l.record(
            "glm-4.7-air",
            TokenUsage {
                input: 7,
                ..Default::default()
            },
        );
        assert_eq!(l.by_model["glm-4.7"].input, 150);
        assert_eq!(l.total().output, 15);
    }

    #[test]
    fn init_mcode_doc_creates_then_skips() {
        let dir = std::env::temp_dir().join(format!("mcode_init_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (p1, created1) = init_mcode_doc(&dir).unwrap();
        assert!(created1);
        assert!(p1.ends_with("MCODE.md"));
        let content1 = std::fs::read_to_string(&p1).unwrap();
        assert!(content1.starts_with("# MCODE.md\n"));
        // 二次调用不覆盖（对齐 TS：already exists）
        std::fs::write(&p1, "user content").unwrap();
        let (_, created2) = init_mcode_doc(&dir).unwrap();
        assert!(!created2);
        assert_eq!(std::fs::read_to_string(&p1).unwrap(), "user content");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
