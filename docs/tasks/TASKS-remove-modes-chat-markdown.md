# TASKS: Remove modes, chat markdown, dashboard UI

| ID | Task | Req | Verify |
| --- | --- | --- | --- |
| T1 | TurnPolicy + remove user modes | FR-* | `cargo test` app/agent |
| T2 | Legacy `[modes]` warning | FR-1308 | `cargo test tc_1311` |
| T3 | Chat markdown + sanitize | FR-1401 | `cargo test tc_1401` |
| T4 | Composer agent picker + memory top bar | UX-1402 | parity test + screenshots |
| T5 | MCP run.mode ignored | INT-* | `cargo test mcp_server` |
| T6 | Gates | NFR | fmt, clippy -D warnings, test --all-features |
