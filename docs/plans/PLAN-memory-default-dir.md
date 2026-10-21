# PLAN: Memory default directory

## Change

Set `memory::DEFAULT_DIR` to `memory`. Update FR-720 text, the README layout, the TUI off hint, and tests that assume the omitted `dir` is `cursor-workflow/memory`.

## Order

1. Spec text (this plan's sibling spec and FR-720).
2. `DEFAULT_DIR` and call sites that join the old default path when `dir` is omitted.
3. Tests named by TC-740, then `cargo test` for `memory`, `settings`, and `memory_mcp`.

## Risks

Configs that omitted `dir` and already wrote `cursor-workflow/memory` will look at `<vault>/memory` after this change. Rollback is restoring `DEFAULT_DIR` to `cursor-workflow/memory`. No database migration.

## Non-goals

Do not rewrite explicit `dir` fixtures that are not asserting the default.
