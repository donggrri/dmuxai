# SPEC: Image attachments

## Requirements

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

## Non-goals

- `ratatui-image` inline preview in chat
