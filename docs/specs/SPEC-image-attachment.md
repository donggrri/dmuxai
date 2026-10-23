# SPEC: Image attachments

Status: Implemented
Related: `docs/TEST-PLAN.md` §1 (M5, FR-IMG-1..5 traceability); [SPEC-clipboard-image-paste.md](SPEC-clipboard-image-paste.md)

## 4. Requirements

*Note: Numeric requirement IDs FR-1890..FR-1895 are aliases for legacy non-numeric IDs FR-IMG-1..FR-IMG-5 and UX-IMG-1.*

| ID | Priority | Requirement |
| --- | --- | --- |
| FR-1890 | Must | Same as FR-IMG-1 (User can attach `png`/`jpg`/`webp` via `/image <path>`) |
| FR-1891 | Must | Same as FR-IMG-2 (Attached files copy under `.dmuxai/attachments/` with 10 MiB limit) |
| FR-1892 | Must | Same as FR-IMG-3 (Submit sends `image_paths` on `AgentRequest`) |
| FR-1893 | Must | Same as FR-IMG-4 (Codex turn text includes `@path` references) |
| FR-1894 | Must | Same as FR-IMG-5 (Cursor and AGY receive images as `@<path>` tokens appended to the prompt) |
| UX-1895 | Should | Same as UX-IMG-1 (Pending attachments visible in prompt chrome) |

### Legacy id map

| ID | Requirement | Tests |
| --- | --- | --- |
| FR-IMG-1 | User can attach `png`/`jpg`/`webp` via `/image <path>` | `image_slash_command_attaches_supported_file` |
| FR-IMG-2 | Attached files copy under `.dmuxai/attachments/` with 10 MiB limit | `attachments::rejects_oversized_images`, `copies_supported_image_into_workspace_store` |
| FR-IMG-3 | Submit sends `image_paths` on `AgentRequest` | `submit_prompt_forwards_image_paths_to_adapter` |
| FR-IMG-4 | Codex turn text includes `@path` references | `codex` unit tests via `prompt_with_image_references` |
| FR-IMG-5 | Cursor and AGY receive images as `@<path>` tokens appended to the prompt (their CLIs have no `--image` flag) | `cursor_passes_image_paths_to_cli`, `agy_passes_image_paths_to_cli` |
| UX-IMG-1 | Pending attachments visible in prompt chrome | manual verification: the chip line is rendered from `ImageAttachmentStore::chip_line` into the prompt title and `clipboard_paste_registers_pending_image_chip` covers the store data; screenshot PNG exports were removed with `docs/screenshots/` |

Clipboard attachment requirements live in
[SPEC-clipboard-image-paste.md](SPEC-clipboard-image-paste.md).

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Test |
| --- | --- | --- | --- |
| TC-1890 | FR-1890 | unit | `src/app.rs` `image_slash_command_attaches_supported_file` |
| TC-1891 | FR-1891 | unit | `src/attachments.rs` `attachments::rejects_oversized_images`, `copies_supported_image_into_workspace_store` |
| TC-1892 | FR-1892 | unit | `src/app.rs` `submit_prompt_forwards_image_paths_to_adapter` |
| TC-1893 | FR-1893 | unit | `src/attachments.rs` `prompt_with_image_references` |
| TC-1894 | FR-1894 | unit | `src/agent/cursor.rs` `cursor_passes_image_paths_to_cli`, `agy_passes_image_paths_to_cli` |
| TC-1895 | UX-1895 | manual | `src/app.rs` `clipboard_paste_registers_pending_image_chip`, manual verification |

## 6. Non-goals

- `ratatui-image` inline preview in chat
