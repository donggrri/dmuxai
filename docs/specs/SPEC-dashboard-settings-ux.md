# SPEC: dashboard-settings-ux

Status: Implemented (draft)

Created: 2026-11-02

Related: (idea input only)

## 1. Decision summary

Improve the dmuxai web dashboard settings screens and run controls using vanilla JS/CSS in `src/web/assets`, served by `src/web`. No React, no Node build, no new dependencies.

## 2. Scope

In scope: `src/web/assets/index.html`, `app.css`, `app.js`, `session.js`, `session.css`, `skills.js`, `memory.js`, `agent-prompts.js`, and the shared button/settings CSS. Out of scope: server API or agent execution changes.

## 3. Terms

- Run controls: the compact row next to the prompt box grouping agent, model, effort (when supported) and mode selectors.
- Settings-like panels: memory, skills, agent prompts, workspace dialog and theme controls.

## 4. Requirements

| ID | Priority | Requirement | Tests |
| --- | --- | --- | --- |
| FR-2360 | Must | Run controls group the agent, model, effort (only when the runner supports it) and mode selectors into one compact control row next to the prompt box. | TC-2360 |
| FR-2361 | Must | The collapsed run control shows the current agent and model at a glance. | TC-2361 |
| FR-2362 | Must | Effort is shown only when the selected runner supports it (its models report efforts). | TC-2362 |
| FR-2363 | Must | Unavailable agents and models are shown disabled with the reason in the title tooltip instead of being hidden. | TC-2363 |
| FR-2364 | Must | Settings-like panels use consistent grouped rows: a bold label on the left, the control on the right, and a one-line muted hint under the label, via shared `.settings-group` and `.settings-row` classes. | TC-2364 |
| FR-2365 | Must | Long help text is collapsed into a small info tooltip rather than a long paragraph. | TC-2365 |
| FR-2366 | Must | `app.css` provides a small button system: primary, secondary (default) and danger variants with consistent height and padding, a visible `:focus-visible` ring, a disabled style, and a busy state. | TC-2366 |
| FR-2367 | Must | A busy button changes its text (for example "Saving…") and is disabled while a request is in flight. | TC-2367 |
| FR-2368 | Must | Destructive actions use an inline confirm (the button turns into "Confirm" + "Cancel" for a few seconds) instead of acting on one click, unless they already confirm. | TC-2368 |
| FR-2369 | Must | New controls are reachable by Tab, Esc closes any popover, icon-only buttons have aria-labels, and animations respect `prefers-reduced-motion`. | TC-2369 |

## 5. Acceptance scenarios and test design

| ID | Requirement | Level | Oracle | Test |
| --- | --- | --- | --- | --- |
| TC-2360 | FR-2360 | unit (asset) | `.session-run-controls` is one nowrap row: the agent picker and the route select sit inline, and the route select uses an aria-label instead of a stacked label. | `src/web/assets.rs` `tc_2360_run_controls_grouped` |
| TC-2361 | FR-2361 | unit (asset) | The agent picker puts "agent · model" in an inner span, sets `title` to that full text, and ellipsizes past 18rem. | `src/web/assets.rs` `tc_2361_collapsed_shows_agent_and_model` |
| TC-2362 | FR-2362 | unit (asset) | Effort controls are only rendered when a model reports efforts. | `src/web/assets.rs` `tc_2362_effort_only_when_supported` |
| TC-2363 | FR-2363 | unit (asset) | Unavailable agents are `disabled` with the reason in `title` in the agent popover and the to-do runner select. Session agents include `available` (omitted in older snapshots means true) and `unavailable_reason`. | `src/web/assets.rs` `tc_2363_unavailable_disabled_with_tooltip`; `src/app.rs` `tc_2363_custom_agent_availability_follows_command`; `src/web/snapshot.rs` `tc_2363_session_agent_defaults_available` |
| TC-2364 | FR-2364 | unit (asset) | Memory rows use `.settings-row-label` and a one-line `.settings-row-hint`, with the control on the right. | `src/web/assets.rs` `tc_2364_settings_shared_classes` |
| TC-2365 | FR-2365 | unit (asset) | Skills help is a `.settings-info` icon whose `title` and aria-label hold the long text. | `src/web/assets.rs` `tc_2365_long_help_info_tooltip` |
| TC-2366 | FR-2366 | unit (asset) | Button system classes and focus ring exist. | `src/web/assets.rs` `tc_2366_button_system` |
| TC-2367 | FR-2367 | unit (asset) | Busy state uses "Saving…" text and disables the button. | `src/web/assets.rs` `tc_2367_busy_state` |
| TC-2368 | FR-2368 | unit (asset) | To-do delete uses inline Confirm/Cancel, and Confirm uses the filled `.btn-danger.btn-confirm` style. | `src/web/assets.rs` `tc_2368_inline_confirm` |
| TC-2369 | FR-2369 | unit (asset) | Tab/aria-label/reduced-motion support present. | `src/web/assets.rs` `tc_2369_keyboard_and_a11y` |

## 6. Exit criteria

TC-2360..TC-2369 pass in local gates.
