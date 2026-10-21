# SPEC: Memory default directory

Status: approved
Owner: dmuxai
Created: 2026-10-22
Baseline: shipped `[memory]` in SPEC-obsidian-memory FR-720
Related: FR-720

## 1. Decision summary

When `[memory] vault` is set and `dir` is omitted, DmuxAI stores notes under `<vault>/memory`. The previous implicit directory `cursor-workflow/memory` is no longer the default. An explicit `dir` is unchanged.

## 2. Scope

### In scope

- Default `dir` is the relative path `memory`.
- User-facing copy that states the default (`README`, TUI "memory is off" line) matches that path.
- FR-720 in SPEC-obsidian-memory states the same default.

### Out of scope / non-goals

- Moving or migrating an existing `cursor-workflow/memory` tree.
- Changing validation of absolute paths or `..`.
- Renaming the vault root itself.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| — | The requested shape `vault/memory` means `<vault>/memory`, not a folder named `vault` inside the vault. | user | Spec approval |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| vault | Existing directory named by `[memory] vault` | A child folder named `vault` |
| dir | Path relative to the vault; default `memory` | The vault path |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-878 | Must | Given a valid `[memory] vault` and no `dir`, or `dir` empty or whitespace | Memory root is `<vault>/memory` and the reported dir is `memory` | An explicit relative `dir` is used as before; absolute or `..` still disables memory with a config warning | TC-878 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-878 | FR-878, FR-720 | unit | Temporary vault directory | `resolve_root` with omitted and empty `dir`; settings load with vault only | Root is `<vault>/memory`; dir string is `memory` | `src/memory.rs` `resolve_root_rejects_absolute_and_dotdot`; `src/settings.rs` `memory_settings_resolve_root_dir_project_and_inject`, `memory_settings_relative_vault_is_workspace_relative` |

## 6. Data, integration, and safety contracts

| Area | Contract | Evidence |
| --- | --- | --- |
| Persistence | New default writes go to `<vault>/memory/`. Existing trees stay where they are until `dir` points at them. | TC-878 |
| Compatibility | Configs that already set `dir = "cursor-workflow/memory"` keep that root. | existing TC-720 override case |
