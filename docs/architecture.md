---
title: HomeBase 架构（指向 SSOT + 实施现状）
ssot: C:\Users\homebase\Desktop\本机操作指南\system\homebase-design.md
updated: 2026-05-07
---

# HomeBase 架构

> 设计原意见 SSOT：[`homebase-design.md`](C:\Users\homebase\Desktop\本机操作指南\system\homebase-design.md)。本文件**只描述已落地的组件结构** · 不替代设计文档。

## SSOT 直达链接

- **§3 整体架构** — System Context + Container 图
- **§4 技术栈** — Tauri 2 + React 19 + SQLite 决策
- **§5 项目布局** — 目录结构（注意 v3 §23 改名 `plugins/` → `extensions/`）
- **§7 数据模型** — SQLite Schema（v2 主体 + v3 §21 增量）
- **§8 API 设计** — Tauri commands + events + 状态管理（具体清单见 [`api.md`](api.md)）
- **§10 整合契约** — 跟 SOPS / agent-env / git / PowerShell 的契约
- **§12 安全考虑** — Tauri 权限 / Secrets 处理 / 进程隔离
- **§18 Future-proofing** — 4 种 AgentDriver（CLI/Service/MCP/Skill）
- **§26-§28** — Phase 14.x / Phase B/C/D 实施增量

## 已实施的关键组件（2026-05-07）

### 4 个 Binary

| Binary | 用途 | 启动模式 |
|---|---|---|
| `homebase` | 主 Tauri 桌面应用 | USER 双击 · setup hook 启 IPC server |
| `homebase-reveal` | SOPS 单 key 查询 (Phase G) | Claude Code apiKeyHelper / Codex auth.command 拉 |
| `homebase-mcp-image` | Studio 生图 MCP server (Phase E2) | Claude / OpenClaw 作 MCP server 拉 |
| `homebase-mcp-secrets` | 共享 Secrets MCP server (S3) | 4 agent 作 MCP server 拉 · 反向 TCP 调主进程 |

### 跨 Agent 共享层（核心数据流）

```text
                                ~/.homebase/shared/
                                ├ context.toml         (S1)
                                ├ mcp-pool.json        (S2)  ──┐
                                ├ skills/<id>/SKILL.md (S4)  ──┤
                                ├ secrets-acl.json     (S3)    │ reconcile
                                └ .ipc-secrets.json    (S3)    │ (HomeBase 启动 / Apply 时)
                                       │                       │
                                       │ 4 路 reconcile         ▼
                                       ├──────────────────► ~/.claude/settings.json
                                       ├──────────────────► ~/.codex/config.toml (toml_edit 保留注释)
                                       ├──────────────────► ~/.gemini/settings.json
                                       └──────────────────► ~/.openclaw/settings.json
                                                            (含 mcpServers + skills/)
```

### Tauri 进程内服务

| 服务 | 启动时机 | 用途 |
|---|---|---|
| TCP IPC server | `lib.rs::setup` hook · 监听 `127.0.0.1:随机` | S3 · 4 agent 反向连 · token 鉴权 · 转发 consent |
| Tauri event emitter | 注入 `secrets_ipc::set_emitter` | S3 · 触发前端 `secrets:consent_request` |
| 流式 task pool | per-feature `XxxTaskMap` | gdnbit / studio / wiki_agent 流式 spawn 子进程 |

### 事件总线

| event | 推送方 | 监听方 |
|---|---|---|
| `secrets:consent_request` | `secrets_ipc.rs` | `SecretsConsentManager` (App.tsx 全局挂) |
| `gdnbit:event` | `gdnbit_*` streaming | WikiPublishPanel · 按 `task_id` 路由 |
| `studio:event` / `studio:image_progress` | `studio_chat_send` / `mcp-image binary` | Studio.tsx |
| `wiki_agent:event` | `wiki_agent_run_ingest` | Knowledge `WikiAgentSection` |

### 关键 adapter 表

| Adapter | 调用对象 | 主要业务 |
|---|---|---|
| `sops` | `C:\HomeBase\Tools\sops\bin\sops.exe` | `decrypt_file` / `decrypt_key` / `list_top_keys` (S3) |
| `git` | `git` (PATH) | `status` / `current_branch` / `log_recent` |
| `powershell` | `powershell.exe -NoProfile` | `run` / `run_json<T>` / `get_psdrives` |
| `agent_env` | `C:\HomeBase\Tools\bin\agent-env.ps1` | spawn agent 子进程 · stdout/stderr channel · kill |
| `mcp_pool` / `mcp_reconcile` | (内部) | S2 · pool 数据 + 4 路 reconcile (JSON+TOML) |
| `shared_context` | (内部) | S1 · context.toml CRUD |
| `skills_pool` | (内部) | S4 · 列共享 skill + 复制到 agent |
| `secrets_acl` / `secrets_store` / `secrets_ipc` | (内部) | S3 三件套 |

> 所有架构变更应先改 SSOT · 此处仅同步落地结果。
