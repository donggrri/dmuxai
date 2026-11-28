# TASKS: Board item work-session links

Spec: [SPEC-board-work-session](../specs/SPEC-board-work-session.md) ·
Plan: [PLAN-board-work-session](../plans/PLAN-board-work-session.md)

- [x] T1 — `SessionLink` model, `BoardItem.sessions` (serde default, max 50),
  `session` event, store append/finish. FR-3820. `cargo test --lib tc_3820`
- [x] T2 — Board run fills the link from child JSON + run manifest; `md_paths`
  from `git status` snapshots; crash/spawn error → `failed`. FR-3821.
  `cargo test --lib tc_3821`
- [x] T3 — `board ask` (answer and `--revise`) appends `ask` links. FR-3822.
  `cargo test --lib tc_3822`
- [x] T4 — Read-only `session_view` + `conversation::read_binding_statuses`;
  CLI `board session`, MCP `board_session`, `GET /api/board/{id}/session`.
  FR-3823, NFR-3820. `cargo test --lib tc_3823` · `cargo test --bin dmuxai tc_3826`
- [x] T5 — `sessions` in show / MCP `board_show` / `GET /api/board/{id}`. FR-3824.
  `cargo test --lib tc_3824` · `cargo test --lib tc_3823`
- [x] T6 — Board modal "Work session" section, `window.dmuxaiOpenRun`. FR-3825.
  `cargo test --lib tc_3825` · `cargo test --test web_board_e2e -- --ignored tc_3825`
- [x] T7 — `board link <id> --run-id R`, MCP `board_link`. FR-3826.
  `cargo test --bin dmuxai tc_3826` · `cargo test --lib tc_3824`
- [x] T8 — Docs (API.md, CHANGELOG), spec status, REVIEW;
  `bash scripts/check-spec.sh docs/specs/SPEC-board-work-session.md`,
  `python3 scripts/check-spec.py --all`.
