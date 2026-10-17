# REVIEW: Implement plan-only and AGY terminal lifecycle

Spec: [SPEC-implement-plan-only.md](../specs/SPEC-implement-plan-only.md)
Date: 2026-10-17

## Spec compliance

| Requirement | Status | Notes |
| --- | --- | --- |
| FR-500 | Met | Implement `agy_prompt` requires direct edits |
| FR-501 | Met | `observe_agy_message` on write tool names |
| FR-502 | Met | Single retry via `run_turn` loop + strict prompt |
| FR-503 | Met | `pending_terminal` + grace kill in `stream_child` |
| FR-504 | Met | Codex/Cursor/AGY/custom concurrent `Send` → `Error` |
| FR-505 | Met | `cancel_selected` clears tracking and calls adapter cancel |
| FR-506 | Met | Terminal `Error` fails turn/graph via existing `finish_turn_outcome` |
| FR-507 | Met | Plan mode notice preserved; terminal deferred |
| FR-508 | Met | `finish(..., require_writes)` unchanged for non-plan-only |

## Regression risk

- Medium: AGY turn lifecycle timing changed; mitigated by grace + idle tests.
- Low: concurrent send now finishes turns on `Error` (more visible than Notice).

## Test evidence

```text
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
```

All tests passed on Linux CI agent environment (2026-10-17).

## Residual risks

- Live AGY plan-only phrasing outside heuristics may fail without retry value; manual dogfood on Implement edits recommended.
