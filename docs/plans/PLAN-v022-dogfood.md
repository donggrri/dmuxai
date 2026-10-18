# PLAN: v0.2.2 dogfood fixes

Status: in progress
Specs: [SPEC-grok-runner §9](../specs/SPEC-grok-runner.md) (FR-980), [SPEC-running-indicators](../specs/SPEC-running-indicators.md) (FR-981, FR-982), [SPEC-first-run](../specs/SPEC-first-run.md) (FR-983), [SPEC-web-dashboard](../specs/SPEC-web-dashboard.md) (FR-984)
Source: dogfooding notes in `docs/reviews/REVIEW-dashboard-full-parity.md` (PR #68), issues 2-5. Issue 1 (missing separator between grok text segments) is handled by PR #72 (`fix/grok-runner-exit`).

## Order

| Order | Change | Files | IDs | Tests |
| --- | --- | --- | --- | --- |
| 1 | Grok tool-call tracker (pure, no I/O) | new `src/grok_activity.rs`, `src/lib.rs` | FR-980 | TC-980 |
| 2 | Wire the tracker into the custom adapter read loop for the built-in grok preset only; fixture mode | `src/agent/custom.rs` (one insertion before `parse_jsonl_line`), `tests/fixtures/fake-grok.sh` | FR-980 | TC-981 |
| 3 | `AgentView.activity` from the TUI activity detail; chip and status pill | `src/web/snapshot.rs`, `src/app.rs`, `src/web/assets/*.js` | FR-981 | TC-982, TC-983 |
| 4 | Cancel notice: `cancel_selected` records the cancelled turn and appends the line once | `src/app.rs` | FR-982 | TC-984, TC-985 |
| 5 | Welcome text from the runner registry | `src/app.rs` | FR-983 | TC-816 |
| 6 | Local-only startup URL notice | `src/app.rs`, `src/main.rs` | FR-984 | TC-986, TC-987 |
| 7 | CHANGELOG (Unreleased), review doc | `CHANGELOG.md`, `docs/reviews/REVIEW-v022-dogfood.md` | all | — |

## Risks

- Conflicts with PR #72, which edits the same read loop in `src/agent/custom.rs` and `ParsedLine`. The tracker lives in its own module and `ParsedLine` is not touched; the loop gets one insertion. Rebase onto main if #72 merges first.
- Cancel: the adapter's late `Cancelled` event is dropped today because `cancel_selected` already removed the active turn. The note must not be duplicated if that changes.
- Token: the notice must stay visible in the TUI. A flag on the message (local-only) is filtered out wherever messages leave the TUI (snapshot, session view, share/context, persistence).

## Rollback

All changes are UI/adapter-local. No schema change. Revert the squash commit.
