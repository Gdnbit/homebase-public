---
title: Extension 开发指南
ssot: C:\Users\homebase\Desktop\本机操作指南\system\homebase-design.md
updated: 2026-05-07
---

# Extension 开发指南

> ⚠️ **HomeBase 内部用 "Extension" 不是 "Plugin"**（v3 §23 改名清单）。
>
> 业界 "Plugin" 已被 VSCode/Obsidian/Tauri/Claude Code 占用 — 我们用 Extension 避免歧义。

> ⚠️ **当前状态（2026-05-07）：Extension 机制未启用**。`extensions/_builtin/` 目录仍为空 · `commands::extensions::extensions_list` 为 stub。
>
> 跨 agent 共享能力在 2026-05-06/07 走了另一条路· 文件化共享层 `~/.homebase/shared/`（详 `ROADMAP.md` P0+S1-S5 节）· 而非 Extension 包。Extension 机制会留到后期需要开放第三方扩展时再启用。

## SSOT

详细规范见：

- [`@C:\Users\homebase\Desktop\本机操作指南\system\homebase-design.md` §9](C:\Users\homebase\Desktop\本机操作指南\system\homebase-design.md) — Extension 机制（v2 主体）
- [`@C:\Users\homebase\Desktop\本机操作指南\system\homebase-design.md` §23](C:\Users\homebase\Desktop\本机操作指南\system\homebase-design.md) — Plugin → Extension 改名清单 + 实施 checklist

## Extension Manifest（TOML）

```toml
# extensions/agents-openclaw/extension.toml
[extension]
id = "agents-openclaw"
name = "OpenClaw Agent Panel"
version = "1.0.0"
kind = "module"           # module | action | integration
icon = "robot"
route = "/agents/openclaw"

[entry]
ui = "ui/OpenClawPanel.tsx"
backend = "backend/openclaw.rs"  # 可选

[permissions]
shell = ["openclaw"]
fs = ["C:\\HomeBase\\Workspace\\Agents\\OpenClaw\\workspace\\**"]
network = []

[depends]
homebase = ">=0.1.0"
```

## Phase 13.2 原计划· 未启动

按 v3 §9.3 内置 Extension（MVP）：dashboard / agents-claude-code / agents-codex / agents-openclaw / agents-gemini / agents-hermes / secrets / knowledge / system / tasks / skills

实际交付走了一条更轻量的路：都是作为主仓库下 `routes/*.tsx` 顶级路由· 不进独立 manifest 加载。Extension 机制 「未来开放第三方扩展」 方启用。

## 三者明确边界

| 术语 | 定义 | 在 HomeBase 中 |
|---|---|---|
| **Skill** | Agent prompt + script 资产 | · 双表示（v2 stub `commands/skills.rs` + S4 `commands/shared_skills.rs` · 后者为现走路径） |
| **Plugin** | 业界统称（VSCode/Obsidian/Tauri/Claude） | **不在 HomeBase 内部使用** |
| **Extension** | HomeBase UI 模块单元 | `extensions/_builtin/` + `extension.toml` · **未启用** |

## 如果你现在要加一个“扩展”

在 Extension 机制启用前· 推荐路径：

1. **顶级路由**：在 `src/routes/` 加一个 `XxxRoute.tsx` + `src/components/layout/Sidebar.tsx` 加路由项。
2. **后端命令**：在 `src-tauri/src/commands/` 加一个 `xxx.rs` · 在 `lib.rs::invoke_handler` 里注册。
3. **跨 agent 共享能力**：考虑在 `~/.homebase/shared/` 下加一个文件，类似 S1-S5。
4. **调外部 CLI**：遵循 adapter 层· 在 `src-tauri/src/adapters/` 加一个 adapter · 走 `spawn_with_timeout` / `run_checked`。

不建议现阶段在 `extensions/_builtin/` 下叠加什么· 则会与尚未实现的 manifest loader 冲突。
