# TASKS: Obsidian memory

| Done | Owner | Task | Requirement IDs | Test command |
| --- | --- | --- | --- | --- |
| [x] | Grok Bot | Spec, API skeleton, `[memory]` settings | FR-720 | `cargo test --lib memory_settings` |
| [x] | Grok Bot | Prompt injection, `/remember`, `/memory`, service wiring, README | FR-722, FR-726, INT-732 | `cargo test --bin dmuxai memory_` |
| [x] | AGY (implementation), cursor composer-2.5 (fix round after AGY hit its quota) | Memory core: context, validation, save, queue, search | FR-721, FR-723, FR-724, FR-725, FR-730, NFR-731 | `cargo test --lib memory::` |
| [x] | opencode deepseek (implementation and fix round), Grok Bot (N1-N3 follow-up) | `dmuxai mcp-memory` stdio MCP server | INT-729, FR-730 | `cargo test --test memory_mcp`; `cargo test --lib memory_mcp` |
| [x] | cursor composer-2.5 (glm endpoint unavailable) | `/api/memory*` routes and the Memory panel | FR-727, UX-728 | `cargo test --lib memory_routes`; `cargo test --lib memory_panel_assets_present` |
| [x] | AGY, cursor composer-2.5 | Cross-reviews and fix rounds | all | `cargo test --all-targets` |

| [x] | Grok Bot | Review record | all | see `docs/reviews/REVIEW-obsidian-memory.md` |
