# REVIEW: Dashboard parity with the TUI

Spec: [SPEC-dashboard-parity](../specs/SPEC-dashboard-parity.md) · Plan:
[PLAN-dashboard-parity](../plans/PLAN-dashboard-parity.md). Stacked on the
agent API (SPEC-agent-api, PR #35).

## Spec compliance

| Requirement | Status | Evidence |
| --- | --- | --- |
| FR-780 | Met | `dashboard_session_snapshot_has_contract_keys` (all keys; none without a command channel), `dashboard_session_transcript_is_capped` (100 messages, 32 KiB/message with marker, 512 KiB total, `serve` has no session) |
| FR-781 | Met | `command_route_contract` (12 actions parse, bad JSON/unknown command 400, 403/404/409/400 mapping, 401 without token, 409 in `serve`), `dashboard_commands_match_tui_actions`, `dashboard_commands_cover_models_share_and_policy`, `dashboard_cancel_targets_the_named_agent`, `dashboard_new_and_load_conversation` |
| FR-782 | Met | `dashboard_prompt_streams_into_transcript` (multi-line text kept, streaming then settled, TUI draft kept); browser run below |
| FR-783 | Met | `dashboard_slash_commands_via_prompt` (slash commands run while busy, no agent request; plain text busy → refused); palette in the browser run |
| FR-784 | Met | `tui_commands_have_dashboard_controls` (all 15 TUI slash commands incl. `/remember`/`/memory` from #33, `/graph` and 7 keys), `parity_matrix_controls_exist_in_session_js` |
| UX-780 | Met | `dashboard_commands_match_tui_actions` (busy refusals, `busy` flag), `session_module_has_parity_controls` (pickers disabled and Cancel shown while busy) |
| NFR-780 | Met | Own `session.js`/`session.css`; `app.css` untouched; `app.js` gains one `dmuxai:snapshot` dispatch so the page keeps a single SSE connection (`session_module_has_parity_controls`, `session_assets_are_served_hardened`) |

## Test evidence

Local Linux results:

- `cargo fmt --check` clean; `cargo clippy --all-targets --all-features --locked -- -D warnings` clean, also for `x86_64-pc-windows-gnu`.
- `cargo test --all-targets --locked`: 439 passed, 0 failed, 5 ignored.
- `scripts/spec-index.sh` lists 34 specs; IDs unique.
- Live run: `dmuxai --web` in a scratch workspace with a fake custom agent, driven by headless Chrome (Playwright): agent picker → `fake`, prompt sent with Enter, reply streamed into the transcript, Shift+Enter inserts a newline, input cleared after send, Ctrl+K palette lists 16 commands and filters (`gra` → `/graph`), Esc closes, `/` in an empty box opens the palette and Esc leaves the `/`, exactly one `/api/events` connection.

Screenshots: `docs/images/web-dashboard-chat.png`, `docs/images/web-dashboard-palette.png`.

## Cross-reviews (through dmuxai)

- cursor (composer-2.5) round 1, CHANGES REQUESTED: stale effort in the `set_effort` response; a second SSE connection from `session.js`; weak TC-786 and missing tests for share/load_models/Cursor pin/empty `allowed_runners`/effort success; per-publish `session_view` cost; share-while-busy differed from the TUI; inconsistent 403 wording; substring-based parity check; F2 row action. All fixed except the cost item (accepted, below).
- The parity test itself caught `/remember` and `/memory` (merged in #33 while this slice was in progress); both now have palette entries.

## Residual risk

- `session_view` is rebuilt on each dashboard publish in `--web` (≤ 5/s, ≤ 1/s while busy): about 100 message clones plus a conversation list refreshed every 5 s. Fine for normal sessions; an incremental transcript would be cheaper.
- Every SSE frame carries the whole snapshot including the transcript tail (≤ 512 KiB by the OQ-781 caps).
- The SSE token stays in the query string (pre-existing dashboard pattern; `no-referrer` set).
- The dashboard and the TUI share one session: a dashboard prompt switches the TUI's selected agent like `/agent` would.
- `/image` takes a workspace path only; no browser upload.

## Rollback

Revert the PR commit. No persisted data changes; `session` is an optional snapshot field that older pages ignore.
