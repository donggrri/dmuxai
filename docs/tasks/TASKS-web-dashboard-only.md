# TASKS: `--web` runs the dashboard only (no TUI)

Spec: [SPEC-web-dashboard-only.md](../specs/SPEC-web-dashboard-only.md)

| Done | Task | Requirement IDs | Test command |
| --- | --- | --- | --- |
| [x] | Headless `--web` run loop: no TUI, no TTY needed, URL on stdout, `web.json` published | FR-1600 | `cargo test --test web_dashboard_only tc_1600_web_runs_without_tty` |
| [x] | SIGINT shuts down headless `--web` with exit 0 and removes `web.json` | FR-1601 | `cargo test --test web_dashboard_only tc_1601_web_exits_on_sigint` |
| [x] | Plain `dmuxai` still needs a TTY; `--help` describes `--web` as no-TUI | FR-1602, FR-1603 | `cargo test --test web_dashboard_only tc_1602_plain_dmuxai_needs_tty_and_help_mentions_no_tui` |
| [x] | Review-round dead-code and doc cleanup: delete the unused `HEADLESS_DASHBOARD_ENV` copy in `src/main.rs`; `workspace.rs` doc marks the var as kept for older child binaries only | FR-1600 | `cargo clippy --all-targets --all-features -- -D warnings` |
| [x] | Headless dashboard-start failure (non-`InvalidInput`) goes to stderr and exits 2 instead of running UI-less | FR-1600 | manual (startup-failure injection is not scripted); recorded in REVIEW |
| [x] | `tests/web_dashboard_only.rs` sets `DMUXAI_STATE_DIR` to the temp state dir (portable with `tests/web_workspace_open.rs`) | FR-1600 | `cargo test --test web_dashboard_only` |
| [x] | README `--web` lines updated to dashboard only | FR-1600, FR-1603 | manual read-through |
| [x] | Manual real-terminal runs: dashboard live with no TUI; grok and cursor answered; switch away and back reconnected with no TUI | FR-1600, FR-1601, FR-1604 | TC-1603, TC-1604 (manual) |
