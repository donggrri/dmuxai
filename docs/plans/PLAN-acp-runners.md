# PLAN: ACP runners and single agent mode

## Order

1. `src/acp/` — JSON-RPC client, permission policy, update → event mapping (FR-1302, FR-1304).
2. `src/agent/acp.rs` — turn runner used by Cursor and grok (FR-1300, FR-1301, FR-1303).
3. Settings `protocol` keys; grok preset argv for ACP (TC-1305, TC-1301).
4. Wire `cursor.rs` / `custom.rs` branches (FR-1305).
5. `TurnPolicy` on `AgentRequest`; remove `ExecutionMode` UX and adapter mode state (FR-1306..FR-1308, FR-1307).
6. Web/dashboard/cli mode removal; legacy ignore warnings.
7. `tests/fixtures/fake-acp-agent` binary.
8. Docs, CHANGELOG, README, superseded notes; `REVIEW-acp-runners.md`.

## Risks

- ACP v1 vs v2: negotiate v1 in `initialize`; extend if vendors require v2.
- `app.rs` mode removal is wide — use `TurnPolicy` to preserve judge read-only semantics.

## Rollback

Set `protocol = "legacy"` for Cursor and grok; revert to branch without mode removal if needed (separate revert commits).
