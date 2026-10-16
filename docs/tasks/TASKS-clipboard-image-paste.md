# TASKS: Clipboard image paste

Status: complete
Spec: [../specs/SPEC-clipboard-image-paste.md](../specs/SPEC-clipboard-image-paste.md)
Plan: [../plans/PLAN-clipboard-image-paste.md](../plans/PLAN-clipboard-image-paste.md)

- [x] T1 - Add `src/clipboard_image.rs` with `read_clipboard_png`,
  injectable `read_clipboard_png_with(&[ClipboardCmd])`, `save_attachment`,
  `decode_base64`, WSL detection, and the PowerShell base64 probe; register it
  in `src/lib.rs`.
  Requirements: FR-201, FR-202, FR-205, FR-206. Verify: `cargo test
  first_failed_command_falls_back_to_next`,
  `cargo test non_png_output_is_rejected`,
  `cargo test all_failed_commands_return_none`,
  `cargo test empty_output_is_rejected`,
  `cargo test base64_powershell_output_decodes_to_png`.
- [x] T2 - Add `ImageAttachmentStore::ingest_saved_file` (with 10 MiB limit)
  for already-saved clipboard files.
  Requirements: FR-203. Verify: `cargo test save_attachment_writes_png_into_workspace_attachments`.
- [x] T3 - Wire Ctrl+V / Alt+V in `src/app.rs` to
  `attach_clipboard_image_with`, save the PNG, register it in `pending_images`,
  and keep the no-op when the clipboard holds no PNG.
  Requirements: FR-203, FR-204. Verify: `cargo test
  clipboard_paste_registers_pending_image_chip`,
  `cargo test clipboard_paste_without_png_is_a_noop`.
- [x] T4 - Remove `--image` handling in `src/agent/cursor.rs` and
  `src/agent/agy.rs`; append `@<absolute path>` tokens via
  `agent::prompt_with_image_tokens`. Update `cursor_passes_image_paths_to_cli`
  and add `agy_passes_image_paths_to_cli`.
  Requirements: FR-207. Verify: `cargo test cursor_passes_image_paths_to_cli`,
  `cargo test agy_passes_image_paths_to_cli`.
- [x] T5 - Delete `docs/screenshots/*`, remove
  `export_documentation_screenshots`/`save_png` and the `ui_png` import from
  `src/app.rs`, update README and spec traceability.
  Requirements: FR-208. Verify: grep for `docs/screenshots` and
  `DMUXAI_EXPORT_SCREENSHOTS` returns only the historical-removal note, and
  `cargo test --all-targets` passes.
- [x] T6 - Run the Rust gates and write the review.
  Requirements: all. Verify: `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets`.
