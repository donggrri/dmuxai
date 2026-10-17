# TASKS: Theme settings for the web dashboard and TUI colors

| Done | Owner | Task | Requirement IDs | Test command |
| --- | --- | --- | --- | --- |
| [x] | opencode deepseek | Theme surface colors, `[theme.agents]`, `color_hex`, `text_on`, `colors_enabled` | FR-600, FR-601, FR-602 | `cargo test --lib theme::` |
| [x] | opencode deepseek | `[web] theme`/`accent` settings and warnings | FR-603 | `cargo test --lib settings::` |
| [x] | AGY | TUI background/foreground/border/accent and per-agent colors | FR-600, FR-601 | `cargo test --bin dmuxai tui_` |
| [x] | AGY | Snapshot `ui` and `agents[].color`, serve warnings | FR-603, FR-604 | `cargo test --bin dmuxai dashboard_agents_carry_tui_colors`; `cargo test --lib snapshot_exposes_ui_theme` |
| [x] | opencode glm | Page palettes, theme switcher, accent picker, chip colors | UX-600, NFR-600, FR-604 | `cargo test --lib web::assets` |
| [x] | opencode glm | README "Themes and colors" | FR-600..FR-604 | — |
| [x] | glm / deepseek | Cross-reviews and fix rounds | all | `cargo test --all-targets` |
