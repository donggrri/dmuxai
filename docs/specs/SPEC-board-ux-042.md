# SPEC: Board detail modal, Markdown path viewer, board chat answers

Status: approved · Target: v0.4.2
Owner: DmuxAI
Created: 2026-11-27
Baseline: `77d4fbc` (v0.4.1)
Related: SPEC-decision-board (FR-3120, FR-3123, FR-3132, FR-3134, FR-3138), SPEC-docs-preview (Markdown renderer, confinement), SPEC-web-local-access (dashboard access rules)

## 1. Problem

Feedback on the v0.4.1 decision board:

1. Clicking a board item opens a right-side drawer; the owner wants a centered
   popup so the detail is easier to read.
2. Agents often answer with a Markdown file path (for example
   `~/tmp/review/NOTES.md` or `docs/specs/SPEC-x.md`). The dashboard shows it as
   plain text; the owner wants to click it and read the file.
3. The owner types a question into the item's side chat and presses Send, and
   nothing ever answers.

### Root cause of 3

`Send` posts `POST /api/board/{id}/messages`, which only appends an `owner`
thread message (FR-3109) and, when a `[board]` notify target is configured,
delivers an owner `message` event (FR-3135). Nothing runs an agent: the side
chat was designed as a mailbox for a coordinator bot that polls
`board wait`. With no bot waiting and no notify hook configured, the question
is stored and no one reads it. The separate "Ask planner" box (FR-3132/3134)
is the only control that runs an agent, and having two text boxes with
different behaviour is confusing.

