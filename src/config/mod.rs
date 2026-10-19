// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! config.json — 唯一事实源，与 CLI 版逐字段互通（对齐 mcode/src/config/config.ts）。
//! 桌面扩展配置（权限规则 / MCP / hooks）放 ~/.mcode/desktop.json，不污染 CLI 互操作面。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub use crate::permission::Mode;

// ---------- CLI 互通 schema（对齐 McodeConfig，逐字段一致） ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct Provider {
    pub name: String,
    /// CLI schema 为 `baseURL`（大写 URL）——camelCase 会产出 baseUrl 导致字段静默丢失，
    /// 必须显式 rename 对齐（曾经因此把用户 config 的 baseURL 丢成空串）
    #[serde(rename = "baseURL")]
    pub base_url: String,
    /// 环境变量名；若不是合法变量名则按明文 key 兜底（对齐 CLI resolveApiKey）
    pub api_key_env: String,
    pub models: Vec<String>,
    /// 接受图片输入的模型名单；不在名单内的模型拒绝图片附件并降级文本
    pub multimodal_models: Vec<String>,
}

impl Provider {
    /// 解析 API key：优先环境变量，非合法变量名则按明文兜底（对齐 config.ts resolveApiKey）
    pub fn resolve_api_key(&self) -> Option<String> {
        let v = self.api_key_env.as_str();
        if v.is_empty() {
            return None;
        }
        if let Ok(from_env) = std::env::var(v)
            && !from_env.is_empty()
        {
            return Some(from_env);
        }
        let is_env_name = {
            let mut cs = v.chars();
            match cs.next() {
                Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                    cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
                }
                _ => false,
            }
        };
        if !is_env_name {
            Some(v.to_string())
        } else {
            None
        }
    }

    pub fn is_multimodal(&self, model: &str) -> bool {
        self.multimodal_models.iter().any(|m| m == model)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum Language {
    #[serde(rename = "zh_CN")]
    #[default]
    ZhCn,
    #[serde(rename = "zh_TW")]
    ZhTw,
    #[serde(rename = "en_US")]
    EnUs,
    #[serde(rename = "ja_JP")]
    JaJp,
    #[serde(rename = "ko_KR")]
    KoKr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// PUA 收敛纪律指令每个会话常驻（/pua:on）
    pub pua_always_on: bool,
    pub language: Language,
    pub active_provider: Option<String>,
    /// 当前 provider 内选中的模型
    pub active_model: Option<String>,
    pub providers: Vec<Provider>,
    pub default_mode: Mode,
    /// 会话保留天数：超过 N 天未活动的会话启动时自动删除（管生也管死）
    pub session_retention_days: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pua_always_on: false,
            language: Language::default(),
            active_provider: None,
            active_model: None,
            providers: vec![],
            default_mode: Mode::Passon,
            session_retention_days: 90,
        }
    }
}

impl Config {
    /// 读取 config.json；文件不存在返回默认骨架（首次启动 → 引导流程）
    pub fn load() -> Self {
        Self::load_from(&crate::util::config_path())
    }

