# REVIEW: Document preview and edit (v0.2.0 slice F)

Status: complete  
Spec: [SPEC-docs-preview.md](../specs/SPEC-docs-preview.md)  
Plan: [PLAN-docs-preview.md](../plans/PLAN-docs-preview.md)  
Tasks: [TASKS-docs-preview.md](../tasks/TASKS-docs-preview.md)  
Reviewed: 2026-10-17  
Target: Linux / Windows / macOS

## Spec compliance

- FR-740 / NFR-751: every path goes through `Workspace::resolve` (`src/docs/confine.rs`):
  absolute paths, `..`, and symlinks that canonicalize outside the root are
  rejected (TC-740, TC-741). View/download tickets re-check the canonical path
  at use time (`view_ticket_rejects_symlink_swap`). `/api/docs*` sits behind the
  dashboard token + Host/Origin middleware (TC-756); `serve` is read-only.
- FR-741: listing skips `.git`, `target`, `node_modules`, `.dmuxai` and caps at
  5000 entries (TC-742).
- FR-742 / FR-744 / FR-745: read returns content + sha256 + mtime for text up to
  2 MiB; PUT saves atomically (temp file + rename) only for editable kinds, with
  a hash check right before the rename, `409 Conflict` on mismatch (TC-746) and
  rejection of `.html` saves, including symlinks pointing at HTML (TC-747).
- FR-743: pulldown-cmark GFM + syntect classes + ammonia; scripts, event
  handlers and `javascript:` URLs are removed (TC-744, TC-745).
- FR-746: HTML is served from a short-lived (5 min) ticket URL with
  `Content-Security-Policy: sandbox; default-src 'none'; ...` and shown in an
  `<iframe sandbox="">` (TC-748).
- FR-747: Office files convert via headless LibreOffice (isolated profile,
  `unshare -rn` when available, 60 s timeout with process-tree kill, one
  conversion at a time, `%PDF-`-checked cache in the state dir); otherwise the
  response is `preview: unavailable` with a download link (TC-749, TC-750).
- FR-748: a workspace scan before/after each agent turn feeds an in-memory
  change log (50 turns x 200 files) exposed at `GET /api/docs/changes?node=`
  for the graph-view panel (TC-751, TC-752). The graph node id is captured when
  the turn starts.
- FR-749 / FR-750: dashboard output links (`file://`, workspace-relative paths,
  markdown links) open the preview (TC-753); the TUI emits OSC 8 hyperlinks and a
  plain left click opens the dashboard URL or the OS opener (TC-754, TC-755).
- UX-753 / INT-754: isolated `docs.js` (`window.DmuxDocs`) and `docs.css`;
  only two lines changed in `app.js` (deep-link survival and a snapshot event)
  (TC-757, TC-759). Headless Chrome e2e renders a table + highlighting (TC-758).

## Review rounds

- opencode glm reviewed deepseek's lib modules (4 major, 9 minor); deepseek
  reviewed glm's UI (3 major, 9 minor). Fixes landed by glm and Cursor.
- Cursor (composer-2.5, Ask mode) reviewed the backend (1 blocker, 5 major,
  6 minor): ticket symlink swap, whole-file Office reads, cache poisoning,
  Windows process-tree kill, save TOCTOU narrowing and `spawn_blocking` error
  mapping were fixed.
- Cursor reviewed the TUI changes: graph node attribution, OSC 8 target
  control characters, Windows opener quoting (now `rundll32
  url.dll,FileProtocolHandler`), and a Windows-flaky polling timeout were fixed.
- Cursor's docs.js review raised two majors, both fixed (implemented by Cursor,
  hardened by Grok Bot after a Cursor re-review): the token no longer appears in
  OSC 8/opener URLs (single-use `/docs/open/{ticket}` page with a nonce CSP), and
  server HTML goes through a client-side allowlist sanitizer (`sanitizeInto`)
  as defense in depth on top of ammonia.

## Test evidence

- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`: pass.
- `cargo test --all-targets`: 464 passed, 0 failed, 8 ignored on Linux (after rebase on #31 and #33); Windows CI green (unix-only tests gated).
- `cargo test --lib web::docs_api -- --include-ignored`: 10 passed (real
  LibreOffice conversion, TC-750).
- `cargo test --test web_docs_preview -- --ignored`: pass (Chrome + python
  Playwright, TC-758; skips when either is missing).
- Screenshots: `docs/images/docs-preview.png`, `docs/images/docs-edit-split.png`.

## Residual risks

- OQ-740: concurrent turns from several agents share one workspace scan window,
  so a file written during overlapping turns can be attributed to both.
- OQ-741: HTML preview runs without scripts by design; script-driven pages look
  incomplete.
- OQ-742: LibreOffice network isolation is best-effort (`unshare -rn` on Linux
  only); the PDF iframe is not sandboxed because Chrome's PDF viewer refuses
  sandboxed frames (served same-origin with `application/pdf` only).
- View tickets are capability URLs, reusable for 5 minutes.
- A writer outside dmuxai can still race the final rename (window narrowed to
  a re-hash before rename).
- The dashboard URL printed at startup still carries `?token=` (pre-existing
  behavior, out of scope). TUI OSC 8 targets are `file://` only, and clicks use
  single-use open tickets whose page stores the token in sessionStorage
  without putting it in any URL.
- Mouse capture can swallow terminal ctrl-click on OSC 8; the in-app click covers it.
- `NO_COLOR` disables both OSC 8 and click hit-testing in the TUI.

## Rollback

No schema change and no persistent state besides the Office PDF cache under
the state dir (`office-cache/`, safe to delete). Revert the PR; removing
`src/docs`, `src/web/docs_api.rs`, `docs.js`/`docs.css` and the wiring lines in
`server.rs`, `main.rs`, `app.rs`, `index.html`, `app.js` restores the old behavior.
