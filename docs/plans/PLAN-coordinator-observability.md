# PLAN: coordinator observability

Status: complete
Spec: [SPEC-coordinator-observability.md](../specs/SPEC-coordinator-observability.md)

1. Settings: accept `run_started`, `awaiting_approval`, `spec_review_requested` in `[board] notify_events` (FR-3853).
2. Board store: new `EventKind`s with optional event detail (`plan_hash`, `plan_revision`, `review_id`, `spec_paths`) flattened into `events.jsonl`, `board events` and `board wait`; `wait_for any` matches the new kinds, `decision`/`message` do not (FR-3854).
3. `awaiting_approval`: item field `awaiting_approval_revision` marks the announced plan revision; `add`, `move`, `plan` (attach/edit) emit once per revision while in `needs_decision` (FR-3851).
4. `run_started`: `BoardStore::record_run_started`, called by `run_item_execute` right before the child starts; a failed event write does not stop the run (FR-3850, NFR-3850).
5. Notify: payload gains `plan_revision`, `review_id`, `spec_paths`; `board::notify_spec_review_requested` hook matches the item by `run_ids`, adds a `spec_review` thread message and the event, logs errors to `board/notify.log` (FR-3852).
6. Runs view: move list/detail logic from `src/web/runs_api.rs` to a non-web `src/runs_view.rs`; the HTTP handlers, the CLI and MCP share it (FR-3855, FR-3856).
7. CLI `runs list|show` and MCP `runs_list`/`runs_show`; the legacy `runs [--limit] [--failed] [--json]` output is unchanged (FR-3855, FR-3856).
8. Docs: docs/API.md, README, CHANGELOG; TC-3857 doc check (FR-3857).

## Risks and rollback

- **Board file format.** New event lines and the item field are additive (`serde(default)`, skipped when empty). The new binary reads the last `seq` from the `seq` field alone, so any future kind keeps `seq` monotonic. Older binaries skip lines with unknown kinds, including when computing the next `seq`: after a downgrade, if the newest lines are new kinds, an old binary can reuse those `seq` numbers. Rollback: downgrade only after a write of an existing kind, or truncate/rotate `board/events.jsonl` (events are a log; items are untouched).
- **Wake semantics.** `board wait --any` wakes on new kinds even when written by another bot, which can wake bots that previously slept through runner-only changes. Self-writes are still filtered.
- **Shared runs logic.** `GET /api/runs` and `/api/runs/{id}` now call `runs_view`; covered by `tests/serve_runs_api.rs`.
- **Rollback.** Revert the new subcommands, MCP tools and event kinds; no data migration.
