---
title: Secret integration playbook
updated: 2026-06-07
---

# Secret Integration Playbook

This document is the fixed workflow for adding a new API key service to HomeBase.

Goal: future agents should read this file first, then add a key service without rediscovering the frontend/backend structure.

## Scope

HomeBase secrets are stored in `C:\HomeBase\Secrets\secrets.yaml` and encrypted by SOPS. The app never stores plaintext API keys in source code, SQLite, or UI state beyond the explicit temporary reveal flow.

Adding a service means registering metadata only:

- backend: short name, long env name, consumers, category, rotate URL, optional health check
- frontend: service card metadata, suggested short name, icon, color, docs URL
- docs: user-facing application URL and usage notes
- tests: backend metadata consistency, TypeScript build

## Naming

Use stable short names in snake_case.

Examples:

| Service | short_name | env var |
|---|---|---|
| OpenAI | `openai_key` | `OPENAI_API_KEY` |
| Deepgram | `deepgram_key` | `DEEPGRAM_API_KEY` |
| AssemblyAI | `assemblyai_key` | `ASSEMBLYAI_API_KEY` |
| ElevenLabs | `elevenlabs_key` | `ELEVENLABS_API_KEY` |
| Hugging Face | `hf_token` | `HF_TOKEN` |

Rules:

- short name must be lowercase, start with a letter, and only use letters, digits, and `_`
- use `_key` for general provider API keys
- use `_token` for token-style services when the provider’s own docs call it a token
- do not include environment names like `prod` unless the service truly needs separate keys

## Backend Registration

File:

`src-tauri/src/commands/secrets.rs`

Add a `meta_for(short_name)` branch:

```rust
"provider_key" => Some(KeyMeta {
    long_name: Some("PROVIDER_API_KEY"),
    consumers: &["Human-readable usage", "MCP or capability name"],
    category: FutureReady,
    rotate_url: Some("https://provider.example.com/api-keys"),
    health: Some(HealthSpec {
        method: Get,
        url: "https://api.provider.example.com/v1/me",
        auth_header: "Authorization",
        auth_format: "Bearer {key}",
    }),
}),
```

Then add the short name to `KNOWN_SHORT_NAMES`.

Categories:

- `Core`: daily HomeBase/MCP keys already wired into core workflows
- `FutureReady`: provider keys available for capabilities, agents, or future tools
- `GdnbitProject`: GDNbit-only project secrets
- `Unknown`: generated automatically for keys present in YAML but not registered

Health check guidance:

- Prefer a cheap authenticated `GET /me`, `GET /models`, `GET /projects`, or equivalent.
- Avoid endpoints that create billable work.
- If the provider has no harmless endpoint, set `health: None`.
- Use the provider’s official auth header format exactly.

## Frontend Registration

File:

`src/lib/service-registry.ts`

Add a `SERVICE_META` entry:

```ts
provider_key: {
  displayName: "Provider",
  logoKind: "initial",
  logoKey: "",
  brandColor: "64748B",
  suggestedShortName: "provider_key",
  suggestedLongName: "PROVIDER_API_KEY",
  suggestedRotateUrl: "https://provider.example.com/api-keys",
  description: "Short capability description",
},
```

Add a heuristic rule if future variants should share the same brand display:

```ts
{ test: (n) => n.startsWith("provider"), meta: { displayName: "Provider", logoKind: "initial", logoKey: "", brandColor: "64748B" } },
```

Icon options:

- `logoKind: "lobe"` for supported `@lobehub/icons` AI/provider icons
- `logoKind: "si"` for `react-icons/si` Simple Icons, with `logoKey` like `SiGithub`
- `logoKind: "initial"` for providers without available icons; set `brandColor`

Keep cards compact. The Add Secret modal should prefill the short name and display name so the user only needs to paste the key.

## User Documentation

Create or update a capability-specific document under `docs/`.

For ASR keys:

`docs/asr-cloud-keys.md`

Minimum table:

| Service | HomeBase short_name | Environment variable | Purpose | Application URL |
|---|---|---|---|---|

Include notes about:

- which key is optional vs required
- whether the service is used by local tools, MCP, or cloud workflows
- whether the user must accept gated model terms

## Validation

Run:

```powershell
cargo test --lib commands::secrets --no-fail-fast
npx tsc --noEmit
npm run build
```

Expected:

- `meta_table_aligns_with_known_shorts` passes
- all short names pass validation
- frontend build passes
- no plaintext key appears in source diffs

## Manual UI Check

Start HomeBase as a Tauri app, not only Vite:

```powershell
npm run tauri:dev
```

Then check:

- `/secrets` lists the new service under missing keys
- Add Secret modal shows the new service card
- selecting the service prefills short name and display name
- paste writes to SOPS and produces a backup
- Test button uses the registered health check
- Rotate button opens the provider URL

## Future MCP Automation

A small MCP tool can automate the repetitive part:

Input:

```json
{
  "short_name": "provider_key",
  "env_var": "PROVIDER_API_KEY",
  "display_name": "Provider",
  "apply_url": "https://provider.example.com/api-keys",
  "health": {
    "method": "GET",
    "url": "https://api.provider.example.com/v1/me",
    "auth_header": "Authorization",
    "auth_format": "Bearer {key}"
  },
  "icon": {
    "kind": "initial",
    "brand_color": "64748B"
  }
}
```

Output:

- patch `secrets.rs`
- patch `service-registry.ts`
- patch or create `docs/<capability>-keys.md`
- run validation

Do not let the MCP tool write actual secret values. It should register service metadata only.
