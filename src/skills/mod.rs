// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! Skill 加载 — 渐进式披露（MCode.md 3.3）：仅 name+description 常驻上下文，命中后展开 body

use anyhow::Context;
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

// ---------- 管理原语（create / install / remove，供 CLI 与桌面端共用） ----------

/// skill 名合法性：字母/数字/连字符/下划线，防路径逃逸与非法文件名
pub fn validate_skill_name(name: &str) -> anyhow::Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    anyhow::ensure!(ok, "非法 skill 名 `{name}`（仅限字母数字-_，≤64 字符）");
    Ok(())
}

/// skill 安装基目录：project_dir 有则项目级 `.mcode/skills`，否则用户级 `~/.mcode/skills`
fn skills_base(project_dir: Option<&Path>) -> std::path::PathBuf {
    match project_dir {
        Some(p) => p.join(".mcode/skills"),
        None => crate::util::mcode_dir().join("skills"),
    }
}

/// 模板创建：写入 `<base>/<name>/SKILL.md`（frontmatter + body），已存在则拒绝
pub fn create_skill(
    name: &str,
    description: &str,
    body: &str,
    project_dir: Option<&Path>,
) -> anyhow::Result<std::path::PathBuf> {
    validate_skill_name(name)?;
    anyhow::ensure!(
        !description.trim().is_empty(),
        "description 不能为空（它是常驻上下文的唯一线索）"
    );
    let dir = skills_base(project_dir).join(name);
    anyhow::ensure!(!dir.exists(), "skill `{name}` 已存在于 {}", dir.display());
    std::fs::create_dir_all(&dir)?;
    let md = format!(
        "---\nname: {name}\ndescription: {}\n---\n{}\n",
        description.trim(),
        body.trim()
    );
    std::fs::write(dir.join("SKILL.md"), md)?;
    Ok(dir)
}

/// 本地目录安装：源目录必须含 SKILL.md，整目录拷贝到 `<base>/<skill名>/`
pub fn install_skill_from_dir(
    src: &Path,
    project_dir: Option<&Path>,
) -> anyhow::Result<std::path::PathBuf> {
    let src_md = src.join("SKILL.md");
    anyhow::ensure!(src_md.is_file(), "源目录缺少 SKILL.md：{}", src.display());
    let text = std::fs::read_to_string(&src_md)?;
    let (name, _, _) = parse_skill_md(&text)
        .with_context(|| format!("SKILL.md frontmatter 解析失败：{}", src.display()))?;
    validate_skill_name(&name)?;
    let dir = skills_base(project_dir).join(&name);
    anyhow::ensure!(!dir.exists(), "skill `{name}` 已存在于 {}", dir.display());
    crate::util::copy_tree(src, &dir)
        .with_context(|| format!("拷贝 {} → {} 失败", src.display(), dir.display()))?;
    Ok(dir)
}

/// 移除：仅限可写层（用户级 / 项目级），内置 assets 层不受此 API 支配。
/// 返回是否真的删除了目录。
pub fn remove_skill(name: &str, project_dir: Option<&Path>) -> anyhow::Result<bool> {
    validate_skill_name(name)?;
    let dir = skills_base(project_dir).join(name);
    if !dir.is_dir() {
        return Ok(false);
    }
    std::fs::remove_dir_all(&dir)?;
    Ok(true)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter() {
        let md = r#"---
name: skill-a
description: demo
---
BODY"#;
        let (n, d, b) = parse_skill_md(md).unwrap();
        assert_eq!(n, "skill-a");
        assert_eq!(d, "demo");
        assert_eq!(b, "BODY");
    }

    #[test]
    fn skill_lifecycle_primitives() {
        let base = std::env::temp_dir().join(format!("mcode-skills-{}-life", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let project = base.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let dir = create_skill("demo-skill", "demo desc", "body text", Some(&project)).unwrap();
        assert_eq!(dir, project.join(".mcode/skills/demo-skill"));
        assert!(dir.join("SKILL.md").is_file());
        // create 重复拒绝
        assert!(create_skill("demo-skill", "x", "y", Some(&project)).is_err());
        // install：名字取自 frontmatter
        let src = base.join("src-skill");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("SKILL.md"),
            "---\nname: inst-al\ndescription: install test\n---\nbody",
        )
        .unwrap();
        let got = install_skill_from_dir(&src, Some(&project)).unwrap();
        assert_eq!(got, project.join(".mcode/skills/inst-al"));
        // install 重复拒绝 / 缺 SKILL.md 拒绝
        assert!(install_skill_from_dir(&src, Some(&project)).is_err());
        assert!(install_skill_from_dir(&base, Some(&project)).is_err());
        // remove 命中 true / 未命中 false / 非法名拒绝
        assert!(remove_skill("inst-al", Some(&project)).unwrap());
        assert!(!remove_skill("ghost", Some(&project)).unwrap());
        assert!(remove_skill("../evil", Some(&project)).is_err());
        // create+load_all roundtrip
        create_skill("roundtrip", "rt desc", "rt body", Some(&project)).unwrap();
        let loaded = load_all(&project);
        let hit = loaded.iter().find(|s| s.entry.name == "roundtrip").unwrap();
        assert_eq!(hit.entry.source, "project");
        assert_eq!(hit.body, "rt body");
    }
}
