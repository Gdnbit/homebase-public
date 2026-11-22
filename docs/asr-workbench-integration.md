---
doc_id: homebase-asr-workbench-integration
title: HomeBase ASR Workbench Integration
role: Quick Panel 内语音转文字工作台的产品形态、数据契约和后端接入边界
canonical_path: C:\HomeBase\source\docs\asr-workbench-integration.md
updated: 2026-07-20
authority: reference
status: current
volatility: periodic
sensitivity: agent-safe
review_after: 2026-08-20
do_not_execute: false
registered_in: C:\Users\homebase\INDEX.md
related:
  - doc_id: homebase-quick-panel
    path: C:\HomeBase\source\docs\quick-panel.md
    rel: parent-ui
  - doc_id: homebase-asr-multi-engine-design
    path: C:\HomeBase\source\docs\asr-multi-engine-design.md
    rel: engine-design
  - doc_id: aihub-asr-presets
    path: C:\HomeBase\AIHub\capabilities\asr-presets.json
    rel: machine-readable-presets
  - doc_id: homebase-model-center-result-workspace
    path: C:\HomeBase\source\docs\model-center-result-workspace.md
    rel: result-workspace
  - doc_id: homebase-asr-production-recommendation-portability
    path: C:\HomeBase\source\docs\asr-production-recommendation-portability.md
    rel: production-boundary
tags: [homebase, quick-panel, asr, workbench, mcp]
---

# ASR Workbench Integration

> **当前验证边界（2026-07-20）：** Runner、Tauri、Quick Panel 与 MCP 的统一任务契约已通过
> 自动测试和本机 6/6 路由预检。预检只证明本地依赖、模型、输出目录和密钥可解析；云端额度、
> 限流和服务健康仍在实际运行时判断。Windows x64 `0.1.0` 安装包与便携包已经完成构建，
> 包内 Doctor 14/14、便携版独立启动和 Secrets 加解密往返均通过；未下载的模型仍不能视为可运行。

HomeBase 的语音转文字入口放在 `Quick Panel` 当前页面中，不新增顶级路由，不侵入 Studio、Inbox、Knowledge 或 Secrets 页面。

## Goal

把 ASR 从“手工跑脚本 + 看输出目录”推进到“人和 Agent 都能按同一套契约启动任务、查看结果、复盘路径”的工作台。

## Human Entry

位置：

```text
C:\HomeBase\source\src\routes\QuickPanel.tsx
```

页面中提供一个可折叠的“语音转文字工作台”：

| 区域 | 作用 |
|---|---|
| 输入 | 录音文件路径、任务名称、语言、场景 |
| 方案 | 选择内置方案，或勾选引擎、命名并保存自定义组合；组合随便携配置迁移 |
| 处理 | 显示自适应预处理、引擎运行、对比融合、最终输出四步 |
| 输出 | 预览任务包路径和会生成的最终文件 |
| Agent 接入 | 显示已启用 MCP / HomeBase command 的调用契约 |
| 结果工作台 | 自动接收完成任务、恢复近期历史、查看多模型原稿、软件内整理，或复制 Agent 深度复核提示词 |

桌面版会执行真实环境预检、后台任务、进度事件、取消和重启恢复；浏览器预览只允许复制运行计划，不会伪造本机模型、密钥或执行状态。

## Output Root

已有机器默认复用：

```text
C:\HomeBase\Wiki\raw\audio\
```

新机器没有该目录时回退到：

```text
%USERPROFILE%\Documents\HomeBase\Wiki\raw\audio\
```

最终以首次运行生成的 `distribution-state.json` 和 Quick Panel 当前绑定为准。

任务包结构沿用 `asr-multi-engine-design.md`：

```text
C:\HomeBase\Wiki\raw\audio\<yyyy-mm-dd>_<topic-or-task-id>\
  source\
  preprocess\
  runs\
  compare\
  final\
  logs\
  exports\
  task.meta.json
  preset.snapshot.json
```

## Result Fusion

ASR 的多引擎输出不要求一个引擎通吃。系统内部可以同时保留：

```text
runs\sensevoice\transcript.md
runs\faster-whisper\transcript.md
runs\cloud-deepgram\transcript.md
runs\cloud-assemblyai\transcript.md
compare\run-matrix.json
compare\alignment-notes.md
final\original-scene-reconstruction.md
final\multi-engine-fused-transcript.md
final\meeting-summary.md
```

当前提供三类整理入口：

