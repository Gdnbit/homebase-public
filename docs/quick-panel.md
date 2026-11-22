---
doc_id: homebase-quick-panel
title: HomeBase Quick Panel
role: HomeBase Quick Panel 当前实现、运行验证边界和后续规划
canonical_path: C:\HomeBase\source\docs\quick-panel.md
updated: 2026-07-19
authority: reference
status: current
volatility: periodic
sensitivity: private
review_after: 2026-08-19
route_contract: true
registered_in: C:\Users\homebase\Desktop\本机操作指南\INDEX.md
related:
  - doc_id: homebase-project
    path: C:\HomeBase\source\README.md
    rel: parent
    route_mode: current
    traversal: fallback
  - doc_id: homebase-asr-workbench-integration
    path: C:\HomeBase\source\docs\asr-workbench-integration.md
    rel: asr-workbench
    route_mode: review-candidate
    traversal: reference
  - doc_id: homebase-model-center-result-workspace
    path: C:\HomeBase\source\docs\model-center-result-workspace.md
    rel: model-center
    route_mode: review-candidate
    traversal: reference
  - doc_id: aihub-capabilities
    path: C:\HomeBase\AIHub\capabilities\README.md
    rel: capability-source
    route_mode: current
    traversal: follow
  - doc_id: aihub-prompts
    path: C:\HomeBase\AIHub\prompts\README.md
    rel: prompt-source
    route_mode: current
    traversal: follow
tags: [homebase, quick-panel, agent, capabilities, mcp, prompts]
---

# HomeBase Quick Panel

Quick Panel 当前是 HomeBase 主窗口中的 `/quick-panel` 页面；always-available 悬浮入口仍为 planned。

状态术语：

- `source-verified`：源码或配置已经接线。
- `runtime-unverified`：本轮没有执行端到端验证。
- `planned`：当前没有实现证据。
- `historical`：旧计划，只保留决策背景。

2026-07-19 静态锚点：`/quick-panel` 在 `src/App.tsx` 注册，侧边栏入口与
`src/routes/QuickPanel.tsx` 均存在。对话输入、通用 action、全局快捷键、tray、
上下文动态注入和 floating window 没有完整实现证据。

It should not become a separate product. HomeBase remains the desktop shell; Quick Panel is a lightweight conversational mode inside HomeBase.

## Planned Product Goal

Give the user a local, always-available AI assistant that understands HomeBase, AIHub, Wiki, local agents, local capabilities, and system state.

Examples:

- Ask it to inspect local agent status.
- Ask it to read one explicitly identified Claude/Codex history session only after the user states the
  session, purpose, allowed output, retention, and whether private content may leave the local machine.
- Ask it to route a recording to ASR and summarize the meeting.
- Ask it to start an Article Agent task for GDNbit.
- Ask it to explain what local capabilities are currently configured.

## Planned Agent Boundary

| Agent | Role | Scope |
|---|---|---|
| HomeBase Agent | Direct user entry | Understands and controls HomeBase, AIHub, local capabilities, agent status, history, Wiki, and system diagnostics. |
| Article Agent | Specialist | Generates and edits personal website articles, images, drafts, and publishing metadata. |

HomeBase Agent can call or route work to Article Agent, but Article Agent should not own HomeBase-wide decisions.

## Entry Status

| Entry | Status | Notes |
|---|---|---|
| HomeBase sidebar button / `/quick-panel` | source-verified | 当前主窗口内的实际入口 |
| Single press Fn | planned | Windows 设备不一定向用户态暴露 Fn |
| Global shortcut | planned | 当前依赖和 Tauri 配置未见全局快捷键接线 |
| Tray icon click | planned | 当前未见 tray 实现 |

以下 show/hide、位置/尺寸记忆和快速浮窗行为均为 planned，不是当前行为合同：

- First trigger shows Quick Panel.
- Second trigger hides Quick Panel.
- Panel should remember last position and size.
- Panel should open fast; no heavy model loading on startup.

## Shape Status

当前形态是普通 `main` Tauri 窗口中的页面。下表是 planned 产品方向：

| Requirement | Decision |
|---|---|
| App shape | Tauri floating window owned by HomeBase. |
| Window behavior | Always-on-top, frameless or minimal frame, translucent shadow, hide-on-blur optional. |
| Visual style | Light, quiet, Apple-like but lighter: restrained colors, simple icons, low density, no decorative heavy gradients. |
| First screen | Conversation console + HomeBase scope cards + quick capability shortcuts. |
| Output | Show concise answer, recommended action, and affected local paths. |

## Capability Sources (planned)

当前能力卡来自 `QuickPanel.tsx` 内的硬编码 `toolGroups`；没有证据表明页面会动态读取
下面这些 Registry/MCP/Prompt/Panel 路径。目标数据源为：

```text
C:\HomeBase\AIHub\capabilities\README.md
C:\HomeBase\AIHub\mcp\servers.d\*.json
C:\HomeBase\AIHub\prompts\**\*.md
C:\HomeBase\AIHub\panels\<capability>\
```

## API Model Catalog

Status: `source-verified / runtime-unverified`.

Quick Panel also exposes an OpenRouter model selector for cloud/API execution.

Reference:

```text
C:\HomeBase\source\docs\openrouter-model-catalog.md
```

Current behavior:

- Fetches live model data from OpenRouter instead of using a fixed local list.
- Groups models by provider/brand.
- Sorts each provider group by a HomeBase capability heuristic.
- Shows price labels from OpenRouter pricing fields.
- Lets the user manually refresh the catalog; the frontend treats data as fresh for 1 hour.

## Model Download Center

