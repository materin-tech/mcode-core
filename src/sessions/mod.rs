// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! 会话持久化 — 对齐 MCode.md 5.1：JSONL 消息流 + meta + resume + rewind 检查点

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

use crate::agent::Message;

/// 会话保留期默认值：90 天未活动即过期删除（对齐 Claude cleanupPeriodDays 思路）
pub const DEFAULT_RETENTION_DAYS: u64 = 90;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub model: String,
    pub mode: String,
    pub started: String,
    pub updated: String,
    pub turns: u32,
    pub tokens_total: u64,
}

impl Default for SessionMeta {
    fn default() -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            title: "未命名会话".into(),
            cwd: std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            model: String::new(),
            mode: "passon".into(),
            started: now.clone(),
            updated: now,
            turns: 0,
            tokens_total: 0,
        }
    }
}

impl SessionMeta {
    /// `updated` 即"动过的痕迹"（追加消息自动刷新）。90 天内有活动则时间刷新重新计时；
    /// 时间戳无法解析的会话视为未过期 — 宁可漏删，不可误删。
    pub fn is_expired(&self, retention_days: u64) -> bool {
        let Ok(updated) = chrono::DateTime::parse_from_rfc3339(&self.updated) else {
            return false;
        };
        let age = chrono::Utc::now() - updated.with_timezone(&chrono::Utc);
        age >= chrono::Duration::days(retention_days as i64)
    }
}

pub struct SessionStore {
    pub meta: SessionMeta,
    jsonl_path: PathBuf,
    meta_path: PathBuf,
}

