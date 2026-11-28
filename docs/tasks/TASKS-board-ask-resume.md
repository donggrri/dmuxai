# TASKS: Ask in the work session (hybrid resume)

Spec: [SPEC-board-ask-resume](../specs/SPEC-board-ask-resume.md) ·
Plan: [PLAN-board-ask-resume](../plans/PLAN-board-ask-resume.md)

- [x] T1 — `work_session_digest`, `set_skip_summary_jobs`; `board::resume`
  summary builder (bounded, masked). FR-3831. `cargo test --lib tc_3831`
- [x] T2 — Headless continuation in `App::run_headless` (existing conversation,
  native resume or summary seed, fresh retry as `summary`, fork). FR-3830,
  FR-3831, NFR-3830. `cargo test --test run_conversation`
- [x] T3 — CLI `run --conversation` (+ hidden board flags), validation, exit 3
  `not_found`, JSON `resume`, MCP `run` `conversation_id`. FR-3830.
  `cargo test --test run_conversation` · `cargo test --bin dmuxai tc_3830`
- [x] T4 — `board ask --resume` (CLI, MCP `board_ask`, HTTP body), OQ-3831
  agent, `no_active_binding`, usage without a work session, `--revise` ignores
  it. FR-3832. `cargo test --lib tc_3832` ·
  `cargo test --lib board_http_ask_and_run_view_only_vs_web`
- [x] T5 — Thread prefix, `resume` in JSON/HTTP, ask link conversation id.
  FR-3833. `cargo test --lib tc_3833`
- [x] T6 — Live run → new summary-seeded conversation (`--fork-summary`),
  native conflict. FR-3834. `cargo test --lib tc_3834`
- [x] T7 — Read-only, no sensitive/trust flags, no decision/check. FR-3835.
  `cargo test --lib tc_3835`
- [x] T8 — Board chat selector and mode badge. FR-3836.
  `cargo test --lib tc_3836` · `cargo test --test web_board_e2e -- --ignored tc_3836`
- [x] T9 — Docs (API.md, CHANGELOG, help), spec status, REVIEW;
  `bash scripts/check-spec.sh docs/specs/SPEC-board-ask-resume.md`,
  `python3 scripts/check-spec.py --all`, `cargo test --test spec_index`.