Status: `source-verified / runtime-unverified`. Built-in cards are currently hardcoded in the
frontend; the external manager and tests define the installed/smoke contract, but this review did not run them.

Quick Panel exposes a built-in model download center for ASR, OCR, and document parsing candidates.

Reference:

```text
C:\HomeBase\source\docs\model-center-result-workspace.md
C:\HomeBase\AIHub\capabilities\model-download-catalog.json
C:\HomeBase\AIHub\tools\model-manager\README.md
```

Current behavior:

- Shows built-in model cards with source, hardware fit, strengths, weak points, storage note, and invocation example.
- Computes a conservative runtime recommendation from HomeBase system APIs, the installed manifest, and `src\lib\model-recommender.ts`.
- Separates live facts from unknown/inferred values; browser preview never impersonates local hardware.
- Shows `recommended`, `api_preferred`, `needs_review`, `partial_install`, runtime/storage blockers, and explicit recovery actions.
- Chooses a preferred non-system model drive at runtime; task outputs still stay under `C:\HomeBase\Wiki\raw`.
- Exposes an expandable environment/migration panel with editable roots and a versioned JSON import/export profile; API keys remain Secrets references.
- Starts durable background downloads for supported Hugging Face entries, shows byte/file progress, supports cancellation, and reconciles interrupted jobs after restart.
- The intended contract marks a model `installed` only after snapshot completeness and a minimum inference smoke test; this review did not verify a completed install.
- Keeps manual installation instructions for entries whose upstream installer cannot yet be automated safely.
- Writes job state under the configured model job root and verified installation state to `C:\HomeBase\AIHub\models\installed-models.json`.

## Result Workspace

Quick Panel also exposes a result workspace for multi-engine outputs.

First version priority:

| Entry | Status | Purpose |
|---|---|---|
| Agent prompt copy | source-verified | Copy a task-folder-aware prompt for Codex/Claude Code to merge multi-model outputs. |
| Software viewing | source-verified | Show model/version lanes and load Markdown/TXT result files from `C:\HomeBase\Wiki` task folders. |
| API fusion | planned | 当前 UI 明确为 future/disabled；不得声称 canonical API fusion 已实现。 |

当前实际接收 ASR 任务；OCR 和文档解析复用同一 task-folder/`runs\<engine-id>`
合同仍是 planned。

## ASR Workbench (source-verified, runtime-unverified)

Quick Panel 源码中已接线文件选择、preflight、任务启动/轮询/事件/取消和输出路径约束；
本轮未执行 ASR 端到端测试。“最终人用入口”仍是产品目标。

Reference:

```text
C:\HomeBase\source\docs\asr-multi-engine-design.md
C:\HomeBase\source\docs\asr-workbench-integration.md
C:\HomeBase\AIHub\capabilities\asr-presets.json
```

Display rule:

- Show a short preset name.
- Add a compact trait label, such as `已装可跑`, `本地+云端`, `低延迟`.
- Put implementation detail in muted small text.
- Keep internal engine combinations hidden from the primary user flow, but visible enough for technical review.
- Show the output package path before running, because recordings, transcripts, compare files, and final notes must stay organized under `C:\HomeBase\Wiki\raw\audio`.
- Allow custom engine combinations to be used immediately or named and saved; migration export includes combination definitions but never secret values.
- Run real preflight immediately before start. Static hardware recommendations guide selection but never override missing models, runtimes, permissions, keys, or provider health.
- Do not surface the 44/46 recordings as formal regression tests yet.
- Do not implement automatic typo/glossary replacement yet; that workflow is deferred until the correction policy is decided.

## Capability Status Matrix

| Module | Current Quick Panel status | Boundary |
|---|---|---|
| Conversation input / HomeBase Agent action routing | planned | 当前是 UI 壳，没有通用 send/action handler |
| Agent/History/Studio/Wiki/System cards | display-only | 相关能力可能存在于其他页面或后端，不等于卡片已接线 |
| OpenRouter catalog | source-verified / runtime-unverified | 当前 provider、quota 和网络需实时探测 |
| Model Download Center | source-verified / runtime-unverified | 本轮未下载或运行 smoke inference |
| ASR Workbench / ASR MCP | source-verified / runtime-unverified | 实现存在；当前会话 MCP 可用性另行确认 |
| Result prompt copy / software viewing | source-verified | 当前可复制提示和读取结果文件 |
| API fusion | planned | 当前 UI disabled |
| Global shortcut / tray / floating window | planned | 当前没有实现证据 |

## Original Implementation Plan (historical planning model)

The rows below do not encode completion state. Use the current route/component/backend code and
validation results before claiming any phase is complete.

| Phase | Scope | Validation |
|---|---|---|
| Q0 Interface | Render HomeBase Agent conversation page and scope cards. | Build + screenshot/manual check. |
| Q1 Window | Add global shortcut + tray fallback + floating conversation window. | Toggle show/hide reliably. |
| Q2 Context | Load HomeBase/AIHub/Wiki capability context into the agent prompt. | Agent can explain configured capabilities. |
| Q3 Actions | Wire safe read-only actions: agent status, history list, wiki lookup. | Commands return structured local data. |
| Q4 Tools | Add write/action tools with consent: ASR, Article Agent, Wiki ingest. | Consent + output path validation. |

## Risks

- Fn key may not be capturable. Do not block the feature on Fn.
- 模型、缓存和生成物落盘前实时检查空间，并遵循当前 disk-plan；不要从旧容量记录推断。
- Quick Panel must not bypass Secrets consent rules.
- Capability registry must stay authoritative; avoid hardcoding every tile in React.
