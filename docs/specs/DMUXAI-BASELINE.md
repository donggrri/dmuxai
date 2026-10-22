# DmuxAI baseline product specification

Status: shipped baseline
Source of truth date: 2026-08-24
Implementation baseline: current `main` worktree
Purpose: describe the behavior already implemented so future changes can be
specified, planned, reviewed, and tested without reverse-engineering the code.

## 1. Product boundary

DmuxAI is a terminal UI that lets one user work with Codex, Cursor Agent, and
AGY in a single project workspace. It launches their structured headless
interfaces directly, normalizes their events for the UI, and stores a durable,
provider-neutral conversation transcript in SQLite.

It is not a terminal multiplexer, an ANSI-screen scraper, an authentication
manager, or a provider capability proxy. Each provider CLI must already be
installed and authenticated independently.

### Actors and terms

| Term | Definition |
| --- | --- |
| Workspace | The canonical directory from which DmuxAI starts; all durable conversations are scoped to it. |
| Runtime session | One live DmuxAI process lifetime, identified independently from its conversations and provider sessions. |
| Conversation | A DmuxAI-owned transcript with a stable UUID, title, ordered turns, and per-agent provider bindings. |
| Turn | One user prompt sent to one selected agent, with queued/running/completed/cancelled/failed/interrupted state. |
| Agent | One of Codex, Cursor, or AGY. Agent session continuity and selected model are independent. |
| Provider binding | The active or stale native thread/session/conversation ID for one `(conversation, agent)` pair. |
| Context projection | A rolling summary plus completed recent turns from all agents in the active conversation, used only when native session resumption does not cover the common conversation head. |
| Implement mode | The only user-selectable mode intended to let an agent modify files inside the workspace. |

### Explicit non-goals

- Completed turns are shared implicitly through the next-request context projection when switching agents; providers still keep separate native sessions.
- DmuxAI does not enable an unrestricted sandbox or a danger/full-access
  provider flag.
- The database is not a cross-workspace global chat list.
- Live provider behavior is not required for the deterministic test suite.
- Tool progress is transient and bounded; full tool arguments and output are
  not persisted in the canonical transcript.

## 2. Functional requirements

### 2.1 Workspace, startup, and providers

| ID | Priority | Requirement |
| --- | --- | --- |
| FR-001 | Must | On start, DmuxAI uses the current directory as the workspace and, when no live peer session exists, restores the most recently updated unarchived conversation for that canonical workspace; if none exists, it creates one. A live peer gets a fresh conversation as specified by FR-050. |
| FR-002 | Must | DmuxAI provides exactly the Codex, Cursor, and AGY adapters, routes the next ordinary prompt to the selected adapter, and presents provider output as normalized UI events. |
| FR-003 | Must | Each provider is launched through structured stdin/stdout protocols, never by interpolating user prompt text into a shell command. |
| FR-004 | Must | **Deferred (Windows dropped 2026-10-22).** On Windows, provider child processes are launched without flashing an additional console window. |
| FR-005 | Should | Provider executable paths may be overridden by `DMUXAI_CODEX_BIN`, `DMUXAI_CURSOR_BIN`, and `DMUXAI_AGY_BIN`; otherwise platform defaults are used. |
| FR-050 | Must | Given one DmuxAI process is live in a canonical workspace, when another process starts from that workspace, the new process starts in a different fresh conversation while the first process remains usable. Existing workspace conversations remain available through the conversation picker. |
| FR-051 | Must | A live peer's queued or running turns and provider bindings are not interrupted by another process's startup recovery; turns owned by a closed, expired, or legacy-unowned session recover as interrupted using the existing behavior. |
| FR-052 | Must | Runtime session ownership and liveness are persisted per process, normal shutdown closes the session, and the active UI refreshes its lease while running. |

### 2.2 Agent selection, modes, and models