A second cause (#357): on a tokenless dashboard (the default since v0.4.1)
`dmuxaiOpenEventSource` returns `null` when there is no token, so the board
never receives SSE updates; an ask gets 202 and the UI stays on "Asking
planner…" forever because `boardAskPending` is cleared only by an SSE event.
`boardFetchJson` also omits the `X-Dmuxai-Client: 1` header other dashboard
fetchers send.

## 2. Decisions

| ID | Topic | Decision |
| --- | --- | --- |
| OQ-3412 | Detail layout | The item detail becomes a centered modal dialog (same content and actions as the drawer). |
| OQ-3413 | Markdown files outside the workspace | Read-only viewing of `.md` files under the OS user's home is allowed, hidden directories under home excluded; anything else is refused. |
| OQ-3414 | Side chat Send | In `--web` (agent launch allowed) one chat box serves both: `Send` stores the owner message and asks the planner read-only (same path as `board ask` without `--revise`); a "Note only" checkbox keeps the old store-only behaviour for a waiting bot. `Ask & revise plan` uses the same box. In view-only `serve`, `Send` stays store-only (no agent may run there). Approval stays human-only: an answer is a `planner` thread message and never a decision. |

## 4. Requirements

| ID | Priority | Requirement |
| --- | --- | --- |
| FR-3412 | Must | Clicking a board card or decision card opens the item detail as a centered modal (`role="dialog"`, `aria-modal="true"`, labelled by the item title) over a dimmed backdrop, width `min(960px, 92vw)`, max height `90vh`, content scrolls inside the modal. It shows everything the drawer showed (summary, plan table and edit, diff, decisions, checks, Run, chat, ask) with the same element ids, so existing actions keep working. |
| FR-3413 | Must | The modal closes on Esc, on a backdrop click, and on a visible close button (`aria-label="Close"`); focus moves into the modal when it opens and returns to the clicked card when it closes; the page behind does not scroll while it is open. A click inside the modal never closes it. |
| FR-3414 | Must | `GET /api/md/view?path=P` (both `--web` and `serve` routers, behind the same `security_and_auth_middleware` as other `/api/*` routes, so token/Host/Origin rules are identical) returns `{path, html, bytes}` where `html` is `crate::docs::markdown::render_markdown` output (sanitized; raw HTML and scripts never pass through). |
| FR-3415 | Must | Path resolution for FR-3414: `~/` or `~` prefix → the OS user's home (`HOME`; refused when unset, empty or `/`); absolute paths as given; anything else relative to the dashboard's current workspace root. The path is canonicalized (`realpath`, which follows symlinks) and allowed only when the canonical file is (a) inside the canonical workspace root, or (b) inside the canonical home with no path component below home that starts with `.`. The canonical file name must end in `.md` (case-insensitive) and be a regular file. Size cap 1 MiB (larger → 413). Traversal (`..`), symlinks that resolve outside the allowed roots, NUL bytes, directories, non-`.md` files → 403/400 with `{error}` that never echoes file content. Not found → 404. After opening the file the server re-checks the opened file (not the path): it must be a regular file, its canonical path (`/proc/self/fd/N` where available, else re-canonicalizing the same path) must equal the validated canonical path and still be inside an allowed root, otherwise 403; this narrows the time-of-check/time-of-use window if a parent directory is swapped for a symlink between validation and open. |
| FR-3416 | Must | Dashboard text that shows agent or board output (board chat bubbles, board item summary, and main chat assistant bubbles) turns Markdown file paths into clickable links: tokens matching `~/…​.md`, `/…​.md`, or a relative `dir/…​.md` / `FILE.md` (no spaces, no `://`), also inside inline code. Linkify runs on text nodes only (never re-parses HTML, never inside existing `<a>`), and is idempotent on re-render. |
| FR-3417 | Must | Clicking such a link opens a read-only Markdown viewer modal (same modal style as FR-3412, Esc/backdrop/close button) that fetches FR-3414 and shows the path, the rendered HTML and errors (403/404/413) as a plain message. The viewer has no edit control. |
| FR-3418 | Must | Board side chat in `--web`: one textarea; `Send` (and Ctrl/Cmd+Enter) calls `POST /api/board/{id}/ask` `{text, revise:false}` unless "Note only" is checked, in which case it calls `POST /api/board/{id}/messages` as before. While the ask runs, a busy line "Asking planner…" shows under the chat and Send is disabled; the answer arrives as a `planner` message through SSE; an ask error is shown in the same line. `Ask & revise plan` uses the same textarea (disabled without a plan). The separate "Ask planner" textarea is removed. |
| FR-3419 | Must | Board side chat in view-only `serve` (`boardLaunchAllowed` false): only `Send` (store-only `/messages`) and no ask/revise controls, as today. |
| FR-3420 | Must | FR-3418 adds no new approval path: `ask` keeps FR-3138 (never writes a decision or check, never moves to approved/rejected); `Send` is triggered only from a click or key handler. Amended by SPEC-md-viewer-chat FR-3842/FR-3845: asks stay read-only; edit turns are allowed only from the viewer chat (FR-3842) or owner CLI (FR-3845) and never write a decision or check. |
| FR-3422 | Must | Tokenless dashboards (#357): `dmuxaiOpenEventSource` opens the stream without a token (adds `token=` only when one exists) and the server accepts that stream under the same tokenless rules as other `/api/*` reads (SPEC-web-local-access); `boardFetchJson` sends `X-Dmuxai-Client: 1` on every request; while an ask is pending the board polls `GET /api/board/{id}` every 3 s (stops when a new `planner` message arrives, the drawer closes, or after 15 min, then shows "No answer yet — check the thread later"), so a missing stream can never leave the UI stuck. |
| FR-3421 | Should | README board section documents: Send asks the planner (read-only) and posts the answer in the thread, "Note only" just leaves a message for a waiting bot, and `[board] planner = "<agent>"` picks the agent. |
| NFR-3412 | Must | No new dependencies. The viewer endpoint reads at most 1 MiB per request; linkify adds no work when the board/chat is not rendering. |

## 5. Acceptance scenarios and test design

| ID | Covers | Test |
| --- | --- | --- |
| TC-3412 | FR-3412, FR-3413 | `tc_3412_board_detail_is_centered_modal_with_close_and_esc` |
| TC-3414 | FR-3414 | `tc_3414_md_view_renders_workspace_markdown_sanitized` |
| TC-3415 | FR-3415 | `tc_3415_md_view_confines_paths_home_workspace_symlink_ext_size` (also: `tc_3415_md_view_rechecks_opened_file_after_swap` — a path that resolved inside the workspace and is then replaced by a symlink to a `.md` outside both roots is refused after open) |
| TC-3416 | FR-3414 | `tc_3416_md_view_requires_same_auth_as_api` |
| TC-3417 | FR-3416, FR-3417, NFR-3412 | `tc_3417_markdown_paths_linkified_and_open_viewer` |
| TC-3418 | FR-3418, FR-3419, FR-3420 | `tc_3418_board_chat_send_asks_planner_unless_note_only` |
| TC-3422 | FR-3422 | `tc_3422_tokenless_board_stream_and_client_header` |
| TC-3421 | FR-3421 | `tc_3421_readme_documents_board_chat_send` |
