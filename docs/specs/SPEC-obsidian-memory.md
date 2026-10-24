# SPEC: Obsidian memory (approved notes injected into every runner)

Status: Implemented in v0.2.0 (slice E + shared memory MCP follow-up)
Owner: DmuxAI
Created: 2026-10-17
Baseline: `1c1f692`
Related: SPEC-web-dashboard (slice E), cursor-atomic-workflow `scripts/memory.mjs`, SPEC-web-dashboard INT-560 (shared `[[mcp.servers]]`)

## 1. Decision summary

Users keep durable notes ("this repo uses X", "never do Y") in an Obsidian
vault and want every agent to see them without retyping. DmuxAI ports the
cursor-atomic-workflow memory: one-line dated entries in plain markdown files
inside the vault, a size-bounded `<memory>` block prepended to every runner's
prompt, explicit saves by the human (`/remember`, dashboard Save), and an
approval queue for agent proposals. The same memory is exposed as a stdio MCP
server (`dmuxai mcp-memory`) so any MCP-capable runner can search it and
propose entries; agents can never write memory without human approval.

## 2. Scope

### In scope

- `[memory]` config: `vault`, `dir` (default `memory`),
  optional `project` and `inject`.
- Vault layout compatible with `memory.mjs`: `human.md`,
  `projects/<project>.md`, `decisions/<name>.md`, `gotchas/<name>.md`,
  `queue/proposals.json`, all under `<vault>/<dir>`.
- Entry format `- [YYYY-MM-DD] text`, budgets (3000 / 900 / 200 chars) and the
  secret filter.
- Context injection into every runner turn (Cursor, AGY, Codex, opencode and
  any `[[agents.custom]]`, including graph nodes).
- TUI `/remember` and `/memory`; dashboard Memory panel; web API.
- `dmuxai mcp-memory` stdio MCP server with `memory_search`,
  `memory_get_context` and `memory_propose`.

### Out of scope / non-goals

- Pipeline captures (`captures/*.json`), `autoSave`, the audit log and the
  vault git commit from `memory.mjs`.
- Editing or deleting existing entries (users do that in Obsidian).
- An Obsidian plugin, vault sync, or embeddings/semantic search.
- Automatic registration of the memory MCP server (removed 2026-10-24, superseded by SPEC-mcp-propagation-removal FR-2083; users configure `dmuxai mcp-memory` in agent configs).
- An MCP tool that writes memory without approval.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-720 | Dates use UTC (`YYYY-MM-DD`), the same as `memory.mjs` (`toISOString`). | Dongjin | Review |
| OQ-721 | Automatic registration superseded (Dongjin decision 2026-10-24): users configure `dmuxai mcp-memory --workspace <dir>` in each agent's own MCP config. | Dongjin | Release |
| OQ-722 | `/remember` and the dashboard Save are human actions and save directly; only agent proposals (MCP) go through the approval queue. | Dongjin | Review |
| OQ-723 | Budgets count Unicode scalar values (`chars()`), not UTF-16 units as `memory.mjs` does; the `…(N omitted)` marker is appended after the 900-char budget exactly like `fitEntries`. | Dongjin | Review |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Memory root | `<vault>/<dir>`; every read and write stays inside it | The Obsidian vault root |
| Entry | One markdown line matching `^- \[\d{4}-\d{2}-\d{2}\] ` | Free text, headings, frontmatter |
| Target | `human.md`, `projects/<name>.md`, `decisions/<name>.md` or `gotchas/<name>.md`; `<name>` matches `^[a-z0-9][a-z0-9_-]{0,47}$` | An arbitrary path |
| Project | Short repo name: `[memory] project`, else the workspace directory basename lowercased, non `[a-z0-9]` runs replaced by `-`, trimmed, at most 24 chars, `repo` if empty | Conversation id |
| Proposal | Queue item `{id, shortRepo, slug, target, text, status, createdAt, decidedAt?}` | An entry |

