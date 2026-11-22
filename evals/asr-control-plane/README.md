# ASR Control Plane Evals

The suite uses generated short WAV fixtures and fake engine adapters for deterministic lifecycle tests. Real providers and long recordings are separate smoke matrices because they incur time or cost.

Required cases:

| Case | Rule coverage |
|---|---|
| Missing source | `ASR-005`, `ASR-018` |
| Output root on C without override | `ASR-002`, non-functional storage rule |
| Unwritable output root | `ASR-005`, `ASR-018` |
| No compatible engines | `ASR-004`, `ASR-018` |
| One success plus one failure | `ASR-009`, `ASR-012` |
| Cancel during deterministic slow engine | `ASR-007` |
| Stale running job reconciliation | `ASR-007` |
| Source checksum and immutable copy | `ASR-002`, `ASR-008` |
| Adaptive profile and fallback record | `ASR-008` |
| Manual versus API fusion | `ASR-010` |
| Secret redaction scan | `ASR-013` |
| Browser preview | `ASR-015` |
