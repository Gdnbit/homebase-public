# ASR Control Plane Module Boundaries

| Module | Owns | Must not own |
|---|---|---|
| `src/lib/model-recommender.ts` | Conservative recommendation from supplied facts | Runtime execution truth or provider billing state |
| `src/lib/asr-portability.ts` | Versioned portable configuration | Secret values or installed-state transfer |
| `src-tauri/commands/asr.rs` | Live probe, child lifecycle, durable job index, cancel/status/events | ASR decoding or transcript fusion logic |
| `AIHub/tools/local-asr/runner/asr_task_runner.py` | Task package, preprocessing, engine orchestration, finalization | HomeBase navigation or secret persistence |
| Engine adapters | One provider/model invocation and canonical run output | Cross-engine final decisions |
| Quick Panel | Human controls and explanation | Direct filesystem mutation or secret decryption |
| MCP server | Agent-facing validation and task calls | Alternate task schema or output root |

All boundaries exchange JSON-safe structures with stable IDs.
