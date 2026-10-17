# PLAN: Theme settings for the web dashboard and TUI colors

Spec: [SPEC-theme-colors](../specs/SPEC-theme-colors.md)
Status: implemented in v0.2.0

## Architecture

- **`src/theme.rs`** owns parsing and color logic:
  - surface colors (`background`, `foreground`, `border`, `accent`);
  - `[theme.agents]` (lowercase keys);
  - `agent_color(id, custom_spec_color)`, which resolves in this order:
    `[theme.agents]`, then the custom spec `color`, then the built-in
    `cursor`/`agy`, then `other`;
  - `color_hex` (named color to fixed hex);
  - `text_on(bg)` (black or white by WCAG);
  - `colors_enabled` (the `NO_COLOR`/force policy, moved from `main.rs`
    unchanged).
- **`src/settings.rs`**:
  - `[web] theme`/`accent` become `WebThemeSettings`;
  - warnings cover invalid values, unknown `[theme.agents]` ids (dropped),
    and empty ids;
  - known custom agent ids are collected across the user and repo configs.
- **`src/app.rs`**:
  - applies the background, foreground, border and accent only when they are
    set, so an unset config looks exactly as before;
  - per-agent message colors go through `agent_color`;
  - `publish_dashboard` fills `agents[].color` and `ui`.
- **`src/web`**: the snapshot gains `ui {theme, accent}` and `agents[].color`.
  `serve` sets `ui` from settings and prints config warnings.
- **Page**: CSS custom properties with dark and light palettes; a header
  select and color picker; `localStorage` stores only the theme and the
  accent; `system` follows `prefers-color-scheme` live; `--on-accent` is
  chosen by WCAG; agent chips show a color dot.

## Ownership (dmuxai runners)

| Part | Runner |
| --- | --- |
| `theme.rs`, `settings.rs` | opencode deepseek-v4.1-flash-free |
| TUI render, snapshot/hub, `main.rs` wiring | AGY gemini-3.8-flash-high |
| Page CSS/JS/HTML, WCAG tests, README | opencode glm-5.3-flash-free |
| Cross-reviews | glm reviews deepseek; deepseek reviews glm and AGY |
