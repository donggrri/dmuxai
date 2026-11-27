# REVIEW: runner status and enable switch

Spec: [SPEC-runner-status.md](../specs/SPEC-runner-status.md)

## Spec compliance

| Requirement | Evidence |
| --- | --- |
| FR-3300..FR-3304 probes, masking | `src/runner_status.rs` tc_3300..tc_3303 |
| FR-3305 cache and refresh limit | tc_3304 |
| FR-3306, FR-3308 CLI | `src/cli.rs` tc_3305; manual runs with clean HOME and API-key env |
| FR-3307 enabled setting and refusal | `src/settings.rs` tc_3306, tc_3307; `src/cli.rs` tc_3312 |
| FR-3309 HTTP | `src/web/server.rs` tc_3308 |
| FR-3310 MCP | `src/mcp_server.rs` tc_3309 |
| FR-3311 dashboard panel | `src/web/assets.rs` tc_3310; headless-browser check: nothing fetched until the panel is opened |
| NFR-3310 no probe on hot path | `src/agent_api.rs` tc_3311 |
| NFR-3311 fixed argv, timeout, no credential files | tc_3303 (fake binaries record argv) |

## Residual risks

- Probe output shapes were observed on cursor-agent 2026.11.22 and agy 1.5.18; a CLI update that changes them degrades to `unknown`/`error`, never to `ok`.
- cursor-agent `status` ignores `CURSOR_API_KEY`; handled by the `models` fallback (FR-3301) which needs network.
- Disabled runners still appear in dashboard pickers (OQ-3302).
- No quota information (no cheap CLI command exists).
