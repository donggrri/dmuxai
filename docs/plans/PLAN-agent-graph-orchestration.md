# PLAN: Agent graph orchestration

1. Extend `model_policy` for role buckets + fast rejection.
2. Add `graph/` (types, planner, policy, store, executor) and `skills/`.
3. Integrate `App`: plan on submit, graph strip UI, `/graph` commands, resume on load.
4. Hook `finish_turn` to advance graph executor.
5. Docs, MISSION update, graph yaml sample, graph-run report, tests.

Rollback: disable with `dmuxai.toml` `[graph] enabled = false` (default true).
