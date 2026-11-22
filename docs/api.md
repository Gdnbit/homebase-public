---
title: HomeBase API（Tauri commands · 真实清单）
ssot: ../control/commands.json
updated: 2026-11-08
---

# HomeBase API

> 当前命令、参数类型和 CLI 可调用状态以 [生成的调用契约](../control/commands.json) 为准，来自实际 Tauri 注册表与函数签名。运行 `hb commands` / `hb describe <command>` 按需查询；下文历史模块表用于导航，不以旧计数判断完成度。AI 控制协议、权限及操作状态见 [AI / CLI 说明](ai-control.md)。

## 概览

| 模块 | 命令数 | 文件位置 |
|---|---|---|
| `secrets` | 6 | `commands/secrets.rs` |
| `agents` | 8 | `commands/agents.rs` |
| `agents_history` | 3 | `commands/agents_history.rs` |
| `system` | 5 | `commands/system.rs` |
| `docs` | 3 | `commands/docs.rs` |
| `wiki` | 4 | `commands/wiki.rs` |
| `topology` | 4 | `commands/topology.rs` |
| `inbox` | 4 | `commands/inbox.rs` |
| `wiki_agent` | 7 | `commands/wiki_agent.rs` |
| `gdnbit` | 10 | `commands/gdnbit.rs` |
| `studio` | 8 | `commands/studio.rs` |
| `shared_mcp` | 5 | `commands/shared_mcp.rs` |
| `shared_skills` | 3 | `commands/shared_skills.rs` |
| `shared_secrets` | 7 | `commands/shared_secrets.rs` |
| `agent_versions` | 1 | `commands/agent_versions.rs` |
| `openrouter` | 1 | `commands/openrouter.rs` |
| `asr` | 6 | `commands/asr.rs` |
| `model_center` | 6 | `commands/model_center.rs` |
| `projects` | 1 | `commands/projects.rs` |
| `extensions` / `tasks` / `skills` | 各 1 | 同名文件 |
| **合计** | **~80** | — |

## 详细清单

### 已有服务凭据引用（本机 DPAPI）

`service_credentials_status` 返回非敏感本机状态，CLI 需要 `--allow-private`，不含密钥、掩码或导出功能。状态 linked/not_configured/blocked 与 remote_state=not_checked 分别表示本机引用状态和远端未验证。后端可复用原 DPAPI 文件；已有 SOPS 条目及其策略优先，蚁小二保留原命名管道封装。设计、限制与测试见 [服务凭据引用](service-credential-references.md)。

### Secrets（SOPS 加密 · 11 keys 单源 `C:\HomeBase\Secrets\secrets.yaml`）

| 命令 | 作用 |
|---|---|
| `secrets_list` | 列已知 secret 名 + masked 值 |
| `secrets_test` | 一键 health check（HTTP probe）|
| `secrets_rotate_url` | 拉某 secret 对应的轮换/购买 URL |
| `secrets_reveal` | 解密单个 secret · 短暂内存 |
| `secrets_save` | 版本化保存密钥和元数据；支持 save / rotate / confirm_revocation / rollback_rotation |
| `secrets_lifecycle_status` | 只读取元数据返回到期策略和待停用轮换状态 |
| `secrets_test_candidate` | 用户主动调用固定服务接口测试候选密钥，不保存候选值 |
| `secrets_set` / `secrets_set_meta` | 兼容入口拒绝旧式写入，要求刷新到版本化客户端 |

### Agents（5 agent 状态 + 启停 + 模型路由）

| 命令 | 作用 |
|---|---|
| `agents_list` | 列内置 5 agent 元数据 |
| `agents_status` | 实测 sysinfo · 4 状态 (running / stopped / misconfigured / not_installed) |
| `agents_start` | 走 `agent-env.ps1 <name>` 启动 |
| `agents_open_config` | 用资源管理器打开 agent 配置目录 |
| `agents_get_model` / `set_model` | 8 alias 预设模型选择 |
| `agents_get_endpoint` / `set_endpoint` | OpenRouter / AnthropicDirect / Custom 端点配置 (Phase B) |
| `agents_test_endpoint` | 端点连通性测试 |

### Agents History（Phase D · 跨 agent 历史聚合）

| 命令 | 作用 |
|---|---|
| `agents_history_list` | 列单 agent session 元数据 |
| `agents_history_read` | 读单 session 全部消息 |
| `agents_history_all` | 跨 agent 聚合 + filter |

### System（磁盘 / CPU / 内存 / GPU / 整机信息）

| 命令 | 作用 |
|---|---|
| `system_disks` | `Get-PSDrive` 真实磁盘容量 |
| `system_info` | OS / 主机名 / 用户名 / **CPU 品牌 + 核心数 / 内存 GB / GPU 名 / 主板厂商型号** |
| `system_metrics` | CPU + RAM 实时利用率（5s 轮询）|
| `system_gpu` | GPU 利用率（10s 独立 · Win32 perf counter）|
| `system_preflight` | 启动自检（SOPS / secrets / agent-env 路径）|
| `system_path_list` | User / Machine PATH 条目与死链状态 |
| `model_center_manifest` | 只读解析 `installed-models.json`；返回 missing/invalid/loaded 及 probe/partial/installed/corrupt 模型状态 |

