---
doc_id: homebase-model-center-result-workspace
title: HomeBase Model Center and Result Workspace
role: Quick Panel 内模型下载中心、多模型结果工作台、下载后调用与 Agent 提示词协议
canonical_path: C:\HomeBase\source\docs\model-center-result-workspace.md
updated: 2026-07-20
authority: reference
status: current
volatility: periodic
sensitivity: agent-safe
review_after: 2026-08-20
do_not_execute: false
registered_in: C:\Users\homebase\Desktop\本机操作指南\INDEX.md
related:
  - doc_id: homebase-quick-panel
    path: C:\HomeBase\source\docs\quick-panel.md
    rel: parent-ui
  - doc_id: aihub-model-download-catalog
    path: C:\HomeBase\AIHub\capabilities\model-download-catalog.json
    rel: machine-readable-catalog
  - doc_id: aihub-model-manager
    path: C:\HomeBase\AIHub\tools\model-manager\README.md
    rel: download-helper
tags: [homebase, quick-panel, model-center, asr, ocr, document-parse, agent-prompts]
---

# Model Center and Result Workspace

> **当前验证边界（2026-07-20）：** 推荐、迁移、下载监督、ASR 任务、MCP、类型检查、
> 生产构建和 1024 px 界面均已复测。模型下载仍需用户显式启动；云端费用、额度和上游模型
> 状态是运行时事实，不能从本文或历史快照推断。

This document defines the first practical version of model download and multi-output review inside HomeBase Quick Panel.

## Scope

First version priority:

| Item | Status | Meaning |
|---|---|---|
| A. Codex-like prompt copy | implemented in UI | Copy a task-specific prompt so Codex, Claude Code, or another local agent can read one task folder and produce fused outputs. |
| C. In-app viewing | implemented for ASR | Show the actual task artifact inventory and lazily read allowlisted files through the typed ASR task API; no global Wiki scan. |
| API fusion | implemented for ASR | Organize a completed task through OpenRouter, or use manual mode with no LLM call; engine originals remain immutable. |
| Full model download | implemented | Durable Tauri jobs expose preflight, progress, cancel, retry/resume metadata, completeness verification, and inference smoke state before installation. |

## Files

| File | Role |
|---|---|
| `C:\HomeBase\source\src\routes\QuickPanel.tsx` | Human interface: Model Download Center + Result Workspace. |
| `C:\HomeBase\AIHub\capabilities\model-download-catalog.json` | Machine-readable catalog for built-in ASR/OCR/document models. |
| `C:\HomeBase\source\src\lib\model-recommender.ts` | Pure runtime recommendation rules for machine portability. |
| `C:\HomeBase\source\src\lib\asr-portability.ts` | Versioned roots, migration bundle, secret-reference policy, and target-machine validation. |
| `C:\HomeBase\source\src-tauri\src\commands\model_center.rs` | Manifest probe plus durable model-download supervisor; separates probe, partial, verified, and corrupt states. |
| `C:\HomeBase\source\scripts\test-model-recommender.mjs` | Minimal regression test for recommendation rules. |
| `C:\HomeBase\source\scripts\test-asr-portability.mjs` | Migration and path-policy regression tests. |
| `C:\HomeBase\AIHub\tools\model-manager\model_manager.py` | CLI helper for `list`, `plan`, `probe`, and guarded `download`. |
| `C:\HomeBase\AIHub\models\installed-models.json` | Manifest written after probe/download; downstream tools read local model paths here. |

## Storage Policy

| Purpose | Path |
|---|---|
| Model snapshots | `C:\HomeBase\Models` |
| Runtime environments | `C:\HomeBase\Runtimes` |
| Probe/cache test | `C:\HomeBase\Cache\homebase-model-download-test` |
| User/task outputs | `C:\HomeBase\Wiki\raw` |

Do not put model weights, runtime environments, or generated task outputs on `C:`.

## Download Helper

```powershell
$py = "D:\Programs\Python313\python.exe"
$mgr = "C:\HomeBase\AIHub\tools\model-manager\model_manager.py"

& $py $mgr list
& $py $mgr plan faster-whisper-large-v3
& $py $mgr probe faster-whisper-large-v3
& $py $mgr download faster-whisper-large-v3 --allow-large
```

Behavior:

- `list`: reads the catalog and returns all built-in model cards.
- `plan <id>`: returns source, hardware fit, strengths, weaknesses, download target, and invocation example.
- `probe <id>`: downloads one small metadata file and writes `installed-models.json`.
- `download <id>`: downloads the full Hugging Face snapshot only when `--allow-large` is present for large entries.

Verified on 2026-07-08:

- `list` succeeded.
- `plan faster-whisper-large-v3` succeeded.
- `probe faster-whisper-large-v3` downloaded `config.json` to `C:\HomeBase\Cache\homebase-model-download-test\hf\faster-whisper-large-v3\config.json`.
- `plan sensevoice-small` succeeded.
- `probe sensevoice-small` downloaded `README.md` to `C:\HomeBase\Cache\homebase-model-download-test\hf\sensevoice-small\README.md`.
- `installed-models.json` was written under `C:\HomeBase\AIHub\models`.
- `download faster-whisper-large-v3` without `--allow-large` was correctly refused.

## Runtime Model Recommendations

Model cards are split into two layers:

