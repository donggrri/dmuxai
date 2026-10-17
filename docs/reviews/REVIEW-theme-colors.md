# REVIEW: Theme settings for the web dashboard and TUI colors

Spec: [SPEC-theme-colors](../specs/SPEC-theme-colors.md)

## Spec compliance

| Requirement | Status | Evidence |
| --- | --- | --- |
| FR-600 | Met | `theme_parses_surface_colors_with_warnings`, `tui_renders_theme_background_and_border`; popups, pickers and the graph status line use the same background/foreground when they are set; nothing changes when they are unset (existing render tests unchanged) |
| FR-601 | Met | `theme_agent_colors_resolve_with_fallbacks` (with real `[theme]`/`[theme.agents]` nesting), `tui_uses_per_agent_text_colors` (including the custom-spec `color` fallback); unknown ids are dropped with a warning; known ids are collected across the user and repo configs |
| FR-602 | Met | `no_color_disables_new_theme_keys`, `tui_renders_default_colors_when_colors_disabled`; `colors_enabled` keeps the old `main.rs` semantics |
| FR-603 | Met | `web_theme_settings_parse_with_fallbacks` (`#AABBCC` -> `#aabbcc`, `DARK` -> dark, named accent -> warning), `snapshot_exposes_ui_theme`; `serve` prints config warnings |
| FR-604 | Met | `dashboard_agents_carry_tui_colors` (configured and fallback hex) |
| UX-600 | Met | `static_page_has_theme_controls` (controls, localStorage keys only `dmuxai.theme`/`dmuxai.accent`, token in sessionStorage, accent guard, no hardcoded `data-theme`) |
| NFR-600 | Met | `theme_palettes_meet_wcag_contrast` (both palettes, a strict `#rrggbb` parser, a JS on-accent formula mirror); `theme::text_on` is used for TUI text drawn on the accent |

## Test evidence

These are local Linux results:

- `cargo fmt --check` is clean.
- `cargo clippy --all-targets --all-features --locked -- -D warnings` is clean.
- `cargo test --all-targets --locked`: 335 passed, 0 failed, 5 ignored.
- `scripts/spec-index.sh` lists 31 specs, with no duplicate IDs.

Screenshot: `docs/images/web-dashboard-light.png`.

## Cross-reviews (through dmuxai)

- glm reviewed deepseek's config code and found 9 issues, all fixed:
  - the "ignored" warning text didn't match the behavior;
  - `serve` dropped config warnings;
  - unknown-agent warnings were spurious for custom agents defined across configs;
  - the accent grammar was too loose;
  - empty ids were skipped silently;
  - the warning wording was wrong;
  - test nesting;
  - normalization assertions;
  - duplicate warnings.
- deepseek reviewed glm's page and found 5 issues, all fixed:
  - snapshot defaults clobbered in-session choices;
  - the JS on-accent formula was untested;
  - TC-607 assertions were weak;
  - the palette parser silently skipped values;
  - light-theme users saw a dark flash.
- deepseek reviewed AGY's TUI work and found 7 issues; 6 were fixed and 1 is a note:
  - text on the accent lacked contrast (now uses `text_on`);
  - the custom-color fallback test raced on the registry;
  - popups were themed inconsistently;
  - a String was allocated per frame;
  - there was no NO_COLOR render test;
  - TC-606/TC-609 assertions were weak;
  - `serve` has no agents, so no colors there (note only).

## Who did what

- AGY (gemini-3.8-flash-high): TUI rendering, snapshot and hub wiring, `main.rs`.
- opencode deepseek: `theme.rs` and `settings.rs`, plus the reviews of glm's and AGY's work.
- opencode glm: page palettes and controls, WCAG tests, README, plus the review of deepseek's work.
- Planner (by hand): spec, plan, tasks, TEST-PLAN rows, this review, and the screenshot.

## Residual risks

- Truecolor `#rrggbb` depends on the terminal; non-truecolor terminals approximate it.
- `serve` mode has no agent list, so it has no chip colors.
