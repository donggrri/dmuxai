# REVIEW: web dashboard slice A

Spec: [SPEC-web-dashboard.md](../specs/SPEC-web-dashboard.md)
Date: 2026-10-17

## Spec compliance

| Requirement | Status | Notes |
| --- | --- | --- |
| FR-520 | Met | `serve` first-arg only, `--web` anywhere, `--port/--bind/--allow-remote`; old `--help`/`--version` semantics kept (TC-520, TC-531) |
| FR-521 | Met | 64-hex token per start; URL first stdout line (`serve`) / TUI notice (`--web`) (TC-521, TC-531) |
| FR-522 | Met | Bearer (case-insensitive scheme) or `?token=`; length-safe constant-time compare; 401 `{"error":"unauthorized"}` (TC-522) |
| FR-523 | Met | Host allow-list for loopback, Origin must equal `http://<Host>`; spec updated for remote binds (any Host) (TC-523) |
| FR-524 | Met | non-loopback needs `--allow-remote` + warning; else exit 2 (TC-524) |
| FR-525 | Met | `dmuxai.dashboard/v1`; truncation char-safe; TUI keeps the last finished run of the current conversation (TC-525, TC-530) |
| FR-526 | Met | per-attempt timings, revision only on change, reset on new graph id (TC-526) |
| FR-527 | Met | SSE `snapshot` events via watch channel, keep-alive 15 s (TC-527) |
| FR-528 | Met | newest parseable run; half-written newest keeps the previous view; missing dir -> null; symlinks skipped (TC-528) |
| FR-529 | Met | embedded page, CSP `default-src 'self'`, no-store, nosniff, no-referrer; token moved to sessionStorage and removed from the address bar (TC-529, manual screenshot) |
| FR-530 | Met | App publishes on change (>=200 ms coalescing, 1 s while busy, immediate on completion) (TC-530, `app_dashboard_clears_last_graph_on_new_conversation`) |
| NFR-520 | Met | `include_str!`, no frontend build; Windows via CI |
| NFR-521 | Met | token absent from payloads (TC-525) |

## Regression risk

- New dependency axum 0.8 (minimal features) + tokio `net`/`signal`; TUI path unchanged unless `--web` is passed.
- CLI parsing rewritten; existing `--help`/`--version`/unknown-arg tests kept and pass.
- `App::run` gains a cheap, throttled publish call that is a no-op without `--web`.

## Test evidence

```text
cargo fmt --check                                           # clean
cargo clippy --all-targets --all-features -- -D warnings    # clean
cargo test --all-targets                                    # lib 93, bin 168 (+5 ignored), integration suites all ok
bash scripts/spec-index.sh                                  # SPEC-web-dashboard: FR-520..561, INT-560..562, NFR-520..560, TC-520..566, UX-540
```

Manual smoke (box, Linux): `dmuxai serve --port 0` against a fixture `.dmuxai/graphs/*.json` - 401 without token, 200 with
Bearer, 403 for foreign Origin/Host, SSE first frame `event: snapshot`, node completion reflected with `elapsed_ms`;
headless Chrome screenshot renders the node list with the running node highlighted. `dmuxai --web --port 0` in tmux:
URL notice shown, `/api/state` returns `source: tui` with all five agents, `/quit` exits cleanly.

## Residual risks

- `serve` timings are when the server first observed a status (persisted runs carry no timestamps).
- `--web` shows only graph runs from the current TUI session, not older persisted runs.
- Windows: verified only by CI build/tests; the WSL -> Windows browser `localhost` path is not yet tested on a real machine.
- Token in the URL can still land in browser history before the page removes it; loopback-only default limits exposure.

## Rollback

Revert the PR: removes `src/web`, the CLI branches and `attach_dashboard`. No persistence change.

## Who did what (dogfood through dmuxai, three instances in tmux)

