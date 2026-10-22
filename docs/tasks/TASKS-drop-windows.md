# TASKS: Drop Windows support (review-fix pass)

Status: Done
Created: 2026-10-22
Plan: ../plans/PLAN-drop-windows.md
Review: ../reviews/REVIEW-drop-windows.md

Follow-up tasks from the review of the Windows removal (branch `drop-windows`).
Workflows and repo config files (`dmuxai.toml`, `.dmuxai/`, `.pr.diff`) are out
of scope for this pass.

| # | Task | Requirement / artifact | Done |
| --- | --- | --- | --- |
| 1 | Baseline NFR-004 wording matches Linux behaviour: Cursor runs `cursor-agent` from PATH (`DMUXAI_CURSOR_BIN` override), no bundled Node entrypoint resolution | `docs/specs/DMUXAI-BASELINE.md` (NFR-004; see `src/process/mod.rs:36`) | [x] |
| 2 | `SPEC-windows-desktop.md`: every FR/NFR "Required observable result" cell prefixed `**Deferred (Windows dropped 2026-10-22).**`; test-table rows whose tests were deleted (`src/launcher`, `tests/windows_packaging.rs`) marked `Deferred (test removed with Windows support)` | docs-only; no FR requirements | [x] |
| 3 | `SPEC-cursor-windows-implement-guard.md`: same deferred markers for rows whose tests were deleted (Windows cursor guard tests) | docs-only; no FR requirements | [x] |
| 4 | Remove `check-spec.ps1` references from `scripts/check-spec.py` docstring and `.github/PULL_REQUEST_TEMPLATE.md` | docs/tooling-only | [x] |
| 5 | `CONTRIBUTING.md`: native Windows not supported; Windows cross-target lint in CI being removed | docs-only | [x] |
| 6 | Drop dead `#[cfg(not(unix))]` blocks and matching `#[cfg(unix)]` attributes so unix code is unconditional | `src/agent/custom.rs` `write_private_temp_file`, `src/web/discovery.rs` `write_discovery`, `src/settings.rs` `is_executable_file` | [x] |
| 7 | Rename `dunce_canonicalize` to `canonicalize_path`; update callers | `src/docs/confine.rs` | [x] |
| 8 | Historical note at top of `docs/ANALYSIS.md` | docs-only | [x] |
| 9 | Gates: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-targets` (with `CARGO_TARGET_DIR`); `bash scripts/check-spec.sh` on every changed spec | repo gates (AGENTS.md "Test") | [x] |
