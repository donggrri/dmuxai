# REVIEW: ACP runners and single agent mode

Status: complete
Spec: `docs/specs/SPEC-acp-runners.md`
PR: #78

## Compliance

| Requirement | Evidence |
| --- | --- |
| FR-1300..FR-1305 ACP + legacy | `src/agent/acp/*`, `cursor.rs`, `custom.rs`, `cursor_acp_turn_with_fake_agent` |
| FR-1306..FR-1308 single agent mode | `TurnPolicy`, `/mode` notice-only, `session.js`, settings warnings |
| FR-1309 docs | README, CHANGELOG, superseded notes in baseline/dashboard specs |
| NFR-1301 Linux-only | SPEC §Platform; no new Windows ACP docs in this change |

## Commands

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

All passed on Linux (2026-10-20).

## Residual risk

- Live ACP against real `cursor-agent` / `grok` not run in CI (OQ-1300, OQ-1301).
- Historical docs (TEST-PLAN, dogfood README) may still mention `/mode`; baseline superseded note is the anchor.

## Rollback

Set `[cursor] protocol = "legacy"` and `[agents] grok_protocol = "legacy"`.

## Round 5 (FR-1410..FR-1418) — opencode free-model review

- `opencode/glm-5.3-flash-free`: **no blockers**; 10 minor findings. Fixed: fmt, `?` in the mode-config scan, crash after denial stays `Error`, HTTP MCP servers only when the agent advertises `mcpCapabilities.http`, stale test comments. Deferred (minor): capability-gating set_mode, dead `session_config_model` path, fixture build into the shared target dir, end-to-end MCP passthrough test, duplicate cancel line on user stop.
- `opencode/deepseek-v4.1-flash-free`: timed out (900 s) without a report.
