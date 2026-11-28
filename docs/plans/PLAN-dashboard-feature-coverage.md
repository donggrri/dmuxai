# PLAN: dashboard feature coverage

Spec: `docs/specs/SPEC-dashboard-feature-coverage.md`

## Order

1. Rename the untagged browser tests to TC-3750..TC-3753 (`web_chat_copy`, `web_chat_single_send`, `web_model_menu`, `web_followup_queue`) and point SPEC-chat-followup-queue TC-1230..1236 at them.
2. FR-3762: make `human_browser_rejection` (`src/web/board_api.rs`) `pub(crate)`, call it from the skills/memory approval handlers in `src/web/server.rs`, send `X-Dmuxai-Human: 1` from `skills.js` and `memory.js`; update the TC-728 tests.
3. HTTP/unit tests TC-3756 (rules PUT), TC-3758 (attachments API), TC-3761 (skills map), TC-3762.
4. Shared browser helper `tests/common/mod.rs`; browser tests TC-3754/3755/3756/3757/3758/3759/3760/3763.
5. Meta test TC-3764 (`tests/web_dashboard_coverage_meta.rs`).

## Risks

- Product bugs found by the new tests are kept as `KNOWN-BUG` output, not fixed (see the review).
- Scripts that called the approval endpoints without browser headers now get 403 (intended, FR-3762).

## Rollback

Revert the handler guard in `src/web/server.rs` and the two asset headers; no persisted data or provider protocol changes.
