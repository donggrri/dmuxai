# Graph execution report: graph on by default + live graph status line

| Node | Role | Runner / model | Output |
|---|---|---|---|
| spec | plan | DmuxAI (worktree `dmux-wt/graph-default`, tmux `dmux-graph`) → AGY `gemini-3.8-flash-medium` | `docs/specs/SPEC-graph-default-status.md` + TEST-PLAN row |
| implement | implement | same | `status_line` on the graph session, single-plan status, layout row above the prompt, tests |
| fix | review | orchestrator | test used a nonexistent `AgentRequest.agent` field → `requests[0].0` |
| gate | test | cargo | fmt, clippy -D warnings, test --all-targets green |
| verify | review | DmuxAI in tmux | `Graph 1/1 · single implement · agy gemini-3.8-flash-medium · running` shown while running, hidden when idle |
