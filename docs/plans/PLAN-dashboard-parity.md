# PLAN: Dashboard parity with the TUI

Spec: [SPEC-dashboard-parity](../specs/SPEC-dashboard-parity.md). Stacked on
the agent API (`feat/agent-api`, SPEC-agent-api FR-765 `POST /api/prompt`).

## Module contracts

| Area | File | Contract |
| --- | --- | --- |
| Snapshot | `src/web/snapshot.rs` | `DashboardSnapshot.session: Option<SessionView>` (skipped when `None`); `SessionView`, `SessionAgent`, `SessionModel`, `SessionMessage`, `SessionConversation`, `SessionCommandInfo`; `cap_session_message`, `cap_session_transcript` and the OQ-781 limits |
| Hub | `src/web/hub.rs` | `DashboardHub::set_session(Option<SessionView>)` bumps the revision only on change; never feeds the event sink |
| Commands | `src/web/commands.rs` | `SessionCommand` (serde, `tag = "command"`, snake_case); `DashboardCommand::Session { command, reply: oneshot<Result<SessionView, RunTodoError>> }`; `RunTodoError::Invalid` (400) and `Missing` (404) |
| HTTP | `src/web/server.rs` | `POST /api/command` → 200 `{ok, session}`, 409 without a command channel, 504 after 10 s; `GET /assets/session.js`, `/assets/session.css` with the same headers as the other assets |
| App | `src/app.rs` | `session_view()` (conversation cache refreshed every 5 s or on switch); `handle_session_command()` reuses the TUI paths (`set_agent_mode`, model pin, `selected_effort_options`, `cancel_selected`, `create_conversation`, `select_conversation`, `share_conversation`, `request_status`, `toggle_all_activity_details`); `publish_dashboard()` sets the session only when a command channel is attached; dashboard prompts keep the TUI draft and let slash commands through while busy |
| Parity | `src/web/parity.rs` | `PARITY` rows `{tui, control, action}`, `control_for`, `page_has_control` |
| Page | `src/web/assets/session.js`, `session.css`, `index.html` | self-contained module (own token read, own SSE with polling fallback), `data-parity` ids per the matrix; `app.js`/`app.css` untouched |

## Ownership and order

1. Spec and plan (orchestrator).
2. Backend types, route, App handlers (hand-coded by the orchestrator while the
   primary implementer runner was unavailable).
3. Page module and styles (orchestrator).
4. Tests TC-780..787 and a live run against `dmuxai --web` with screenshots.
5. Cross-review by cursor (composer-2.5); fixes; PR stacked on `feat/agent-api`.
