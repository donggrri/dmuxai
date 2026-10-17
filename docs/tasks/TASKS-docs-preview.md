# TASKS: Document preview and edit

| Done | Owner | Task | Requirement IDs | Test command |
| --- | --- | --- | --- | --- |
| [x] | Grok Bot (hand-coded) | SPEC/PLAN, `src/docs/mod.rs` (kinds, limits, errors), `src/docs/confine.rs` | FR-740 | `cargo test --lib docs::confine` |
| [x] | AGY (round 1) + Grok Bot hardening | `src/docs/file.rs`, `src/docs/office.rs`, `src/web/docs_api.rs`, server/main wiring | FR-742, FR-744..FR-747, NFR-751 | `cargo test --lib web::docs_api` |
| [x] | opencode deepseek, fix round Cursor (composer-2.5) | `src/docs/{list,markdown,links,changes}.rs` | FR-741, FR-743, FR-748, FR-749 | `cargo test --lib docs::` |
| [x] | opencode glm (+ glm fix round) | `docs.js`, `docs.css`, `index.html` hooks, asset tests, README | UX-753, INT-754 | `cargo test --lib web::assets` |
| [x] | Cursor (composer-2.5); AGY hit its quota | App per-turn change tracking and TUI file links (OSC 8 + click) | FR-748, FR-750 | `cargo test --bin dmuxai tui_file_link` |
| [x] | Grok Bot | Headless Chrome e2e (python Playwright) and screenshots | UX-753 | `cargo test --test web_docs_preview -- --ignored` |
| [x] | glm <-> deepseek, Cursor (composer-2.5) reviewer | Cross-reviews (lib/UI, backend security, TUI); fixes by the runners and Grok Bot | all | `cargo test --all-targets` |
