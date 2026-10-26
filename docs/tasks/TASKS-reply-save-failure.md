# TASKS: A reply that cannot be saved must not disappear

Spec: [SPEC-agent-api.md](../specs/SPEC-agent-api.md) (FR-2140)
Plan: [PLAN-reply-save-failure.md](../plans/PLAN-reply-save-failure.md)

## Part A (Code implementation)

| Done | Task | Requirement IDs | Test command |
| --- | --- | --- | --- |
| [x] | `with_busy_retry(budget, attempt)` is the shared busy-retry loop; the open phase keeps its behaviour | FR-2140 | `cargo test --lib busy_open_tests` |
| [x] | `ConversationEngine::record_turn_outputs_with_retry` retries a busy save with a short attempt timeout and reports `ConversationError::Busy` when the budget is spent; a spent budget still probes the database without waiting | FR-2140 | `cargo test --lib tc_2140` |
| [x] | `src/app.rs` saves every turn output through the retrying call and counts the waiting of a failed save against the budget of the turn | FR-2140 | `cargo test --bin dmuxai tc_2140` |
| [x] | A failed save still applies the event to the visible transcript, and the "Could not save <agent> output" notice stays | FR-2140 | `cargo test --bin dmuxai tc_2142` |
| [x] | `App::run_headless` maps a failed save to `ok: false` with exit code 4 (busy) or 6 (other) and an error that names the reply | FR-2140 | `cargo test --bin dmuxai tc_2141`, `cargo test --bin dmuxai tc_2143`, `cargo test --bin dmuxai tc_2144` |
| [x] | A close that failed against the locked database is attempted once; after a spent save budget shutdown closes with a short busy timeout so `Drop` does not wait the default timeout again | FR-2140 | `cargo test --lib tc_2140`, `cargo test --bin dmuxai tc_2140` |
| [x] | The headless outcome and the failure JSON report `error_kind` `conflict` (exit 4) or `internal` (exit 6), including the locked open of #196 | FR-2140 | `cargo test --bin dmuxai tc_214`, `cargo test --bin dmuxai tc_2120` |

## Part B (Documentation and gates)

| Done | Task | Requirement IDs | Test command |
| --- | --- | --- | --- |
| [x] | `docs/specs/SPEC-agent-api.md` documents FR-2140 and TC-2140..TC-2144 | FR-2140 | `python3 scripts/check-spec.py docs/specs/SPEC-agent-api.md` |
| [x] | `docs/API.md` lists the locked conversation database as a cause of exit code 4 | FR-2140 | manual review |
| [x] | `docs/API.md` documents `error_kind` and the non-busy save error behind exit 6 | FR-2140 | manual review |
| [x] | `CHANGELOG.md` has an `Unreleased` → `Fixed` entry | FR-2140 | manual review |
| [x] | Plan, tasks and review artifacts for this change | FR-2140 | manual review |
| [x] | Spec and TC link gates stay green | FR-2140 | `python3 scripts/check-spec.py --all`, `python3 scripts/check-tc-links.py` |
