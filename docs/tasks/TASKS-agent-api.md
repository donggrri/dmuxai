# TASKS: Agent API (SPEC-agent-api)

| # | Task | Requirement IDs | Owner | Verification |
| --- | --- | --- | --- | --- |
| 1 | Spec, plan, module contracts | all | maintainer | `scripts/check-spec.ps1`, `tests/spec_index.rs` |
| 2 | Event log, state tracker, stall detector, event sink; hub integration | FR-764, FR-768, FR-769 | opencode-deepseek | `cargo test events::` (TC-765, TC-769, TC-770, TC-771) |
| 3 | `[notify]` settings, webhook and command hook notifier; dashboard discovery file and token env | FR-770, FR-771, FR-772, FR-773, NFR-761 | opencode-glm | `cargo test notify:: discovery::` (TC-772, TC-773, TC-774) |
| 4 | Headless App loop, to-do store attach, `SubmitPrompt` command, `POST /api/prompt` | FR-761, FR-762, FR-765, NFR-762 | AGY | `cargo test headless_ prompt_route_contract` (TC-761, TC-762, TC-766) |
| 5 | Agent lifecycle events from the App; `/api/events/log` and SSE `state-change` frames | FR-766, FR-768 | AGY | `cargo test events_log_and_sse_state_change app_emits_agent_lifecycle_events` (TC-767, TC-775) |
| 6 | CLI parsing and dispatch (`run`, `todo`, `status`, `events`, `mcp`), status JSON, MCP stdio server, wiring sinks/discovery into `serve`/`--web`/TUI | FR-760, FR-763, FR-764, FR-767, FR-773 | maintainer | `cargo test parse_cli_agent_api_subcommands mcp_stdio_protocol_and_tools`; `cargo test --test agent_api_cli` (TC-760..TC-765, TC-768) |
| 7 | `docs/API.md`, README "Calling dmuxai from an agent" | UX-760 | opencode-deepseek | manual review |
| 8 | Cross-review: glm ↔ deepseek, deepseek → AGY server routes, AGY → maintainer CLI/MCP | all | all | `docs/reviews/REVIEW-agent-api.md` |
