# ASR Reuse and Duplication Review

## Reuse

- Reuse `cloud_asr_runner.py` provider adapters until each is migrated behind the canonical engine interface.
- Reuse `sensevoice_runner.py` in its isolated Python 3.11 environment.
- Reuse faster-whisper parameters and task artifacts from `local_asr_runner.py`.
- Reuse HomeBase `secrets_test`, SOPS adapters, Tauri event conventions, and cancellable child-process patterns.
- Reuse `asr-presets.json`, model catalog IDs, and Wiki task storage.

## Intentional new code

- One orchestration runner is necessary because the existing scripts disagree on task creation, output root, engine selection, and lifecycle.
- One ASR Tauri command module is necessary because no existing command provides durable file-backed jobs or ASR-specific validation.

## Prohibited duplication

- Do not create another preset list in Rust or MCP.
- Do not create another secret store.
- Do not create another human ASR page.
- Do not copy cloud provider HTTP code into the React layer.
