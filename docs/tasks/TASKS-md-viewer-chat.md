# TASKS: Chat beside the Markdown viewer, with edits in the work session

Spec: [SPEC-md-viewer-chat](../specs/SPEC-md-viewer-chat.md) ·
Plan: [PLAN-md-viewer-chat](../plans/PLAN-md-viewer-chat.md)

- [x] T1 — `md_file_context` (FR-3415 rules, ≤ 64 KiB masked content), HTTP
  ask `file`, "File in question" prompt section, `[file · P]` tag. FR-3841.
  `cargo test --lib tc_3841`
- [x] T2 — HTTP `mode: "edit"` with human headers, edit argv (work
  conversation, no `--read-only`, `--no-spec-first`), seed summary without a
  work session, 409 during a live run. FR-3842. `cargo test --lib tc_3842`
- [x] T3 — No `--allow-sensitive`/`--trust-workspace-config`/`--approve-plan`;
  exit 7 → "needs approval — not applied"; other exits masked. FR-3843.
  `cargo test --lib tc_3843`
- [x] T4 — Changed paths (git status before/after plus fingerprint, ≤ 50),
  ask link `mode: "edit"`, planner message, lane/plan/decision/checked
  unchanged. FR-3844. `cargo test --lib tc_3844` ·
  `cargo test --test web_board_e2e -- --ignored tc_3844`
- [x] T5 — CLI `board ask --edit [--file P]` owner-only; MCP `board_ask`
  rejects `edit`/`mode`. FR-3845. `cargo test --bin dmuxai tc_3845` ·
  `cargo test --lib tc_3845`
- [x] T6 — Viewer chat panel (board.js context hook, md-viewer.js panel,
  board.css), no timers or streams unless an answer is pending. FR-3840,
  NFR-3840. `cargo test --lib tc_3840` ·
  `cargo test --test web_board_e2e -- --ignored tc_3840`
- [x] T7 — Amend SPEC-board-ux-042 FR-3420. FR-3846.
  `cargo test --test spec_index tc_3846`
- [x] T8 — Docs (API.md, README, CHANGELOG), spec status, REVIEW;
  `bash scripts/check-spec.sh docs/specs/SPEC-md-viewer-chat.md`,
  `bash scripts/check-spec.sh docs/specs/SPEC-board-ux-042.md`,
  `python3 scripts/check-spec.py --all`.
