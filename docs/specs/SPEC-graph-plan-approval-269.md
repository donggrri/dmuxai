# SPEC: Headless graph plan limit and exact-plan approval (#269)

Status: in progress
Owner: DmuxAI
Created: 2026-11-08
Baseline: `2b1f4f6`
Related: SPEC-plan-gate, SPEC-agent-graph-orchestration FR-958, SPEC-agent-api, SPEC-mcp-run-parity

## 1. Decision summary

1. The first graph plan is validated against the effective step cap
   `min(8, [graph] max_nodes)` — the same cap the approval path already uses —
   and the planner prompt states that cap. An over-limit plan goes through the
   existing single repair turn; if it is still invalid the run fails with the
   validation error. It is never shown for approval (no exit 7 for a plan that
   can never run).
2. Headless approval can execute exactly the reviewed plan. On exit 7 the
   validated plan document is saved under the state dir as
   `graph-pending/<plan_hash>.json` (`plan_hash` = sha256 hex of the plan JSON,
   first 16 chars) together with the user prompt, and `plan_hash` is returned
   in the run JSON. `dmuxai run --graph --approve-plan-id <hash> -` loads that
   file, refuses if it is missing (exit 3) or its prompt differs from the
   current prompt (exit 2), re-validates it with the current limits/runners,
   and executes it without asking the planner again. Plain `--approve-plan`
   keeps the current behaviour (fresh plan, auto-approved).
3. MCP `run` accepts `approve_plan_id` (string) and forwards it as
   `--approve-plan-id`; it requires `graph: true`.

Minimal-design rationale: a content hash of a persisted file needs no daemon
and no database change, and comparing the stored prompt prevents approving a
plan for a different task.

## 2. Requirements

| ID | Requirement |
|----|-------------|
| FR-2542 | Initial (and repair) plan validation uses `min(8, graph max_nodes)` as the step/task cap. |
| FR-2543 | The planner prompt states the effective step cap instead of a fixed 8. |
| FR-2544 | An over-limit plan triggers the existing repair turn; a second invalid plan fails (exit 1) and never reaches approval/exit 7. |
| FR-2545 | Headless exit 7 for a graph plan saves `<state>/graph-pending/<plan_hash>.json` (prompt + plan doc) and reports `plan_hash` in JSON output. |
| FR-2546 | `--approve-plan-id <hash>` (implies plan approval, requires `--graph`) executes the stored plan without a new planner turn; missing file → exit 3, prompt mismatch → exit 2, invalid against current limits → exit 1. The pending file is removed after it is used. |
| FR-2547 | `--approve-plan-id` does NOT confirm sensitive gates beyond what `--approve-plan` confirms today. |
| FR-2548 | MCP `run` forwards `approve_plan_id` (requires `graph`). |

## 3. Tests

| ID | Covers |
|----|--------|
| TC-2544 | max_nodes=3, planner returns 4 steps then 3 → repair turn used, approval shows 3 steps. |
| TC-2545 | max_nodes=3, planner returns 4 steps twice → exit 1, no exit 7. |
| TC-2546 | Plan prompt contains the effective cap. |
| TC-2547 | exit 7 writes pending file and plan_hash; `--approve-plan-id` runs that plan with no planner turn. |
| TC-2548 | `--approve-plan-id` missing → 3; prompt mismatch → 2. |
| TC-2549 | CLI parse / MCP forwarding of approve_plan_id. |