- AGY `gemini-3.8-flash-high`: `src/web/*.rs` (snapshot, hub, auth, server, file source, assets) and TC-521..TC-529, plus review fixes (token length bypass, per-attempt timings, file-source fallbacks, 500 on serialization failure).
- opencode `deepseek-v4.1-flash-free`: `src/main.rs` CLI/startup, `src/app.rs` publishing, `tests/web_serve.rs` (TC-520, TC-530, TC-531); cross-reviewed the page.
- opencode `glm-5.3-flash-free`: page assets, TEST-PLAN rows, this review's cross-review sections (src/web and CLI/App).
- Planner: spec/plan/tasks, module contract, Cargo deps, review rounds, smoke tests.

## Cross-review (glm)

Scope: `src/web/{snapshot,hub,auth,server,file_source,assets}.rs` reviewed
against FR-521..FR-529, NFR-520, NFR-521 and [PLAN-web-dashboard](../plans/PLAN-web-dashboard.md).
Read-only for Rust; findings only — fixes belong to the owning agent (AGY).

Test state at review time: `cargo test --lib web::` — 10 passed, 0 failed.

### Findings

| # | Location | Severity | Finding and concrete fix |
| --- | --- | --- | --- |
| 1 | `src/web/hub.rs:65-105` (esp. 76-93) | Major | Retry re-run: executor cycles `Running -> Ready -> Running` with `attempt` bumped (`src/graph/executor.rs:75-78`), never passing `Failed`; the hub keys `timings` by node id only, so `started_at_ms` stays at the first attempt's start and the live elapsed clock on the page spans failed attempts. Literal ("first observed running"), but observable as attempt-overcounting. Fix decision needed with the planner: either key `timings` by `(node.id, node.attempt)` (reset start when attempt changes; keep track of previous attempts only if a "total time" is wanted) or keep cross-attempt semantics and document it in FR-526 + extend TC-526 with a running->ready->running case. If per-attempt: `let key = (node.id.clone(), node.attempt); state.timings.entry(key)` and clear when the plan's id changes as today. |
| 2 | `src/web/file_source.rs:46-53` | Major | FR-528 promises "the most recently modified **parseable** `.dmuxai/graphs/*.json`", but the code takes only the newest file; if it is malformed AND `last` is `None` (fresh start, first poll hits a malformed newest file while an older valid file exists — or the valid file was never polled), `poll()` returns `None` instead of the older valid run. TC-528 passes only because `b` was polled before `c` was written. Fix: iterate `json_files` (already sorted newest-first); for each, `read_to_string` + `serde_json::from_str`; return the first successful parse; keep `self.last` only when every file fails to parse. Update the TC-528 fixture to cover first-poll-malformed-newest (create `c` (malformed) and `b` (valid) before the first poll; expect `b`). |
| 3 | `src/web/file_source.rs:21-23` | Minor | If `.dmuxai/graphs` is deleted mid-session, `poll()` keeps returning `self.last`, so a removed run keeps rendering forever; FR-528 boundary says a missing directory gives `graph: null`. Fix: on `!dir.is_dir()` clear `last` (`self.last = None`); keep the transient `read_dir()`-error path (`:25-28`) returning `last` unchanged so a filesystem hiccup does not blank the view. |
| 4 | `src/web/file_source.rs:33-40` | Minor | A `.json` symlink is accepted: `path.is_file()` follows the target while `entry.metadata()` does not (mixed semantics), and `read_to_string` reads the target, which may point outside the workspace graph dir. Fix: skip symlinks explicitly: `entry.file_type().map(|ft| !ft.is_symlink())` before pushing; keep symlink-independent test or note as platform edge. |
| 5 | `src/web/server.rs:164-168` | Minor | Bearer scheme matched only for exact `Bearer ` / `bearer `; RFC 7235-5.1 requires a case-insensitive scheme, so `BEARER <tok>` or `bEaReR <tok>` 401s. Fix: parse case-insensitively: `let mut parts = auth_val.splitn(2, ' '); let (scheme, tok) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default()); if scheme.eq_ignore_ascii_case("bearer") { provided_token = Some(tok.trim()) }` and drop the `bearer ` special case. Spec conformance is not affected (FR-522 names `Bearer`), so this may be planned as informational. |
| 6 | `src/web/server.rs:171-184` | Minor | `?token=` is matched on the raw, un-percent-decoded query value. Safe today (tokens are `[0-9a-f]{64}`, so `encodeURIComponent` is a no-op and the client page in `src/web/assets/app.js` agrees), but fails closed with a mismatch (401) for any future token charset. Fix: percent-decode the value before `token_matches` (or use the axum `Query` extractor), and add a 401/200 test pair with a percent-encoded token. Do not invert into `starts_with`-style matching (fail-open risk). |
| 7 | `src/web/server.rs:155-156` | Minor | Auth gate is `path.starts_with("/api/") \|\| path == "/api"` on the raw URI path. With today's route table there is no bypass (`//api/state`, `/api/../state`, `/api%2fstate` fail route matching -> 404 without data), and HEAD/OPTIONS are also covered by the middleware (axum `get` includes HEAD; the layer is method-agnostic). Hardening for future catch-all routes: normalize once (collapse `//`, resolve `.`/`..`) or flag suspicious paths as auth-required; add negative tests `//api/state` and `/api/../api/state` expecting 401 (fail-closed) to pin the behavior. |
| 8 | `src/web/server.rs:252-260` | Minor | For non-loopback binds `allowed_hosts = None` disables Host validation entirely (any Host accepted). This matches PLAN (`None` for remote binds) but FR-523 says the Host must match the bound address. Either keep it and record the intentional deviation in the spec, or build the allow-list from the bind IP/hostname (and keep HTTP-origin check `Origin == http://<Host>` either way). Needs a planner decision; not a fix before that. |
| 9 | `src/web/server.rs:99-100`, `114-117` | Minor | `serde_json::to_string(...).unwrap_or_default()` returns HTTP 200 with an empty body for `/api/state` on serialization failure, and an empty SSE `data:` frame (client `JSON.parse('')` throws). Fix: log-and-500 (or skip the event/keep the previous one) instead of emitting a malformed payload; aligns with TC-525's stable-schema assertion. |
| 10 | `src/web/server.rs:520` (test) | Minor | TC-527's second-phase match `s.contains("\"revision\":1")` is a substring match that also hits `"revision":10`, `:11`, ... as soon as a test publishes more revisions; not flaky today (only revision 1), but brittle. Fix: match `"revision":1,` (comma follows in serde output) or parse the SSE frame JSON and compare numerically. |
| 11 | `src/web/snapshot.rs:148-247` (test) | Minor | TC-525 truncation case is ASCII-only; `truncate_with_ellipsis` (`:77-85`) is char-boundary-safe (`chars().take(200)`), so this is a test-gap only. Fix: add a 300-char CJK/emoji case and assert `job.chars().count() == 201` and no `from_utf8` errors, so a future byte-based `&job[..200]` refactor cannot land. |