    /// 文件存在但解析失败 → Err（带路径与错误上下文）；文件不存在 → Ok(默认骨架)。
    /// CLI 主流程应使用本方法把损坏配置暴露给用户，而不是静默回退后把用户的 config.json 覆盖掉。
    pub fn try_load_from(path: &Path) -> Result<Self> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(anyhow::anyhow!("读取 {} 失败: {e}", path.display())),
        };
        serde_json::from_slice(&bytes).with_context(|| {
            format!(
                "解析 {} 失败（文件存在但不是合法的 mcode 配置）",
                path.display()
            )
        })
    }

    pub fn load_from(path: &Path) -> Self {
        Self::try_load_from(path).unwrap_or_else(|e| {
            eprintln!("mcode: {e:#}；已回退默认配置（原文件未改动，请修复后重启）");
            Self::default()
        })
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&crate::util::config_path())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        // 原子写：先写同目录临时文件再 rename，进程被杀/磁盘满不会留下半个 config.json
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn active_provider(&self) -> Option<&Provider> {
        let name = self.active_provider.as_ref()?;
        self.providers.iter().find(|p| &p.name == name)
    }

    /// 当前激活模型名（对齐 resolveActiveModel：activeModel 优先，回落 provider 首个）
    pub fn active_model(&self) -> Option<String> {
        if let Some(m) = self.active_model.as_deref()
            && !m.is_empty()
        {
            return Some(m.to_string());
        }
        self.active_provider()
            .and_then(|p| p.models.first().cloned())
    }

    /// 首次启动判定：文件不存在或 providers 为空（对齐 CLI 强制检查）
    pub fn needs_onboarding(&self) -> bool {
        self.providers.is_empty()
    }

    /// 供应商就绪检查（每回合评估，对齐 providerIssue：永不缓存）
    pub fn provider_ready(&self) -> Result<()> {
        let Some(p) = self.active_provider() else {
            anyhow::bail!("未配置模型供应商，请先完成引导（设置 → 供应商）");
        };
        if p.resolve_api_key().is_none() {
            anyhow::bail!(
                "供应商 {} 的 API key 不可用：环境变量 {} 未设置且值不是明文 key",
                p.name,
                p.api_key_env
            );
        }
        if self.active_model().is_none() {
            anyhow::bail!("未选择模型");
        }
        Ok(())
    }

    // ---------- Provider CRUD（设置页 / CLI 子命令共用的原语） ----------

    /// 新增 provider：重名拒绝、至少一个模型；providers 为空时自动激活并选中首个模型
    /// （对齐 CLI add_provider 子命令行为）
    pub fn add_provider(&mut self, p: Provider) -> Result<()> {
        anyhow::ensure!(!p.name.trim().is_empty(), "供应商名称不能为空");
        anyhow::ensure!(!p.models.is_empty(), "供应商 {} 至少需要一个模型", p.name);
        anyhow::ensure!(
            !self.providers.iter().any(|x| x.name == p.name),
            "供应商 {} 已存在",
            p.name
        );
        let is_first = self.providers.is_empty();
        self.providers.push(p);
        if is_first {
            self.active_provider = self.providers.last().map(|x| x.name.clone());
            self.active_model = self
                .providers
                .last()
                .and_then(|x| x.models.first().cloned());
        }
        Ok(())
    }

    /// 按 name 整体替换 provider（不存在报错）；active 状态不受影响
    pub fn update_provider(&mut self, p: Provider) -> Result<()> {
        anyhow::ensure!(!p.models.is_empty(), "供应商 {} 至少需要一个模型", p.name);
        let entry = self
            .providers
            .iter_mut()
            .find(|x| x.name == p.name)
            .with_context(|| format!("供应商 {} 不存在", p.name))?;
        *entry = p;
        Ok(())
    }

    /// 删除 provider；删除的是 active 时回落到剩余首个（无剩余则清空激活态）
    pub fn remove_provider(&mut self, name: &str) -> Result<()> {
        anyhow::ensure!(
            self.providers.iter().any(|x| x.name == name),
            "供应商 {name} 不存在"
        );
        self.providers.retain(|x| x.name != name);
        if self.active_provider.as_deref() == Some(name) {
            self.active_provider = self.providers.first().map(|x| x.name.clone());
            self.active_model = self
                .providers
                .first()
                .and_then(|x| x.models.first().cloned());
        }
        Ok(())
    }

    /// 切换激活 provider；active_model 重置为该 provider 首个模型（对齐 CLI switch）
    pub fn set_active_provider(&mut self, name: &str) -> Result<()> {
        let first = self
            .providers
            .iter()
            .find(|x| x.name == name)
            .with_context(|| format!("供应商 {name} 不存在"))?
            .models
            .first()
            .cloned();
        self.active_provider = Some(name.to_string());
        self.active_model = first;
        Ok(())
    }
}

// ---------- 桌面扩展配置（~/.mcode/desktop.json，CLI 不读取） ----------

