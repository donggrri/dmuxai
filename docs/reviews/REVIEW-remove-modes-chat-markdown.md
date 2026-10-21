# REVIEW: Remove modes, chat markdown, dashboard UI

## Spec compliance

| Req | Status | Evidence |
| --- | --- | --- |
| FR-1308 legacy modes | Met | `tc_1311_legacy_modes_section_is_ignored_with_warning` |
| FR-1401 chat markdown | Met | `tc_1401_chat_markdown_renders_gfm_and_sanitizes` |
| Plan/sensitive gates | Met | existing `app.rs` gate tests |
| UX-1402 picker + memory | Met | `index.html`, `session.js`, `memory.js`; embedded asset tests |
| Mode removal | Met | CLI rejects `--mode`; dashboard no MODE control |

## Residual risks

- Live-provider smoke tests unchanged (`#[ignore]`).
- `tests/web_running.rs` still references `#agents` spinner; agents card removed (may need follow-up if CDP test runs in CI).

## Test evidence

Recorded in PR description from `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features`.