### Checked and OK (no change requested)

- Auth: 401 body is exactly `{"error":"unauthorized"}` with no snapshot state; token compare is length-safe and effectively constant-time (`auth.rs:47-64`); missing Host -> 403 before auth; `Origin: null` and foreign origins -> 403; IPv6 allow-list entry `[::1]:{port}` matches what browsers send (`server.rs:254-256`).
- FR-521/FR-524: 64 lowercase-hex token (two UUIDv4s), URL format with bracketed IPv6 and `/?token=`, loopback bind default with non-loopback opt-in + warning text.
- FR-527: SSE response first frame is the current snapshot (`WatchStream` yields the watched value immediately), watch-channel coalescing satisfies "slow clients receive the latest snapshot", keep-alive comments every 15 s; event name `snapshot` matches the client.
- Client/server contract with `src/web/assets/app.js` (re-verified after review): event name `snapshot`; `Authorization: Bearer <token>`; `?token=` query param; `dmuxai.dashboard/v1` schema; all field names snake_case per `serde(rename_all)` including `waiting_gate` status, `started_at_ms`/`finished_at_ms`/`elapsed_ms` and the seven count keys — the page and server agree field-for-field.
- NFR-520/Windows: no unix-only APIs in `src/web/` (no `std::os::unix`, `libc`, `nix`); tokio/net + `std::fs` used everywhere; `set_modified` is test-only cross-platform. Embedded assets are LF (no CRLF byte-exact risk; tests use `contains`).
- Flakiness: SSE test binds port 0 with 5 s timeouts (no sleeps found in `src/web/`); hub test uses a deterministic injected clock; file-source test sets explicit mtimes far apart, so no timing ties in practice; oneshot tests use no ports.

