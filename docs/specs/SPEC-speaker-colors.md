# SPEC: Speaker colors and color forcing

Status: Implemented (PR #5, #6); some TCs proposed  
Owner: DmuxAI  
Created: 2026-10-17  
Baseline: `73e3f4d`  
Related: dogfood #2, README theme section

## 1. Decision summary

Chat lines are colored per speaker (You, Cursor, AGY, opencode/custom, DmuxAI system) from a `[theme]` section in `dmuxai.toml`. `NO_COLOR` disables colors unless forced by `DMUXAI_FORCE_COLOR=1` or `[theme] force_color = true`; `DMUXAI_FORCE_COLOR=0` forces them off.

## 2. Scope

### In scope

- `src/theme.rs`, `src/settings.rs` theme parsing
- Chat rendering in `src/app.rs`

### Out of scope / non-goals

- Full color-scheme editor

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| Q-1 | Proposed TCs (marked *proposed*) have no code yet and must be implemented before this spec is Approved. | DmuxAI | Release gate |

## 3. Terms and state model

- Speaker: author of a chat line.
- Color decision: env + config → on/off.

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-320 | Must | Default or configured theme. | Each speaker maps to a distinct color; named and hex colors parse. | Invalid color falls back to default. | TC-320, TC-321 |
| FR-321 | Must | Chat is rendered. | Rendered buffer cells carry the speaker color. | — | TC-322 |
| FR-322 | Must | `NO_COLOR` / `DMUXAI_FORCE_COLOR` / `force_color` combinations. | NO_COLOR off unless forced; FORCE=0 off. | — | TC-323 |
| UX-320 | Should | Colors disabled by NO_COLOR at startup. | A one-time notice explains how to enable colors. | — | TC-324 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-320 | FR-320 | unit | theme toml | parse | defaults + overrides | `cargo test from_toml_defaults_and_overrides`; `parse_color_named`; `parse_color_hex`; `parses_theme_section` |
| TC-321 | FR-320 | unit | default theme | color per speaker | distinct | `cargo test color_for_maps_each_speaker` |
| TC-322 | FR-321 | unit | TestBackend with messages | render | cell colors per speaker | `cargo test chat_lines_render_with_speaker_theme_colors` |
| TC-323 | FR-322 | unit | env matrix | resolve | as specified | `cargo test no_color_disables_unless_forced` |
| TC-324 | UX-320 | unit | NO_COLOR=1, no force | start app | notice shown once | *proposed*: `no_color_startup_notice_is_shown` |

## 6. Data, integration, and safety contracts

- Colors only affect rendering; stored transcript is unchanged.

## 7. Implementation plan

See status above; proposed tests are tracked in [TEST-PLAN.md](../TEST-PLAN.md).

## 8. Review and test exit criteria

- TC-320..323 pass; live checklist L-05 passes.
