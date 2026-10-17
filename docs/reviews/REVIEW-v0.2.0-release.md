# REVIEW: v0.2.0 release audit

Date: 2026-10-17 (Asia/Seoul). Base: `main` at 73ff3e6 (#34), plus the
release PR #28.

## Scope

These are the v0.2.0 slices that came after the #1-#26 work:

- A, B and C (SPEC-web-dashboard): dashboard, to-do board, shared MCP
- D (SPEC-theme-colors): themes
- E and the FR-733 follow-up (SPEC-obsidian-memory, #33 and #38): memory
- F (SPEC-docs-preview, #34): document preview and edit
- G1 (SPEC-agent-api, #35): agent API
- G2 (SPEC-dashboard-parity, #36): dashboard parity
- H (SPEC-graph-view, #32): graph view
- #37: events-lock fix (covered by TC-770 in SPEC-agent-api)

## Method

For every SPEC above:

1. Each `| FR/NFR/UX/INT-… |` requirement row must be covered by at least
   one `| TC-… |` row, either in the SPEC or in its TEST-PLAN row. A
   requirement whose Tests column says "Manual" is listed separately below.
2. Each TC must have a row in `docs/TEST-PLAN.md`.
3. Each test name in backticks in the TC row and the TEST-PLAN row must exist
   as an `fn` in `src/` or `tests/`. A `prefix_*` pattern counts when at
   least one test function matches it.
4. No TEST-PLAN row may point at a TC that no SPEC in these ID bands defines.
5. The SPEC, PLAN and TASKS Status lines and task rows must say done or
   explicitly deferred.
6. README and CHANGELOG claims must match the SPECs. Checked by reading them
   against the specs, plus a stdio smoke test of `dmuxai mcp`.

Checks 1-5 ran as a throwaway script over the Markdown tables. Check 6 was
done by hand. `scripts/spec-index.sh` and `tests/spec_index.rs`
(cross-SPEC ID uniqueness) both pass.

## Result after fixes

| SPEC | Reqs | TCs | Uncovered reqs | TC without TEST-PLAN row | Missing test fns | Open tasks |
| --- | --- | --- | --- | --- | --- | --- |
| web-dashboard (A, B, C) | 29 | 38 | 0 | 0 | 0 | 0 |
| theme-colors (D) | 7 | 10 | 0 | 0 | 0 | 0 |
| obsidian-memory (E + FR-733) | 14 | 15 | 0 (INT-732 manual: README) | 0 | 0 | 0 |
| graph-view (H) | 8 | 6 | 0 | 0 | 0 | 0 |
| agent-api (G1, #37) | 18 | 16 | 0 (UX-760 manual: README/API.md) | 0 | 0 | 0 |
| dashboard-parity (G2) | 7 | 8 | 0 | 0 | 0 | 0 |
| docs-preview (F) | 15 | 20 | 0 | 0 | 0 | 1, explicitly deferred: OQ-740..742 |

No orphan TEST-PLAN rows were found in these ID bands.

## Gaps found and fixed in this PR

| # | Gap | Fix |
| --- | --- | --- |
| 1 | SPEC-obsidian-memory TC-720..TC-731 had no `docs/TEST-PLAN.md` rows | Rows added, generated from the SPEC TC table |
| 2 | TC-729 named only `tests/memory_mcp.rs`, no test functions | Test names added to the SPEC row and the TEST-PLAN row |
| 3 | NFR-780 (parity UI isolation) was "Manual review" only | Linked to TC-786 (`session_assets_are_served_hardened` asserts `session.js` has no `EventSource`), plus manual review |
| 4 | No `docs/tasks/TASKS-dashboard-parity.md` | Added, with TC-to-test mapping |
| 5 | TASKS-agent-api had no done column | Added a Done column; all rows done |
| 6 | Status lines still said "In progress" or "planned" (SPEC web-dashboard, theme-colors, agent-api, dashboard-parity, docs-preview; PLAN web-dashboard, theme-colors, obsidian-memory, docs-preview), or were missing (PLAN graph-view, agent-api, dashboard-parity) | Set to "Implemented in v0.2.0"; docs-preview notes OQ-740..742 as deferred |
| 7 | README code map listed only the v0.1 modules | Added cli/agent_api, mcp_server, events/notify, graph, todo, mcp, memory, theme, web |
| 8 | README top dashboard screenshot dated from slice A | Replaced with the current page: Chat panel, agents, to-dos, documents, graph view, memory. Demo graph, token redacted |
| 9 | README had no Claude Desktop MCP config | Linux and Windows/WSL examples added, marked untested with Claude Desktop. `dmuxai mcp` answers `initialize` (`protocolVersion` 2024-11-05, `serverInfo` dmuxai 0.2.0) and `tools/list` (`todo_add`, `todo_list`, `run`, `status`, `events_since`); a `status` tool call returns data |
| 10 | Personal Windows paths in README and review notes | Removed (release commits) |

No code gaps were found, so nothing needed opencode-haiku.

## Known, deferred (not part of the slice scope)

- **SPEC-docs-preview OQ-740..742:** per-turn attribution for overlapping
  turns, scripted HTML preview, and stronger LibreOffice network isolation.
  These are recorded as deferred in TASKS-docs-preview.
- **Specs from before the slices (#1-#26 era):** several still lack
  TEST-PLAN rows or test names in their TC rows. Affected: activity-rendering,
  agent-test-helpers, agy-idle-timeout, compact-summarize-focus,
  custom-agent-provider, graph-default-status, implement-plan-only,
  model-policy, permission-hardening, speaker-colors, spec-index,
  state-per-repo, tui-stabilize and windows-desktop. Some referenced names
  are UI labels or were renamed:
  - codex-effort-tool-usage TC-041..045
  - model-policy TC-315
  - permission-hardening TC-303
  - speaker-colors TC-324

  These features shipped before the v0.2.0 slices and are covered by
  `cargo test`, but their traceability rows are behind. Tracked as v0.2.x
  doc debt.
- **Check 3 only proves a function with that name exists.** That it runs as
  a test is covered by `cargo test --all-targets`.

## Release gates

Filled in at merge time; see PR #28.
