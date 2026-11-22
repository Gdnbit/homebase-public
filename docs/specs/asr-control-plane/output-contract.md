# ASR Task Output Contract

## Task states

`queued`, `preflight`, `preprocessing`, `running`, `fusing`, `success`, `partial`, `failed`, `cancelled`, `interrupted`.

## Engine states

`pending`, `running`, `success`, `failed`, `skipped`, `blocked`, `cancelled`.

## Directory layout

```text
<output-root>/<YYYY-MM-DD_topic_job>/
  task.meta.json
  preset.snapshot.json
  execution/
    plan.json
    state.json
  source/
    <original-file>
    input.meta.json
    checksum.sha256
  preprocess/
    audio-profile.json
    audio.normalized.wav
    preprocessing.meta.json
  runs/<engine-id>/
    run.meta.json
    transcript.md
    transcript.json
    raw.json                 # optional
    segments.json            # optional
    speaker-turns.json       # optional
    subtitles.srt            # optional
  compare/
    run-matrix.json
    alignment-notes.md
    fusion.meta.json         # optional
  final/
    transcript.md
    multi-engine-fused-transcript.md
    original-scene-reconstruction.md
    meeting-summary.md
    title-summary.json
  exports/
    transcript.txt
    transcript.md
    transcript.srt           # when timestamps exist
    transcript.vtt           # when timestamps exist
  logs/
    runner.log
    events.jsonl
```

## Mutable state safety

- `task.meta.json`, `execution/state.json`, and `compare/run-matrix.json` use write-to-temp plus replace.
- Every state change includes `updated_at` and a monotonically increasing `revision`.
- Errors expose category and repair action, but never request headers, API keys, decrypted payloads, or complete provider response bodies.

## Minimum successful result

A task is `success` or `partial` only when at least one engine has a non-empty `transcript.md`. The canonical `final/transcript.md` identifies its source engines and preserves uncertainty markers. A task with no usable transcript is `failed`.

For every `success` or `partial` task, `final/title-summary.json` is present and uses one of these semantic states:

- `success`: title, summary, topics, decisions, action items, risks, and uncertainties were produced.
- `not_requested`: manual mode was selected and no LLM was called.
- `unavailable`: semantic processing failed; transcripts remain usable and the error is recorded without secret material.

`final/meeting-summary.md` mirrors that state in human-readable form. Reorganization may replace derived files in `final` and `compare/fusion.meta.json`, but never files in `source`, `preprocess`, or `runs`.
