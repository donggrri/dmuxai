# PLAN: Clipboard image paste

Status: complete
Spec: [../specs/SPEC-clipboard-image-paste.md](../specs/SPEC-clipboard-image-paste.md)
Created: 2026-10-17

## Implementation order

1. New library module `src/clipboard_image.rs` (registered in `src/lib.rs`)
   with `read_clipboard_png`, `read_clipboard_png_with(&[ClipboardCmd])`,
   `save_attachment`, the hand-rolled `decode_base64`, and the
   strategy/WSL/PowerShell construction. Requirements FR-201, FR-202, FR-206,
   FR-205; tests TC-101..TC-105.
2. `src/attachments.rs`: add `ImageAttachmentStore::ingest_saved_file` so an
   already-saved clipboard PNG is registered without a second copy, keeping the
   `[Image #N]` index behavior. Requirements FR-203; part of TC-107.
3. `src/app.rs`: intercept Ctrl+V / Alt+V in `handle_key` after picker handling,
   route through injectable `attach_clipboard_image_with`; no-op when no PNG is
   found. Requirements FR-203, FR-204; tests TC-107, TC-108.
4. Provider adapters: drop the `--image` arg loops in `src/agent/cursor.rs` and
   `src/agent/agy.rs`; instead append `@<absolute path>` tokens via the new
   `agent::prompt_with_image_tokens` helper before writing stdin / the AGY JSON
   message. Update `cursor_passes_image_paths_to_cli`, add
   `agy_passes_image_paths_to_cli`. Requirement FR-207; test TC-109.
5. Docs cleanup: delete `docs/screenshots/*`, remove
   `export_documentation_screenshots` / `save_png` / the `ui_png` import in
   `src/app.rs`, and update README + spec traceability. Requirement FR-208;
   test TC-110. `src/ui_png.rs` stays a public module.
6. Run gates: `cargo fmt`, `cargo clippy --all-targets -- -D warnings`,
   `cargo test --all-targets`.

## Risks

- Blocking clipboard subprocess calls run on the UI thread; they are short
  (one probe at a time, headless with stdin closed) and only execute on an
  explicit paste keypress.
- Provider prompt-text images depend on the provider understanding `@path`
  tokens; scope is limited to Cursor/AGY, which already accept that convention
  in prompts.
- `docs/specs/SPEC-image-attachment.md` traceability is updated in the same
  change so no doc points at the removed screenshot test.

## Rollback notes

No persistence schema changes; the added `clip-*.png` files are inert data.
Rolling back is a plain revert of this change set; provider protocol rollback
restores the `--image` loop in the two adapters.