| Layer | Source | Meaning |
|---|---|---|
| Generic model facts | `model-download-catalog.json`; mirrored in Quick Panel for now | Source, strengths, weak points, estimated size, minimum RAM, recommended VRAM, runtime requirements, API fallback. |
| Runtime machine profile | HomeBase `system_info`, `system_metrics`, `system_gpu`, `system_disks`, `system_path_list` | Current CPU/RAM/GPU/disk/PATH state for this machine. |
| Recommendation result | `model-recommender.ts` | Conservative status plus reason, next action, confidence, warnings, blockers, and evidence. |

Runtime rules:

| Situation | Result |
|---|---|
| Manifest says `probe` / `probed` | `partial_install`; a source check is not an install. |
| Download directory exists but smoke test is absent | `partial_install`. |
| Manifest path, runtime, and smoke test are verified | `installed`. |
| Estimated model size plus safety reserve exceeds the preferred non-system disk | `space_blocked` |
| RAM is below `min_ram_gb` | `api_preferred` when API fallback exists, otherwise `unsupported` |
| GPU/VRAM is absent or insufficient | `api_preferred` when API fallback exists, otherwise `unsupported`. |
| GPU/VRAM is unknown or only inferred | `api_preferred` or `needs_review`; never auto-approved. |
| Python/CUDA or another declared runtime is missing | `missing_runtime` or `api_preferred`. |
| Runtime version is incompatible | `runtime_mismatch` or `api_preferred`. |
| Target directory is unwritable | `storage_blocked`. |
| Hardware and runtime are sufficient and `local_priority = recommended` | `recommended` |
| Hardware and runtime are sufficient but model is not a default local lane | `optional` |

The recommender remains intentionally rule-based and explainable. Its production boundary and complete edge matrix are defined in `docs/asr-production-recommendation-portability.md`.

Model cards must show:

- source and application URL;
- strengths and weak points;
- hardware fit;
- storage estimate note;
- runtime recommendation status and reason;
- probe command;
- full download command;
- invocation example after download.

## Result Workspace Contract

All engines write into one task folder:

```text
C:\HomeBase\Wiki\raw\<type>\<yyyy-mm-dd>_<task>\
  source\
  preprocess\
  runs\
    <engine-id>\
      transcript.md or content.md
      segments.json
      speaker-turns.json
      layout.json
  compare\
    run-matrix.json
    alignment-notes.md
  final\
    original-scene-reconstruction.md
    multi-engine-fused-transcript.md
    meeting-summary.md
```

The exact files vary by type:

- Audio: `transcript.md`, `segments.json`, `speaker-turns.json`, `audio-profile.json`.
- OCR/image: `content.md`, `blocks.json`, low-confidence regions, original images.
- PDF/document: `content.md`, `layout.json`, `tables.json`, source files, extracted images.

## Agent Prompt Entry

Quick Panel generates a prompt with the selected task folder and type. The prompt tells Codex/Claude Code to:

- read all available `runs/*` outputs;
- compare model disagreements;
- correct obvious ASR/OCR errors;
- mark uncertain spans instead of inventing content;
- write final fused files under `final\`;
- write difference notes under `compare\`;
- list all files actually read.

This is the A-priority route. It does not require HomeBase to expose raw file contents to a cloud API.

## In-App Viewing Entry

Quick Panel displays actual per-engine and final artifacts when the selected ASR task is under the configured output root. Completed tasks are handed over automatically and recent task packages are rediscovered from that root after navigation or restart.

Displayed artifact groups:

- preprocessing metadata;
- each local/cloud engine's transcript, segments, speaker turns, subtitles, and run metadata;
- comparison matrix, alignment notes, and fusion state;
- final transcript, scene reconstruction, title/summary, meeting notes, and exports.

Current reader:

- Calls `asr_task_list`/`asr_task_get` to discover canonical task packages.
- Requires the task directory to remain inside the current portable profile's `output_root`.
- Calls `asr_task_read_artifact` for a bounded allowlist; source recordings and adjacent directories are never exposed.
- Calls `asr_task_organize` to regenerate derived final artifacts without rerunning engines.

Remaining product improvements:

- open file;
- open containing folder;
- compare side-by-side;
- approve one version as final;
- rerun one engine.
- add equivalent typed task adapters for OCR and document routes; those types do not fall back to the old global Wiki reader.

## Research Basis

The first catalog is based on current public project docs and model cards:

- faster-whisper: `https://github.com/SYSTRAN/faster-whisper`
- faster-whisper large-v3 model: `https://huggingface.co/Systran/faster-whisper-large-v3`
- FunASR: `https://github.com/modelscope/FunASR`
- SenseVoiceSmall: `https://huggingface.co/FunAudioLLM/SenseVoiceSmall`
- PaddleOCR: `https://github.com/PaddlePaddle/PaddleOCR`
- PaddleOCR-VL: `https://huggingface.co/PaddlePaddle/PaddleOCR-VL`
- MinerU: `https://github.com/opendatalab/MinerU`
- Docling: `https://github.com/docling-project/docling`
- Marker: `https://github.com/datalab-to/marker`
- Hugging Face Hub download docs: `https://huggingface.co/docs/huggingface_hub/en/guides/download`
- Hugging Face CLI docs: `https://huggingface.co/docs/huggingface_hub/en/guides/cli`
- ModelScope download docs: `https://modelscope.cn/docs/models/download`

## Completed Backend Steps

1. Resumable Tauri download jobs wrap `model_manager.py` with durable state and cancellation.
2. Manifest `installed` requires completeness and minimum inference evidence.
3. ASR Result Workspace attaches to real task folders through a root-bound typed API.
4. Software/API organization writes to the canonical `final` and `compare` paths without modifying engine runs.
5. `homebase-local-asr` MCP exposes the same task and post-processing contract to local Agents.