Proposal states: `pending` -> `approved` (entry written) or `rejected`.
Both are terminal; deciding a non-pending proposal is an error.

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-720 | Must | Given `[memory] vault = "<path>"` in the user or workspace `dmuxai.toml` (workspace wins per key) | Memory is enabled with root `<vault>/<dir>`; `dir` defaults to `memory`; `~/` in `vault` expands to the home directory and a relative `vault` is relative to the workspace; `project` overrides the derived project name; `inject = false` disables prompt injection only | No `[memory]` or no `vault` -> memory disabled silently; a vault path that is not an existing directory, or a `dir` that is absolute or contains `..` -> a config warning and memory disabled; an invalid `project` -> warning and the derived name is used | TC-720 |
| FR-721 | Must | Given a memory root with entries | `build_context(project)` returns a `<memory>` block with sections in this order: `## User` (last 20 `human.md` entries), `## Project (<project>)`, `## Decisions`, `## Gotchas` (files sorted by name); only entry lines are used; CRLF files read the same as LF | No entries or a missing root -> empty string (prompts are unchanged); each section keeps whole entries within 900 chars and appends `…(N omitted)`; the body is cut at 3000 chars (on a char boundary) with `…(truncated)` | TC-721, TC-722 |
| FR-722 | Must | Given memory enabled with a non-empty context and any runner (built-in, opencode, custom, graph node) starting a turn | The prompt sent to the runner is `<memory block>\n\n<prompt>`; it is injected on a turn that starts a fresh provider session, and again on a resumed session only when the context changed since that agent last received it in that conversation (recorded only after the send succeeds) | Memory disabled, `inject = false` or an empty context -> the prompt is byte-for-byte unchanged; a read error never blocks the turn; the stored conversation keeps the user's prompt without the block | TC-726 |
| FR-723 | Must | Given any text to save or propose | It is accepted only if, after trimming, it is non-empty, a single line, at most 200 chars (Unicode scalar values), and matches none of the secret patterns (`password`, `passwd`, `비밀번호`, `secret`, word `token`, `api key`/`api_key`/`apikey`, `bearer <x>`, `sk-…`, `ghp_…`, `-----BEGIN … PRIVATE KEY-----`, case-insensitive where `memory.mjs` is) and the target is valid | Rejections return a reason (`empty`, `multi-line`, `too long`, `looks like a secret`, `invalid target`) and write nothing | TC-723 |
| FR-724 | Must | Given a valid entry and target | `save` appends `- [YYYY-MM-DD] <text> <!-- src: dmuxai/<project> -->` to the target file under the root; a missing file is created with `---\ntype: <kind>\nname: <name>\n---\n\n# <name>\n\n` (`human.md` gets `type: human`, `name: human`); a placeholder line `(아직 기록 없음)` or `(no entries yet)` is removed; an `index.md` that exists gets `- [[<kind>/<name>]]` once | An identical text already present in the file -> nothing appended and the result says `already saved`; I/O errors are reported, never panic | TC-724 |
| FR-725 | Must | Given a proposal batch | Valid items are appended to `queue/proposals.json` as `pending` with a unique `id`; `approve(id)` writes the entry (FR-724) and marks it `approved` with `decidedAt`; `reject(id)` marks it `rejected`; `list_pending` returns pending items oldest first (`createdAt`, then `id`); the JSON stays readable by `memory.mjs` | Invalid items and duplicates of a non-rejected item with the same target and text are reported as rejected; unknown id -> `not found`; non-pending -> `already <status>`; a corrupt queue file is reported as an error and never overwritten; a batch whose `shortRepo` is not a valid name or whose `slug` is empty is refused as `invalid source` | TC-725 |
| FR-726 | Must | Given the TUI with memory enabled | `/remember <text>` saves to `projects/<project>.md`; `/remember --human <text>`, `/remember --decision <name> <text>` and `/remember --gotcha <name> <text>` choose other targets (a flag must be the whole first word); `/memory` lists pending proposals (id, target, text); `/memory approve <id>` and `/memory reject <id>` decide them; each prints one system line with the result | Memory disabled -> a system line explaining `[memory] vault`; validation errors print the reason; nothing is sent to any agent | TC-727 |
| FR-727 | Must | Given the dashboard server (`serve` or `--web`) and a valid token | `GET /api/memory` returns `{enabled, project, dir, context, pending}`; `POST /api/memory/save` `{text, target?}` saves (target defaults to `projects/<project>.md`); `POST /api/memory/proposals/{id}/approve` and `/reject` decide | Memory disabled -> `GET` returns `{enabled:false}` and writes return 409; invalid text/target -> 400 with the reason; unknown id -> 404; non-pending -> 409; no token -> 401; the absolute vault path is never returned | TC-728 |
| UX-728 | Must | Given the dashboard | A Memory panel shows the injected context preview, a one-line input with a target selector and a Save button, and the pending proposals each with Approve and Reject buttons; results show inline and the panel refreshes | Memory disabled -> the panel shows how to set `[memory] vault` and hides the controls | TC-731 |
| INT-729 | Must | Given `dmuxai mcp-memory [--workspace DIR] [--vault P] [--dir D] [--project N]` | A newline-delimited JSON-RPC 2.0 MCP server on stdio: `initialize` (protocol `2024-11-05`, `serverInfo.name = "dmuxai-memory"`, `capabilities.tools`), `tools/list`, `tools/call`, `ping`; tools `memory_get_context {project?}`, `memory_search {query, limit?}`, `memory_propose {text, target?}`; results are `{content:[{type:"text",text}], isError}` | Unknown method -> error `-32601`; a request without a string `method` -> `-32600`; malformed JSON, invalid UTF-8 or a line over 4 MiB -> error `-32700` and the server keeps serving; notifications get no response; memory disabled or a rejected proposal -> `isError: true` with the reason; stdout carries only protocol messages; EOF exits 0 | TC-729 |
| FR-730 | Must | Given `memory_search {query, limit}` | Returns entries containing every whitespace-separated query term (case-insensitive) as `<target>: <entry>` lines in the fixed order `human.md`, `projects/*`, `decisions/*`, `gotchas/*` (files by name, newest entry first within a file), at most `limit` (default 20) | Empty query -> `isError`; a `limit` that is not an integer in 1..=100 -> `isError`; no hits -> `no matches` | TC-730 |
| NFR-731 | Must | Given any save, approve or proposal | Writes stay inside the memory root (targets are validated by pattern, never joined from raw input); the queue is written to a temp file and renamed; tests use a temporary vault only; docs use generic paths such as `~/Obsidian/Vault` | A symlinked root is allowed (Obsidian setups), but no target can contain `/`, `\`, `..` beyond the fixed layout | TC-723, TC-725 |
| INT-732 | Removed | Given slice C's shared `[[mcp.servers]]` | **Removed** (2026-10-24): superseded by SPEC-mcp-propagation-removal FR-2083. | — | TC-2083 |
| FR-733 | Removed | Given `[memory]` resolves to a valid config (FR-720) after the user and workspace `dmuxai.toml` are merged | **Removed** (2026-10-24): superseded by SPEC-mcp-propagation-removal FR-2083. | — | TC-2083 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-720 | FR-720 | unit | temp vault dir, user + workspace toml | load settings | root, default dir, `~/` expansion, project override, warnings for missing vault / absolute or `..` dir / bad project | `src/settings.rs` `memory_settings_*` |
| TC-721 | FR-721 | unit | temp root with human, project, decisions, gotchas (one CRLF file) | `build_context` | exact section order and lines; empty root -> `""` | `src/memory.rs` `context_*` |
| TC-722 | FR-721 | unit | many long entries | `build_context` | section ≤ 900 with `…(N omitted)`; body ≤ 3000 + `…(truncated)`; multibyte safe | `src/memory.rs` `context_budget_*` |
| TC-723 | FR-723, NFR-731 | unit | — | validate texts/targets | each secret pattern, empty, multi-line, 201 chars (and 200 Korean chars OK), `../x.md`, `projects/A.md` rejected | `src/memory.rs` `validate_*` |
| TC-724 | FR-724 | unit | temp root | `save` twice, into new and placeholder files, with `index.md` | file created with frontmatter, line format, dedup, placeholder removed, index link once | `src/memory.rs` `save_*` |
| TC-725 | FR-725, NFR-731 | unit | temp root | propose / approve / reject / duplicate / unknown / corrupt queue | states, JSON fields, errors, corrupt file untouched | `src/memory.rs` `queue_*` |
| TC-726 | FR-722 | unit | `test_app` with a temp memory root | submit prompts (fresh, resumed, changed, disabled) | adapter request prompt starts with the block only when required; stored prompt unchanged | `src/app.rs` `memory_context_*` |
| TC-727 | FR-726 | unit | `test_app` with a temp memory root | `/remember …`, `/memory`, `/memory approve|reject` | files/queue changed, system lines, no agent request | `src/app.rs` `memory_slash_*` |
| TC-728 | FR-727 | integration | router with a temp memory root | GET/POST routes with and without token | statuses and JSON per FR-727; no absolute path in body | `src/web/server.rs` `memory_routes_*` |
| TC-729 | INT-729 | integration | built binary, temp vault | spawn `dmuxai mcp-memory --vault …`, send JSON-RPC lines | initialize, tools/list names, each tool result, `-32601`, `-32700`, disabled server `isError` | `tests/memory_mcp.rs` `mcp_memory_serves_tools_against_a_vault`, `mcp_memory_reports_disabled_memory`, `mcp_memory_rejects_*`, `mcp_memory_null_id_notification_is_silent`, `mcp_memory_oversized_line_is_parse_error_and_serving_continues` |
| TC-730 | FR-730 | unit | temp root | search terms, limit, empty | hits, limit, error | `src/memory.rs` `search_*` |
| TC-731 | UX-728 | unit + manual | embedded assets | read index/app assets | Memory panel ids and API paths exist; screenshot via headless Chromium recorded in the review | `src/web/assets.rs` `memory_panel_assets_present` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | Markdown files and `queue/proposals.json` in the vault; no SQLite change | Same layout as `memory.mjs`; removing `[memory]` disables everything | TC-724, TC-725 |
| Provider protocol | Prompt prefix only; no adapter protocol change | `inject = false` turns it off | TC-726 |
| Security / permissions | Secret filter; fixed target pattern; agents only propose; dashboard token required; no absolute path in API output | Must not weaken existing token/Host/Origin checks | TC-723, TC-728 |
| Platform | CRLF-tolerant reads; LF writes; paths via `Path::join` | Linux | TC-721 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Memory core (context, validate, save, queue, search) | `src/memory.rs` | FR-721, FR-723, FR-724, FR-725, FR-730, NFR-731 | TC-721..TC-725, TC-730 |
| 2 | `[memory]` settings | `src/settings.rs` | FR-720 | TC-720 |
| 3 | Injection and slash commands | `src/app.rs` | FR-722, FR-726 | TC-726, TC-727 |
| 4 | Web API and Memory panel | `src/web/server.rs`, `src/web/assets/*` | FR-727, UX-728 | TC-728, TC-731 |
| 5 | MCP stdio server | `src/memory_mcp.rs`, `src/main.rs` | INT-729, FR-730 | TC-729 |
| 6 | README section | `README.md` | INT-732 | Manual |
| 7 | Built-in shared `dmuxai-memory` MCP entry (superseded by SPEC-mcp-propagation-removal) | `src/settings.rs` | FR-733, INT-732 | TC-2083 |

## 8. Review and test exit criteria

- [ ] All Must requirements map to passing evidence.
- [ ] The diff has been reviewed against scope and non-goals.
- [ ] Required Rust checks pass: format, all targets test, and clippy.
- [ ] External-provider smoke tests are marked run, skipped, or blocked with a
  reason; skipped is never reported as passing.
- [ ] `../reviews/REVIEW-obsidian-memory.md` records commands, result, residual risk, and rollback
  notes.
