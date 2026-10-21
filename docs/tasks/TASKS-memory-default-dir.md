# TASKS: Memory default directory

- [x] T1 — FR-740: default dir is `memory`. Tests: `cargo test --lib resolve_root_rejects_absolute_and_dotdot memory_settings_resolve_root_dir_project_and_inject memory_settings_relative_vault_is_workspace_relative`
- [x] T2 — INT-729 paths that omit `--dir` read and write `<vault>/memory`. Test: `cargo test --test memory_mcp`
