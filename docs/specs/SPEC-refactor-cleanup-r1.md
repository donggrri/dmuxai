# SPEC-refactor-cleanup-r1

Status: Implemented

Owner: dmuxai
Created: 2026-10-24
Baseline: `376e86d`
Related: refactor scan findings #3 and #6; R-1

## 1. Decision summary

During live agent execution, tool calls and model thinking events are merged incrementally into activity lists. Previously, the merge logic was duplicated between `src/activity.rs` (`merge_activity`) and `src/app.rs` (`merge_activity_item` in `App`). Both implemented the exact same domain rule:
- A `Tool` replaces the most recent `Running` tool with the same name, or is appended if none exists.
- A `Thinking` event merges into the most recent in-progress thinking item (`finished_ms` is `None`), setting `finished_ms` if incoming has it, setting `started_ms` if existing is 0 and incoming non-zero, and appending incoming text when non-empty, or is appended if no live thinking exists.

Duplicating this rule in `src/app.rs` over `Message` structures created divergence risk between in-memory TUI presentation, offscreen turn buffers, the web dashboard, and SQLite durable storage (`ConversationEngine::record_activity`).

Refactor R-1 (Part A) extracts a generic merge engine `dmuxai::activity::merge_activity_in` parameterized by item view and constructor projections. `merge_activity` and `App::merge_activity_item` delegate to this single core implementation, ensuring identical behavior across in-memory transcripts, offscreen buffers, and durable storage.

## 2. Scope

### In scope

- Generic activity merge core `merge_activity_in` in `src/activity.rs`.
- Re-implementation of `merge_activity` in `src/activity.rs` as a delegation to `merge_activity_in`.
- Replacement of duplicate merge logic in `src/app.rs` with `dmuxai::activity::merge_activity_in`.
- Unit tests verifying that `merge_activity_in` skips non-activity items and preserves ordering and indices.
- Parity tests verifying that App's message merge and engine plain vector merge produce identical sequences.

### Out of scope / non-goals

- Refactor R-1 Part B (`PlanContext` deduplication, FR-1955+).
- Changes to `ActivityItem` serialization or database schema.
- Changes to UI rendering or tool status semantics.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-1950 | Merging activity items across non-activity rows (e.g. system or user messages) in `messages` must continue to skip non-activity messages and target the latest matching activity item. | dmuxai | Part A implementation |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Activity item | A structured tool execution or thinking trace event (`ActivityItem`). | Chat message text. |
| Generic activity merge | Core merge algorithm operating over `Vec<T>` with view and wrap closures (`merge_activity_in`). | Type-specific `merge_activity` or `merge_activity_item`. |
| Running tool | A tool item with status `ToolStatus::Running` eligible for replacement by subsequent status events. | Completed or failed tool item. |
| Live thinking | A thinking item with `finished_ms: None` eligible for delta text append and finish timestamp update. | Completed thinking item. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-1950 | Must | Given an incoming `ActivityItem` (Tool or Thinking) and a sequence of elements containing activity items | Single activity merge rule: App message list, offscreen buffer, dashboard and durable storage all use `activity::merge_activity_in`/`merge_activity`; behavior unchanged; a Tool replaces the most recent Running tool with the same name (or pushes if none exists); a Thinking merges into the most recent live thinking (`finished_ms: None`) (updating `finished_ms`, updating `started_ms` if 0, and appending text) or pushes if none exists | Non-activity elements in the collection are skipped by the search; indices returned reflect the exact updated or appended position | TC-1950 |
| FR-1951 | Must | Given an identical sequence of `ActivityItem` items applied to an App message list and a plain `Vec<ActivityItem>` | App merge equals engine merge for the same sequence; the sequence of `Role::Activity` items in `messages` equals the plain vector in order and content | Non-activity messages (user, agent, system) interleaved in `messages` do not alter the merge outcome or order of activity items | TC-1951 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-1950 | FR-1950 | unit | A `Vec` of an enum/struct with non-activity elements interleaved | Call `merge_activity_in` for Tool running, Tool completed (replace across non-activity rows), Tool push when none running, Thinking live, Thinking delta/finish (merge across non-activity rows), and Thinking push after finished | Tool replaces latest running tool of same name across non-activity rows or pushes; Thinking merges into live thinking across non-activity rows or pushes after finished; returned indices are correct | `src/activity.rs` `tc_1950_merge_activity_in_skips_non_activity_rows` |
| TC-1951 | FR-1951 | unit | An identical sequence of `ActivityItem` items (running tools, completed replacements, multiple tools, thinking start/deltas/finish/new) | Apply the sequence to an App message list via `merge_activity_item` and to a plain `Vec<ActivityItem>` via `merge_activity` | The `Role::Activity` items extracted from `messages` equal the plain vector in order and content exactly | `src/app.rs` `tc_1951_app_activity_merge_matches_engine_rule` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Activity merge | `merge_activity_in` is the single authoritative implementation of activity merging across TUI, offscreen buffers, dashboard, and SQLite storage | Fully backward-compatible; pure in-memory refactor; no persistence schema changes | TC-1950, TC-1951 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Add generic `merge_activity_in` and re-implement `merge_activity` | `src/activity.rs` | FR-1950 | `cargo test --lib activity` |
| 2 | Replace duplicate merge in `App` with `merge_activity_in` delegation | `src/app.rs` | FR-1950 | `cargo check` |
| 3 | Add unit test `tc_1950_merge_activity_in_skips_non_activity_rows` | `src/activity.rs` | FR-1950 | `tc_1950_merge_activity_in_skips_non_activity_rows` |
| 4 | Add parity test `tc_1951_app_activity_merge_matches_engine_rule` | `src/app.rs` | FR-1951 | `tc_1951_app_activity_merge_matches_engine_rule` |

## 8. Review and test exit criteria

- [x] All Must requirements (FR-1950..FR-1951) map to passing test evidence (TC-1950..TC-1951).
- [x] Duplicate activity merge rule eliminated from `src/app.rs`.
- [x] Single source of activity merge rule in `src/activity.rs` used by App and durable storage.
- [x] Required Rust checks pass: format, clippy (`-D warnings`), and tests.
- [x] Spec validation tools (`scripts/check-spec.py` and `scripts/check-tc-links.py`) pass with 0 errors.
