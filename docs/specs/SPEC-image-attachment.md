# SPEC: Image attachments

## Requirements

| ID | Requirement | Tests |
| --- | --- | --- |
| FR-IMG-1 | User can attach `png`/`jpg`/`webp` via `/image <path>` | `image_slash_command_attaches_supported_file` |
| FR-IMG-2 | Attached files copy under `.dmuxai/attachments/` with 10 MiB limit | `attachments::rejects_oversized_images`, `copies_supported_image_into_workspace_store` |
| FR-IMG-3 | Submit sends `image_paths` on `AgentRequest` | `submit_prompt_forwards_image_paths_to_adapter`, cursor/agy CLI tests |
| FR-IMG-4 | Codex turn text includes `@path` references | `codex` unit tests via `prompt_with_image_references` |
| UX-IMG-1 | Pending attachments visible in prompt chrome | `export_documentation_screenshots` (manual PNG) |

## Non-goals

- Clipboard image bytes (only filesystem paths)
- `ratatui-image` inline preview in chat
