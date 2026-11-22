# ASR Control Plane Delivery Tasks

| ID | Status | Goal | Validation evidence | Stop condition |
|---|---|---|---|---|
| `ASR-T01` | complete | Canonical Python runner, preflight, adaptive preprocessing, deterministic fixture mode, task package | Runner contract tests and fixture partial-success run | No atomic state or source preservation |
| `ASR-T02` | complete | Tauri live probe and durable start/list/get/cancel/reconcile commands | Rust command tests; secret redaction tests | Secret appears in args/logs |
| `ASR-T03` | complete | Quick Panel actual start/progress/cancel/provider check/results handoff | TypeScript/build passed; native desktop interaction and supported-width QA passed | Requires new route or breaks preview mode |
| `ASR-T04` | complete | Model download job supervisor and verified manifest writer | Fixture download/cancel/completeness tests and Rust supervisor tests | Installed can be reached without smoke test |
| `ASR-T05` | complete | Custom presets stored in portable profile | 19 portability edge cases; v1 to v2 migration; explicit runner custom-ID tests | Secret value or machine install state exported |
| `ASR-T06` | complete | Local ASR MCP using the same runner contract | MCP protocol, fixture task, and artifact path-safety tests | MCP invents alternate schema |
| `ASR-T07` | complete | Real-machine and supported-viewport acceptance, docs/status reconciliation | Automated gates, 6/6 preflight, real SenseVoice task, verified local models, live MCP assignment, multi-GPU selection, default-width and 1024 px native QA passed | Broad claim lacks matching evidence |
| `ASR-T08` | complete | Close the result loop: automatic task handoff/history, task-scoped artifact reader, actual title/summary output, post-hoc organization, and MCP parity | 25 Python tests, 6 targeted Rust tests, 184 full Rust tests, 39 recommendation cases, 19 portability cases, TypeScript check, Vite build, and current-machine 6/6 preflight passed on 2026-07-20 | Result browsing still scans the Wiki, reorganization reruns engines, or semantic output is a placeholder |
