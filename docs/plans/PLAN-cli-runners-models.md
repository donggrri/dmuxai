# PLAN: cli-runners-models

Spec: [SPEC-cli-runners-models](../specs/SPEC-cli-runners-models.md)

| Order | Change | Files | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | agy policy + runner-accurate message | `src/model_policy.rs` | FR-2834, FR-2835 | TC-2835, TC-2836 |
| 2 | grok/ACP turn login → `auth` | `src/agent/mod.rs`, `src/agent/acp/client.rs`, `src/agent/custom.rs`, docs | FR-2836, FR-2837 | TC-2837, TC-2838 |
| 3 | status `allowed` / `graph_allowed` | `src/agent_api.rs` | FR-2838 | TC-2839 |
| 4 | `runners` / `models` CLI + MCP `models` tool, agy listing | `src/agent_api.rs`, `src/cli.rs`, `src/main.rs`, `src/mcp_server.rs` | FR-2831..FR-2833 | TC-2831..TC-2834, TC-2838 |
| 5 | per-subcommand help | `src/cli.rs`, `src/main.rs` | FR-2830 | TC-2830 |

Risks: `status.allowed` semantics change (documented); `models` spawns external CLIs (bounded timeout). Rollback: revert the PR; no persistent data changes.
