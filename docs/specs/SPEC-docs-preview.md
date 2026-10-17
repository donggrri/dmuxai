# SPEC: Document preview and edit in the web dashboard

Status: Implemented in v0.2.0 (slice F; OQ-740..742 deferred)
Owner: DmuxAI
Created: 2026-10-17
Baseline: `1c1f692`
Related: SPEC-web-dashboard (FR-520..FR-530, slice B), SPEC-theme-colors (UX-600), SPEC-graph-view (node side panel)

## 1. Decision summary

Agents write documents (Markdown, HTML, text, code, Office files) and the
user wants to read and fix them without leaving DmuxAI. The web dashboard
gets a document browser and a preview pane: Markdown is rendered GitHub-style
and can be edited in a split view, text and code files are editable, HTML is
preview-only in a sandboxed frame, and Office files are previewed through a
LibreOffice PDF conversion or offered as a download. Each agent turn reports
the files it created or changed, and file links in agent output (dashboard and
TUI) open the preview. Everything is confined to the workspace, uses the
existing token/Host/Origin checks, and `dmuxai serve` stays read-only.

## 2. Scope

### In scope

- A workspace document list and a per-turn / per-graph-node "files changed"
  list, both over the authenticated API.
- Preview for Markdown (rendered), text/code (highlighted), HTML (sandboxed
  frame), PDF and Office (via `soffice` when available, else download).
- Editing of Markdown, plain text, JSON, YAML, TOML, CSV and code files with a
  hash-based concurrency check.
- Clickable file links in dashboard agent output and OSC 8 / click links in the
  TUI transcript.

### Out of scope / non-goals

