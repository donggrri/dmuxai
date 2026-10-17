# PLAN: Obsidian memory

Spec: [SPEC-obsidian-memory](../specs/SPEC-obsidian-memory.md)
Status: implemented, in review

## Architecture

- **`src/memory.rs`** (new, pure filesystem) is the port of
  cursor-atomic-workflow `memory.mjs`: target parsing, the text validator and
  secret filter, `build_context`, direct `save`, the approval queue
  (`propose`/`list_pending`/`approve`/`reject`), and `search`.
  `MemoryHandle` bundles the store, project, display `dir` and `inject` for
  the front ends.
- **`src/settings.rs`**: `[memory]` keys merge per key (workspace wins) and
  resolve once into `DmuxSettings::memory` with warnings (FR-720).
- **`src/app.rs`**: `inject_memory` runs in `try_submit_to_agent`, the one
  path for every runner turn, graph nodes included, and touches only the
  adapter request (FR-722); `/remember` and `/memory` (FR-726).
- **`src/web/server.rs`** + `src/web/assets/memory.js`: `/api/memory*`
  routes and a self-contained Memory panel. `app.js` is untouched, so it
  rebases cleanly against the parallel dashboard PRs (FR-727, UX-728).
- **`src/memory_mcp.rs`** + `dmuxai mcp-memory`: a newline-delimited
  JSON-RPC stdio MCP server (INT-729). Registration is a documented
  `[[mcp.servers]]` entry until slice C lands (OQ-721).

## Risks and rollback

- No SQLite or adapter protocol change. Removing `[memory]` turns every
  feature off; `inject = false` stops only the prompt prefix.
- The TUI, dashboard and an MCP child process may write the queue at the same
  time. Writes are atomic (temp file + rename) and serialized within a
  process. A cross-process lost update is possible but unlikely, and it is
  recorded in the review.
