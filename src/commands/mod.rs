// MCode_Desktop — AI coding agent desktop app
// Copyright (c) 2026 Materin-tech. All rights reserved.
// SPDX-License-Identifier: MIT

//! slash 指令全集 — 对齐 MCode.md 2.4（38 条，含 MCode 特有 /token /language /pua）

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandDef {
    pub name: &'static str,
    pub group: &'static str,
    pub desc: &'static str,
    /// MCode 特有指令
    pub mcode_only: bool,
}

pub const COMMANDS: &[CommandDef] = &[
    CommandDef {
        name: "/clear",
        group: "会话与上下文",
        desc: "清空上下文，重置对话",
        mcode_only: false,
    },
    CommandDef {
        name: "/compact",
        group: "会话与上下文",
        desc: "手动压缩历史（支持附加聚焦指令）",
        mcode_only: false,
    },
    CommandDef {
        name: "/context",
        group: "会话与上下文",
        desc: "查看上下文占用分布",
        mcode_only: false,
    },
    CommandDef {
        name: "/export",
        group: "会话与上下文",
        desc: "导出当前会话为文件或剪贴板",
        mcode_only: false,
    },
    CommandDef {
        name: "/resume",
        group: "会话与上下文",
        desc: "恢复历史会话",
        mcode_only: false,
    },
    CommandDef {
        name: "/rewind",
        group: "会话与上下文",
        desc: "回退到检查点（代码与对话）",
        mcode_only: false,
    },
    CommandDef {
        name: "/todos",
        group: "会话与上下文",
        desc: "查看/管理当前任务列表",
        mcode_only: false,
    },
    CommandDef {
        name: "/copy",
        group: "会话与上下文",
        desc: "复制最后一条回复",
        mcode_only: false,
    },
    CommandDef {
        name: "/save",
        group: "会话与上下文",
        desc: "导出 ECC save-session 格式快照",
        mcode_only: true,
    },
    CommandDef {
        name: "/config",
        group: "配置与状态",
        desc: "打开配置面板",
        mcode_only: false,
    },
    CommandDef {
        name: "/model",
        group: "配置与状态",
        desc: "交互式切换模型",
        mcode_only: false,
    },
    CommandDef {
        name: "/status",
        group: "配置与状态",
        desc: "展示版本、模型、cwd、权限模式",
        mcode_only: false,
    },
    CommandDef {
        name: "/cost",
        group: "配置与状态",
        desc: "token 与费用统计汇总",
        mcode_only: false,
    },
    CommandDef {
        name: "/token",
        group: "配置与状态",
        desc: "按模型打印 token 明细",
        mcode_only: true,
    },
    CommandDef {
        name: "/language",
        group: "配置与状态",
        desc: "切换语言环境",
        mcode_only: true,
    },
    CommandDef {
        name: "/effort",
        group: "配置与状态",
        desc: "思考深度 default/low/medium/high",
        mcode_only: true,
    },
    CommandDef {
        name: "/doctor",
        group: "配置与状态",
        desc: "诊断安装与配置健康度",
        mcode_only: false,
    },
    CommandDef {
        name: "/output-style",
        group: "配置与状态",
        desc: "切换输出风格",
        mcode_only: false,
    },
    CommandDef {
        name: "/statusline",
        group: "配置与状态",
        desc: "配置状态栏",
        mcode_only: false,
    },
    CommandDef {
        name: "/terminal-setup",
        group: "配置与状态",
        desc: "终端按键适配",
        mcode_only: false,
    },
    CommandDef {
        name: "/vim",
        group: "配置与状态",
        desc: "vim 编辑模式开关",
        mcode_only: false,
    },
    CommandDef {
        name: "/memory",
        group: "配置与状态",
        desc: "编辑记忆文件（MCODE.md / 规则文件）",
        mcode_only: false,
    },
    CommandDef {
        name: "/permissions",
        group: "权限与安全",
        desc: "权限规则管理（allow/deny 列表）",
        mcode_only: false,
    },
    CommandDef {
        name: "/add-dir",
        group: "权限与安全",
        desc: "追加工作目录",
        mcode_only: false,
    },
    CommandDef {
        name: "/agents",
        group: "扩展生态",
        desc: "subagent 管理与查看",
        mcode_only: false,
    },
    CommandDef {
        name: "/goal",
        group: "会话与上下文",
        desc: "设置目标并持续工作直到达成",
        mcode_only: true,
    },
    CommandDef {
        name: "/hooks",
        group: "扩展生态",
        desc: "hook 管理与查看",
        mcode_only: false,
    },
    CommandDef {
        name: "/mcp",
        group: "扩展生态",
        desc: "MCP server 管理与连接状态",
        mcode_only: false,
    },
    CommandDef {
        name: "/init",
        group: "扩展生态",
        desc: "扫描仓库生成 MCODE.md",
        mcode_only: false,
    },
    CommandDef {
        name: "/review",
        group: "扩展生态",
        desc: "代码评审",
        mcode_only: false,
    },
    CommandDef {
        name: "/pr-comments",
        group: "扩展生态",
        desc: "获取 PR 评论",
        mcode_only: false,
    },
    CommandDef {
        name: "/pua",
        group: "扩展生态",
        desc: "PUA 效率教练模式开关",
        mcode_only: true,
    },
    CommandDef {
        name: "/skills",
        group: "扩展生态",
        desc: "列出已加载技能",
        mcode_only: true,
    },
    CommandDef {
        name: "/skill",
        group: "扩展生态",
        desc: "技能创建/安装/移除：create|install|remove",
        mcode_only: true,
    },
    CommandDef {
        name: "/agent",
        group: "扩展生态",
        desc: "subagent 创建/安装/移除：create|install|remove",
        mcode_only: true,
    },
    CommandDef {
        name: "/provider",
        group: "配置与状态",
        desc: "模型供应商管理：add|list|use|remove",
        mcode_only: true,
    },
    CommandDef {
        name: "/history",
        group: "账号与帮助",
        desc: "诊断输入历史召回",
        mcode_only: true,
    },
    CommandDef {
        name: "/exit",
        group: "账号与帮助",
        desc: "退出 mcode",
        mcode_only: false,
    },
    CommandDef {
        name: "/help",
        group: "账号与帮助",
        desc: "帮助",
        mcode_only: false,
    },
    CommandDef {
        name: "/login",
        group: "账号与帮助",
        desc: "模型供应商配置",
        mcode_only: false,
    },
    CommandDef {
        name: "/logout",
        group: "账号与帮助",
        desc: "清除当前供应商",
        mcode_only: false,
    },
    CommandDef {
        name: "/bug",
        group: "账号与帮助",
        desc: "问题反馈",
        mcode_only: false,
    },
    CommandDef {
        name: "/release-notes",
        group: "账号与帮助",
        desc: "版本说明",
        mcode_only: false,
    },
    CommandDef {
        name: "/upgrade",
        group: "账号与帮助",
        desc: "检查更新",
        mcode_only: false,
    },
    CommandDef {
        name: "/ide",
        group: "账号与帮助",
        desc: "CC 专属（暂未支持）",
        mcode_only: false,
    },
    CommandDef {
        name: "/install-github-app",
        group: "账号与帮助",
        desc: "CC 专属（暂未支持）",
        mcode_only: false,
    },
];

/// `/` 补全过滤：对齐 CLI 补全面板行为（大小写不敏感、按 name 前缀/包含匹配）
pub fn filter_commands(query: &str) -> Vec<&'static CommandDef> {
    let q = query.to_lowercase();
    COMMANDS
        .iter()
        .filter(|c| c.name.to_lowercase().contains(&q))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_command_set_loaded() {
        assert!(
            COMMANDS.len() >= 38,
            "指令全集应 ≥ 38 条，实际 {}",
            COMMANDS.len()
        );
        assert!(COMMANDS.iter().any(|c| c.name == "/token"));
        assert!(COMMANDS.iter().any(|c| c.name == "/language"));
        assert!(COMMANDS.iter().any(|c| c.name == "/pua"));
    }

    #[test]
    fn filter_matches_cli_behavior() {
        let got = filter_commands("to");
        let names: Vec<_> = got.iter().map(|c| c.name).collect();
        assert!(names.contains(&"/todos"));
        assert!(names.contains(&"/token"));
        assert_eq!(filter_commands("不存在的指令").len(), 0);
    }
}
