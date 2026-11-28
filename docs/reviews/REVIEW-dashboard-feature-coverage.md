# REVIEW: dashboard feature coverage

Status: complete
Spec: `docs/specs/SPEC-dashboard-feature-coverage.md`

## Compliance

| Requirement | Evidence |
| --- | --- |
| FR-3750..FR-3753 | existing browser tests renamed; SPEC-chat-followup-queue TC-1230..1236 evidence updated |
| FR-3754, FR-3755 | `tests/web_dashboard_views.rs` |
| FR-3756 | `board_api.rs` `tc_3756_*`; `web_board_e2e.rs` `tc_3756_*` |
| FR-3757 | `web_board_e2e.rs` `tc_3757_*` |
| FR-3758 | `server.rs` `tc_3758_*`; `tests/web_attachments.rs` |
| FR-3759, FR-3760 | `web_session_sidebar.rs`, `web_workspace_dir.rs` |
| FR-3761, FR-3762 | `server.rs`, `assets.rs` tests; `skills.js`, `memory.js` send `X-Dmuxai-Human: 1` |
| FR-3763 | `tests/web_memory_panel.rs` |
| NFR-3750 | `tests/web_dashboard_coverage_meta.rs` + `tests/common/mod.rs` (55 s deadline, port 0, temp env, kill on drop) |

## Commands

```text
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test --lib -- web::
cargo test --test 'web_*'
cargo test --no-fail-fast --test 'web_*' -- --ignored --test-threads=1   # headless Chrome, all passed
```

## KNOWN-BUG (kept as passing tests, not fixed here)

- View-only `serve`: `POST /api/skills` still writes the file (TC-3761).
- View-only `serve`: More-panel write controls (e.g. Memory Save) stay enabled (TC-3755, TC-3763).
- Board rules panel does not show the server error text on a rejected save (TC-3756).
- No `DELETE /api/attachments/{name}` route; the chip remove is client-side only (TC-3758).

## Residual risk

- `web_all_projects` `tc_2984` failed once when all `web_*` ignored tests ran in parallel and passed alone and with `--test-threads=1` (pre-existing, untouched file).
- Playwright does not expose `Sec-Fetch-*` request headers; browser tests rely on the server accepting the request.