Result of app.js fixes made during this review (glm-owned files only): poll loop now stops on HTTP 401 and asks the user to reopen the URL printed by dmuxai (token rotates per start), instead of retrying every 2 s with a generic error. `node --check` passes; `cargo test --lib web::` 10/10 after the change (TC-529 string assertions unaffected).

## Cross-review 2 (glm): CLI and App

Scope: `src/main.rs` (`parse_cli` serve/`--web`, `run_serve`, `run_tui`,
`attach_web_dashboard`), `src/app.rs` (`dashboard`, `last_graph_run`,
`attach_dashboard`, `publish_dashboard`, run-loop call, TC-530 test) and
`tests/web_serve.rs` against FR-520, FR-521, FR-524, FR-530 / TC-520, TC-530,
TC-531. Read-only for Rust (AGY owns `src/main.rs`/`src/app.rs` during slice A).

Test state at review time (run by me, foreground): `cargo test --bin dmuxai parse_cli` 6/6,
`app_publishes_graph_state_to_dashboard` 1/1, `cargo test --test web_serve` 1/1.

### Findings

| # | Location | Severity | Finding and concrete fix |
| --- | --- | --- | --- |
| 1 | `src/main.rs:98-100` | Minor | FR-520 says "--help/--version keep current behavior", but `--version` moved from "honored only when it is the sole argument" to "wins from any position": old parse returned `Unknown` (exit 2) for `dmuxai foo --version`, `--version bogus`, `dmuxai --version --web`; the new pre-scan returns `Version` for all of them. Fix: keep the `--help` pre-scan (old code also had help-wins-anywhere) but honor `-V`/`--version` only when `args.len() == 1`, i.e. inside the arg walk as today; pin it with regression cases in TC-520 (`["--version", "--web"] -> Unknown`, `["foo", "--version"] -> Unknown`). |
| 2 | `src/main.rs:136-143` (`--bind` arm) | Minor | `--bind [::1]` exits 2: `"[::1]".parse::<IpAddr>()` rejects brackets, yet FR-521/FR-523 and the printed URL (`auth.rs` `dashboard_url`) use the bracketed form, so a user copying the URL host form is refused. Fix: strip optional surrounding `[`/`]` before `IpAddr::parse` (one line, still Unknown for garbage), document `--bind ::1` in USAGE, and add the `[::1]` case to TC-520. |
| 3 | `src/app.rs:624-626` (with `:58` `EVENT_LOOP_PAUSE`, `:650`) | Minor | Publish cadence: the gate `drain.changed \|\| dirty` fires on every loop iteration while `dirty` persists between draws (draws are capped at `RENDER_INTERVAL` 25 ms) and on every drained event batch (~5 ms loop pause) — measured worst case ~200 publishes/s while an agent streams. FR-530's non-blocking guarantee holds (sync `Mutex` + `watch::send_replace`, hub dedups), but each call clones the whole `GraphRun` (`src/app.rs:2783`) and rebuilds agent views. Fix: throttle with an `Instant` timestamp — publish immediately on `drain.changed`-carried turns, and for dirty ticks only when `RENDER_INTERVAL` has elapsed (reusing `last_publish: Instant`) — cutting duplicate work to ≤40/s with zero visible latency change. |
| 4 | `src/app.rs:2779-2785` (plus `create_conversation` `:1838`, `restore_snapshot` `:1875`) | Minor | `/new` and `/resume` (conversation switch) never clear `last_graph_run` (nor `graph_session`), so the dashboard keeps rendering the previous conversation's frozen graph after the user explicitly starts a new one. Retention past completion inside the same conversation is FR-525-intended; retention across an explicit switch is a cross-conversation leak. Fix (planner decision, since it touches pre-existing session semantics): in `restore_snapshot`, when `graph_session` is `None` (or its `conversation_id` != the new id and the run is finished), also set `self.last_graph_run = None`; still publish runs whose session remains active for their conversation. Add a regression test alongside TC-530. |
| 5 | `src/main.rs:216-221` (serve loop, mirrors cross-review 1 #3) | Minor | `serve` publishes `FileGraphSource::poll()` every 500 ms; `poll()` clones `self.last` forever, even after the TUI deleted/emptied `.dmuxai/graphs` — the serve dashboard shows a ghost finished run for the life of the process. FR-528 only specifies "missing directory gives graph: null". Fix (in `file_source.rs`, AGY): when `json_files.is_empty()` (or the dir is gone), clear `self.last = None` before returning; TC-528 needs one addition: empty-dir-after-file case expecting `None`. |

### Checked and OK (no change requested)

- Exit codes: unknown arg/flag -> exit 2 with usage (`src/main.rs:170-173`); missing/invalid `--port`/`--bind` -> `Unknown` -> exit 2; bind-policy violation -> exit 2 in both modes (`run_serve` `:186-189`, `attach_web_dashboard` maps `InvalidInput` `:296-306`); server-start error -> exit 2 (`:199-203`); server-start failure in `--web` is a TUI notice, not a crash (`:303-307`, FR-530 boundary); TTY error keeps exit 1; help/version exit 0.
- `serve` really works without a TTY: `run_serve` has no TTY gate; `tests/web_serve.rs` runs it with `stdin: null` and piped stdout/stderr and passes.
- stdout first-line contract: URL is the first stdout line, `println!` + explicit `io::stdout().flush()` (`src/main.rs:206-207`); the bind warning goes to stderr (`:208-210`) and `--web` pushes URL + warning as TUI startup notices (FR-521).
- Ctrl+C handling: `serve` exits via `tokio::select!` + `ctrl_c()` (`:216-222`, tokio `signal` feature is already on); the TUI aborts the dashboard server after `app.shutdown()` (`:276-278`).
- FR-530 state coverage: synchronous graph/agent changes set `dirty` (crossterm branch `:643`, busy tick `:583-587`) and `drain.changed` covers turn events, so the next iteration publishes ≤5 ms later; gate transients (PauseForGate from a key handler) are covered by the persistent local `dirty` from the event branch.
- TC-530 test quality: uses `test_app()` (`:memory:` DB, recording adapters, no credentials/li live provider); deterministic sync assertions; verified passing by me.
- `AgentView` construction matches the plan contract in `publish_dashboard` (`:2754-2799`): `id = runner_name(agent)`, `label`, `busy` from `busy_agents`, `phase` from activity label, `model` from the selected catalog entry; `last_graph_run` only updates when `graph_session` is `Some`, matching the retention test.
- Windows compile: no unix-only APIs (`io::IsTerminal`, tokio signal/net, `std::process`, `TcpStream` are cross-platform); `child.kill()`+`wait()` in `ChildGuard` are fine on Windows; no `#[cfg(unix)]` needed.
- Flakiness in `tests/web_serve.rs`: `--port 0` (no fixed port), 10 s `recv_timeout`/read timeouts, first-line parsing tolerant of port resolution (`:94` rejects `:0`), `ChildGuard` kills/reaps the child on every path including panics before the response asserts; the detached stdout reader thread ends on EOF after the kill; no sleeps in the test. Residual nit: `TcpStream::connect` itself is unbounded, but connect succeeds only after the URL line arrives, so the timeout bound effectively covers the whole exchange.
- Env-var hygiene: the test removes `DMUXAI_WORKSPACE` (`:58`) so `resolve_workspace_root` falls back to the temp cwd; `serve` reads no state dirs (`XDG_STATE_HOME`/`DMUXAI_STATE_DIR` are only consumed by `run_tui`'s `prepare_repo_db` and the AGY adapter) and opens no DB — verified: `run_serve` touches only the workspace `.dmuxai/graphs` dir, so the user's real conversation state cannot be touched by the test.
- TC-520 rows: mutually exclusive `serve`/`--web` (`:104-109`), `serve` only as first arg, flags rejected without serve/`--web` (`--port 9000` alone -> `Unknown`), all covered by the test and passing.

Note: findings 1-5 are for the deepseek/AGY agents to fix; I changed only
`docs/reviews/REVIEW-web-dashboard.md` in this round (no `.rs` or Cargo files
touched, `cargo fmt` not run, nothing committed).
