# PLAN: open model lists

Status: in progress  
Spec: [SPEC-open-model-lists.md](../specs/SPEC-open-model-lists.md)

1. Policy: open `agy`, add `validate_model_for_runner_with(runner, id, allow_fast)` plus a process-wide `allow_fast` switch defaulting to false (FR-3000..FR-3003, TC-3000, TC-3001).
2. Config: user-level `[models] allow_fast`; workspace value ignored with a warning; binary entry points call the setter once (FR-3003, TC-3008).
3. Listing: dynamic `cursor-agent --list-models` and `agy models` with 10 s timeout, 60 s cache, static fallbacks, fast filtering (FR-3004..FR-3007, NFR-3000, TC-3002..TC-3006).
4. Surfaces: status `model_policy` text, graph node validation, F2/dashboard pickers, Cursor argv (FR-3008..FR-3010, TC-3009..TC-3012).
5. Update the tests that asserted the old AGY allow-list or the one-model Cursor listing; CHANGELOG and docs.

## Risks and rollback

- **Fast opt-in leaks.** Only the user config can enable it; tests use the pure `_with` variant so the process-wide switch is never flipped in unit tests.
- **Slow CLI.** Listing is bounded by 10 s and cached; validation never spawns.
- **Rollback.** Restore the AGY allow-list and the static one-model Cursor listing; remove `[models]`.
