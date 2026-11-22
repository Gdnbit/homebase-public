# HomeBase Agent Runtime

Pi runs in a credential-free child process. Rust owns model requests, audited tools, approvals, stable job IDs, encrypted checkpoint persistence and all personal data.

The `pi-worker.mjs` entry point embeds the pinned Pi Agent Core loop with a host-only model transport and sequential host tools. It compacts large active turns at complete tool-batch boundaries and stores the latest bounded turn checkpoint through the protected host protocol. HomeBase carries earlier conversation summaries across turns and owns reminders, approvals, requirements and original job identifiers. Pi profiles, executables, provider credentials, and default tools stay outside this integration.

Install pinned dependencies from this directory with `npm ci --ignore-scripts`. Requires Node >=22.19.0. Run `node --test test/worker.test.mjs`; tests use synthetic model/tool callbacks and no provider or WeChat account.

The JSON-lines protocol accepts one versioned turn; callbacks are `model`, `tool`, `checkpoint`, followed by `complete` or `failed`. The host acknowledges durable checkpoint storage before execution proceeds. SDK checkpoint operations restore graph state; host fingerprints prevent duplicate model/tool actions. Results that may have reached a provider without a saved response are not retried.

Only host tools are exposed to the model. Built-in file/Shell/subagent actions are unavailable. Three audited Skill texts are embedded in the Rust host; the worker does not scan personal files or execute skill scripts. This process boundary is not an OS sandbox.

Windows packaging installs a fresh lockfile dependency tree and copies Node alongside `worker.mjs`. Never recursively copy development `node_modules` (it may contain local links). `scripts/test-agent-runtime-package.mjs` verifies this subset in a newly created explicit test root without removing existing staging.

See `docs/assistant.md` and `docs/personal-agent-implementation-2026-11-12.md` in the repository for public interfaces, isolation evidence and remaining scope.
