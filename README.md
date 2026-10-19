# mcode-core

MCode 的核心库：从终端 CLI（mcode-cli）与桌面版（MCode_Desktop）两个消费方中沉淀出的独立内核。

## 能力

- **agent**：回合循环（25 轮工具上限）、流式 SSE、指数退避、80% 模型窗口自动压缩、goal 跨回合续跑、subagent
- **tools**：bash / read / write / edit / glob / grep / webfetch / websearch / skill / subagent，junk 目录过滤
- **config**：camelCase JSON schema（与 TS 版 config.json 互通），损坏配置显式报错
- **sessions**：JSONL 消息流 + meta + 跨项目列举 + resume + 90 天保留期自动清理
- **permission**：工具级权限裁决；**hooks**：PreToolUse/PostToolUse 等事件；**mcp**：MCP 服务器接入
- **skills / plugins**：技能发现与斜杠分派、插件资产物化

## 消费方

- [mcode-cli](https://github.com/materin-tech/mcode-cli) — 终端 TUI（ratatui），依赖本仓库 git tag
- MCode_Desktop — gpui 桌面版（迁移中）

## 用法

```toml
[dependencies]
mcode-core = { git = "https://github.com/materin-tech/mcode-core", tag = "v0.1.0-beta.6" }
```

本地联调（同时开发 core 与消费方时），在 `~/.cargo/config.toml` 加：

```toml
[patch."https://github.com/materin-tech/mcode-core"]
mcode-core = { path = "../mcode-core" }
```

## 开发

```bash
cargo build
cargo test
cargo fmt --check
cargo clippy -- -D warnings
```

发版：更新 `Cargo.toml` version → 提交 → 打 `v<version>` tag 并推送。

## License

MIT
