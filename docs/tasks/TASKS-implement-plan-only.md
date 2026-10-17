# TASKS: Implement plan-only and AGY terminal lifecycle

Spec: [SPEC-implement-plan-only.md](../specs/SPEC-implement-plan-only.md)

| Done | Task | Requirement IDs | Test command |
| --- | --- | --- | --- |
| [x] | AGY write tools + plan-only helpers in `implement_outcome.rs` | FR-501, FR-503 | `cargo test agy_write_tools plan_only plan_review` |
| [x] | AGY deferred terminal, grace kill, plan-only retry loop | FR-500, FR-502, FR-503, FR-506 | `cargo test --bin dmuxai agy_result_grace implement_plan_only result_event_defers` |
| [x] | Adapter concurrent send uses `Error` | FR-504 | `cargo test --all-targets` |
| [x] | App cancel clears tracked turns | FR-505 | `cargo test cancel_clears_active_turn` |
| [x] | Docs, dogfood #24, TEST-PLAN | FR-500..508 | `bash scripts/spec-index.sh`; `cargo test spec_index` |
| [x] | Review artifact + clippy | all | `cargo clippy --all-targets --all-features -- -D warnings` |