> **Superseded (user-facing modes):** SPEC-acp-runners FR-1306..FR-1308 removed Ask/Plan/Implement
> pickers and `/mode`. The baseline rows below remain historical; shipped behavior is single
> agent mode (former Implement) plus internal read-only turns.

| ID | Priority | Requirement |
| --- | --- | --- |
| UX-001 | Must | `/agent` opens an agent picker. Changing the selected agent affects only later submissions; completed turns from the active conversation are automatically projected into the next request. |
| UX-002 | Must | `/mode` opens a mode picker for the selected agent and is rejected while that agent has an active turn. Changing an idle agent's mode retires only that agent's native binding, so the next prompt starts a fresh mode-scoped session with the completed shared context. Modes remain independent per agent. |
| INT-001 | Must | Codex Ask and Plan send `sandboxPolicy.type: "readOnly"`; Implement sends `sandboxPolicy.type: "workspaceWrite"`. Codex always uses an approval policy of `never`, and Plan prepends an instruction not to implement. |
| INT-002 | Must | Cursor maps Ask to `--mode ask`, Plan to `--mode plan`, and supported-platform Implement to `--force --sandbox enabled`; it uses `--print --output-format stream-json --stream-partial-output --trust --workspace`. |
| INT-003 | Must | AGY maps Ask/Plan to `--mode plan --sandbox` and Implement to `--mode accept-edits --sandbox`; Ask and Plan prompts explicitly prohibit implementation. |
| INT-060 | Must | **Deferred (Windows dropped 2026-10-22).** On Windows, selecting Cursor Implement is rejected before mode state or the Cursor provider binding changes, with an actionable platform limitation message; Cursor Ask and Plan remain selectable. |
| NFR-060 | Must | **Deferred (Windows dropped 2026-10-22).** A Cursor Implement request is rejected before command construction or child-process spawning on Windows; DmuxAI never falls back to an unsandboxed Cursor mode. |
| INT-061 | Must | On supported Linux platforms, Cursor retains its existing Ask, Plan, and Implement command mappings and NDJSON protocol. |
| UX-003 | Must | `F2` loads the selected agent's model catalog lazily. Model selection is per agent and a provider error is displayed rather than silently changing the model. In the Codex model picker, `Tab` selects the highlighted model and opens that model's effort picker. |
| FR-040 | Must | For idle Codex, `/effort` shows the selected model's advertised reasoning-effort values and applies the selection to subsequent Codex turns; other agents and active Codex turns are rejected without changing a setting. |
| FR-043 | Must | Changing an idle agent's mode retires only its active provider binding. The next prompt preserves completed DmuxAI context but does not resume the retired native session. |

### 2.3 Conversation and turn lifecycle

| ID | Priority | Requirement |
| --- | --- | --- |
| FR-010 | Must | `/new [title]` creates and selects a new conversation. A blank title uses `New conversation`; the first submitted prompt derives its title. |
| FR-011 | Must | `/conversations` and `F3` list conversations in the current workspace and load the selected transcript. `/resume <id-prefix>` loads exactly one matching conversation, otherwise reports no or ambiguous match. |
| FR-012 | Must | A conversation records immutable ordered user and assistant messages with turn state. A turn may transition from queued to running and then to one terminal state: completed, cancelled, failed, or interrupted. |
| BR-001 | Must | A selected agent may own at most one queued or running turn in one conversation. Submitting while it is busy preserves the prompt and shows an actionable error. Another idle agent may still work. |
| FR-013 | Must | A user turn and provider events are correlated by DmuxAI conversation ID, turn ID, and agent. An output whose stored ownership does not match is rejected, and a failed multi-output batch rolls back atomically. |
| FR-014 | Must | Provider session identifiers are stored independently by conversation and agent. A valid binding is reused only by the same agent, for the same conversation, when the native session is valid through the projected history. |
| FR-015 | Must | On startup recovery, queued and running turns in the current workspace become interrupted; partial assistant text remains visible and their provider bindings become stale. Other workspaces are unchanged. |
| FR-016 | Must | `Ctrl+C` requests cancellation only for the selected agent's active turn. Cancellation becomes a terminal UI and persisted state and invalidates the relevant provider binding. |
| FR-017 | Must | Switching to another conversation must not discard or misattribute delayed output from a live turn. Returning to the original conversation restores its durable state. |
| FR-041 | Must | Codex tool item start/completion events are correlated with their active turn and rendered as bounded transient chat progress; malformed or misowned items are ignored and full arguments/output are not persisted. |

