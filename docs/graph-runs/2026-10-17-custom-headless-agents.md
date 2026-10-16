# Graph execution report: generic headless-command agents (`[[agents.custom]]`)

| Node | Role | Runner / model | Output |
|---|---|---|---|
| spec | plan | orchestrator | `docs/agents/custom-agents.md` (schema, placeholders, failure rules) |
| core | implement | **DmuxAI → AGY gemini-3.8-flash-medium** (Implement, in tmux) | `src/custom_agent.rs`: spec structs, validation, argv build, JSONL parse, failure rules, 12 tests |
| review-core | review | orchestrator | fixed raw-string fixture, single-pass substitution (no re-scan injection), clippy fixes |
| runtime | implement | orchestrator (direct; AGY long turns hang, dogfood #8) | `src/agent/custom.rs` runtime (process group, timeout, temp config, stdin/arg), registry, `Agent::Custom`, theme, policy, DB schema v2, config warnings |
| docs | implement | **DmuxAI → opencode glm-5.3-flash-free** (via the new custom agent) | README section, MISSION.md item 8 |
| test | test | cargo | fmt, clippy `-D warnings`, `test --all-targets` green; fake agent `tests/fixtures/fake-agent.sh` |
| verify | review | DmuxAI in tmux | opencode via `[[agents.custom]]` answered `PONG` in orange; F2 lists `-free` models |

Also fixes AGY Implement silent-empty turns (dogfood #7).
