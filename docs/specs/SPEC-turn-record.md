# SPEC-turn-record

Status: Implemented

Owner: dmuxai
Created: 2026-10-24
Baseline: `1e813d8`
Related: #99, #100, #132; PR-6

## 1. Decision summary

Turn execution in DmuxAI combines user-visible intent with durable execution traces. Previously, when complex prompts were injected (e.g., skill interviews, graph step instructions, or `/plan` runs), the entire synthetic prompt was stored as both the prompt and the displayed label, overwriting user intent and corrupting conversation titles (#99, #100). Furthermore, tool and model thinking activities were stored only in memory during a live session, disappearing on application restart or conversation reload (#132).

PR-6 introduces the turn record: decoupling visible turn text (`display_text`) from the raw prompt dispatched to runner agents, allowing pre-dispatch title stabilization (`set_title_if_untitled`), and persisting turn activities (tools and thinking streams) in SQLite table `activities` across reload. Part A implements the engine layer: SQLite schema v3 migration, `BeginTurn.display_text`, `TranscriptMessage.shown_text()`, `set_title_if_untitled`, `ActivityItem` serde and merging, bounded durable activity persistence (`record_activity`), and activity projection in `ConversationSnapshot`.

## 2. Scope

### In scope

- SQLite schema version 3 migration (`migrate_to_v3`): nullable `messages.display_text` column and `activities` table with `ON DELETE CASCADE`.
- Turn display text in engine: `BeginTurn.display_text` storage in `messages.display_text` when non-empty, trimmed, and distinct from prompt.
- Conversation title derivation from `display_text` (fallback to `prompt`) for new conversations.
- `set_title_if_untitled` for active conversations to set title before prompt dispatch.
- `ActivityItem` and `ToolStatus` serde serialization/deserialization.
- Public `merge_activity` in `src/activity.rs` replicating app activity merging logic.
- Durable activity persistence via `record_activity`: transactional load, merge, and upsert with bounds (400 activities/turn, 16,000 chars text/detail, 200 chars summary).
- `ConversationSnapshot.activities` loading ordered by turn sequence and ordinal, gracefully skipping malformed JSON.

### Out of scope / non-goals

- App UI layer wiring of pending display text and deferred turns (Part B).
- App UI layer durable activity buffering and reload restoration (Part C).
- Interactive interview label rewrite cleanup in `src/app.rs` (Part B).

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-1900 | Activity rows survive restart and reload; UI projection is handled in App (part C). | dmuxai | Part A implementation |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Display text | User-facing label or prompt summary shown in the conversation transcript. | Raw prompt sent to the LLM agent. |
| Raw prompt | The exact prompt text transmitted to the model/adapter. | User-visible display text. |
| Activity row | Durable representation of a tool call or model thinking trace in SQLite. | Transient TUI message. |
| Ordinal | Zero-based index of an activity item within a single turn. | Message ordinal (0 for user, 1 for assistant). |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-1900 | Must | Given a conversation database with schema version < 3 | `ConversationEngine::open` applies `migrate_to_v3`, adding nullable `display_text` to `messages`, creating `activities` table with CASCADE delete on turn, and setting `user_version = 3` | Version > 3 is rejected with `ConversationError::UnsupportedSchema`; existing transcript messages are preserved with NULL `display_text` | TC-1900 |
| FR-1901 | Must | Given a `BeginTurn` request with `display_text: Option<String>` | `begin_turn` stores `display_text` in `messages.display_text` only when it is Some, non-empty after trim, and different from `prompt`; `TranscriptMessage.display_text` exposes it and `shown_text()` returns `display_text` if Some, else `text` | Stored as NULL when None, empty after trim, or equal to prompt; context projection and summaries continue using `messages.text` | TC-1901 |
| FR-1902 | Must | Given a conversation whose title is 'New conversation' and a `begin_turn` call | `begin_turn` sets the conversation title using `title_from_prompt(display_text or prompt)` | If title is already different from 'New conversation', title is unchanged | TC-1902 |
| FR-1903 | Must | Given an active conversation and user text | `set_title_if_untitled(text)` sets the title to `title_from_prompt(text)` and returns `true` only if current title is 'New conversation' and text is non-empty after trim | Returns `false` if text is blank after trim or title is not 'New conversation'; returns `Err(NoActiveConversation)` if no conversation is active | TC-1903 |
| FR-1904 | Must | Given an `ActivityItem` or `ToolStatus` | `ToolStatus` serializes/deserializes with snake_case; `ActivityItem` uses tagged serde (`type: "tool" \| "thinking"`); `merge_activity` replaces the most recent Running tool of same name or merges into live Thinking text/finish | If no matching Running tool or live Thinking exists, pushes a new item; returns the index updated or pushed | TC-1904 |
| FR-1905 | Must | Given an active turn ID and an `ActivityItem` | `record_activity(turn_id, item)` executes an immediate transaction to load existing rows, call `merge_activity`, and upsert the row at the returned ordinal with kind, summary, state, and payload_json | If `turn_id` is unknown, returns `ConversationError::InvalidOutput` and writes nothing | TC-1905 |
| FR-1906 | Must | Given activity recording with many items or long content | A new item beyond `MAX_ACTIVITIES_PER_TURN` (400) is ignored without error while existing row updates still apply; Thinking `text` and Tool `detail` are bounded to `MAX_ACTIVITY_TEXT_CHARS` (16,000) characters safely; summary is bounded to 200 characters | Payload JSON and summary truncate at character boundaries; turn activity count does not exceed 400 | TC-1906 |
| FR-1907 | Must | Given a conversation snapshot load or turn deletion | `ConversationSnapshot.activities` returns all turn activities ordered by turn seq then ordinal; malformed payload JSON rows are skipped; deleting a turn cascades and deletes all associated activities | Engine load does not fail on corrupted activity JSON; activities clean up automatically on turn deletion | TC-1907 |
| FR-1910 | Must | Given a skill interview turn | The full injected prompt is stored as `messages.text`, while the `[planning interview n/m] ...` label is stored as `display_text`; the live user message shows the label; after `restore_snapshot` the user message still shows the label, not the injected prompt | Display text survives reload; no leak of injected prompt into visible transcript | TC-1910 |
| FR-1911 | Must | Given a skill-/graph-/stage-prefixed prompt or a `/plan` or graph run start | The conversation title is the user's task text and never starts with an injected prefix such as "The user attached these skills" or `WRITE_STEP_NOTE`; `set_title_if_untitled` is called before any node turn starts | Title remains 'New conversation' only when no task text is available | TC-1911 |
| FR-1912 | Must | Given `pending_display_text` set and then a busy or refused submit | `pending_display_text` is consumed at the top of `try_submit_to_agent` and never leaks; the next normal turn's user message and stored row use its own prompt | No display text leaks across turns or early returns | TC-1912 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-1900 | FR-1900 | unit | A v2-shaped database with conversation and messages | Open with `ConversationEngine::open` | Schema migrated to user_version 3, display_text column exists and is NULL for old rows, activities table exists, old transcript unchanged, reopening is a no-op | `tests/conversation.rs` `tc_1900_v2_to_v3_migration_adds_display_text_and_activities` |
| TC-1901 | FR-1901 | unit | Engine with active conversation | Call `begin_turn` with display_text Some, None, and equal to prompt | Stored display_text matches rules, shown_text() returns display_text or prompt, context projection preserves sent prompt text | `tests/conversation.rs` `tc_1901_begin_turn_stores_display_text_and_preserves_prompt_projection` |
| TC-1902 | FR-1902 | unit | New untitled conversation | Call `begin_turn` with injected prompt and display_text label | Conversation title uses display_text label instead of prompt prefix | `tests/conversation.rs` `tc_1902_begin_turn_sets_conversation_title_from_display_text` |
| TC-1903 | FR-1903 | unit | Active conversation with default or custom title | Call `set_title_if_untitled` with blank, valid text | Returns true and updates title only when title was 'New conversation'; ignores blank text; errors when no conversation active | `tests/conversation.rs` `tc_1903_set_title_if_untitled_updates_only_new_conversation` |
| TC-1904 | FR-1904 | unit | Tool and thinking ActivityItems | Test serde round-trips and `merge_activity` operations | Serde JSON matches expected tags/snake_case; tool replace/push and thinking merge/finish rules produce exact indices and content | `src/activity.rs` `tc_1904_activity_item_serde_and_merge_rules` |
| TC-1905 | FR-1905 | unit | Conversation with an active turn | Call `record_activity` with tool running, ok, and thinking deltas | Rows upserted and survive engine reopen; unknown turn ID returns error | `tests/conversation.rs` `tc_1905_record_activity_merges_and_upserts_durable_rows` |
| TC-1906 | FR-1906 | unit | Active turn with long text and 400+ activities | Record 401 distinct items, long text, long detail | Cap at 400 ignores new items but allows existing updates; text and detail bounded to 16,000 chars; summary bounded to 200 chars | `tests/conversation.rs` `tc_1906_record_activity_enforces_bounds_and_caps` |
| TC-1907 | FR-1907 | unit | Turns with activities and malformed activity row | Load snapshot and delete turn | Snapshot activities ordered by turn seq then ordinal; malformed JSON skipped; turn deletion cascades activities | `tests/conversation.rs` `tc_1907_conversation_snapshot_activities_ordered_and_cascade` |
| TC-1910 | FR-1910 | integration | Skill interview turn with injected prompt | Submit interview turn, switch away and back (or reload) | Stored `messages.text` is the full injected prompt, `display_text` is the interview label; live and restored user message show the label | `src/app.rs` tests `tc_1910_*` |
| TC-1911 | FR-1911 | integration | Graph or `/plan` run starting from user task text | Start run and inspect title | Conversation title equals the user task text and does not start with injected prefixes | `src/app.rs` tests `tc_1911_*` |
| TC-1912 | FR-1912 | unit | `pending_display_text` set followed by busy/refused submit | Submit while busy, then normal submit | Next turn's user message and stored row use its own prompt with no leaked display text | `src/app.rs` tests `tc_1912_*` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Schema v3 | Additive nullable `display_text` column on `messages` and `activities` table with CASCADE delete | Backward incompatible for older binaries (user_version = 3 rejected by v0.2.2); rollback requires dropping activities table and restoring user_version = 2 | TC-1900 |
| Activity bounds | Hard cap of 400 activities per turn, 16,000 chars per text/detail, 200 chars summary | Prevents runaway disk usage or memory exhaustion from agent event floods | TC-1906 |
| Resilience | Malformed activity JSON payload skipped during snapshot load | Preserves conversation availability even if individual activity rows are damaged | TC-1907 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Schema v3 migration and display_text storage | `src/conversation.rs` | FR-1900, FR-1901, FR-1902 | `tc_1900`, `tc_1901`, `tc_1902` |
| 2 | `set_title_if_untitled` | `src/conversation.rs` | FR-1903 | `tc_1903` |
| 3 | Activity serde and `merge_activity` | `src/activity.rs` | FR-1904 | `tc_1904` |
| 4 | Durable activities and snapshot projection | `src/conversation.rs` | FR-1905, FR-1906, FR-1907 | `tc_1905`, `tc_1906`, `tc_1907` |
| 5 | App display text and pending_display_text | `src/app.rs` | FR-1910, FR-1911, FR-1912 | `tc_1910`, `tc_1911`, `tc_1912` |

## 8. Review and test exit criteria

- [x] All Must requirements (FR-1900..FR-1912) map to passing test evidence (TC-1900..TC-1912).
- [x] The diff has been reviewed against scope and non-goals.
- [x] Required Rust checks pass: format, test, and clippy with `-D warnings`.
- [x] `scripts/check-spec.py` passes and `tests/spec_index.rs` verifies spec indexing.
