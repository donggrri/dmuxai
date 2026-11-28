# SPEC: Board web fixes (WSL human actions, plan save fields, dynamic plan hash, stale-plan notice)

Status: implemented
Owner: Dongjin
Created: 2026-11-28
Baseline: `068b920`
Related: #358, #359, #360, #369, SPEC-decision-board FR-3106 FR-3107 FR-3123 FR-3138, SPEC-web-local-access FR-2953 FR-2960, SPEC-board-dashboard-acceptance FR-3605

## 1. Decision summary

Four board defects block the owner from using the board as the coordinator's
decision surface. A Windows browser that reaches the WSL dashboard through the
WSL IP gets 403 on every human action because plain-HTTP non-loopback origins
get no `Sec-Fetch-*` headers (#358). Saving an edited plan drops each step's
`task` and `sensitive` fields (#359). With `[graph] dynamic_nodes = true` the
board computes a different plan hash than the planner (#360). After a stale
approve (409) the "Plan changed — reload" notice is never shown (#369). This
spec fixes all four without adding any new approval path.

## 2. Scope

### In scope

- Human-action header rule for the WSL interface listener only.
- Plan save keeps non-edited step fields.
- Pending plan hash includes `dynamic_cap`.
- Stale-plan notice after a 409 on a decision.

### Out of scope / non-goals

- Spec-first steps on board plans (#374, handled by the spec review gate work).
- Token-in-URL for SSE (#367); loopback peer check (#355).

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-3810 | The relaxed header rule applies only to requests that arrive on the WSL interface listener (FR-2960) from the trusted Windows host peer; loopback and `--allow-remote` listeners keep the strict rule | Dongjin | Answered 2026-11-28 |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| WSL interface listener | The extra listener on the WSL default-route IPv4 address (SPEC-web-local-access FR-2960) | A `--bind` remote listener |
| human headers | `X-Dmuxai-Human: 1` sent by board.js only from a click or key handler, plus browser fetch metadata | The dashboard token |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-3810 | Must | Given an owner write (`POST /decision`, `/check`, `/lane`, `PUT /rules`) on the WSL interface listener from the trusted Windows host peer, with `Sec-Fetch-Site` and `Sec-Fetch-Mode` both absent | Then it is accepted when `X-Dmuxai-Human: 1` is present and `Origin` equals `http://<Host>` of that listener | Present `Sec-Fetch-Site` other than `same-origin` → 403; missing `X-Dmuxai-Human` or wrong/missing Origin → 403; any other peer on that listener → 403 | TC-3810 |
| FR-3811 | Must | Given the loopback listener or a `--bind --allow-remote` listener | Then the human header rule is unchanged (both `Sec-Fetch-*` headers required) | — | TC-3811 |
| FR-3812 | Must | Given the owner edits plan steps in the board modal and presses Save | Then every saved step keeps the original step's fields that the table does not edit (`task`, `sensitive` and any other field); a newly added step gets `task` = the first task id when the plan has tasks; a graph plan with tasks saves with 200 | A plan that still fails validation shows per-step errors as today | TC-3812 |
| FR-3813 | Must | Given `[graph] dynamic_nodes = true` and a pending plan file that carries `dynamic_cap` | Then the board hash check (`board add --plan-hash`, `board plan --hash`, `board ask --revise`) computes the same hash the planner wrote, and plan edits keep `dynamic_cap` | Plans without `dynamic_cap` hash exactly as before | TC-3813 |
| FR-3814 | Must | Given the owner clicks Approve (or another verdict) and the server answers 409 because the plan changed | Then the modal reloads the item and shows the notice `Plan changed — reload` (element `board-plan-notice`) until the owner makes a new decision, closes the modal or opens another item | No decision is recorded and no run starts (unchanged) | TC-3814 |
| NFR-3810 | Must | Any request | No added work on routes other than board owner writes | — | TC-3811 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-3810 | FR-3810 | http (router) | WSL interface access with a trusted host peer | POST decision with Origin + human header, no Sec-Fetch; variants without human header, wrong Origin, Sec-Fetch-Site cross-site, other peer | 200 for the first; 403 for the rest | `src/web/board_api.rs` `tc_3810_wsl_listener_human_headers_fallback` |
| TC-3811 | FR-3811, NFR-3810 | http (router) | Loopback access | POST decision with Origin + human header but no Sec-Fetch | 403 (unchanged) | `src/web/board_api.rs` `tc_3811_loopback_keeps_strict_human_rule` |
| TC-3812 | FR-3812 | unit (asset) + e2e (browser, ignored) | Item with a graph plan that has tasks and a sensitive step | Edit one job, add a step, Save | 200; saved steps keep `task` and `sensitive`; new step has first task id | `src/web/assets.rs` `tc_3812_board_save_keeps_step_fields`, `tests/web_board_e2e.rs` `tc_3812_*` |
| TC-3813 | FR-3813 | unit | Pending plan with `dynamic_cap: Some(3)` written by the headless path | Attach by hash, edit, re-hash | Hash matches; `dynamic_cap` kept | `src/board/plan.rs` `tc_3813_dynamic_cap_hash_roundtrip` |
| TC-3814 | FR-3814 | e2e (browser, ignored) | Item whose plan changes after the modal opened | Click Approve | 409; notice visible; no decision | `tests/web_board_e2e.rs` `tc_3814_stale_approve_shows_notice` |
