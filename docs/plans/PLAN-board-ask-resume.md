# PLAN: Ask in the work session (hybrid resume)

Spec: [SPEC-board-ask-resume](../specs/SPEC-board-ask-resume.md)
(FR-3830..FR-3836, NFR-3830). Builds on
[SPEC-board-work-session](../specs/SPEC-board-work-session.md) (`item.sessions`,
`work_session`) and
[SPEC-board-approval-hardening](../specs/SPEC-board-approval-hardening.md)
(run lock).

## Order

1. `src/conversation.rs`
   - `WorkSessionDigest` and `ConversationEngine::work_session_digest` (projected
     turns plus the latest stored summary document; `None` for an unknown id).
   - `set_skip_summary_jobs`, so a continuation turn never queues a summary job.
     That keeps it at one provider turn (FR-3830, NFR-3830).
2. `src/board/resume.rs` (new)
   - `ResumeMode` (`auto`/`native`/`fresh`/`off`) and `ResumeReport`.
   - `build_work_session_summary`: header `Work session summary`, ≤ 12 000
     chars, masked, step texts ≤ 1 500 chars, thread tail 10.
   - `summary_steps`, from the newest graph run of the conversation.
   - `board_summary_context` (md paths and thread).
   - `default_ask_agent` (OQ-3831) and `ask_answer_json`.
3. `src/app.rs`
   - `HeadlessContinuation`, set through `set_headless_continuation`, so the 40+
     `HeadlessRequest` literals stay unchanged.
   - `run_headless` selects the existing conversation, or creates a new one for
     a fork, and forces no-spec-first.
   - At turn start, `force_fresh` drops the provider session. A fresh turn's
     prompt is the summary, then the restored projection, then the request.
   - The FR-2325 fresh-retry uses the same seeded prompt and is reported as
     `summary`.
   - `HeadlessOutcome.resume`.
4. `src/cli.rs`
   - `run --conversation` (visible) plus hidden `--board-item`,
     `--fresh-session` and `--fork-summary`.
   - Usage validation runs before any store access.
   - `conversation_continuation` returns exit 3 / `not_found` for an unknown id
     and builds the summary.
   - JSON `resume`; MCP `run` `conversation_id` forwarding.
   - `board ask --resume`.
5. `src/board/runner.rs`
   - `AskParams.resume`.
   - `plan_ask_resume` handles the work session, OQ-3831 agent, live run,
     binding check and `no_active_binding` conflict.
   - `ask_resume_run_args` always passes `--read-only`.
   - `ask_item_report` / `ask_item_execute_report`: thread prefix, ask link
     conversation id, `ChildOutcome.resume_mode`.
6. Surfaces
   - MCP `board_ask` `resume` and `run` `conversation_id` schemas.
   - HTTP ask `resume`, `202` planned `resume`, `409` `reason`.
7. Dashboard: `board.js` selector and mode badge; `board.css`.
8. Tests from spec §5, plus CLI parse and HTTP tests.
9. Docs: API.md, CHANGELOG, help text, spec status, TASKS, REVIEW.

## Risks

- The summary-mode prompt also carries the existing restored projection, as
  FR-3830 requires ("fresh session with the projection"). The projection is not
  re-masked, the same as every other fresh turn today.
- The OQ-3831 agent comes from the graph file of the work session's run link.
  If the graph file is missing, the link's agent is used.
- An ask during a live run creates a new conversation per ask, so the
  conversation list grows.

## Rollback

Revert the change set. No schema or file format changes:

- conversation databases are only read, or extended by normal turns;
- board items keep using the existing `sessions` and thread fields;
- `resume` is an extra JSON key that older clients ignore.