/// 思考深度（对标 Claude Code /effort；OpenAI Responses API reasoning.effort 语义）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingEffort {
    /// 不发送 reasoning 参数（请求形态与 CLI 完全一致，兼容不支持 reasoning 的供应商）
    #[default]
    Default,
    Low,
    Medium,
    High,
}

impl ThinkingEffort {
    pub fn as_str(&self) -> &'static str {
        match self {
            ThinkingEffort::Default => "default",
            ThinkingEffort::Low => "low",
            ThinkingEffort::Medium => "medium",
            ThinkingEffort::High => "high",
        }
    }

    /// 解析 /effort 指令参数（大小写不敏感）
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "default" => Some(ThinkingEffort::Default),
            "low" => Some(ThinkingEffort::Low),
            "medium" => Some(ThinkingEffort::Medium),
            "high" => Some(ThinkingEffort::High),
            _ => None,
        }
    }

    /// 请求体 reasoning.effort 值（Default → None，不发参数）
    pub fn request_value(&self) -> Option<&'static str> {
        match self {
            ThinkingEffort::Default => None,
            ThinkingEffort::Low => Some("low"),
            ThinkingEffort::Medium => Some("medium"),
            ThinkingEffort::High => Some("high"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct DesktopConfig {
    /// 思考深度（"default"/"low"/"medium"/"high"，desktop.json 持久化，CLI 无此概念）
    #[serde(default)]
    pub thinking_effort: String,
    /// 权限规则（allow/deny，/permissions 面板数据源）
    pub permission_rules: Vec<PermissionRule>,
    /// MCP server 进程配置（stdio transport）
    pub mcp_servers: Vec<McpServerConfig>,
    /// hooks 生命周期配置
    pub hooks: Vec<HookConfig>,
    /// 最近打开的工作区（桌面端启动选择器数据源，最近在前，上限 8）
    #[serde(default)]
    pub recent_workspaces: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRule {
    pub tool: String,
    /// 子串匹配模式，如 `bash(rm *)` 或 `edit_file(src/**)`
    pub pattern: String,
    pub allow: bool,
}

/// MCP stdio server：进程命令 + 参数 + 环境注入
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// 是否随应用启动自动连接
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// hook 规则：事件 + 工具匹配器 + shell 命令
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookConfig {
    pub event: HookEvent,
    /// 工具名匹配（`*` 全部，否则子串匹配）
    pub matcher: String,
    pub command: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    Stop,
}

impl DesktopConfig {
    /// 解析后的思考深度（空串/非法值 → Default）
    pub fn thinking_effort(&self) -> ThinkingEffort {
        ThinkingEffort::parse(&self.thinking_effort).unwrap_or_default()
    }

    pub fn load() -> Self {
        Self::load_from(&crate::util::desktop_config_path())
    }

    /// 同 `Config::try_load_from`：存在但损坏 → Err；不存在 → Ok(默认)
    pub fn try_load_from(path: &Path) -> Result<Self> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(anyhow::anyhow!("读取 {} 失败: {e}", path.display())),
        };
        serde_json::from_slice(&bytes)
            .with_context(|| format!("解析 {} 失败（文件存在但不是合法的配置）", path.display()))
    }

    pub fn load_from(path: &Path) -> Self {
        Self::try_load_from(path).unwrap_or_else(|e| {
            eprintln!("mcode: {e:#}；已回退默认配置（原文件未改动，请修复后重启）");
            Self::default()
        })
    }

    pub fn save(&self) -> Result<()> {
        let path = crate::util::desktop_config_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        // 原子写：同 config.json（temp + rename）
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    // ---------- MCP server CRUD（拓展页 / CLI /mcp 子命令共用原语） ----------

    /// 按 name 幂等 upsert：已存在则整体替换，不存在则追加
    pub fn upsert_mcp_server(&mut self, s: McpServerConfig) {
        match self.mcp_servers.iter_mut().find(|x| x.name == s.name) {
            Some(slot) => *slot = s,
            None => self.mcp_servers.push(s),
        }
    }

    /// 删除 MCP server；返回是否确有删除
    pub fn remove_mcp_server(&mut self, name: &str) -> bool {
        let before = self.mcp_servers.len();
        self.mcp_servers.retain(|x| x.name != name);
        self.mcp_servers.len() != before
    }

    /// 启用/停用 server（停用后启动不再自动连接）；不存在报错
    pub fn set_mcp_enabled(&mut self, name: &str, enabled: bool) -> Result<()> {
        let slot = self
            .mcp_servers
            .iter_mut()
            .find(|x| x.name == name)
            .with_context(|| format!("MCP server {name} 不存在"))?;
        slot.enabled = enabled;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_cli_config() {
        // 对齐本机真实 ~/.mcode/config.json 的字段形态
        let raw = r#"{
            "language": "zh_CN",
            "activeProvider": "glm",
            "activeModel": "glm-5.3-flash",
            "providers": [{
                "name": "glm",
                "baseURL": "https://open.bigmodel.cn/api/v1",
                "apiKeyEnv": "MY_KEY_ENV",
                "models": ["glm-5.3-flash"],
                "multimodalModels": []
            }],
            "defaultMode": "passon",
            "puaAlwaysOn": true
        }"#;
        let cfg: Config = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.active_provider.as_deref(), Some("glm"));
        assert_eq!(cfg.active_model().as_deref(), Some("glm-5.3-flash"));
        assert!(cfg.pua_always_on);
        assert_eq!(cfg.providers[0].models, vec!["glm-5.3-flash".to_string()]);
        // 序列化回 CLI 可读的 camelCase
        let out = serde_json::to_string(&cfg).unwrap();
        assert!(out.contains("\"activeProvider\""));
        assert!(out.contains("\"puaAlwaysOn\""));
        assert!(out.contains("\"multimodalModels\""));
    }

    #[test]
    fn api_key_env_then_literal_fallback() {
        let mut p = Provider {
            name: "t".into(),
            base_url: "http://x".into(),
            api_key_env: "MCODE_TEST_KEY_ENV".into(),
            models: vec![],
            multimodal_models: vec![],
        };
        // SAFETY: 测试进程单线程设置环境变量，无并发读取
        unsafe { std::env::set_var("MCODE_TEST_KEY_ENV", "from-env") };
        assert_eq!(p.resolve_api_key().as_deref(), Some("from-env"));
        // SAFETY: 同上
        unsafe { std::env::remove_var("MCODE_TEST_KEY_ENV") };
        // 合法变量名但未设置 → None
        assert_eq!(p.resolve_api_key(), None);
        // 非合法变量名 → 明文兜底
        p.api_key_env = "sk-literal-key".into();
        assert_eq!(p.resolve_api_key().as_deref(), Some("sk-literal-key"));
    }

    #[test]
    fn base_url_key_matches_cli_schema_exactly() {
        // 回归：camelCase 产出 baseUrl 与 CLI 的 baseURL 错位，读丢 → 写坏用户 config
        let raw = r#"{"name":"glm","baseURL":"https://open.bigmodel.cn/api/v1","apiKeyEnv":"K","models":["m"]}"#;
        let p: Provider = serde_json::from_str(raw).unwrap();
        assert_eq!(p.base_url, "https://open.bigmodel.cn/api/v1");
        let out = serde_json::to_string(&p).unwrap();
        assert!(
            out.contains("\"baseURL\":\"https://"),
            "序列化必须产出 baseURL 键: {out}"
        );
    }

    #[test]
    fn corrupt_config_surfaces_error_instead_of_silent_fallback() {
        // 回归：损坏的 config.json 曾被静默吞成默认值，下次 save 会把用户配置覆盖掉
        let dir = std::env::temp_dir().join(format!("mcode-cfg-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.json");
        std::fs::write(&p, "{ not valid json !!!").unwrap();

        assert!(Config::try_load_from(&p).is_err(), "存在但损坏 → 必须 Err");
        // 文件不存在 → Ok(默认骨架)，首次启动引导流程不受影响
        let missing = dir.join("does-not-exist.json");
        let cfg = Config::try_load_from(&missing).unwrap();
        assert!(cfg.providers.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn desktop_config_thinking_effort_roundtrip() {
        let dir = std::env::temp_dir().join(format!("mcode-cfg-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("desktop.json");
        let d = DesktopConfig {
            thinking_effort: "high".into(),
            recent_workspaces: vec!["/tmp/x".into()],
            ..Default::default()
        };
        std::fs::write(&p, serde_json::to_string_pretty(&d).unwrap()).unwrap();
        let loaded = DesktopConfig::load_from(&p);
        assert_eq!(loaded.thinking_effort, "high");
        assert_eq!(loaded.thinking_effort(), ThinkingEffort::High);
        assert_eq!(loaded.recent_workspaces, vec!["/tmp/x".to_string()]);
        // 旧版 desktop.json 无该字段 → Default（向后兼容）
        std::fs::write(&p, r#"{"permissionRules":[]}"#).unwrap();
        let loaded = DesktopConfig::load_from(&p);
        assert_eq!(loaded.thinking_effort(), ThinkingEffort::Default);
        // 非法值 → Default（fail-safe）
        std::fs::write(&p, r#"{"thinkingEffort":"turbo"}"#).unwrap();
        assert_eq!(
            DesktopConfig::load_from(&p).thinking_effort(),
            ThinkingEffort::Default
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn onboarding_detection() {
        assert!(Config::default().needs_onboarding());
        let mut c = Config::default();
        c.providers.push(Provider::default());
        assert!(!c.needs_onboarding());
    }

    fn prov(name: &str, models: &[&str]) -> Provider {
        Provider {
            name: name.into(),
            base_url: format!("https://{name}.example.com/v1"),
            api_key_env: format!("{name}_KEY").to_uppercase(),
            models: models.iter().map(|s| s.to_string()).collect(),
            multimodal_models: vec![],
        }
    }

    #[test]
    fn provider_crud_full_lifecycle() {
        let mut c = Config::default();
        // 空 providers → 新增首个自动激活
        c.add_provider(prov("glm", &["glm-1", "glm-2"])).unwrap();
        assert_eq!(c.active_provider.as_deref(), Some("glm"));
        assert_eq!(c.active_model().as_deref(), Some("glm-1"));

        // 重名拒绝
        assert!(c.add_provider(prov("glm", &["x"])).is_err());
        // 空名拒绝
        assert!(c.add_provider(prov("  ", &["x"])).is_err());

        // 第二个 provider 不抢占 active
        c.add_provider(prov("minimax", &["MiniMax-M3"])).unwrap();
        assert_eq!(c.active_provider.as_deref(), Some("glm"));

        // update：整体替换且 active 保持
        let mut m = prov("minimax", &["MiniMax-M3", "abab6"]);
        m.base_url = "https://api.minimaxi.com/anthropic".into();
        c.update_provider(m).unwrap();
        assert_eq!(c.active_provider.as_deref(), Some("glm"));
        assert_eq!(
            c.providers
                .iter()
                .find(|p| p.name == "minimax")
                .unwrap()
                .base_url,
            "https://api.minimaxi.com/anthropic"
        );
        // update 不存在的
        assert!(c.update_provider(prov("ghost", &["g"])).is_err());

        // 切换 → active_model 重置为首模型
        c.set_active_provider("minimax").unwrap();
        assert_eq!(c.active_provider.as_deref(), Some("minimax"));
        assert_eq!(c.active_model().as_deref(), Some("MiniMax-M3"));

        // 删 active → 回落到剩余首个 + 首模型
        c.remove_provider("minimax").unwrap();
        assert_eq!(c.active_provider.as_deref(), Some("glm"));
        assert_eq!(c.active_model().as_deref(), Some("glm-1"));

        // 删不存在的报错；删到只剩空 → active 清空
        assert!(c.remove_provider("ghost").is_err());
        c.remove_provider("glm").unwrap();
        assert!(c.active_provider.is_none() && c.providers.is_empty());
    }

    #[test]
    fn save_to_is_atomic_no_tmp_left_behind() {
        let dir = std::env::temp_dir().join(format!("mcode-cfg-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let mut c = Config::default();
        c.add_provider(prov("glm", &["glm-1"])).unwrap();
        c.save_to(&path).unwrap();
        // 无 .tmp 残留
        let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(entries.len(), 1, "目录应只剩 config.json: {entries:?}");
        // roundtrip 等值
        let back = Config::try_load_from(&path).unwrap();
        assert_eq!(back.active_provider.as_deref(), Some("glm"));
        assert_eq!(back.providers.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mcp_server_crud() {
        let mut d = DesktopConfig::default();
        let s0 = McpServerConfig {
            name: "fs".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "@modelcontextprotocol/server-fs".into()],
            env: Default::default(),
            enabled: true,
        };
        d.upsert_mcp_server(s0.clone());
        // upsert 幂等覆盖
        let mut s1 = s0.clone();
        s1.command = "node".into();
        d.upsert_mcp_server(s1);
        assert_eq!(d.mcp_servers.len(), 1);
        assert_eq!(d.mcp_servers[0].command, "node");

        // enable/disable + 不存在报错
        d.set_mcp_enabled("fs", false).unwrap();
        assert!(!d.mcp_servers[0].enabled);
        assert!(d.set_mcp_enabled("ghost", true).is_err());

        d.upsert_mcp_server(McpServerConfig {
            name: "git".into(),
            command: "uvx".into(),
            args: vec![],
            env: Default::default(),
            enabled: false,
        });
        d.set_mcp_enabled("git", true).unwrap();
        // remove
        assert!(d.remove_mcp_server("fs"));
        assert!(!d.remove_mcp_server("fs"), "重复删除返回 false");
        assert_eq!(d.mcp_servers.len(), 1);
    }
}

#[cfg(test)]
mod provider_ready_tests {
    use super::*;

    /// 回归：Config 是 rename_all = "camelCase" —— snake_case 键（active_provider）
    /// 会被静默忽略并落到 default None，provider 看似配置实则从未生效。
    /// 用 camelCase 键写盘 → 解析 → provider_ready 必须通过。
    #[test]
    fn camel_case_config_keys_are_required() {
        let dir = std::env::temp_dir().join(format!("mcode_cfg_cam_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        std::fs::write(
            &path,
            r#"{
  "puaAlwaysOn": false,
  "language": "zh_CN",
  "activeProvider": "mock",
  "activeModel": "mock-1",
  "providers": [
    {
      "name": "mock",
      "baseURL": "http://127.0.0.1:9/v1",
      "apiKeyEnv": "MCODE_TEST_CFG_KEY",
      "models": ["mock-1"],
      "multimodalModels": []
    }
  ],
  "defaultMode": "passon"
}"#,
        )
        .unwrap();
        let cfg = Config::try_load_from(&path).expect("camelCase config 应解析成功");
        assert_eq!(cfg.active_provider.as_deref(), Some("mock"));
        // SAFETY: 单线程测试设置一次性 env
        unsafe { std::env::set_var("MCODE_TEST_CFG_KEY", "k") };
        match cfg.provider_ready() {
            Ok(()) => {}
            Err(e) => panic!("provider_ready 应通过，实际：{e:#}"),
        }

        // 反例：同样的内容用 snake_case 键 → active_provider 被吞成 None
        let bad = dir.join("config_snake.json");
        std::fs::write(
            &bad,
            r#"{"pua_always_on": false, "language": "zh_CN", "active_provider": "mock",
  "active_model": "mock-1",
  "providers": [{"name": "mock", "baseURL": "http://127.0.0.1:9/v1",
    "api_key_env": "MCODE_TEST_CFG_KEY", "models": ["mock-1"], "multimodal_models": []}],
  "default_mode": "passon"}"#,
        )
        .unwrap();
        let snake = Config::try_load_from(&bad).unwrap();
        assert!(
            snake.active_provider.is_none(),
            "snake_case 键应被静默忽略（这正是要防的坑）"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
