# REVIEW: Memory default directory

## Spec compliance

FR-740 and the amended FR-720: omitted or empty `dir` resolves to `<vault>/memory`. Explicit `dir` still wins. Absolute paths and `..` are unchanged. README and the TUI off hint name `memory` as the default.

Non-goal held: no migration of an existing `cursor-workflow/memory` tree. A config that already sets `dir` keeps that path. Fixtures that pass `cursor-workflow/memory` explicitly were left in place.

## Evidence

```text
cargo test --lib resolve_root_rejects_absolute_and_dotdot
cargo test --lib memory_settings_resolve_root_dir_project_and_inject
cargo test --lib memory_settings_relative_vault_is_workspace_relative
cargo test --test memory_mcp
```

All passed (1, 1, 1, and 10 tests).

## Residual risk

A vault that already has notes only under `cursor-workflow/memory`, with `dir` omitted, will look empty until `dir` is set back to that path. The installed `dmuxai` binary is still the previous build until it is reinstalled.
