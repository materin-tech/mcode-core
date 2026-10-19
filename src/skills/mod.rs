// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! Skill 加载 — 渐进式披露（MCode.md 3.3）：仅 name+description 常驻上下文，命中后展开 body

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    /// 来源：ecc-core（内置）/ user（~/.mcode）/ project（.mcode）
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub entry: SkillEntry,
    /// 第二层：命中后注入的完整指令内容
    pub body: String,
}

/// 解析 SKILL.md 的 frontmatter（--- 包裹的 key: value 头部）
fn parse_skill_md(text: &str) -> Option<(String, String, String)> {
    let rest = text.strip_prefix("---")?;
    let end = rest.find("---")?;
    let (front, body) = rest.split_at(end);
    let body = body.trim_start_matches("---").trim().to_string();
    let mut name = None;
    let mut desc = None;
    for line in front.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let v = v.trim().trim_matches('"').to_string();
            match k.trim() {
                "name" => name = Some(v),
                "description" => desc = Some(v),
                _ => {}
            }
        }
    }
    Some((name?, desc?, body))
}

/// 加载一个目录下的 skills（<dir>/<name>/SKILL.md）
fn load_dir(dir: &Path, source: &str, out: &mut Vec<Skill>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path().join("SKILL.md");
        if let Ok(text) = std::fs::read_to_string(&p)
            && let Some((name, description, body)) = parse_skill_md(&text)
        {
            out.push(Skill {
                entry: SkillEntry {
                    name,
                    description,
                    source: source.to_string(),
                },
                body,
            });
        }
    }
}

/// 加载顺序：内置 ecc-core → 内置 pua-core → 用户级 → 项目级（后加载可覆盖同名，对齐 3.2）
pub fn load_all(project_dir: &Path) -> Vec<Skill> {
    let mut skills = vec![];
    load_dir(
        &crate::util::mcode_dir().join("assets/ecc-core/skills"),
        "ecc-core",
        &mut skills,
    );
    load_dir(
        &crate::util::mcode_dir().join("assets/pua-core"),
        "pua-core",
        &mut skills,
    );
    load_dir(
        &crate::util::mcode_dir().join("skills"),
        "user",
        &mut skills,
    );
    load_dir(&project_dir.join(".mcode/skills"), "project", &mut skills);
    dedup(skills)
}

/// 同名去重：保留最后加载者，并输出覆盖关系
fn dedup(skills: Vec<Skill>) -> Vec<Skill> {
    let mut map: std::collections::BTreeMap<String, Skill> = Default::default();
    for s in skills {
        map.insert(s.entry.name.clone(), s);
    }
    map.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter() {
        let md = "---\nname: pua\ndescription: 效率教练\n---\n正文内容";
        let (n, d, b) = parse_skill_md(md).unwrap();
        assert_eq!(n, "pua");
        assert_eq!(d, "效率教练");
        assert_eq!(b, "正文内容");
    }
}
