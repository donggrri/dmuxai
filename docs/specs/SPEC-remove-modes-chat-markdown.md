# SPEC: Remove execution modes and dashboard chat markdown

Status: draft  
Owner: dmuxai  
Baseline: `main`

## Decision summary

Remove user-facing Ask/Plan/Implement modes; every user turn runs in a single write-capable agent policy. Keep plan approval and sensitive-step gates. Render assistant chat as sanitized GFM in the dashboard. Move agent/model selection into the composer and memory status into the top bar.

## Requirements (selected)

| ID | Summary | Tests |
| --- | --- | --- |
| FR-1308 | Legacy `[modes]` in `dmuxai.toml` loads with warning, no crash | TC-1311 |
| FR-1401 | Assistant chat HTML from `render_chat_markdown` strips scripts | TC-1401 |
| UX-1402 | Composer agent picker; Memory in top bar | manual screenshots |

## Test mapping

| Test ID | Command |
| --- | --- |
| TC-1311 | `cargo test tc_1311_legacy_modes_section_is_ignored_with_warning` |
| TC-1401 | `cargo test tc_1401_chat_markdown_renders_gfm_and_sanitizes` |
| TC-1309+ | existing plan gate / sensitive gate tests in `app.rs` |
