# PLAN: Remove modes, chat markdown, dashboard UI (Part 3)

## Scope

- Replace `ExecutionMode` with internal `TurnPolicy` (agent vs read-only gates only).
- Dashboard: GFM assistant messages; composer agent picker; memory in top bar.
- Legacy `[modes]` / MCP `mode` ignored with warning.

## Order

1. Agent layer (`TurnPolicy`, adapters, `AgentRequest`).
2. App/CLI/settings deprecation.
3. Web snapshot + `chat_markdown.rs` + session.js rendering.
4. Dashboard HTML/CSS/JS (remove AGENTS/MEMORY cards).
5. Tests, clippy/fmt, screenshots, review doc.

## Risks

- Cursor/custom adapters: manual merge without ACP from PR-78.
- MCP clients still sending `mode`: must not pass `--mode` to CLI.

## Rollback

Revert branch; no migration beyond ignored config keys.
