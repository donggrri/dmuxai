# TASKS-state-per-repo

Spec: [SPEC-state-per-repo.md](../specs/SPEC-state-per-repo.md) · Plan: [PLAN-state-per-repo.md](../plans/PLAN-state-per-repo.md)

| Task | Requirements | Evidence |
|---|---|---|
| 1. Add `src/state_path.rs` with `state_base`, `repo_key` (FNV-1a 64), `repo_db_path`, `legacy_db_path`, `prepare_repo_db` + unit tests | FR-450, FR-451, FR-452, FR-453 | TC-450..TC-453, `cargo test --all-targets` |
| 2. Wire `main.rs` to `state_path::prepare_repo_db`; show migration notice | FR-450, FR-451, FR-452 | TC-450..TC-452 |
| 3. `UnsupportedSchema` includes the DB path and `DMUXAI_STATE_DIR` hint in its message | FR-454 | TC-454 |
| 4. Run gates: `cargo fmt --check`, `cargo test --all-targets`, `cargo clippy --all-targets --all-features -- -D warnings` | — | Gate output in review |
