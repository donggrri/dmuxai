# PLAN: Board item work-session links

Spec: [SPEC-board-work-session](../specs/SPEC-board-work-session.md)
(FR-3820..FR-3826, NFR-3820).

## Order

1. `src/board/mod.rs` — `SessionLink` (`kind`, ids, `workspace_path`,
   `agent`/`model`, `status`, `exit_code`, `started_ms`, `ended_ms`, `md_paths`),
   `SessionKind`, `SessionStatus`; `BoardItem.sessions` with `#[serde(default)]`
   so old items load; `SESSIONS_MAX = 50`, `SESSION_MD_PATHS_MAX = 50`; links are
   re-validated and masked in `mask_item_fields` (FR-3820, FR-3821).
2. `src/board/store.rs` — `EventKind::Session`; `append_session`,
   `finish_session` and `update_sessions` under the store lock (oldest links
   dropped past 50). Session events never wake `board wait`.
3. `src/board/session.rs` (new) — `git status` snapshots and the changed `.md`
   diff (workspace-relative, no `..`/absolute/`.dmuxai/`, sorted, ≤ 50);
   `finished_link` from the child JSON and run manifest
   (`run_journal::JournalStore::load`); `link_run` (FR-3826); `session_view`
   (FR-3823) reading bindings through the new read-only
   `conversation::read_binding_statuses` (agent + status, never the provider
   session id; no create/migrate).
4. `src/board/runner.rs` — `ChildOutcome.links` (conversation/graph id, agent
   from `run --json`); `run_item_execute` appends the `running` link and takes
   the "before" snapshot before spawning, fills the link (or marks it `failed`
   on spawn error) before `record_run_result`; `ask_item_execute` appends an
   `ask` link for both answer and `--revise` (FR-3820..FR-3822). All session
   writes are best effort; the run-lock/launch-token and `record_run_result`
   rules of SPEC-board-approval-hardening are unchanged.
5. Surfaces — CLI `board session`, `board link --run-id`; MCP `board_session`,
   `board_link`; HTTP `GET /api/board/{id}/session` on the shared board router
   (serve and `--web`).
6. Dashboard — `board.js` "Work session" section (newest first, href-less
   buttons, `textContent` only), `runs.js` `window.dmuxaiOpenRun`, `board.css`.
7. Tests from spec §5; docs (API.md, CHANGELOG), TASKS, REVIEW.

## Risks

- `md_paths` depends on `git status` before/after the child: a `.md` file that
  was already modified before the run and is modified again keeps the same
  status and is not reported. The graph file does not record spec-gate paths,
  so the "graph" part of the union is empty today.
- Two extra `git status` calls per board run (plus `git rev-parse` when the
  workspace is a sub-folder of the repository); negligible next to a run.
- Session writes bump the item version, so a bot's `--if-version` update racing
  a run start or finish gets a conflict (expected optimistic-lock behavior).

## Rollback

Revert the change set. Items keep an extra `sessions` field and
`events.jsonl` may contain `session` events; an older binary ignores the
unknown item field and skips event lines it cannot parse, so no data migration
is needed. Because it skips them when computing the next sequence number, an
older binary may reuse the `seq` of a trailing `session` event once; delete
`session` lines from `events.jsonl` after a rollback if bots rely on `board
events --since`. Conversation databases and run manifests are only read.