impl SessionStore {
    pub fn create(cwd: &std::path::Path) -> Result<Self> {
        let dir = crate::util::sessions_dir(cwd);
        std::fs::create_dir_all(&dir)?;
        let meta = SessionMeta {
            cwd: cwd.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let s = Self {
            jsonl_path: dir.join(format!("{}.jsonl", meta.id)),
            meta_path: dir.join(format!("{}.meta.json", meta.id)),
            meta,
        };
        s.flush_meta()?;
        // 空消息流也落盘：桌面端「新建会话」立即入列且可直接 resume（load 无条件读 jsonl）
        std::fs::write(&s.jsonl_path, b"")?;
        Ok(s)
    }

    /// 列出当前目录的历史会话（resume 选择器数据源，按 updated 倒序）。
    /// 过滤掉超过默认保留期的过期会话作为兜底 — 删除由启动时的 cleanup_expired 负责。
    pub fn list(cwd: &std::path::Path) -> Result<Vec<SessionMeta>> {
        let dir = crate::util::sessions_dir(cwd);
        let mut metas = vec![];
        if !dir.exists() {
            return Ok(metas);
        }
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".meta.json")
                && let Ok(bytes) = std::fs::read(entry.path())
                && let Ok(m) = serde_json::from_slice::<SessionMeta>(&bytes)
                && !m.is_expired(DEFAULT_RETENTION_DAYS)
            {
                metas.push(m);
            }
        }
        metas.sort_by(|a, b| b.updated.cmp(&a.updated));
        Ok(metas)
    }

    /// 跨项目列出全部会话（TS listAllSessions 对齐：本目录无会话时 --resume/--continue 的全局兜底）。
    /// meta 自带 cwd，恢复时以 meta.cwd 作为项目目录读取消息流。
    pub fn list_all() -> Vec<SessionMeta> {
        let projects = crate::util::mcode_dir().join("projects");
        let mut metas = vec![];
        let Ok(rd) = std::fs::read_dir(&projects) else {
            return metas;
        };
        for proj in rd.flatten() {
            let sessions = proj.path().join("sessions");
            let Ok(entries) = std::fs::read_dir(&sessions) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.ends_with(".meta.json")
                    && let Ok(bytes) = std::fs::read(entry.path())
                    && let Ok(m) = serde_json::from_slice::<SessionMeta>(&bytes)
                    && !m.is_expired(DEFAULT_RETENTION_DAYS)
                {
                    metas.push(m);
                }
            }
        }
        metas.sort_by(|a, b| b.updated.cmp(&a.updated));
        metas
    }

    /// 清理全部项目中超过保留期的过期会话（meta + 消息流 + 检查点一并删除）。
    /// 返回删除的会话数；meta 缺失 / 解析失败 / 时间戳非法的一律跳过不动。
    pub fn cleanup_expired(retention_days: u64) -> usize {
        cleanup_expired_in(&crate::util::mcode_dir().join("projects"), retention_days)
    }

    pub fn append_message(&mut self, m: &Message) -> Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.jsonl_path)?;
        writeln!(f, "{}", serde_json::to_string(m)?)?;
        self.meta.updated = chrono::Utc::now().to_rfc3339();
        self.flush_meta()
    }

    /// 恢复历史会话：读 JSONL 消息流（resume 数据源）
    pub fn load(cwd: &std::path::Path, id: &str) -> Result<(Self, Vec<Message>)> {
        let dir = crate::util::sessions_dir(cwd);
        let jsonl_path = dir.join(format!("{id}.jsonl"));
        let meta_path = dir.join(format!("{id}.meta.json"));
        let meta: SessionMeta = serde_json::from_slice(&std::fs::read(&meta_path)?)?;
        let mut messages = vec![];
        for line in std::fs::read_to_string(&jsonl_path)?.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(m) = serde_json::from_str::<Message>(line) {
                messages.push(m);
            }
        }
        let s = Self {
            meta,
            jsonl_path,
            meta_path,
        };
        Ok((s, messages))
    }

    /// 会话标题取首轮用户输入截断（对齐 CLI）
    pub fn ensure_title(&mut self, first_user_text: &str) {
        if self.meta.title == "未命名会话" && !first_user_text.is_empty() {
            let t: String = first_user_text.chars().take(24).collect();
            self.meta.title = t;
            let _ = self.flush_meta();
        }
    }

    /// 会话重命名（桌面端右键菜单数据源）：读 meta → 校验标题 → 写回。
    /// 仅改标题不动消息流；meta 缺失报错（含 id 拼写错误防御）。
    pub fn rename(cwd: &std::path::Path, id: &str, title: &str) -> Result<()> {
        let dir = crate::util::sessions_dir(cwd);
        let meta_path = dir.join(format!("{id}.meta.json"));
        let mut meta: SessionMeta = serde_json::from_slice(&std::fs::read(&meta_path)?)?;
        let title = title.trim();
        if title.is_empty() {
            anyhow::bail!("会话标题不能为空");
        }
        meta.title = title.to_string();
        std::fs::write(&meta_path, serde_json::to_string_pretty(&meta)?)?;
        Ok(())
    }

    /// 删除会话：meta + 消息流 + 检查点一并删除（对齐 cleanup_expired_in 的文件清单）。
    /// meta 不存在报错（重复删除/拼写错误防御），其余文件缺失忽略。
    pub fn delete(cwd: &std::path::Path, id: &str) -> Result<()> {
        let dir = crate::util::sessions_dir(cwd);
        let meta_path = dir.join(format!("{id}.meta.json"));
        if !meta_path.exists() {
            anyhow::bail!("会话不存在：{id}");
        }
        for suffix in [".meta.json", ".jsonl", ".checkpoints.jsonl"] {
            let _ = std::fs::remove_file(dir.join(format!("{id}{suffix}")));
        }
        Ok(())
    }

    /// 检查点：写文件 / bash 修改前留存快照标记（rewind 数据源）
    pub fn add_checkpoint(&mut self, desc: &str) -> Result<()> {
        let ck_path = self.jsonl_path.with_extension("checkpoints.jsonl");
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(ck_path)?;
        writeln!(
            f,
            "{}",
            serde_json::to_string(&serde_json::json!({
                "ts": chrono::Utc::now().to_rfc3339(),
                "desc": desc,
            }))?
        )?;
        Ok(())
    }

    fn flush_meta(&self) -> Result<()> {
        std::fs::write(&self.meta_path, serde_json::to_string_pretty(&self.meta)?)?;
        Ok(())
    }
}

