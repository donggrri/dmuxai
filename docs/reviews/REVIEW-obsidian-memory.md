# REVIEW: Obsidian memory

Status: complete  
Spec: [SPEC-obsidian-memory.md](../specs/SPEC-obsidian-memory.md)  
Plan: [PLAN-obsidian-memory.md](../plans/PLAN-obsidian-memory.md) · Tasks: [TASKS-obsidian-memory.md](../tasks/TASKS-obsidian-memory.md)  
Reviewed: 2026-10-17  
Target: Linux / Cross-platform (Windows CI)

## Spec compliance

- FR-720 / FR-723: `[memory] vault` and `dir` (default `cursor-workflow/memory`) are merged per key in `src/settings.rs`. A relative vault resolves against the workspace. `memory::resolve_root` rejects a missing vault and a `dir` that escapes the vault. Project names must match `[a-z0-9][a-z0-9_-]{0,47}`.
- FR-721 / FR-722: `MemoryStore::build_context` respects the 3000/900/200 character budgets. `inject_memory` prepends the context to every runner turn on the single `try_submit_to_agent` path, including graph nodes and to-dos. The context is injected again only when it changes, and the hash is recorded per (conversation, agent) only after a successful send.
- FR-724 / NFR-731: `contains_secret` and `validate_text` reject secrets, oversized text, invalid targets and invalid sources before anything is written. Queue writes are atomic (temp file + rename) and serialized in-process by `QUEUE_MUTEX` and `FILE_MUTEX`.
- FR-725 / FR-726: `/remember` saves directly, `/remember --propose` queues a proposal, and `/memory list|approve|reject` manages the queue (pending entries sorted by createdAt, then id).
- FR-727 / UX-728: `/api/memory*` routes sit behind the dashboard token. `assets/memory.js` is isolated from the other dashboard scripts. Screenshot: `docs/images/web-dashboard-memory.png`.
- INT-729 / FR-730: `dmuxai mcp-memory` serves JSON-RPC 2.0 over stdio (protocol 2024-11-05) with the tools memory_get_context, memory_search and memory_propose.
  - stdout carries protocol only; warnings go to stderr.
  - Error codes: -32700 for parse errors and lines over 4 MiB, -32600 for invalid requests, -32601 for unknown methods.
  - A message with no id, or a null id with a `notifications/*` method, is a notification.
  - Startup errors exit 2.
- INT-732: README documents a minimal `[[mcp.servers]]` registration. Full registration waits for slice C.

## Cross-review record

| Round | Reviewer | Target | Outcome |
| --- | --- | --- | --- |
| 1 | AGY | Grok Bot skeleton, settings, app | Findings 1-4, 6, 7 fixed |
| 2 | cursor composer-2.5 | memory core (AGY) | 1, 2, 3, 10, 11, 12 fixed; 4, 6, 7 intentional (OQ-723); 5 clarified in the spec; 8 is a residual risk |
| 3 | cursor composer-2.5 | MCP server (deepseek) | 1-7, 9, 10 fixed; 8 (-32602) intentional; 11 fixed in round 5 |
| 4 | cursor composer-2.5 | app wiring | send-failure test added (`memory_context_not_recorded_when_send_fails`) |
| 5 | cursor composer-2.5 | N1-N3 follow-up | VERDICT OK. Findings 1 (stdin writer thread), 2 (null-id ping), 4 (1 s extra-response window) fixed; 3 (warn rather than exit) intentional; 5 covered by Windows CI |

## Test evidence

- `cargo fmt --check`: pass
- `cargo clippy --all-targets --all-features -- -D warnings`: pass
- `cargo test --all-targets` (Linux): 397 passed, 0 failed, 5 ignored
- Every test uses a temporary vault. No test writes into a real Obsidian vault.

## Residual risks

- Queue locking is in-process only. Two dmuxai processes approving at the same moment can lose an update (core review finding 8). Writes are atomic, so the file is never torn.
- Budgets count `chars()`, not tokens (OQ-723).
- `--project` without configured memory only prints a warning; the server still starts with memory disabled.
- dmuxai issues found while running this slice:
  - After Ctrl+C cancels a turn, the runner can stay stuck with "already has an active turn".
  - The graph fallback planner hard-codes `model_id: composer-2.5` for every node, whatever the runner.
  - The opencode-muse runner hangs with no output when OpenRouter rejects the request (404 "Paid model training violation"), instead of reporting the error.

## Rollback

There is no migration. Memory stays off unless `[memory] vault` is set. To roll back, revert the PR.