| Entry | Status | Notes |
|---|---|---|
| Copy prompt for Codex/Claude Code | implemented in Quick Panel | The prompt points the agent at the task folder and asks it to read all `runs\*` outputs before writing `final\` and `compare\` files. |
| In-app version viewing | implemented | Quick Panel obtains recent task packages through the typed ASR task API and lazily reads only the selected task's allowlisted artifacts. It does not scan unrelated Wiki files. |
| Software organization | implemented | Quick Panel or MCP can organize an existing task through OpenRouter without rerunning ASR. It writes the fused transcript, structured title/summary JSON, and human-readable meeting summary while preserving every engine transcript. |

`manual` disables all LLM calls and records `not_requested`. `auto` tries semantic fusion/summary and keeps a successful primary transcript when the semantic provider is unavailable. LLM processing is therefore optional for the workflow and required only when the user requests semantic fusion or summary generation.

Fusion rules:

- Correct obvious ASR mistakes and homophone errors.
- Prefer the version that fits surrounding context and timeline.
- Preserve uncertain spans as `[疑似: ...]` or `[听不清]`.
- Do not delete raw `runs\` outputs.
- Write a file-read list at the end of the fused result.

## Preset Source

机器可读预设源：

```text
C:\HomeBase\AIHub\capabilities\asr-presets.json
```

Canonical runner and MCP read this JSON directly. Quick Panel keeps compact display metadata for the built-in cards and stores user combinations in portable profile schema v2. Every run resolves to explicit engine IDs, so display names never decide execution.

Custom combinations carry only IDs and behavior settings:

```text
name + engine_ids + fusion_mode + scene_id + language
```

They never export API-key values, installation claims, or machine-specific health results. Version 1 profiles import as version 2 with an empty custom-combination list.

## Dynamic Detection Boundary

The model and preset recommendations are recomputed from the current machine rather than copied from this machine's historical profile. The desktop UI probes CPU/core count, RAM, physical GPU type/vendor/VRAM, disks and configured roots, runtime state, and the verified model manifest. Probe failures are isolated with `Promise.allSettled`; missing facts become `unknown` or an API fallback instead of a guessed local recommendation.

This mechanism handles CPU-only machines, no GPU, integrated/virtual/multiple GPUs, unknown VRAM, low RAM, low or unwritable storage, missing runtimes, partial/corrupt model installs, browser preview, and empty/invalid catalog states. The model catalog and hardware thresholds are still versioned product data, not automatically invented by the detector; new models require a catalog update and regression cases. Provider quota and availability are execution-time facts.

## Backend Boundary

已注册的 Tauri commands：

```text
asr_environment_probe
asr_task_start
asr_task_get
asr_task_list
asr_task_reconcile
asr_task_cancel
asr_task_read_artifact
asr_task_organize
```

Execution ownership:

```text
Quick Panel / MCP
  -> canonical runner: <resolved AIHub>\tools\local-asr\runner\asr_task_runner.py
  -> durable task package: <resolved output root>\<task>
  -> per-engine state + final/compare artifacts
```

Task and download workers write their own atomic JSON state. Page recovery is read-only: records outside the current instance's owned task map remain unchanged and are shown as unconfirmed, rather than rewritten to `interrupted`. Owned child monitoring is matched to the same state file, not a PID alone. Successful sibling engines remain available when another engine fails. See [task recovery](quick-panel-task-recovery.md).

Execution layer:

```text
开发机既有位置：C:\HomeBase\AIHub\tools\local-asr\runner\
发布版用户副本：%LOCALAPPDATA%\io.github.gdnbit.homebase\distribution\AIHub\tools\local-asr\runner\
```

有非系统数据盘时继续优先使用数据盘；只有 C 盘的新机器允许使用用户数据回退目录，
但推荐器必须提高安全余量并明确提示空间风险。

## Agent Entry

Live MCP server:

```text
C:\HomeBase\AIHub\mcp\servers\local-asr\
```

目标工具：

| Tool | Purpose |
|---|---|
| `transcribe_audio` | 录音转写并生成任务包 |
| `plan_transcription` | 只生成任务规划，不执行 |
| `list_asr_tasks` | 列出统一输出根目录中的近期任务包 |
| `get_asr_result` | 返回指定任务的 final / compare / runs 路径 |
| `cancel_asr_task` | 取消 MCP 启动的后台任务并保留已生成文件 |
| `read_asr_artifact` | 在任务目录边界内读取有限大小的结果文件 |
| `organize_asr_result` | 使用既有引擎原稿生成融合稿、标题和会议纪要，不重跑 ASR |
| `build_fusion_prompt` | 生成与同一输出契约一致的 Codex/Claude Code 提示词 |

Shared MCP ID is `homebase-local-asr`. Human and Agent calls use the same preset IDs, explicit engine IDs, states, task paths, and result contract.

## Deferred

本轮不做两件事：

- 不把 44/46 建成正式回归评测集。
- 不实现错字词典/专有名词词典的自动替换链路。

这两项后续再由用户确认。
