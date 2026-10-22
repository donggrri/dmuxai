# SPEC: Clipboard image paste

Status: complete
Owner: DmuxAI maintainers
Created: 2026-10-17
Baseline: `cb38adf` (branch `cursor/image-attachment-c906`)
Related: [SPEC-image-attachment.md](SPEC-image-attachment.md), [docs/WORKFLOW.md](../WORKFLOW.md)

## 1. Decision summary

DmuxAI users screenshot things and want to send the picture on the clipboard to
an agent without touching a filesystem path. This change adds a clipboard PNG
reader with a platform fallback chain, stores accepted clipboard images as
workspace attachments, and shows the standard pending-image chip. Because
neither `cursor-agent` nor `agy` ships an `--image` flag, images are forwarded
to those providers as `@<absolute path>` tokens appended to the prompt text
instead of CLI arguments. The documentation screenshot PNGs and their export
test are removed because the shot files are no longer kept in repo.

## 2. Functional requirements

| ID | Priority | Requirement |
| --- | --- | --- |
| FR-201 | Must | Given a clipboard holding PNG data, when the user presses Ctrl+V or Alt+V in the prompt, then DmuxAI reads the PNG through `wl-paste`, then `xclip`, then (on WSL only) `powershell.exe`, in that order, using the first command that succeeds. |
| FR-202 | Must | Only command output that starts with the 8-byte PNG signature (`\x89PNG\r\n\x1a\n`) is accepted; every other result is treated as no image. |
| FR-203 | Must | Given an accepted clipboard PNG, when it is pasted, then the bytes are saved as `<workspace_root>/.dmuxai/attachments/clip-<uuid>.png` and registered in the pending attachment store so the prompt chip shows `[Image #N]`. |
| FR-204 | Must | Given a clipboard without PNG data, when Ctrl+V or Alt+V is pressed, then nothing changes: no attachment is registered, no error is raised, and text paste behavior stays unchanged. |
| FR-205 | Must | The clipboard fallback command list is injectable so tests can simulate the commands without a real clipboard. |
| FR-206 | Must | On WSL (`/proc/version` contains `microsoft`), the PowerShell fallback writes base64-encoded PNG to stdout, which DmuxAI decodes without a new dependency and validates against the PNG signature. |
| FR-207 | Must | Cursor and AGY requests never carry `--image` CLI arguments; image paths are appended to the text prompt as `@<absolute path>` tokens separated by spaces. |
| FR-208 | Should | The repo keeps no documentation screenshot PNGs and no test or doc flow references them. |

## 3. Scope

### In scope

- Clipboard PNG reading (`wl-paste`, `xclip`, WSL PowerShell), PNG validation,
  saving into `.dmuxai/attachments/`, pending-chip registration.
- Provider handoff: removing `--image` from Cursor/AGY and switching to `@path`
  prompt tokens; equivalent AGY test coverage.

### Non-goals

- Reading clipboard images in other formats, or resizing/re-encoding them.
- Inline image preview in the chat (`ratatui-image`).
- Any change to model names, model policy, or provider sandbox behavior.
- New network or binary dependencies (the base64 decoder is hand-rolled).

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-001 | Live WSL clipboard paste (TC-111) cannot run deterministically in CI; accepted as a manual-verification path. | maintainers | resolved |

## 4. Acceptance scenarios and current evidence

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-101 | FR-201 | unit | fake command list: first probe exits 1, second prints PNG bytes | `read_clipboard_png_with` | second probe's bytes are returned | `cargo test first_failed_command_falls_back_to_next` |
| TC-102 | FR-202 | unit | a probe prints non-PNG bytes | `read_clipboard_png_with` | `None` | `cargo test non_png_output_is_rejected` |
| TC-103 | FR-204 | unit | all probes fail or print junk | `read_clipboard_png_with` | `None` | `cargo test all_failed_commands_return_none` |
| TC-104 | FR-205 | unit | empty stdout from a successful probe | `read_clipboard_png_with` | `None` (no PNG signature) | `cargo test empty_output_is_rejected` |
| TC-105 | FR-206 | unit | PowerShell emits `iVBORw0KGgo=` | `decode_base64` | bytes decode to the 8-byte PNG signature | `cargo test base64_powershell_output_decodes_to_png` |
| TC-106 | FR-203 | unit | temp workspace | `save_attachment` | file written under `.dmuxai/attachments/clip-*.png` with original bytes | `cargo test save_attachment_writes_png_into_workspace_attachments` |
| TC-107 | FR-203 | integration | test app, fake `sh -c printf` PNG probe | `App::attach_clipboard_image_with` | pending store shows `[Image #1]`, file saved under the workspace attachments dir; a second paste yields `[Image #1] [Image #2]` | `cargo test clipboard_paste_registers_pending_image_chip` |
| TC-108 | FR-204, FR-205 | integration | test app, probes that fail or print junk | `App::attach_clipboard_image_with` | pending store stays empty and the prompt text is untouched | `cargo test clipboard_paste_without_png_is_a_noop` |
| TC-109 | FR-207 | unit | `AgentRequest` with two image paths | `configure_command` (Cursor, then AGY) | args contain no `--image`; `prompt_with_image_tokens` appends `@path` tokens after the prompt | `cargo test cursor_passes_image_paths_to_cli`, `cargo test agy_passes_image_paths_to_cli` |
| TC-110 | FR-208 | review | repo tree | grep/lint pass | no `docs/screenshots` files remain and no `DMUXAI_EXPORT_SCREENSHOTS`/`export_documentation_screenshots` test references remain | see test evidence in `REVIEW-clipboard-image-paste.md` |
| TC-111 | FR-201, FR-206 | manual | WSL session with a screenshot in the clipboard | Ctrl+V in the prompt | chip `[Image #1]` appears and the image lands under `.dmuxai/attachments/` | manual only: requires a live WSL + PowerShell clipboard |

TC-111 is manual because a live WSL + PowerShell clipboard runtime cannot be
exercised deterministically in CI; FR-205's injectable
commands plus TC-105's base64 validation cover the same code deterministically.

## 5. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | Clipboard images become new files under `.dmuxai/attachments/clip-*.png`; no schema change | Files are inert; deleting them loses nothing beyond that turn | TC-106, TC-107 |
| Provider protocol | Cursor/AGY prompts gain `@path` tokens and `--image` args disappear; Codex unchanged | Providers accept `@path` references in text; rollback restores the flag loop | TC-109 |
| Security / permissions | Clipboard probes read stdout into memory only; PowerShell runs headless with `-NoProfile` and no network | No new process surface beyond the existing clipboard tools | TC-101 |
| Platform | Wayland → X11 → (WSL) Windows clipboard order; the WSL check reads `/proc/version` | Missing tools fall through to the next probe or to no-op | TC-101, TC-104 |

## 6. Implementation plan

See [../plans/PLAN-clipboard-image-paste.md](../plans/PLAN-clipboard-image-paste.md)
and [../tasks/TASKS-clipboard-image-paste.md](../tasks/TASKS-clipboard-image-paste.md).

## 7. Review and test exit criteria

- [x] All Must requirements map to passing evidence (TC-111 documented as manual).
- [x] The diff has been reviewed against scope and non-goals.
- [x] Required Rust checks pass: format, all targets test, and clippy.
- [x] External-provider smoke tests remain marked `#[ignore]` and were not run.
- [x] [../reviews/REVIEW-clipboard-image-paste.md](../reviews/REVIEW-clipboard-image-paste.md)
  records commands, result, residual risk, and rollback notes.
