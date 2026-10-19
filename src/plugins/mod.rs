// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! 插件加载策略 — 对齐 MCode.md 3.2：万物皆插件，内置 → 用户 → 项目 覆盖加载

use serde::Deserialize;
use std::path::Path;

/// 插件清单（plugin.json）
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    /// 注册的 slash 指令（name -> 描述）
    pub commands: Vec<PluginCommand>,
    /// 注册的 subagent
    pub agents: Vec<PluginAgent>,
}

impl Default for PluginManifest {
    fn default() -> Self {
        Self {
            name: String::new(),
            version: "0.0.0".into(),
            commands: vec![],
            agents: vec![],
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginCommand {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginAgent {
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    /// 工具白名单，缺省继承全部
    pub tools: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub source: &'static str,
}

/// 扫描目录下的 <plugin>/plugin.json
fn scan(dir: &Path, source: &'static str, out: &mut Vec<LoadedPlugin>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let manifest = e.path().join("plugin.json");
        if let Ok(bytes) = std::fs::read(&manifest)
            && let Ok(m) = serde_json::from_slice::<PluginManifest>(&bytes)
        {
            out.push(LoadedPlugin {
                manifest: m,
                source,
            });
        }
    }
}

/// 加载顺序：内置 ecc-core → ~/.mcode/plugins → <project>/.mcode/plugins
pub fn load_all(project_dir: &Path) -> Vec<LoadedPlugin> {
    let mut out = vec![];
    scan(
        &crate::util::mcode_dir().join("assets/ecc-core/plugins"),
        "ecc-core",
        &mut out,
    );
    scan(&crate::util::mcode_dir().join("plugins"), "user", &mut out);
    scan(&project_dir.join(".mcode/plugins"), "project", &mut out);
    out
}

/// 同名注册冲突解析：项目级 > 用户级 > 内置，覆盖时给出提示（对齐 registry 去重策略）
pub fn resolve_conflicts(plugins: &[LoadedPlugin]) -> Vec<String> {
    use std::collections::BTreeMap;
    let mut seen: BTreeMap<String, &'static str> = Default::default();
    let mut notes = vec![];
    for p in plugins {
        for c in &p.manifest.commands {
            if let Some(prev) = seen.get(&c.name) {
                notes.push(format!("指令 {} 由 {} 覆盖 {}", c.name, p.source, prev));
            }
            seen.insert(c.name.clone(), p.source);
        }
    }
    notes
}
