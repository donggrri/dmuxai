# PLAN: Document preview and edit

Spec: [SPEC-docs-preview](../specs/SPEC-docs-preview.md)
Status: in progress

## Architecture

- **`src/docs/`** (library, no HTTP): `mod.rs` (DocKind, limits, DocsError,
  `sha256_hex`, `mtime_ms`), `confine.rs` (Workspace, FR-740), `list.rs`
  (FR-741), `file.rs` (read/save, FR-742/744/745), `markdown.rs` (FR-743),
  `office.rs` (FR-747), `links.rs` (FR-749), `changes.rs` (FR-748). Each
  submodule file starts with its Rust contract.
- **`src/web/docs_api.rs`**: `pub struct DocsService { workspace, read_only,
  changes: ChangeLog, soffice: Option<PathBuf>, tickets }` and
  `pub fn router(service: Arc<DocsService>) -> Router<()>`-compatible routes,
  merged into `build_router_with` *before* the auth middleware layer so the
  same token/Host/Origin checks apply. `DashboardServices` gains
  `pub docs: Option<Arc<DocsService>>`; no docs service -> docs routes 503.
- **Assets**: `src/web/assets/docs.js`, `src/web/assets/docs.css` served at
  `/assets/docs.js` and `/assets/docs.css` (same headers as app.js/app.css).
  `index.html` only gains the two tags and one `<section id="docs">`. `app.js`
  gets two small hooks only (keep `#doc=` when stripping `?token=`, and a
  `dmuxai:snapshot` window event), to stay isolated from parallel PRs C/E/G/H.
- **`src/main.rs`**: builds `DocsService` (serve: read_only = true; `--web`:
  false, shares its `ChangeLog` with the App).
- **`src/app.rs`**: on turn start, scan workspace on a worker thread; on turn
  end, scan again, diff, push `TurnChanges` (agent, graph/node id, output tail,
  links). TUI transcript: file links get OSC 8 `file://` targets + click-to-open
  through a single-use open ticket; the App never holds the API token (FR-750).

## HTTP API (all under the dashboard token)

| Method | Path | Body / query | 2xx | Errors |
| --- | --- | --- | --- | --- |
| GET | `/api/docs` | – | `{entries:[{path,kind,size,mtime_ms,editable}], truncated}` | – |
| GET | `/api/docs/file` | `?path=` | `{path,kind,editable,size,mtime_ms,sha256,content}` | 400 dir, 403 outside, 404 |
| PUT | `/api/docs/file` | `{path,content,base_sha256}` | `{sha256,mtime_ms,size}` | 403 html/read-only/outside, 404, 409 `{error,sha256,mtime_ms}`, 413, 415 |
| GET | `/api/docs/render` | `?path=` (markdown) | `{html}` | 403/404/413/415 |
| POST | `/api/docs/render` | `{text}` | `{html}` | 413 |
| POST | `/api/docs/view` | `{path}` | html/pdf: `{preview:"frame", kind, url}`; office: `{preview:"pdf", url, download}` or `{preview:"unavailable", reason, download}`; other: `{preview:"download", download}` | 403/404 |
| GET | `/docs/view/{ticket}` | – | raw bytes (html: sandbox CSP; pdf: `application/pdf` inline) | 404 |
| GET | `/docs/download/{ticket}` | – | raw bytes, `Content-Disposition: attachment` | 404 |
| GET | `/api/docs/changes` | `?node=` | `{turns:[TurnChanges]}` newest first | – |
| GET | `/docs/open/{ticket}` | – (single-use, 60 s, minted by the TUI click) | no-store page that stores the token in sessionStorage and replaces itself with `/#doc=<path>` (nonce CSP) | 404 |

Error bodies are `{"error": "..."}` as in slice B. Tickets: 128-bit random hex,
5-minute expiry, bound to one canonical path and one purpose (view/download),
at most 256 live (oldest evicted). Ticket URLs are outside `/api/` so the token
is not needed; the ticket is the capability.

## JS module contract (`docs.js`)

`window.DmuxDocs = { open(path), close(), linkify(element, links), changes(nodeId) -> Promise<turns> }`.
The module reads the token the same way `app.js` does (sessionStorage key used
by app.js), renders the file browser, the per-turn changes list (with output
tail linkified by `links`), and the preview pane. Deep link `#doc=<path>`.
Server HTML is inserted only through `sanitizeInto`, a client-side allowlist
sanitizer layered on top of ammonia.
