# SPEC: Theme settings for the web dashboard and TUI colors

Status: Implemented in v0.2.0 (slice D)
Owner: DmuxAI
Created: 2026-10-17
Baseline: `0c0c7fc`
Related: SPEC-speaker-colors, SPEC-web-dashboard (UX-540)

## 1. Decision summary

Users want DmuxAI to match their terminal and browser look. This feature
does three things:

- The web dashboard gets a light, dark or system theme with an accent color.
  Both can be switched in the page and default from `[web]` config.
- The TUI `[theme]` adds a background, a default text color, border and
  accent colors, and a text color for any agent, including custom and opencode
  agents (`[theme.agents]`).
- The dashboard reuses the TUI agent colors for its agent chips.

Invalid values never break startup: they become config warnings, and the
built-in default is used. `NO_COLOR` and `force_color` keep their current
meaning.

## 2. Scope

### In scope

- `[web] theme = "light" | "dark" | "system"` (default `system`) and
  `[web] accent = "#rrggbb"`.
- A page theme switcher; the choice persists in `localStorage`.
- `[theme] background`, `foreground`, `border`, `accent`, and
  `[theme.agents] <agent id> = "<color>"` for any agent id.
- The snapshot exposes agent colors as `#rrggbb` for the dashboard chips.

### Out of scope / non-goals

- Per-node colors in the graph view, full theme files, or 256-color palettes
  beyond the existing named colors and `#rrggbb`.
- Changing the colors when `NO_COLOR` is set without `force_color`.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-600 | The `[theme] other` key stays as the fallback for agents without their own color, so existing configs keep working. | Dongjin | Review |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Agent color | The color used for an agent's messages in the TUI and its chip on the dashboard | Speaker (`user`/`system`) |
| Effective web theme | The page's theme after resolving `system`: the `localStorage` choice, else `[web] theme`, else `system` | The TUI theme |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-600 | Must | Given `[theme]` with `background`, `foreground`, `border`, `accent` | The TUI draws the conversation/prompt area background, default text, block borders and highlights with those colors | Invalid values -> a config warning naming the key, and the default is used; unset keys keep today's look (terminal default background) | TC-600, TC-601 |
| FR-601 | Must | Given `[theme.agents]` with entries such as `opencode-glm = "#ff8800"` or `codex = "magenta"` | That agent's messages use the color; this works for built-in agents (cursor, agy, codex), the opencode presets and any `[[agents.custom]]` id; the `[[agents.custom]] color` stays the fallback, then `[theme] other` | An unknown agent id -> a warning; an invalid color -> a warning and the fallback is used; ids match case-insensitively | TC-602, TC-603 |
| FR-602 | Must | Given `NO_COLOR` set without `force_color` | No color is emitted for any new key (same rule as speaker colors) | `force_color = true` or `DMUXAI_FORCE_COLOR=1` restores them | TC-604 |
| FR-603 | Must | Given `[web] theme` and `[web] accent` | The snapshot carries `ui: { theme, accent }`, and the page applies them on first load when no `localStorage` choice exists | An invalid theme -> `system` with a warning; an invalid accent -> the default accent with a warning | TC-605, TC-606 |
| UX-600 | Must | Given the page | A theme control (Light/Dark/System) and an accent color input in the header switch the page immediately, and the choice persists in `localStorage` across reloads; `system` follows `prefers-color-scheme` live | `localStorage` unavailable -> the switch still works for the session | TC-607 |
| NFR-600 | Must | Given any built-in theme and any accent | Body text and the background meet a WCAG 2.1 contrast of at least 4.5:1, and badges/buttons at least 3:1; text drawn on the accent color picks black or white, whichever has the higher contrast | A low-contrast accent never makes text unreadable: it is used for borders and highlights only | TC-608 |
| FR-604 | Should | Given TUI agent colors | The snapshot `agents[]` include `color` (`#rrggbb` or null), and the dashboard chips use it as a border or dot | Named colors are mapped to fixed hex values; null -> the page default | TC-609 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-600 | FR-600 | unit | TOML with background/foreground/border/accent, one invalid | `Theme::from_toml` + warnings | parsed colors; invalid key -> default + warning naming it | `src/theme.rs` `theme_parses_surface_colors_with_warnings` |
| TC-601 | FR-600 | unit | App test helper with a themed background/border | render one frame into a test backend | cells in the chat area use the background; the block border uses `border` | `src/app.rs` `tui_renders_theme_background_and_border` |
| TC-602 | FR-601 | unit | `[theme.agents]` for `opencode-glm`, `codex`, an unknown id and an invalid color | resolve agent colors | per-agent colors; warnings for unknown/invalid; case-insensitive | `src/theme.rs` `theme_agent_colors_resolve_with_fallbacks` |
| TC-603 | FR-601 | unit | App with a custom agent message and a codex message | render | each message's fg equals the configured agent color | `src/app.rs` `tui_uses_per_agent_text_colors` |
| TC-604 | FR-602 | unit | color policy with `NO_COLOR` set, with and without force | `color_output_enabled` / theme application | disabled without force; enabled with force | `src/theme.rs` `no_color_disables_new_theme_keys` |
| TC-605 | FR-603 | unit | `[web] theme="dark" accent="#3366ff"`; invalid values | settings load + snapshot | `ui.theme == "dark"`, `ui.accent == "#3366ff"`; invalid -> `system`/default + warnings | `src/settings.rs` `web_theme_settings_parse_with_fallbacks` |
| TC-606 | FR-603 | integration | router with a hub that has `ui` set | GET `/api/state` | JSON contains `ui.theme` and `ui.accent`; no token | `src/web` `snapshot_exposes_ui_theme` |
| TC-607 | UX-600 | integration | router | GET `/`, `/assets/app.js`, `/assets/app.css` | markup has the theme control and the accent input; JS uses `localStorage` and `prefers-color-scheme`; CSS defines `[data-theme="light"]` and `[data-theme="dark"]` variables | `src/web` `static_page_has_theme_controls` |
| TC-608 | NFR-600 | unit | the CSS variables for both themes, parsed in a test | compute WCAG contrast ratios | text/background >= 4.5, badge/button text >= 3.0 in both themes; `text_on(accent)` returns black or white with the higher ratio | `src/web` `theme_palettes_meet_wcag_contrast` |
| TC-609 | FR-604 | unit | App with `[theme.agents]` set for one agent | publish the dashboard snapshot | that agent has `color: "#rrggbb"`; others null or their default hex | `src/app.rs` `dashboard_agents_carry_tui_colors` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Config | New optional keys only; existing `[theme] user/cursor/agy/other/system/force_color` unchanged | Old configs load unchanged | TC-600, TC-602 |
| HTTP API | Additive `ui` object and `agents[].color` within `dmuxai.dashboard/v1` | The page ignores missing fields | TC-606, TC-609 |
| Security | `localStorage` holds only `theme`/`accent`, never the token | — | TC-607 |
| Platform | Truecolor `#rrggbb` is used as-is; terminals without truecolor show the nearest color (terminal behavior) | — | — |