`system_info` 和 `system_gpu` 均返回 `gpu_status`（`detected` / `not_detected` / `unknown`）与 `gpu_message`。`gpu_name: null` 不再单独表达所有失败状态：成功查询但没有物理显卡为 `not_detected`；查询失败、超时或结果格式无效为 `unknown`。GPU 名称查询使用兼容 Windows PowerShell 5.1 的数组序列化；虚拟显示适配器不计入物理 GPU。实时利用率不可用时保持 `gpu_percent: null`，界面显示未知及原因，不显示为 0%。

`system_info.issues` 逐项说明缺失/查询失败的硬件或系统字段，包含 `field`、`status`、`message`；系统版本通过系统 API 读取，不固化为 Windows 11。`system_metrics` 遇到缺失/无效 CPU 或内存采样会报错，不返回伪造零值。`system_disks` 容量缺失、溢出或总容量为零时明确报错，不省略该盘后报告其余全部健康。进程清单采样失败不能判定为没有进程或代理停止。

任务列表、历史/文档、审计/快照和配置查询遇到不可读或损坏数据会报错；可选目录尚不存在、有效空文件/列表及业务未提供的可选元数据按各命令契约保留。共享 MCP/Skills 读取不会创建目录、保存默认配置或迁移原文件；持久化由显式保存/应用操作完成。生图进度查询保留损坏及过期文件：损坏记录报错，超过五分钟未更新的进度返回 unknown 和说明。Wiki 通用扫描跳过 raw/_raw 私人输入子树。

### Docs / Wiki / Topology（Knowledge 模块）

| 命令 | 作用 |
|---|---|
| `docs_list` | INDEX.md parse · 项目文档结构 |
| `docs_read` | 读 markdown 文件 |
| `obsidian_health` | Obsidian 4-state 健康检查 |
| `wiki_list` / `wiki_dirs` / `wiki_read` | C:\HomeBase\Wiki vault master-detail |
| `docs_open_in_typora` | 用 Typora 打开 markdown |
| `topology_load` / `list_dir` / `open_dir` / `parse_refs` | 拓扑图 + drive_groups |

### Inbox（投料口 · raw 层低门槛输入）

| 命令 | 作用 |
|---|---|
| `inbox_submit` | 提交一条 inbox · 写 `C:\HomeBase\Wiki\raw\inbox\<day>\<HHmmss-shortid>.md` |
| `inbox_attach` | 附加文件 · 拖拽 / 粘贴入口 |
| `inbox_open_obsidian` | 用 Obsidian 打开 inbox 目录 |
| `inbox_list_recent` | 最近 N 条 inbox 元数据 |

### Wiki Agent（Phase 14.5c · headless `claude -p` ingest）

| 命令 | 作用 |
|---|---|
| `wiki_agent_status` | 看是否在跑 / 哪个任务 |
| `wiki_agent_prepare_prompt` | 拼系统提示 + 用户 prompt |
| `wiki_agent_open_in_terminal` | 在新 cmd 跑 |
| `wiki_agent_get_config` / `set_config` | 模型 / max_turns / 输出风格等 |
| `wiki_agent_run_ingest` | 启动 ingest task · 6 种事件流 |
| `wiki_agent_cancel_ingest` | kill 子进程 |

### GDNbit（Phase C · 发布闭环）

| 命令 | 类型 | 作用 |
|---|---|---|
| `gdnbit_get_config` / `set_config` | sync | 读写 `~/.homebase/gdnbit.json` |
| `gdnbit_doctor` | sync | 本地 6 项检查 |
| `gdnbit_kit_doctor` | streaming | `publish.mjs doctor`（API 联通 / 鉴权）|
| `gdnbit_sync_vault` | streaming | `obsidian-sync.mjs`（dry-run / force / status / folder）|
| `gdnbit_publish_file` | streaming | 单文件 `publish.mjs preview\|publish` |
| `gdnbit_render_file` | one-shot | 单文件本地 HTML 预览 |
| `gdnbit_list_remote` | one-shot | 服务器内容列表 |
| `gdnbit_ai_optimize` | streaming | Claude CLI 按 STYLE_GUIDE 优化 |
| `gdnbit_cancel` | sync | kill 任意流式任务 |

### Studio（Phase D2.2 · 写作工坊）

| 命令 | 作用 |
|---|---|
| `studio_list_sessions` | 会话列表（自 `~/.homebase/studio/`） |
| `studio_load_session` | 读单 session 历史 |
| `studio_chat_send` | 流式 Claude CLI · `--allowedTools Read,Write,Edit` |
| `studio_persist_message` | 单条持久化 |
| `studio_restore_checkpoint` | 一键回滚（zip 快照恢复 _raw）|
| `studio_read_attachment_b64` | 读图片附件 base64 给前端预览 |
| `studio_list_image_progress` | 列生图进度（mcp-image binary 写）|
| `studio_cancel` | kill 会话 |