/// 在给定 projects 根目录下删除过期会话（测试可注入临时根目录）
fn cleanup_expired_in(root: &std::path::Path, retention_days: u64) -> usize {
    let Ok(projects) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut removed = 0;
    for proj in projects.flatten() {
        let sessions = proj.path().join("sessions");
        let Ok(entries) = std::fs::read_dir(&sessions) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_suffix(".meta.json") else {
                continue;
            };
            let meta = std::fs::read(entry.path())
                .ok()
                .and_then(|b| serde_json::from_slice::<SessionMeta>(&b).ok());
            let Some(meta) = meta else {
                continue;
            };
            if !meta.is_expired(retention_days) {
                continue;
            }
            for suffix in [
                ".meta.json".to_string(),
                ".jsonl".to_string(),
                ".checkpoints.jsonl".to_string(),
            ] {
                let _ = std::fs::remove_file(sessions.join(format!("{id}{suffix}")));
            }
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Role;

    fn meta_with_updated(updated: &str) -> SessionMeta {
        SessionMeta {
            updated: updated.to_string(),
            ..Default::default()
        }
    }

    fn rfc3339_days_ago(days: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339()
    }

    #[test]
    fn fresh_session_is_not_expired() {
        assert!(!meta_with_updated(&rfc3339_days_ago(0)).is_expired(90));
    }

    #[test]
    fn session_past_retention_is_expired() {
        assert!(meta_with_updated(&rfc3339_days_ago(91)).is_expired(90));
    }

    #[test]
    fn session_at_retention_boundary_is_expired() {
        // 恰好 90 天未活动 = 已过期（≥ 保留期即删）
        assert!(meta_with_updated(&rfc3339_days_ago(90)).is_expired(90));
    }

    #[test]
    fn unparseable_timestamp_never_expires() {
        // 宁可漏删不可误删
        assert!(!meta_with_updated("").is_expired(90));
        assert!(!meta_with_updated("not-a-timestamp").is_expired(90));
    }

    #[test]
    fn cleanup_deletes_expired_and_keeps_fresh() {
        let root =
            std::env::temp_dir().join(format!("mcode-sessions-test-{}", uuid::Uuid::new_v4()));
        let sessions = root.join("proj").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();

        for (id, days_ago) in [("expired-one", 100), ("fresh-one", 1)] {
            let m = meta_with_updated(&rfc3339_days_ago(days_ago));
            std::fs::write(
                sessions.join(format!("{id}.meta.json")),
                serde_json::to_string(&m).unwrap(),
            )
            .unwrap();
            std::fs::write(sessions.join(format!("{id}.jsonl")), "").unwrap();
            std::fs::write(sessions.join(format!("{id}.checkpoints.jsonl")), "").unwrap();
        }

        let removed = cleanup_expired_in(&root, 90);

        assert_eq!(removed, 1);
        assert!(!sessions.join("expired-one.meta.json").exists());
        assert!(!sessions.join("expired-one.jsonl").exists());
        assert!(!sessions.join("expired-one.checkpoints.jsonl").exists());
        assert!(sessions.join("fresh-one.meta.json").exists());
        assert!(sessions.join("fresh-one.jsonl").exists());
        assert!(sessions.join("fresh-one.checkpoints.jsonl").exists());

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cleanup_skips_unparseable_meta() {
        let root =
            std::env::temp_dir().join(format!("mcode-sessions-test-{}", uuid::Uuid::new_v4()));
        let sessions = root.join("proj").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(sessions.join("broken.meta.json"), "not-json").unwrap();

        assert_eq!(cleanup_expired_in(&root, 90), 0);
        assert!(sessions.join("broken.meta.json").exists());

        std::fs::remove_dir_all(&root).unwrap();
    }

    fn temp_cwd() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("mcode-sessions-test-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn create_touches_empty_jsonl_so_resume_works() {
        let cwd = temp_cwd();
        let s = SessionStore::create(&cwd).unwrap();
        // 新建会话（未发消息）即可被 load：桌面端新建即入列 + 直接 resume 的前提
        let (_, msgs) = SessionStore::load(&cwd, &s.meta.id).unwrap();
        assert!(msgs.is_empty());
        let _ = std::fs::remove_dir_all(crate::util::sessions_dir(&cwd).parent().unwrap());
    }

    #[test]
    fn rename_updates_meta_title_on_disk() {
        let cwd = temp_cwd();
        let mut s = SessionStore::create(&cwd).unwrap();
        s.append_message(&Message::text(Role::User, "hi")).unwrap();
        SessionStore::rename(&cwd, &s.meta.id, "  新标题  ").unwrap();
        let (s2, _) = SessionStore::load(&cwd, &s.meta.id).unwrap();
        assert_eq!(s2.meta.title, "新标题");
        let _ = std::fs::remove_dir_all(crate::util::sessions_dir(&cwd).parent().unwrap());
    }

    #[test]
    fn rename_rejects_blank_title_and_missing_session() {
        let cwd = temp_cwd();
        let s = SessionStore::create(&cwd).unwrap();
        assert!(SessionStore::rename(&cwd, &s.meta.id, "   ").is_err());
        assert!(SessionStore::rename(&cwd, "no-such-id", "x").is_err());
        let _ = std::fs::remove_dir_all(crate::util::sessions_dir(&cwd).parent().unwrap());
    }

    #[test]
    fn delete_removes_all_files_and_second_delete_fails() {
        let cwd = temp_cwd();
        let mut s = SessionStore::create(&cwd).unwrap();
        s.append_message(&Message::text(Role::User, "hi")).unwrap();
        s.add_checkpoint("ck").unwrap();
        SessionStore::delete(&cwd, &s.meta.id).unwrap();
        let dir = crate::util::sessions_dir(&cwd);
        assert!(!dir.join(format!("{}.meta.json", s.meta.id)).exists());
        assert!(!dir.join(format!("{}.jsonl", s.meta.id)).exists());
        assert!(
            !dir.join(format!("{}.checkpoints.jsonl", s.meta.id))
                .exists()
        );
        // 重复删除：meta 已不存在，明确报错而非静默成功
        assert!(SessionStore::delete(&cwd, &s.meta.id).is_err());
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }
}
