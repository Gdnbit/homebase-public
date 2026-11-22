# ASR Control Plane Requirements

## Scope

This specification turns the existing Quick Panel ASR board into the shared human and Agent control plane for local and cloud transcription. It remains inside `/quick-panel`; no second application or unrelated HomeBase route is introduced.

## Rules

| Rule ID | Requirement |
|---|---|
| `ASR-001` | The board must route by preset or explicit engine selection; no single engine is expected to handle every recording. |
| `ASR-002` | Every task must preserve the immutable source and all intermediate, per-engine, comparison, final, log, and export artifacts under one dated task package. |
| `ASR-003` | Local, cloud, hybrid, full-audit, and user-defined combinations must use one canonical task contract. |
| `ASR-004` | Recommendation and execution truth are separate. A recommendation never proves that a runtime, model, key, quota, or provider is executable. |
| `ASR-005` | Before execution, the system must probe the actual runtime, model path, writable output root, provider-key presence, and selected-engine compatibility. |
| `ASR-006` | PATH presence, a downloaded directory, a catalog entry, or a historical success report cannot by itself produce a ready state. |
| `ASR-007` | Long-running work must be cancellable, persist its state to disk, survive UI navigation, and become explicitly `interrupted` after an application restart rather than remaining falsely `running`. |
| `ASR-008` | The original recording is never overwritten. Adaptive preprocessing must save its profile, parameters, normalized audio, and fallback decision. |
| `ASR-009` | A preset may complete partially when at least one engine succeeds. Failed and skipped engines remain visible with reason and retryability. |
| `ASR-010` | Multi-engine tasks must support manual comparison and optional LLM fusion. Fusion cannot delete or overwrite per-engine outputs. |
| `ASR-011` | Speaker-labelled output must be available through compatible cloud diarization engines; local diarization remains conditional on a verified compatible runtime. |
| `ASR-012` | A completed task must expose a readable transcript, original-scene reconstruction, comparison matrix, meeting summary state, and export paths. |
| `ASR-013` | Secrets are referenced by short name and resolved only in the execution process. Secret values never enter task JSON, command previews, logs, clipboard plans, migration bundles, or MCP responses. |
| `ASR-014` | Human UI, Tauri commands, CLI runner, and MCP tools must share preset IDs, task states, engine IDs, and output paths. |
| `ASR-015` | Browser preview must remain useful but must not claim local-file access, runtime readiness, provider health, or task execution. |
| `ASR-016` | Model downloads must be explicit jobs with size preflight, progress, cancel, retry/resume metadata, verification, and installation smoke-test state. |
| `ASR-017` | The installation manifest is authoritative only after path, completeness, runtime compatibility, and minimum inference verification pass. |
| `ASR-018` | The board must always provide an actionable state: runnable route, API route, missing input, repair action, or explicit unsupported mode. |
| `ASR-019` | Result Workspace must discover completed and recent tasks through the typed task API and read only a task-scoped artifact allowlist. It must not scan the entire Wiki to infer ASR outputs. |
| `ASR-020` | Existing successful or partial task packages can be reorganized without rerunning ASR engines. Reorganization may replace derived `final`/`compare` outputs but must preserve `source`, `preprocess`, and `runs`. |
| `ASR-021` | LLM use is an optional semantic stage for fusion, title generation, and meeting summary. Task discovery, file routing, state management, artifact inventory, and manual comparison must work without an LLM. |
| `ASR-022` | Machine recommendations must be recomputed from live CPU, memory, GPU, disk, runtime, and verified-install probes. Missing or conflicting facts produce a conservative state and repair action rather than a guessed recommendation. |

## Non-functional requirements

- UTF-8 JSON and Markdown outputs.
- No models, audio, runtime environments, task caches, or generated artifacts are written to `C:` by default.
- Atomic JSON writes for mutable job/task state.
- Bounded child processes and network timeouts.
- No destructive cleanup of source or successful engine outputs.
- Desktop and narrow-window layouts remain usable without horizontal overflow.

## Explicit deferments

- Live microphone capture and true streaming captions are a separate transport task. File-based execution must mark the realtime preset as unsupported until that transport exists.
- Automatic speaker identity enrolment by biometric voiceprint is not enabled by default. This board supports speaker diarization and user labels; biometric identity requires separate consent, security, and retention design.
