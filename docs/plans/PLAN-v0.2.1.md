# PLAN: v0.2.1

Status: implemented in v0.2.1 (released 2026-10-18)
Specs: SPEC-cursor-stream-dedupe, SPEC-first-run, SPEC-running-indicators, SPEC-docs-tree, SPEC-todo-sidebar, SPEC-cursor-latency. Roadmap: [docs/ROADMAP.md](../ROADMAP.md)

## Process

- Each item gets its own worktree under `/workspace/dmux-wt/<task>`, with its own dmuxai running in tmux.
- Spec first: this PR lands every SPEC. Each implementation PR flips its SPEC's Status to `Implemented (v0.2.1)` and its TEST-PLAN rows from ⏳ to ✅.
- Runner roles (Dongjin, 2026-10-17 23:47):
  - AGY (non-fast flash) takes the hardest parts;
  - opencode-haiku takes the rest;
  - haiku reviews AGY's code and AGY reviews haiku's;
  - Cursor is **not** an implementer or reviewer. It appears only as the subject of the latency measurement;
  - no fast models.
- The OpenRouter budget (shared, about $3.5 left at the start) is checked every few tasks.
- Merge rule: squash-merge when there are no blockers and CI is green. Never push main.

## Order and design notes

1. **Dedupe (#40, PR #39)**: done. Re-reviewed by haiku instead of Cursor.
2. **First run (#45, #46)**:
   - built-in opencode preset (`custom_agent::opencode_preset()`), registered in `Settings` resolution after the user and workspace custom agents, when `which opencode` succeeds;
   - catalog error text for an empty allowed list;
   - `Cli::Init` subcommand that writes a template;
   - README "First run" and install fixes;
   - a doc test against `release.yml`.
3. **Running indicators (#42)**:
   - `RunState` per agent in the App, filled from the existing turn events; serialized into the snapshot agents and `status --json`;
   - TUI: a spinner frame derived from `Instant` (100 ms tick only while busy) in `draw_status` and the pending assistant line;
   - web: a header pill, chip spinner and graph-node spinner in CSS; `requestAnimationFrame`-free 1 s elapsed ticker; title and favicon swap.
4. **Docs tree (#43)**:
   - `docs::list::list_children(ws, dir, limit)` reuses the walk rules;
   - `/api/docs/tree` route;
   - a sidebar shell (`#sidebar` with tabs) in index.html;
   - a tree component in docs.js with lazy fetch and `localStorage`.
5. **To-do sidebar (#44)**: move `#todos` into the sidebar tab, add the header button and badge, state filter and search, and Esc to close.
6. **Cursor pre-warm (#41)**: a `WarmProcess {argv, child, spawned_at}` slot in the cursor adapter. Reuse on an exact argv match; kill and reap on mismatch, idle timeout (App 1 s tick), agent removal, `/new` or quit. Own process group / job object. Disabled for `run` and graph nodes.
7. **Release v0.2.1**: CHANGELOG, version bump to 0.2.1, release PR, tag, then verify the musl tarball (`--version`) and the Windows zip.