- Creating, renaming or deleting files from the dashboard.
- Editing HTML, Office or binary files; WYSIWYG editing.
- Images referenced from previewed Markdown (shown as alt text links).
- Real-time collaborative editing; the check only detects conflicts.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-740 | Per-turn changes are detected by comparing a workspace scan before and after the turn, so concurrent turns (parallel graph nodes) may each list a file another turn wrote. Accepted for v0.2.0. | Dongjin | Review |
| OQ-741 | HTML preview runs without scripts (`sandbox=""`). Documents that need JavaScript show their static markup only. | Dongjin | Review |
| OQ-742 | LibreOffice network isolation is best effort (isolated profile; `unshare -rn` on Linux when it works). | Dongjin | Review |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Workspace path | A `/`-separated path relative to the workspace root, never absolute, never containing `..` | An OS path |
| Document kind | One of `markdown`, `text`, `html`, `pdf`, `office`, `binary`, derived from the file extension | MIME type |
| Editable | `markdown` or `text` kind, at most 2 MiB, valid UTF-8 | Previewable |
| Base hash | The SHA-256 (hex) of the file content the editor loaded | mtime |
| View ticket | A random, path-bound, 5-minute URL token that serves one file's raw bytes to an iframe or download | The dashboard API token |
| Turn changes | The files created, modified or deleted while one agent turn ran, with the agent id, optional graph/node id and the tail of the agent output | Git status |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-740 | Must | Given any docs API request naming a path | The path resolves to a file inside the canonical workspace root | Absolute paths, `..` components, NUL bytes, drive prefixes, and symlinks resolving outside the root -> 403 `{"error":"path outside workspace"}`; missing -> 404 | TC-740, TC-741 |
| FR-741 | Must | `GET /api/docs` | Lists workspace files as `{path, kind, size, mtime_ms, editable}` sorted by path, skipping `.git`, `target`, `node_modules`, `.dmuxai` and symlinked directories | More than 5000 files -> the first 5000 and `"truncated": true` | TC-742 |
| FR-742 | Must | `GET /api/docs/file?path=` | Returns `{path, kind, editable, size, mtime_ms, sha256, content}`; `content` is the UTF-8 text for `markdown`/`text`/`html` kinds | Text larger than 2 MiB or not UTF-8 -> `content: null`, `editable: false`; a directory -> 400 | TC-743 |
| FR-743 | Must | `GET /api/docs/render?path=` or `POST /api/docs/render {text}` for Markdown | Returns `{html}` rendered with GFM tables, task lists, strikethrough, autolinks and footnotes; fenced code is highlighted with `hl-*` classes; external links get `rel="noopener noreferrer"` and `target="_blank"` | All output is sanitized: no `<script>`, `<iframe>`, `<style>`, `on*` attributes or `javascript:`/`data:` link URLs survive; input over 2 MiB -> 413 | TC-744, TC-745 |
| FR-744 | Must | `PUT /api/docs/file {path, content, base_sha256}` in `dmuxai --web` | An editable file is replaced atomically and the response is `{sha256, mtime_ms, size}` | Current hash differs from `base_sha256` -> 409 `{error, sha256, mtime_ms}` and the file is unchanged; content over 2 MiB -> 413; missing file -> 404 (no creation) | TC-746 |
| FR-745 | Must | A save request for a non-editable file or in `dmuxai serve` | Nothing is written | `.html`/`.htm` -> 403 `html is preview-only`; other non-editable kinds -> 415; `serve` -> 403 `read-only: start dmuxai --web` | TC-747 |
| FR-746 | Must | The user previews an HTML file | `POST /api/docs/view {path}` returns `{url}` for a view ticket; the page shows it in `<iframe sandbox="">` (no `allow-scripts`, no `allow-same-origin`); the response carries `Content-Security-Policy: sandbox; default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:` | Unknown, expired or other-path tickets -> 404; tickets never authorize `/api/*` | TC-748 |
| FR-747 | Must | The user previews an Office file (`docx`, `doc`, `pptx`, `ppt`, `xlsx`, `xls`, `odt`, `odp`, `ods`, `rtf`) | With `soffice` available (`DMUXAI_SOFFICE`, else `soffice`/`libreoffice` on PATH) the file is converted headless to PDF in a temp dir with an isolated profile, cached by content hash, and the view returns a PDF ticket | No `soffice`, a failure, or a 60 s timeout (process killed) -> `{"preview":"unavailable","reason", "download":url}` and the page offers Download | TC-749, TC-750 |
| FR-748 | Must | An agent turn ends (single agent or graph node) | `GET /api/docs/changes[?node=ID]` lists turn records newest first: `{id, agent, graph_id, node_id, started_ms, finished_ms, files:[{path, change}], output_tail, links}`; `change` is `created`, `modified` or `deleted` | At most 50 turns and 200 files per turn are kept; a scan over 20000 files stops and marks `truncated`; `serve` returns an empty list | TC-751, TC-752 |
| FR-749 | Must | Agent output contains `file://` URIs, workspace-relative paths with an extension, or Markdown links | `links` holds the deduplicated workspace paths of existing files they name; the dashboard turns exactly those into links that open the preview, and `#doc=<path>` opens a document directly | Paths outside the workspace or that do not exist are not linked; URLs with other schemes are not file links | TC-753 |
| FR-750 | Should | The TUI transcript shows a linkable file path (FR-749 rules) | The path is emitted as an OSC 8 `file://` hyperlink and a left click (no drag) opens the dashboard preview through a single-use 60 s open ticket (`/docs/open/{ticket}`; the API token never appears in a TUI target or URL) when the web server runs, else the OS default opener for the file | Opener failure -> a status message, never a crash; `NO_COLOR`/dumb terminals keep plain text (no OSC 8 and no click hit-testing) | TC-754, TC-755 |
| NFR-751 | Must | Any docs route | Docs API routes require the dashboard token and pass the Host/Origin checks of FR-523; view tickets are bound to one canonical path | Missing token -> 401; foreign Origin/Host -> 403 | TC-756 |
| NFR-752 | Must | Large or hostile input | Read/save/render limit 2 MiB, list 5000 entries, conversion 60 s, changes 50 x 200, output tail 40 lines of at most 500 chars | Limits return the documented errors without panics | TC-743, TC-746, TC-752 |
| UX-753 | Must | The user opens a document in the dashboard | Markdown shows the rendered preview first; Edit opens a split view (editor left, live preview right, debounced render); Save/Cancel; a 409 shows a conflict notice with Reload; HTML/Office show a "Preview only" badge; `serve` hides Edit with a hint; the pane uses the dashboard theme variables | Unsaved edits prompt before closing or switching documents | TC-757, TC-758 |
| INT-754 | Must | Other dashboard modules (graph view side panel, prompt/transcript) | `docs.js` exposes `window.DmuxDocs.open(path)`, `window.DmuxDocs.linkify(element, links)` and `window.DmuxDocs.changes(nodeId)`; docs assets live in `/assets/docs.js` and `/assets/docs.css` only | Missing module -> other panels keep working | TC-759 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-740 | FR-740 | unit | temp workspace with a file | resolve `../x`, `/etc/passwd`, `a/../../x`, `C:\x`, `a\0b`, `ok.md` | only `ok.md` resolves; others are `OutsideWorkspace` | `cargo test --lib docs::confine::tests::rejects_traversal_and_absolute_paths` |
| TC-741 | FR-740 | unit (unix) | symlink `link -> /tmp/outside/secret.md` and `inner -> sub/ok.md` | resolve both | outside link rejected, inner link accepted | `cargo test --lib docs::confine::tests::rejects_symlink_escape` |
| TC-742 | FR-741 | unit | tree with `.git`, `target`, `node_modules`, nested docs | list | ignored dirs absent, sorted, kinds/editable correct; limit truncates | `cargo test --lib docs::list` |
| TC-743 | FR-742, NFR-752 | integration | router over a temp workspace | GET file for md, binary, >2 MiB, dir | content/sha256 present for md; null for binary/large; 400 for dir | `cargo test --lib web::docs_api::tests::read_file_contract` |
| TC-744 | FR-743 | unit | GFM sample (table, tasks, ~~del~~, bare URL, footnote, rust fence) | render | `<table>`, disabled checkboxes, `<del>`, `<a href="https://…" rel="noopener noreferrer"`, footnote ref, `hl-` spans | `cargo test --lib docs::markdown::tests::renders_gfm_features` |
| TC-745 | FR-743 | unit | Markdown with `<script>`, `<img onerror>`, `[x](javascript:…)`, `<iframe>`, `<style>` | render | none of them survive | `cargo test --lib docs::markdown::tests::sanitizes_script_and_handlers` |
| TC-746 | FR-744, NFR-752 | integration | `--web` router, file loaded | PUT with right hash, then stale hash, then 3 MiB | 200 + new hash; 409 with current hash and file unchanged; 413 | `cargo test --lib web::docs_api::tests::save_with_concurrency_check` |
| TC-747 | FR-745 | integration | `--web` and serve routers | PUT `.html`, `.png`, and `.md` in serve | 403 html, 415 png, 403 read-only; files unchanged | `cargo test --lib web::docs_api::tests::save_rejections` |
| TC-748 | FR-746 | integration | html file | POST view, GET ticket URL, GET with bogus ticket, GET `/api/state?token=<ticket>` | sandbox CSP headers; 404 bogus; 401 api | `cargo test --lib web::docs_api::tests::html_view_ticket_is_sandboxed` |
| TC-749 | FR-747 | integration | `DMUXAI_SOFFICE` pointing to a missing binary (injected config) | POST view for `x.docx` | `preview: unavailable` + download URL that serves attachment bytes | `cargo test --lib web::docs_api::tests::office_falls_back_without_soffice` |
| TC-750 | FR-747 | integration (ignored) | real `soffice` on PATH | convert a generated `.odt`/`.docx` | PDF ticket served as `application/pdf` | `cargo test --lib -- --ignored office_converts_with_soffice` |
| TC-751 | FR-748 | unit | temp workspace scanned, then a file created, one modified, one deleted, one under `target/` | diff scans | three changes with correct kinds; ignored dir absent | `cargo test --lib docs::changes::tests::scan_diff_reports_created_modified_deleted` |
| TC-752 | FR-748, NFR-752 | integration | change log with records for nodes `a`, `b`, 60 turns | GET changes, `?node=a` | newest first, 50 kept, node filter applied; serve -> empty | `cargo test --lib web::docs_api::tests::changes_api_filters_and_bounds` |
| TC-753 | FR-749 | unit | output text with `file://<root>/docs/a.md`, `src/x.rs:12`, `[b](docs/b.md)`, `/etc/passwd`, `missing.md`, `https://x.y` | extract links | exactly `docs/a.md`, `src/x.rs`, `docs/b.md` | `cargo test --lib docs::links::tests::extracts_workspace_links_only` |
| TC-754 | FR-750 | unit | web running / not running | compute open target for `docs/a.md` | dashboard URL with `#doc=docs%2Fa.md` / OS opener for the absolute file | `cargo test --bin dmuxai tui_file_link_open_target` |
| TC-755 | FR-750 | unit | transcript line with a link rendered in a test backend | render | link cells carry an OSC 8 sequence; plain text without links unchanged | `cargo test --bin dmuxai tui_file_links_emit_osc8` |
| TC-756 | NFR-751 | integration | docs routes | request without token, with foreign Origin | 401 / 403 | `cargo test --lib web::docs_api::tests::docs_routes_require_auth` |
| TC-757 | UX-753, INT-754 | unit | embedded assets | GET `/`, `/assets/docs.js`, `/assets/docs.css` | index loads docs assets and has the docs panel; JS has Edit split view, conflict handling, `sandbox=""`, no `allow-same-origin` | `cargo test --lib web::assets::tests::static_page_has_docs_panel` |
| TC-758 | UX-753 | e2e (ignored) | `dmuxai serve` over a fixture workspace, Chrome driven by python Playwright (skips when missing) | open `#doc=README.md` | DOM has rendered Markdown (`.md-body table`) | `cargo test --test web_docs_preview -- --ignored` |
| TC-759 | INT-754 | unit | docs.js | inspect | defines `DmuxDocs.open`, `DmuxDocs.linkify`, `DmuxDocs.changes` | `cargo test --lib web::assets::tests::docs_module_exposes_api` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | No schema change. Turn changes live in memory only. Office PDFs are cached under the dmuxai state dir (`office-cache/<sha256>.pdf`, falling back to the OS temp dir) and re-checked for a `%PDF-` header on every hit. | Removing the feature leaves no state | TC-751 |
| Security / permissions | Workspace confinement, token + Host/Origin checks, sandboxed HTML with CSP, no HTML saves, serve read-only, atomic writes to the canonical target | Must not weaken FR-523/FR-526 | TC-740, TC-741, TC-745..TC-748, TC-756 |
| Platform | Windows: drive and UNC paths rejected, symlink test unix-only, `rename` replaces existing files; opener `cmd /c start`, macOS `open`, Linux `xdg-open` | Without `soffice` Office falls back to download | TC-740, TC-749, TC-754 |

## 7. Implementation plan

See [PLAN-docs-preview](../plans/PLAN-docs-preview.md) and [TASKS-docs-preview](../tasks/TASKS-docs-preview.md).

## 8. Review and test exit criteria

- [ ] All Must requirements map to passing evidence.
- [ ] The diff has been reviewed against scope and non-goals.
- [ ] Required Rust checks pass: format, all targets test, and clippy.
- [ ] Ignored tests (TC-750, TC-758) are reported as run or skipped.
- [ ] `../reviews/REVIEW-docs-preview.md` records commands, results and residual risk.
