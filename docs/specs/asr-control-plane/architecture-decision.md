# Architecture Decision: Shared File-backed ASR Control Plane

## Decision

Use a layered modular monolith:

```text
Quick Panel / MCP
  -> typed Tauri or CLI command
  -> durable job supervisor
  -> AIHub Python orchestration runner
  -> local/cloud engine adapters
  -> canonical task package in C:\HomeBase\Wiki\raw\audio
```

HomeBase owns process lifecycle, UI events, cancellation, and job discovery. AIHub owns ASR domain orchestration and engine adapters. The Wiki task package is the durable handoff shared by humans, Agents, CLI, and MCP.

Task discovery and artifact routing are deterministic control-plane responsibilities. Result Workspace uses Tauri task APIs and a canonical artifact allowlist; it does not ask an LLM to locate files or infer task state. An LLM may be invoked only as a replaceable semantic worker for multi-engine fusion, titles, summaries, and requested deep review. Manual comparison and all task navigation remain available without it.

## Reasons

- Reuses the five existing runners and HomeBase task-process patterns.
- Keeps heavy Python ASR dependencies outside the Tauri binary.
- Allows local and cloud engines to evolve without rewriting the UI.
- Makes task artifacts inspectable and recoverable without a database migration.
- Preserves the existing rule that HomeBase is the human shell and AIHub is the capability registry/implementation home.

## Rejected alternatives

- One monolithic Rust ASR implementation: duplicates mature Python model ecosystems.
- UI directly spawning scripts: no durable truth, cancellation, recovery, or MCP parity.
- A second standalone ASR application: violates the current-board integration requirement.
- SQLite-only results: makes raw artifacts harder for Agents and Wiki workflows to inspect.
