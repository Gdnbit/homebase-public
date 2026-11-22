# ASR Control Plane Acceptance

## P0 execution

1. A valid local audio file can be started from Quick Panel and produces a known task directory before the child process starts.
2. The UI shows queued, preflight, preprocessing, engine, fusion, completed, partial, failed, cancelled, and interrupted states.
3. Cancelling a task kills the child and persists `cancelled` without deleting existing artifacts.
4. The task package contains the files defined by `output-contract.md`.
5. At least one verified local engine can run through the canonical runner on this machine.
6. Cloud engines are attempted only when their secret exists; health and quota failures are recorded per engine.

## P0 truth and recovery

1. Missing runtime/model/key/output permission produces a concrete blocker and repair action.
2. A stale on-disk running job with no in-memory child becomes `interrupted` and retryable.
3. Provider or engine failure does not hide successful sibling outputs.
4. No task/log/plan payload contains decrypted secret material.

## P1 workflow

1. Presets and custom combinations resolve to explicit engine IDs.
2. Result Workspace loads the completed task and shows per-engine, comparison, and final files.
3. LLM fusion can be disabled; manual comparison remains available.
4. Human and MCP calls return the same task ID, task path, state, and result paths.
5. A completed task is handed to Result Workspace automatically and remains discoverable from task history after navigation or restart.
6. Result Workspace reads a bounded allowlist inside the selected task; it does not search unrelated Wiki content.
7. Existing engine outputs can be reorganized into a fused transcript, title, and meeting summary without rerunning or modifying engine transcripts.
8. Manual organization performs no LLM request and writes an explicit `not_requested` semantic state.

## Validation gates

- Python contract and edge tests pass.
- Rust command tests pass.
- TypeScript build passes.
- Tauri runner preflight succeeds on the current machine without revealing secrets.
- A short fixture completes locally; cancel and interrupted recovery are exercised with deterministic fixtures.
- Native Quick Panel QA passes at the default desktop width and the HomeBase-supported minimum width of 1024 px.

Run the acceptance checks with synthetic recordings in an isolated workspace. Local engine and provider readiness should be verified on the target machine.