### 2.4 Context continuity and explicit sharing

| ID | Priority | Requirement |
| --- | --- | --- |
| FR-020 | Must | When native resumption is valid, DmuxAI sends only the current prompt and resumes the provider-native session. |
| FR-021 | Must | When a native session does not cover the common conversation head, DmuxAI sends a shared-context envelope containing the rolling summary and completed recent turns from all agents, followed by a clearly delimited current request. |
| BR-002 | Must | The context projection is bounded to 32,000 Unicode characters. Its recent tail is bounded to 12,000 characters; the rolling summary budget is therefore 20,000 characters. |
| FR-022 | Must | When history exceeds the projection budget, DmuxAI creates a summary job and defers the requested turn once. A successful summary retries that deferred prompt; a failed or oversized summary restores it for the user. |
| FR-023 | Must | `/summarize` requests an incremental summary for the selected idle agent. It sends only completed new common turns since the latest valid checkpoint and never replaces the canonical transcript. |
| BR-003 | Must | A summary commit rejects stale source history, a busy agent, or rendered text over 20,000 Unicode characters. On success it stales only that `(conversation, agent)` native binding. |
| BR-004 | Must | The summary prompt treats prior summary and transcript as untrusted data, requests a JSON object with the documented fields, and commits only a parsable result. A summary output above 80,000 characters is cancelled and discarded. |
| FR-024 | Must | `/share <codex|cursor|agy>` remains an optional explicit context import for the named idle agent. Existing imports are excluded so repeated manual sharing does not recursively nest them; ordinary completed turns are already shared automatically. |

### 2.5 Terminal interaction and rendering

| ID | Priority | Requirement |
| --- | --- | --- |
| UX-010 | Must | Typing `/` shows matching slash commands. Up/Down selects a command, and Enter or Tab executes it. A whitespace-bearing command is treated as ordinary prompt text unless it is a documented argument form. |
| UX-011 | Must | The supported commands are `/agent`, `/mode`, `/effort`, `/status`, `/new`, `/conversations`, `/resume`, `/summarize`, `/share`, and `/quit`; invalid or incomplete argument forms show usage rather than invoking a provider. Submitting exactly `/quit` or selecting it from the slash preview exits through normal shutdown without starting a provider turn. |
| FR-042 | Must | `/status` for Codex displays active model/effort plus provider token activity and each available rate-limit window with used/remaining percentage and reset time; unavailable provider data is labeled and does not start or fail a turn. |
| UX-012 | Must | `Esc` closes an open overlay or slash preview; otherwise it leaves the application running and preserves the prompt input. `Esc` never exits the application. |
| UX-013 | Must | Chat scroll moves independently of the selected agent. The chat/prompt divider can be dragged while preserving minimum chat and input heights. |
| UX-014 | Must | Mouse selection operates on rendered terminal cells, is inclusive and direction-independent, preserves complete wide graphemes and combining sequences, and copies the result through the native clipboard. |
| NFR-001 | Must | UI-facing agent commands are non-blocking and bounded. A full command queue reports an error instead of blocking keyboard handling; provider events use a shared bounded channel. |
| NFR-002 | Must | Rendering is coalesced and the event loop yields to provider tasks so streaming output does not block terminal input. |

### 2.6 Persistence, platform behavior, release

