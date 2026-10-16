# REVIEW: Clipboard image paste

Status: complete
Spec: [../specs/SPEC-clipboard-image-paste.md](../specs/SPEC-clipboard-image-paste.md)
Plan: [../plans/PLAN-clipboard-image-paste.md](../plans/PLAN-clipboard-image-paste.md)
Reviewed: 2026-10-17

## Result

Clipboard PNG paste (Ctrl+V / Alt+V) now saves accepted images under
`.dmuxai/attachments/clip-<uuid>.png` and registers them in the pending
attachment store, showing the standard `[Image #N]` chip. Reading goes through
`wl-paste`, then `xclip`, then a WSL-only PowerShell base64 probe; output is
accepted only when it starts with the 8-byte PNG signature. Cursor and AGY
requests no longer carry `--image` CLI arguments (neither CLI has such a flag);
live image paths are appended to the prompt text as `@<absolute path>` tokens.
Documentation screenshots and their export test were removed. No model name,
model-policy, sandbox, persistence-schema, or dependency change was made.

## Requirement review

- FR-201: fallback order implemented in `read_clipboard_png` /
  `read_clipboard_png_with` and injectable (FR-205). Covered by TC-101.
- FR-202: `stdout_starts_with_png` validates the 8-byte signature; empty,
  text, and failed output all reject. Covered by TC-102, TC-104, TC-108.
- FR-203: `save_attachment` + `ImageAttachmentStore::ingest_saved_file`
  (10 MiB limit preserved via `AttachmentError::TooLarge`) wire paste into the
  chip line. Covered by TC-106 and TC-107.
- FR-204: when no probe yields PNG bytes, `attach_clipboard_image_with` is a
  silent no-op; text paste (`Event::Paste`) and the `Char` handling were not
  modified. Covered by TC-103 and TC-108.
- FR-206: PowerShell writes base64 PNG to stdout (`-NoProfile`, headless,
  stdin nulled), decoded by the hand-rolled `decode_base64` with no new
  dependency. Covered by TC-105; live WSL path is TC-111 (manual).
- FR-207: `--image` loops removed from both adapters;
  `agent::prompt_with_image_tokens` appends `@path` tokens after the prompt.
  Covered by TC-109 (`cursor_passes_image_paths_to_cli`, new
  `agy_passes_image_paths_to_cli`).
- FR-208: `docs/screenshots/` deleted; `export_documentation_screenshots`,
  `save_png`, and the `ui_png` import removed from `src/app.rs`; README's
  regeneration section replaced by the clipboard paste instructions. Residual
  `docs/screenshots` mentions exist only in the dated
  `docs/graph-runs/image-attachment.md` historical report plus the note
  recording their removal. `src/ui_png.rs` remains a public library module.
  Evidence: grep in test evidence below.

## Spec-compliance and regression checks

- Persistence: no schema change; new `clip-*.png` files are inert data.
- Provider protocol: Codex path (via `prompt_with_image_references`) and the
  `runner_image_support_message` notice behavior are untouched;
  `RunnerImageSupport` now documents Cursor/Agy/Codex as `PromptReferences`,
  matching what the adapters actually do.
- Cancellation/error paths: clipboard probes are fail-through; a failed save
  surfaces as a system message and the pending store stays empty.
- Platform: WSL detection only on Linux targets reading `/proc/version`;
  absent tools fall through to the next probe.
- `#[ignore]` provider smoke tests remain ignored (5) and were not run.

## Unmet / residual risks

- TC-111 (real WSL + PowerShell clipboard) is not automatable in CI; the
  deterministic base64/injection tests cover the same code.
- A stuck clipboard tool (for example `xclip` waiting on selection ownership)
  would block the UI thread for as long as that tool runs; stdin is closed and
  the tools normally exit promptly, but a hard timeout was not added.
- `--image` handling in provider CLIs: if a future Cursor/AGY version restores
  a real image flag, the `prompt_with_image_tokens` helper is the single point
  to switch back.

## Test evidence

Commands run from the repository root with `~/.cargo/bin/cargo`:

- `cargo fmt --check`: passed.
- `cargo clippy --all-targets -- -D warnings`: passed (also verified with
  `--all-features`).
- `cargo test --all-targets`: 139 passed, 5 ignored, 0 failed
  (lib 21, app bin 96, `tests/conversation.rs` 19,
  `tests/model_policy_repo.rs` 3; ignored tests require authenticated provider
  CLIs).
- `grep -r export_documentation_screenshots|DMUXAI_EXPORT_SCREENSHOTS` over
  `src/`, `README.md`, `docs/`: no workflow references remain (only the
  historical-removal note in `docs/graph-runs/image-attachment.md`).
- `scripts/check-spec.ps1` could not run on this Linux host (no PowerShell
  binary); the same validation rules were replicated with a local Python check:
  `docs/specs/SPEC-clipboard-image-paste.md`: PASS (8 unique FR rows, 11 unique
  TC rows, all Must IDs mapped).

## Rollback

Plain revert of this change set. No data migration is involved; deleting
`.dmuxai/attachments/clip-*.png` files has no functional impact. Provider
protocol rollback restores the `--image` argument loops in
`src/agent/cursor.rs` and `src/agent/agy.rs`.
