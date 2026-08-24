# TASKS: Codex effort, tool progress, and usage status

Status: complete
Spec: [SPEC-codex-effort-tool-usage.md](SPEC-codex-effort-tool-usage.md)

- [x] T1 — Extend agent contracts and Codex adapter request/event parsing for
  effort, tool lifecycle, and status. Requirements: FR-040, FR-041, FR-042,
  NFR-040. Verify: `cargo test codex_`.
- [x] T2 — Add `/effort` picker, `/status` command, status-line display, and
  scoped transient tool-progress rendering. Requirements: FR-040, BR-040,
  FR-041, BR-041, FR-042. Verify: `cargo test effort_picker` and
  `cargo test codex_tool_progress`.
- [x] T3 — Add deterministic success, malformed, boundary, and ownership
  tests. Requirements: TC-040, TC-041, TC-042, TC-043, TC-044, TC-045.
  Verify: `cargo test --all-targets`.
- [x] T4 — Validate docs and run repository gates. Requirements: all.
  Verify: `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath SPEC-codex-effort-tool-usage.md`, `cargo fmt --check`, `cargo test --all-targets`, and `cargo clippy --all-targets --all-features -- -D warnings`.