| ID | Priority | Requirement |
| --- | --- | --- |
| BR-010 | Must | The SQLite database uses foreign keys, WAL mode, and transactional schema migration. A failed initial migration leaves no partially applied schema and an unknown newer schema version is rejected. |
| BR-011 | Must | Persistent conversation data includes conversations, turns, messages, provider bindings, context checkpoints, runtime sessions, and turn ownership. All are isolated by canonical workspace and appropriate conversation/agent/session keys. |
| BR-050 | Must | Each queued or running turn records the runtime session that created it; startup recovery only interrupts turns whose owner is missing, closed, or lease-expired. |
| BR-051 | Must | The at-most-one queued or running turn rule for one `(conversation, agent)` is enforced atomically across separate SQLite connections; separate instance conversations may run the same agent concurrently. |
| NFR-003 | Must | The application restores raw terminal mode, alternate screen, mouse capture, and cursor visibility on normal and error return paths. |
| NFR-050 | Must | Runtime-session storage is an additive transactional extension; provider request/event protocols and existing conversation data remain compatible. |
| FR-030 | Should | **Deferred (Windows dropped 2026-10-22).** Windows installation downloads a published release asset, verifies its SHA-256 checksum, installs `dmuxai.exe` under the current user's local application data, and adds that directory to the user PATH without requiring administrator privileges. |
| NFR-004 | Should | Linux defaults to `codex`, `cursor-agent`, and `agy`, and resolves Cursor's bundled Node entrypoint, when no override is supplied. |

## 3. Data and safety invariants

| ID | Invariant | Consequence of violation |
| --- | --- | --- |
| INV-001 | Turn sequence is unique and strictly ordered inside one conversation. | Reject/write rollback; never reorder transcript. |
| INV-002 | A provider binding is unique by `(agent, provider_session_id)` and scoped to one `(conversation, agent)`. | Never resume a session under a different owner. |
| INV-003 | A terminal turn cannot receive a new state-changing event. | Reject invalid output; preserve terminal state. |
| INV-004 | User prompt text is sent through process stdin or structured JSON, not shell interpolation. | Security regression; block review. |
| INV-005 | Ask and Plan modes do not grant write capability; only supported Implement modes are intended to modify workspace files. | Safety regression or an unsafe unsandboxed fallback; block review. |
| INV-006 | Canonical transcript remains intact when summary parsing, commit, or provider execution fails. | Preserve stored messages and restore deferred input where applicable. |

## 4. Acceptance scenarios and current evidence

The following matrix is intentionally phrased as Given/When/Then so a new test
can be generated without rediscovering intent. `Existing evidence` names the
current automated test or required manual command; future changes should keep
or replace this evidence deliberately.

