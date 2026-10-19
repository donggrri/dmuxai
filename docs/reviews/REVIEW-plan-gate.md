# REVIEW: plan-gate

Status: In progress
Spec: [SPEC-plan-gate](../specs/SPEC-plan-gate.md)
Tasks: [TASKS-plan-gate](../tasks/TASKS-plan-gate.md)

## Task reviews (diff-only, reviewer differs from the author)

| Task | Author | Reviewer | Verdict |
| --- | --- | --- | --- |
| P01 | opencode deepseek-v4.1-flash-free (WIP) + composer-2.5 | glm-5.3-flash-free: BLOCK (judge overflow leak, busy teardown); claude-sonnet-5-5-high re-review | APPROVE after fixes |
| P02 | opencode deepseek-v4.1-flash-free + composer-2.5 fix | claude-sonnet-5-5-high | BLOCK (replan lost tasks) → fixed; follow-up moved to P09 |
| P03 | composer-2.5 (v0.2.2 graph run) | claude-sonnet-5-5-high | APPROVE |

## Dogfood findings (v0.2.2 binary driving #67)

- **Run e3a7ceb5 (P03), node dispatched to a busy runner:** step-1 (research, cursor) finished and step-2 (test, cursor) was sent at once; the cursor adapter was still winding down and refused with `Cursor is already running a turn`, twice (retries), so the run failed. Covered by FR-976 / TC-1001 (P03b), extending the FR-949 per-runner wait to the adapter's own busy state.
- **Same run, stale graph state after a node failure:** the header and chat showed `Graph failed: graph node step-2 failed` while the graph panel kept the run active with step-1 running and step-2 pending. Covered by FR-977 / TC-1002 (P03b).
- **v0.2.2 node prompts are cut to 120 chars** (`job_for_role` summarize), so dogfood task briefs live in a gitignored `.dmuxai/tasks/*.md` file named in the prompt. The agent-planned DAG of #67 writes full jobs (FR-952).
- The v0.2.2 keyword planner orders roles research → test → plan → review → implement, so a review node can never follow implement; reviews of #67 tasks therefore run outside the graph on the diff.
- opencode free models were blocked for this network from about 20:00 KST (`abusive traffic from this IP`), so from P02's fix on, composer-2.5 implements and claude-sonnet-5-5-high reviews.