### 共享 MCP 池（S2）

| 命令 | 作用 |
|---|---|
| `shared_mcp_get` | 读 `~/.homebase/shared/mcp-pool.json` |
| `shared_mcp_save` | 写 pool 文件 · 不 reconcile |
| `shared_mcp_apply` | 不改 pool · 重新推送到 4 agent 配置文件 |
| `shared_mcp_save_and_apply` | 保存 + reconcile · 默认按钮走这个 |
| `shared_mcp_agent_ids` | 4 agent id 列表（前端列头用） |

### 共享 Skills 池（S4）

| 命令 | 作用 |
|---|---|
| `shared_skills_list` | 列 `~/.homebase/shared/skills/` 下 skill |
| `shared_skills_apply` | 复制到 Claude / OpenClaw 的 skills 目录 |
| `shared_skills_open_dir` | 资源管理器打开共享目录 |

### 共享 Secrets · Runtime 询问（S3）

| 命令 | 作用 |
|---|---|
| `shared_secrets_list_names` | secrets.yaml 顶层 key 名（不解密） |
| `shared_secrets_ipc_endpoint` | IPC server endpoint + token（前端展示用） |
| `shared_secrets_acl_list` | 列所有永久授权 + 矩阵 |
| `shared_secrets_acl_revoke` | 撤销某 (agent, secret) 永久允许 |
| `shared_secrets_consent_resolve` | 弹窗决定回传后端 · 释放 IPC pending |
| `shared_secrets_open_yaml` | 弹新 cmd 让 USER 用 SOPS 编辑 secrets.yaml |
| `shared_secrets_agent_ids` | 4 agent id 列表 |

### Agent Versions（S5）

| 命令 | 作用 |
|---|---|
| `agent_versions_check` | 5 agent 版本检测 · 1h 缓存 · npm latest + standalone fallback |

### OpenRouter Model Catalog

| Command | Purpose |
|---|---|
| `openrouter_models_catalog` | Fetch live OpenRouter model catalog, group by provider/brand, attach price labels, and sort each group by HomeBase capability heuristic. |

### ASR Control Plane

| Command | Purpose |
|---|---|
| `asr_environment_probe` | Resolve a preset or explicit engine combination and check source, output, runtime, model and secret presence without transcribing. |
| `asr_task_start` | Start the canonical runner as a durable background job and return job/task paths immediately. |
| `asr_task_get` | Read persisted job state plus current task metadata. |
| `asr_task_list` | List durable ASR jobs for an output root. |
| `asr_task_reconcile` | Read-only recovery observation; preserve persisted status, return runtime_state/runtime_message. Private scope, no write grant. |
| `asr_task_cancel` | Terminate the owned child, persist `cancelled`, and preserve existing artifacts. |

### Model Download Jobs

| Command | Purpose |
|---|---|
| `model_center_manifest` | Read the verified/partial installed-model manifest. |
| `model_download_start` | Start an explicit catalog download and verification job. |
| `model_download_get` | Read one persisted download job. |
| `model_download_list` | List download jobs under the configured jobs root. |
| `model_download_reconcile` | Read-only recovery observation; only owned task files regain monitoring, other active records are unconfirmed. Private scope, no write grant. |
| `model_download_cancel` | Request cancellation and stop the owned child without claiming installation. |

### 其他

| 命令 | 作用 |
|---|---|
| `projects_list` | git 项目枚举 |
| `extensions_list` | Extension 占位 · v3 §9 未启用 |
| `tasks_list` | Tasks 占位 · v3 §21.1 未启用 |
| `skills_list` | Skills 占位 · v3 §21.2 未启用（被 S4 部分覆盖）|

## 流式事件总线

部分命令通过 Tauri event 推送中间结果 · 前端用 `listen<T>(name, cb)` 订阅。

| event 名 | 推送方 | payload kind |
|---|---|---|
| `gdnbit:event` | `gdnbit_*` streaming | `started / stream / stderr / completed / failed / cancelled` |
| `studio:event` | `studio_chat_send` | 同上 |
| `studio:image_progress` | `homebase-mcp-image` binary | 生图进度 |
| `wiki_agent:event` | `wiki_agent_run_ingest` | 6 种 |
| `secrets:consent_request` | `secrets_ipc.rs` IPC server | `{ request_id, agent_id, secret_name, purpose }` |
| `asr-job` | `commands/asr.rs` | `started / progress / diagnostic / finished` |
| `model-download` | `commands/model_center.rs` | `started / progress / diagnostic / finished` |

按 `task_id` / `request_id` 区分。

## 前端调用规范（v3 §8.3）

- **Server state**（Tauri 命令结果）→ TanStack Query
- **UI state**（局部）→ `useState`
- **全局 UI state** → Zustand (`src/stores/app-store.ts`)
- **URL state** → React Router
- **类型镜像**：`src/lib/tauri-api.ts`（1400+ 行）按模块封装 `xxxApi` 对象 · 全部类型与 Rust struct 对齐