| Test ID | Requirements | Given | When | Then | Existing evidence |
| --- | --- | --- | --- | --- | --- |
| TC-001 | FR-001, FR-010, FR-011 | A temporary database and workspace | Create, restart, list, and select conversations | Only that workspace's latest conversation is restored; title and messages persist | `tests/conversation.rs`: `completed_conversation_and_binding_survive_restart`; `src/app.rs`: `a_new_app_instance_loads_the_latest_conversation` |
| TC-002 | FR-002, UX-001, BR-001 | Test adapters and an idle application | Select each agent and submit; submit twice to a busy agent | Request reaches only selected adapter; second prompt is retained; idle other agent can start | `submitting_routes_to_the_selected_agent`; `another_idle_agent_can_start_after_switching_conversations` |
| TC-003 | INT-001, INT-002, INT-003, INT-060, INT-061, NFR-060, INV-005 | Constructed agent requests in every mode | Build Codex, Cursor, and AGY commands/payloads | Correct structured Codex sandbox type, mode flag, approval policy, and Plan/Ask instruction are present | Adapter mode unit tests in `src/agent/*.rs`, including `cursor_mode_validation_matches_platform` |
| TC-004 | UX-003 | Per-agent model catalogs | Load/select a Cursor model | Cursor selection changes without changing Codex selection | `model_catalog_and_selection_are_kept_per_agent` |
| TC-005 | FR-012, FR-013, INV-001, INV-003 | A stored turn and provider output envelope | Record valid stream events, then a mismatched or terminal event | Valid text is appended; mismatched batch rolls back; terminal state remains correct | `a_mismatched_output_rolls_back_the_entire_batch` and conversation lifecycle tests |
| TC-006 | FR-014, FR-020, FR-021, INV-002 | Same conversation with provider bindings and shared summary state | Start later same/different agent turns | Only a binding covering the common head resumes; otherwise shared all-agent context is injected | `provider_session_binding_is_reused_by_the_same_agent`; `switching_agents_automatically_shares_the_completed_context` |
| TC-007 | FR-015, FR-016, FR-017, INV-006 | Running/queued turns in two workspaces | Reopen one workspace, cancel selected agent, or switch conversations | Only affected current-workspace turn is interrupted/cancelled; partial output and correct ownership remain | `workspace_recovery_handles_all_live_turns_without_touching_other_workspaces`; `delayed_output_is_saved_to_its_original_conversation_only` |
| TC-008 | FR-022, FR-023, BR-002, BR-003, BR-004 | Long completed mixed-agent history | Request another turn or `/summarize` | Summary is incremental over common turns, bounded, committed only if fresh, binding staled, and deferred prompt retried/restored | `summary_checkpoint_is_used_with_a_gap_free_recent_tail`; `automatic_summary_retries_the_deferred_prompt_once`; `oversized_summary_is_rejected_without_invalidating_the_binding` |
| TC-009 | FR-024 | Conversation containing a prior explicit import | Share twice to another idle agent | New import contains canonical user/agent messages but not nested previous imports | `repeated_share_does_not_nest_previous_import_payloads` |
| TC-010 | UX-002, UX-010, UX-011, UX-012 | TUI test app with command input and overlays | Type `/`, select `/quit`, press Esc, supply malformed command | Preview/picker behavior is correct; `/quit` exits through the normal path; Esc only closes and never exits; malformed form gives usage | slash-command and overlay tests in `src/app.rs` |
| TC-011 | UX-013, NFR-002 | Rendered TUI with mouse events | Scroll and drag divider | Agent selection remains unchanged; pane sizes respect minima | `mouse_scroll_moves_chat_without_changing_agent`; `prompt_resize_keeps_minimum_chat_and_input_heights` |
| TC-012 | UX-014 | ASCII, wide, combining, backwards, and clipped selection lines | Extract selected text | Exact inclusive graphemes are returned, never a partial wide/combining character | `src/chat_selection.rs` tests |
| TC-013 | FR-003, FR-004, FR-005, NFR-004, INV-004 | Override and default executable values | Resolve program and build headless commands | Override wins; OS default is correct; no shell is used | `src/process/mod.rs` tests; adapter command tests |
| TC-014 | BR-010, BR-011 | Temporary database with a blocked migration and multiple workspaces | Open/migrate/recover | Migration rolls back atomically; workspace data remains isolated | `failed_initial_migration_rolls_back_every_schema_change`; recovery tests |
| TC-015 | NFR-003, FR-030 | Windows release or local terminal smoke environment | Start/exit or run installer | **Deferred (Windows dropped 2026-10-22).** Terminal is restored; installer checksum and installation flow succeed | Manual command documented in release review |
| TC-016 | NFR-001 | An adapter command channel at capacity and the shared event-channel constructor | Attempt another UI command and inspect channel capacity | The UI receives a queue-full error without blocking; the event channel remains bounded at 64 | Coverage gap: add deterministic adapter/channel capacity tests before changing queue behavior |
| TC-017 | FR-040, FR-041, FR-042 | Codex App Server fixtures and active-turn UI state | Select effort, forward tool lifecycle items, and request status | Effort is sent only on later Codex turns; owned tool progress is bounded; usage/rate-limit data or explicit unavailable text renders | `src/agent/codex.rs`: effort/tool/status tests; `src/app.rs`: effort/tool/status tests |
| TC-018 | UX-002, UX-003, FR-043 | Idle agent binding and Codex model catalog | Press Tab on a highlighted Codex model or change mode before a follow-up | Tab opens model-scoped effort choices; mode change starts a fresh context-projected provider session | `src/app.rs`: `model_picker_tab_selects_codex_model_and_opens_its_effort_picker`; `mode_change_reopens_the_agent_with_shared_context` |
| TC-019 | UX-011 | TUI test app with an idle prompt | Submit exactly `/quit` or select it from `/q` preview | Input is cleared, no provider turn starts, and the application enters its existing exit state | `src/app.rs`: `exact_quit_command_exits_without_starting_a_turn`; `slash_preview_can_execute_quit_with_enter`; `slash_preview_can_execute_quit_with_tab` |
| TC-020 | UX-012 | TUI test app with no, preview, and picker overlays | Press `Esc` in each state | The relevant overlay closes and the application remains running | `src/app.rs`: `escape_never_exits_the_application` |
| TC-050 | FR-050, FR-052 | One live app runtime and a temporary database | Start a second runtime from the same workspace | The second runtime selects a fresh conversation; the first conversation remains in the shared list | `src/app.rs`: `multiple_live_instances_start_in_separate_conversations` |
| TC-051 | FR-050, FR-051, BR-050 | First runtime has a queued turn and a fresh heartbeat | Start a second runtime, then deliver output through the first | The first turn remains queued/running and can complete without peer recovery | `src/app.rs`: `multiple_live_instances_start_in_separate_conversations` |
| TC-052 | FR-051, BR-050 | Closed and lease-expired session owners with partial output | Open a later engine and recover the workspace | Only orphaned turns become interrupted; partial text remains and bindings become stale | `tests/conversation.rs`: `closed_or_expired_session_turns_are_recovered` |
| TC-053 | FR-001, FR-052 | Completed conversation and cleanly closed prior runtime | Start a new runtime with no live peer | Latest conversation and transcript are restored | `src/app.rs`: `a_new_app_instance_loads_the_latest_conversation` |
| TC-054 | BR-051 | Separate engines select the same conversation and agent | Start a turn in engine A, then request one in engine B | Engine B receives `AgentBusy` and no duplicate turn is inserted | `tests/conversation.rs`: `busy_agent_is_rejected_across_runtime_sessions` |
| TC-055 | BR-051 | Two instance conversations in one workspace | Start the same agent in both | Both requests are accepted because busy state is conversation-scoped | `tests/conversation.rs`: `same_agent_can_run_in_separate_instance_conversations` |
| TC-056 | BR-010, BR-011, NFR-050 | Existing v1-shaped database with canonical data | Install the additive runtime-session schema and reopen | Schema/data remain readable and the user-version contract is preserved | `tests/conversation.rs`: `runtime_session_schema_extension_preserves_existing_data` |

## 5. Test execution contract

For code changes, the default deterministic gates are:

```powershell
cargo fmt --check
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
```

Authenticated provider smoke tests are intentionally ignored in normal runs.
They may be run only when the named CLI is installed and authenticated:

```powershell
cargo test real_codex_app_server_handshake -- --ignored --nocapture
cargo test real_codex_turn_streams_to_completion -- --ignored --nocapture
cargo test real_codex_turn_can_be_interrupted -- --ignored --nocapture
cargo test real_cursor_turn_streams_to_completion -- --ignored --nocapture
cargo test real_agy_turn_streams_to_completion -- --ignored --nocapture
```

Record a live test as `passed`, `failed`, `skipped` (with reason), or `blocked`
(with the failing prerequisite). Never treat skipped or blocked as passed.

## 6. Change protocol

Any change to this baseline must create a `docs/specs/SPEC-<slug>.md` from
`docs/templates/FEATURE-SPEC.md`, identify which IDs are modified, and state
whether the new behavior is backward-compatible. Changes to INV, BR, INT, or
NFR requirements require a test that demonstrates both the new result and the
preserved safety boundary.
