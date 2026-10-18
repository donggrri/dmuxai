# SPEC: Built-in grok runner

Status: Implemented
Owner: DmuxAI
Created: 2026-10-18
Baseline: `787436b`
Related: SPEC-first-run FR-810..FR-813, SPEC-custom-agent-provider FR-330..FR-335, SPEC-model-policy FR-910..FR-912, SPEC-graph-allowed-runners FR-430

## 1. Decision summary

DmuxAI gains a built-in `grok` runner when a `grok` executable can be found and `[agents] builtin_grok` is not false. It is a built-in preset of the custom-agent machinery, the same shape as `opencode`: headless argv, NDJSON text, a stored session id, and process-group cancel. The turn command is `grok -p <prompt> -m <model> --cwd <dir> --output-format streaming-json --no-wait-for-background` with `GROK_AUTO_WAKE=0` (FR-913). The `end` line finishes the turn (FR-914). Ask and Plan stay read-only with `--permission-mode plan`. Implement passes `--always-approve`. Models come from `grok models`, default to `grok-4.7`, and only `grok-*` ids that do not contain `fast` are offered. If `grok` is not logged in, the runner still appears and selecting it or sending a prompt tells the user to run `grok login`. The runner is excluded from the default graph runner set unless `[graph] allowed_runners` lists it. ACP (`grok agent stdio`) is not used.

## 2. Scope

### In scope

- Built-in preset registration, replacement by a user `[[agents.custom]]` named `grok`, and `[agents] builtin_grok`
- Headless argv, streaming-json parsing, resume, mode flags, errors, and process-group cancel
- Model listing, the `grok-4.7` default, the `fast` filter, and the login message
- TUI and dashboard color `#6366f1`
- Graph-node use only when `allowed_runners` lists `grok`
- A Unix mock `grok` fixture for tests, plus one ignored live smoke test
- README, CHANGELOG, and the `dmuxai init` starter comments for this runner

### Out of scope / non-goals

- ACP / `grok agent stdio`
- `--prompt-file`, `--no-plan`, `--disable-web-search`, and `--permission-mode` values other than `plan` on Ask and Plan
- Reading `~/.grok/auth.json` or any other grok auth file
- A grok-specific image flag or MCP temp-config merge
- Changing the shared custom-agent cancel signal from SIGTERM to SIGKILL (the FR-914 clean-up after an `end` line escalates to SIGKILL only for that case)
- Putting `grok-*` models on repo graph nodes in `docs/dmuxai-graph.yaml` (that file stays composer-2.5; see SPEC-model-policy FR-912)
- A second copy-paste recipe in `docs/agents/presets.toml` (the builtin plus the init comment is the supported path)

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| A-1 | Ask and Plan both pass `--permission-mode` `plan`. The CLI's default permission mode is not used. Implement passes `--always-approve` and does not pass `--permission-mode`. | DmuxAI | Spec approval |
| A-2 | The custom-agent cancel path (`src/agent/custom.rs` `kill_group`) runs `kill -TERM -- -<pgid>`. This runner reuses that path. Cursor's later SIGKILL belongs to the Cursor adapter and is not copied. | DmuxAI | Spec approval |
| A-3 | Authenticated means exit code 0 and stdout contains `logged in` (ASCII case-insensitive). The verified line is `You are logged in with grok.com.` A non-zero exit is not authenticated. | DmuxAI | Spec approval |
| A-4 | `fast` means the substring `fast` in the trimmed lowercase id. `flash` does not contain `fast`. | DmuxAI | Spec approval |
| A-5 | An `end` line whose `stopReason` is other than `end_turn` does not fail a turn that exited 0 and produced text. The session id is still `sessionId` on that object. | DmuxAI | Spec approval |

No Must requirement depends on an unanswered question.

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| grok preset | The built-in custom-agent spec named `grok`, `builtin = true`, produced for the resolved executable | A user `[[agents.custom]]` named `grok`, which has `builtin = false` and replaces the preset |
| streaming-json | NDJSON on stdout from `--output-format streaming-json`. One JSON object per line | opencode's `--format json` (`/part/text`, `/sessionID`) |
| Allowed grok model | One token matching `(?i)^grok-[a-z0-9][a-z0-9._-]*$` whose lowercase form does not contain `fast` | The decorated `grok models` line (`* grok-4.7 (default)`) |
| Authenticated | `grok models` exited 0 and its stdout contains `logged in` | A successful `grok -p` turn |

The preset's program string is the resolved executable path. Registration does not start a process. Model listing and auth probing run when the runner is selected or a prompt is sent, with the existing 10 second builtin listing timeout. A hung `grok models` cannot block process startup; startup stays lazy, as for opencode.

| State | Entry | Exit |
| --- | --- | --- |
| Hidden | No executable, `builtin_grok = false`, or a user agent named `grok` replaced the preset | Executable appears and the flags allow registration (next config load) |
| Listed, auth unknown | Preset is in the registry; `grok models` has not returned | Probe returns |
| Listed, logged out | Probe is non-zero, missing `logged in`, or times out | A later probe is authenticated |
| Ready | Probe is authenticated and at least one allowed id remains | User sends, or listing is refreshed |
| No allowed model | Probe is authenticated and every id was dropped | A later probe returns an allowed id |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-890 | Must | No `[[agents.custom]]` entry is named `grok`, `[agents] builtin_grok` is not false, and a grok executable is found. Lookup order: a non-empty `DMUXAI_GROK_BIN` is authoritative (the preset exists only when that path is an existing file); otherwise the first `grok` on `PATH` (`grok.exe`, `grok.cmd`, and `grok.bat` on Windows, via the existing PATH search); otherwise `~/.grok/bin/grok` when that file is executable (`~/.grok/bin/grok.exe` on Windows). | The registry contains a `grok` preset whose command starts with that resolved path. It is listed in the TUI `/agent` picker, the dashboard agent chips, `dmuxai status --json` (`agents[].id` = `grok`), and the MCP agent list. Chat selection still works while the preset is excluded from graph routing (FR-899). The preset is appended after agents already registered, including the opencode preset, so earlier custom indexes stay put. `builtin_grok` follows the opencode merge: the last config wins, default true, and a file that only sets the flag does not clear `[[agents.custom]]` from the other config. | A set but missing `DMUXAI_GROK_BIN` does not fall through to `PATH` or `~/.grok/bin/grok`. `PATH` wins over `~/.grok/bin/grok`. | TC-890, TC-891 |
| FR-891 | Must | A user or workspace `[[agents.custom]]` is named `grok`, or `builtin_grok = false`, or no executable is found. | The named custom entry is the `grok` runner and the preset is absent (`builtin` stays false). `builtin_grok = false` or a missing executable registers nothing and shows no error. Workspace `builtin_grok` overrides the user file. | An invalid custom `grok` entry follows SPEC-custom-agent-provider FR-330 (rejected with a config warning, no preset stuffed in behind it). | TC-890 |
| FR-892 | Must | A ready grok turn is started in Ask, Plan, or Implement. | The process argv is, in order: the resolved program, `-p`, the prompt as one element, `-m`, the selected allowed model, `--cwd`, the workspace path, `--output-format`, `streaming-json`, `--no-wait-for-background` (FR-913), then the mode flag. Ask and Plan append `--permission-mode` and `plan`. Implement appends `--always-approve` and does not append `--permission-mode`. The prompt is not passed through a shell. `prompt_via` is arg. | A prompt containing spaces, quotes, `;`, or `$()` stays a single argv element after `-p`. No `-p` process starts while auth has failed (FR-896) or no allowed model exists (FR-895). | TC-892, TC-894, TC-907 |
| FR-893 | Must | The child prints streaming-json and exits. | Reply text is the concatenation, in order and with no added separator other than the FR-916 paragraph break, of string values at `/data` on lines whose `/type` equals `text`. The stored provider session id is the string at `/sessionId` (the `end` line). Lines of type `thought`, `available_commands`, and `usage` contribute neither reply text nor an error. Malformed JSON lines are ignored. | Exit non-zero: the turn fails and the message includes stderr, or the existing empty-stderr custom-agent message when stderr is empty. Exit 0 with no `type=text` data: the turn fails with the empty-reply error. A missing `end` line on a successful text turn leaves the session id unset. `stopReason` alone does not fail the turn (A-5). | TC-893, TC-894 |
| FR-894 | Must | A later turn in the same conversation has a stored grok session id. | Argv appends `-r` and that session id as the next element, after the mode flag. The id is the `sessionId` string unchanged. | The first turn, and any turn with no stored id, omits `-r`. | TC-892, TC-893, TC-894 |
| FR-895 | Must | `grok models` is authenticated. Stdout is parsed into ids. A candidate line is trimmed, a single leading `*` or `-` is stripped, a trailing `(default)` is stripped, and the remainder is one token matching `(?i)^grok-[a-z0-9][a-z0-9._-]*$`. Tokens whose lowercase form contains `fast` are dropped. Order is kept and duplicates are dropped. | The picker and `-m` use only those ids. When `grok-4.7` is in the list it is the selected model until the user picks another allowed id, including when another line is marked `(default)`. When `grok-4.7` is absent, the first allowed id is selected. `grok-4.7-build-fast` is never listed and never sent as `-m`. | An authenticated listing with zero allowed ids leaves the runner visible. Selecting it or sending a prompt shows a message that names the `^grok-` rule and that ids containing `fast` are rejected, and does not start `grok -p`. An absent id is not inserted to satisfy the default. SPEC-model-policy FR-910 and FR-911 also reject the id. | TC-894, TC-895 |
| FR-896 | Must | The preset is registered and `grok models` exits non-zero, times out (10s), fails to spawn, or exits 0 without `logged in` in stdout. | The runner stays in the picker and the dashboard chips. Confirming it in the TUI `/agent` picker, selecting it with the dashboard session agent control, and sending a prompt each surface a message that contains the exact command `grok login`. The send does not start `grok -p`. The same message is the models-list error. | The login message is distinct from the zero-allowed-model message in FR-895. A probe is not skipped just because a default model string is already filled in. DmuxAI does not open an auth file (NFR-890). | TC-896, TC-897, TC-904 |
| FR-897 | Must | The user cancels a grok turn, or the turn exceeds its timeout (preset `timeout_secs` 900; tests may shorten it). | Cancel and timeout use the existing custom-agent process-group kill. The child is started in its own process group. On Unix the signal command is `kill -TERM -- -<pgid>`. Descendants in that group die with the leader. | No grok-specific killer is added. Non-Unix cancel matches the other custom runners. | TC-898 |
| FR-898 | Must | The grok preset is shown in the TUI and the dashboard. | The preset color is `#6366f1`. TUI reply text for that agent uses it. The dashboard agent chip receives it as `AgentView.color` (`#6366f1`) and paints the chip dot with it. The color differs from user cyan, cursor green, agy magenta, codex/other white, system gray, opencode `#ffa500`, the hermes example `#d4af37`, the pi example `#40e0d0`, the opencode-glm example `#ff8800`, and the dashboard accents `#4aa8ff` and `#0b57d0`. | A `[theme.agents]` entry `grok` overrides the preset color in both the TUI and the dashboard, using the existing theme order. | TC-899, TC-900 |
| FR-899 | Must | Graph routing resolves a runner, `dmuxai status --json` reports `allowed`, or `/graph` status is shown. | With `allowed_runners` unset, the built-in preset is excluded: `is_runner_allowed("grok")` is false, `status` reports `allowed: false`, `assign_runner` does not return `grok` even when it is the routing default, and `/graph override` onto that preset is rejected. Listing `grok` in `allowed_runners` admits it, and assignment may return it. A user custom agent named `grok` (not builtin) keeps the ordinary rule and is admitted while the list is unset. A grok graph turn does not receive `composer-2.5` as `-m`; with no F2 selection it uses the grok default from FR-895. An F2 selection of an allowed grok id wins. `/graph` status, when the list is unset and a builtin grok preset is registered, names `grok` in the exception. When the opencode preset is also registered, the text names both. | Chat pickers stay available (FR-890). The CHANGELOG states that a machine with `grok` on `PATH` gains a chat runner that takes no graph nodes until it is listed. | TC-891, TC-901, TC-902, TC-903 |
| FR-900 | Must | Docs for a release that contains this runner. | The README First run section says how `grok` is detected (`PATH`, `DMUXAI_GROK_BIN`, `~/.grok/bin/grok`), that models are `grok-*` without `fast` and default to `grok-4.7`, that a logged-out CLI still lists the runner and asks for `grok login`, that `[agents] builtin_grok = false` hides it, that a custom entry named `grok` replaces it, and that graph nodes need `allowed_runners`. The CHANGELOG states the new chat runner and the graph exception. `dmuxai init` comments `builtin_grok` and, when grok is detected, a commented `[[agents.custom]]` example equal to the preset. The starter file still parses as defaults. `docs/agents/custom-agents.md` notes that the built-in grok runner is this preset and that `grok agent stdio` is not used. | When grok is not detected, the init example block is omitted. The commented example does not set `builtin`. | TC-905, TC-906 |
| NFR-890 | Must | Any grok turn or auth probe runs. | The prompt and the model id are each one argv element. The process image is the grok executable, not `sh -c` or `cmd /c`. Auth is decided only from `grok models` stdout and its exit status. | DmuxAI does not read `~/.grok/auth.json`, `~/.grok/` credential files, or tokens. Tests use the mock fixture and do not need a logged-in grok. | TC-892, TC-896 |

## 5. Acceptance scenarios and test design

The Unix mock is `tests/fixtures/fake-grok.sh`. Tests invoke it as the grok program (`PATH` or `DMUXAI_GROK_BIN`). It must not touch the network.

Default `models` stdout:

```text
You are logged in with grok.com.
Available models:
  * grok-4.7 (default)
  - grok-4.6
  - grok-4.7-build-fast
```

`GROK_FAKE_AUTH=0` prints the same body without the logged-in line and exits 0. `GROK_FAKE_AUTH=fail` exits 1 and prints `not logged in` on stderr. Prompt mode (`-p`) prints this NDJSON and exits 0 unless a mode below applies. The text `data` echoes a fixed `OK` (the unit test covers multi-chunk concatenation with a string fixture):

```text
{"type":"available_commands","data":[]}
{"type":"thought","data":"thinking"}
{"type":"text","data":"OK"}
{"type":"usage","usage":{"input":1}}
{"type":"end","stopReason":"end_turn","sessionId":"11111111-1111-1111-1111-111111111111","cwd":"/tmp","num_turns":1}
```

`GROK_FAKE_MODE=fail` exits 3 with stderr `boom` and no text line. `GROK_FAKE_MODE=empty` exits 0 with only the `end` line. `GROK_FAKE_MODE=sleep` sleeps and forks a grandchild sleeper, writing the grandchild pid to `GROK_FAKE_PIDFILE`. When `GROK_FAKE_ARGV` is set, the script appends its argv, one element per line, and does not interpret the prompt.

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-890 | FR-890, FR-891 | unit | Temp executable on `PATH`, at `~/.grok/bin/grok`, or via `DMUXAI_GROK_BIN`; configs with a custom `grok`, `builtin_grok = false`, and a user/workspace flag split | Load settings | Preset only when the lookup and flags allow it; custom name wins; disabled or missing binary adds no preset and no error; `PATH` beats the home path; a missing `DMUXAI_GROK_BIN` does not fall through; a pre-existing custom index stays before grok | `src/settings.rs` `builtin_grok_preset_registration` |
| TC-891 | FR-890, FR-899 | integration | Empty config, fake grok on `PATH` | `dmuxai status --json` | `agents` contains `id` `grok` with `allowed` false; cursor stays `allowed` true; clearing `PATH` and the home path drops `grok` | `tests/agent_api_cli.rs` `fresh_install_lists_grok` |
| TC-892 | FR-892, FR-894, NFR-890 | unit | Preset plus Ask, Plan, Implement, a session id, and a prompt `hello; $(rm)` | Build argv | Exact argv in FR-892; Implement has `--always-approve` and no `--permission-mode`; Ask and Plan have `--permission-mode` `plan` and no `--always-approve`; resume adds `-r` and the id; the prompt is one element; argv has no `sh` or `cmd` | `src/custom_agent.rs` `grok_preset_argv_is_headless_and_not_a_shell` |
| TC-893 | FR-893, FR-894 | unit | NDJSON string: thought, two `type=text` chunks `OK` and `!`, usage, `end` with `sessionId` `11111111-1111-1111-1111-111111111111`, plus one broken line | Parse | Reply `OK!`; session id is that uuid; thought and usage are absent from the reply | `src/custom_agent.rs` `grok_streaming_json_extracts_text_and_session` |
| TC-894 | FR-893, FR-894, FR-895 | integration | Unix mock, authenticated models | Run a turn, then a second turn | First reply is `OK`; stored session id is the fixture uuid; second argv contains `-r` and that uuid; `-m` is `grok-4.7`; fail mode surfaces `boom`; empty mode surfaces the empty-reply error; `grok-4.7-build-fast` is not an argv element | `src/agent/custom.rs` `grok_mock_turn_returns_text_and_reports_errors` |
| TC-895 | FR-895 | unit | Fixture models stdout, a listing whose `(default)` marker is on `grok-4.6` while `grok-4.7` is present, a listing without `grok-4.7`, and a listing of only `grok-4.7-build-fast` | Parse and choose | Ids are `grok-4.7` and `grok-4.6` in that order; `grok-4.7-build-fast` is absent; selected id is `grok-4.7` when present, else the first allowed id; the fast-only listing yields no id and the policy message | `src/custom_agent.rs` `grok_models_parse_default_filter_and_fast` |
| TC-896 | FR-896, NFR-890 | unit | Stdout with and without `logged in`; exit 0 and exit 1 | Decide auth from stdout and status only | Logged-in line plus exit 0 is authenticated; missing line or non-zero is not; the login message contains `grok login`; the function takes no path | `src/custom_agent.rs` `grok_auth_is_models_stdout_only` |
| TC-897 | FR-896 | integration | Unix mock with `GROK_FAKE_AUTH=0` and with `GROK_FAKE_AUTH=fail` | List models, then send | Both probes error with `grok login`; the runner spec is still the preset; argv has no `-p` process; the message differs from the authenticated fast-only listing | `src/agent/custom.rs` `grok_unauthenticated_send_explains_login` |
| TC-898 | FR-897 | integration | Unix mock in sleep mode with a grandchild pid file | Cancel the turn | The turn ends cancelled; the leader and the grandchild are not running; the kill path is the custom-agent group signal | `src/agent/custom.rs` `grok_cancel_kills_the_process_group` |
| TC-899 | FR-898 | unit | Preset color and the runner colors named in FR-898 | Compare | Preset color is `#6366f1` and is not equal to any color named in FR-898 | `src/custom_agent.rs` `grok_preset_color_is_distinct` |
| TC-900 | FR-898 | unit | App whose custom agent 0 is the grok preset; a grok chat line; dashboard publish; then `[theme.agents] grok = "#112233"` | Draw and publish | Cell foreground is `Rgb(0x63, 0x66, 0xf1)` and the grok `AgentView.color` is `#6366f1`; after the theme override both are `#112233` | `src/app.rs` `grok_chat_and_dashboard_use_preset_color` |
| TC-901 | FR-899 | unit | Settings with the builtin preset, routing default `grok`, then `allowed_runners = ["grok"]`; a non-builtin custom `grok` | Resolve runners | Builtin is excluded until listed; once listed, assignment can return `grok`; the user custom is allowed while the list is unset | `src/graph/policy.rs` `builtin_grok_is_not_graph_routed_unless_listed` |
| TC-902 | FR-899 | unit | Grok agent, planned model `composer-2.5`, no F2 selection; then an F2 selection `grok-4.6` | Resolve the effective model | Unselected result is not `composer-2.5` (provider default / none, so the preset default applies at send); F2 result is `grok-4.6` | `src/app.rs` `grok_effective_model_skips_composer_default` |
| TC-903 | FR-899 | unit | Unset `allowed_runners`, grok builtin registered, with and without the opencode builtin | Show `/graph` status | The status text names `grok`; with both builtins it names `grok` and `opencode`; it does not report an unqualified all-runners set | `src/app.rs` `graph_status_names_builtin_grok_exception` |
| TC-904 | FR-896 | unit | App with a grok adapter that records `request_models` | Confirm grok in the TUI picker and via dashboard `SelectAgent` | Each confirmation calls `request_models` on the grok adapter. Chat selection is accepted while graph `allowed` is false | `src/app.rs` `selecting_grok_requests_a_model_probe` |
| TC-905 | FR-900 | unit | README.md and CHANGELOG.md | Read | README First run names `DMUXAI_GROK_BIN`, `~/.grok/bin/grok`, `grok-4.7`, `fast`, `grok login`, `builtin_grok`, and `allowed_runners`. CHANGELOG states the new runner and the graph exception | `tests/readme_install.rs` `readme_and_changelog_mention_builtin_grok` |
| TC-906 | FR-900 | unit | Starter text with grok detected and without | Build starter | Both texts contain commented `builtin_grok` and still parse to default settings. The detected text contains a commented `name = "grok"` example. The undetected text has no grok custom example. `custom-agents.md` says `grok agent stdio` is not the built-in path | `src/init.rs` `starter_config_mentions_builtin_grok` |
| TC-907 | FR-892, FR-893 | live, ignored | A real `grok` that `grok models` reports as logged in | `grok -p` one short prompt through the preset | Reply text is non-empty and an `end` `sessionId` is stored. `#[ignore]`. Default `cargo test` does not run it. Missing binary or a logged-out CLI fails this ignored test with a reason, and that result is not a default-suite pass | `src/agent/custom.rs` `grok_live_smoke_headless_turn` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | Session id is the existing provider-session field. No schema change. | Clearing the binding drops `-r` on the next turn. | TC-893, TC-894 |
| Provider protocol | streaming-json as in FR-893. Unknown object types are ignored. | A grok upgrade that renames `sessionId` or `/data` fails TC-893 until the preset paths change. | TC-893, TC-894 |
| Security / permissions | Ask and Plan: `--permission-mode plan`. Implement: `--always-approve`. Prompt is argv. No auth file reads. | Implement remains the only mode given the write flag. | TC-892, TC-896, NFR-890 |
| Platform | Unix tests use `fake-grok.sh`. Windows uses the same PATH/`*.exe` lookup as other builtins. Process-group kill assertion is Unix. | Non-Unix cancel matches other custom runners. | TC-890, TC-898 |
| Graph files | No node model in `docs/dmuxai-graph.yaml` changes. Comment-only edit is FR-912. | Repo scans stay composer-2.5. | TC-912 in SPEC-model-policy |

Model allow/deny for `validate_model_for_runner` is SPEC-model-policy FR-910 and FR-911, not a second definition of those IDs.

## 7. Implementation plan

Phase 1 is this spec, the model-policy rows, [PLAN-grok-runner.md](../plans/PLAN-grok-runner.md), and [TASKS-grok-runner.md](../tasks/TASKS-grok-runner.md). Phase 2 implements only those requirements.

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Preset, models parser, auth decision, argv | `src/custom_agent.rs`, `tests/fixtures/fake-grok.sh` | FR-892, FR-893, FR-894, FR-895, FR-896, NFR-890 | TC-892, TC-893, TC-895, TC-896, TC-899 |
| 2 | Registration and lookup | `src/settings.rs` | FR-890, FR-891 | TC-890 |
| 3 | Runtime turn, cancel, login gate | `src/agent/custom.rs` | FR-893, FR-894, FR-895, FR-896, FR-897 | TC-894, TC-897, TC-898, TC-907 |
| 4 | Policy and graph exception | `src/model_policy.rs`, `src/graph/policy.rs`, `src/app.rs`, `docs/dmuxai-graph.yaml` | FR-898, FR-899, FR-910, FR-911, FR-912 | TC-900..TC-904, TC-910..TC-912 |
| 5 | Init, README, CHANGELOG, custom-agents note | `src/init.rs`, `README.md`, `CHANGELOG.md`, `docs/agents/custom-agents.md` | FR-900 | TC-905, TC-906 |
| 6 | Status integration | `tests/agent_api_cli.rs` | FR-890, FR-899 | TC-891 |

## 8. Review and test exit criteria

- [x] Every Must row maps to a passing TC in section 5.
- [x] `cargo test --test spec_index` passes (Status line and unique IDs).
- [x] Deterministic tests pass with the mock and without grok credentials.
- [x] `grok_live_smoke_headless_turn` stays `#[ignore]` and is reported separately.
- [x] `docs/reviews/REVIEW-grok-runner.md` records commands and residual risk after phase 2.

## 9. Addendum: turn end, background work, and text breaks

Added after v0.2.2 testing found that long `grok -p` runs printed their final summary and `end` line but never exited, and that text on either side of a tool call ran together. Investigation and evidence: [REVIEW-grok-runner-exit.md](../reviews/REVIEW-grok-runner-exit.md).

Root cause (grok 1.0.62): when a shell command outlives `block_until_ms` it moves to the background. If it finishes while the turn is still active, auto-wake queues a synthetic `task-completed-<id>` prompt. In headless `-p` mode that prompt is never started after the turn ends, so the process idles in `epoll_pwait` with no children and never exits. It is not stdin (DmuxAI already passes `/dev/null`) and not a child holding stdout. `--background-wait-timeout` does not help because nothing is running.

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-913 | Must | A grok preset turn starts. | The preset environment sets `GROK_AUTO_WAKE=0` and argv contains `--no-wait-for-background` before the mode flag, so a background task that finishes during the turn does not queue a prompt the headless driver never runs. The `dmuxai init` commented example matches the preset. | A user `[[agents.custom]]` named `grok` controls its own env and argv. | TC-913, TC-906 |
| FR-914 | Must | A custom agent with `done_when` (grok: `/type` equals `end`) prints a matching JSONL line. | DmuxAI stops reading stdout at that line and does not wait for EOF. The process gets 3 s to exit, then its process group gets SIGTERM and, after 2 s more, SIGKILL. The group also gets SIGTERM after a normal exit, so a leftover child in that group that holds stdout or stderr is stopped. Commands that grok itself runs are in their own session and process group (observed: `setsid` per command), so they are outside that group; with `--no-wait-for-background` grok stops them when it exits (verified with a 90 s background `sleep`). The turn's success is decided as if the process exited 0 (text, error path, and failure regex still apply). stderr is read for at most 2 s after exit. | The 900 s timeout stays as the last resort for a turn that never prints the line. Lines after the match are not read. | TC-914, TC-915 |
| FR-915 | Must | A custom agent turn has no `done_when` or never prints the line. | The turn ends at stdout EOF and process exit, as before. | A leftover child that holds only stderr does not delay the turn by more than 2 s. | TC-916, TC-894 |
| FR-916 | Must | A custom agent with `text_break_when` (grok: `/type` equals `tool_call`) prints a matching line, and later prints more reply text. | The next text chunk is prefixed with a blank line (`\n\n`) in both the streamed delta and the reply, when the reply is not empty, does not end in whitespace, and the chunk does not start with whitespace. | No break is added before the first text or twice in a row. | TC-917, TC-918 |
| FR-917 | Should | Someone delegates repository work to Grok Build outside DmuxAI. | [docs/agents/grok-build-delegation.md](../agents/grok-build-delegation.md) documents the invocation (`GROK_AUTO_WAKE=0`, `--no-wait-for-background`, `--effort`, `--max-turns`), how to split work into short turns, and how to measure each turn with `grok usage`. | Guidance only; DmuxAI does not change the effort level of the preset. | Doc review |

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-913 | FR-913, FR-914, FR-916 | unit | The grok preset | Inspect env, argv, and parse `end`, `text`, `tool_call`, `tool_call_update` lines | `GROK_AUTO_WAKE=0`; `--no-wait-for-background` in argv; `done_when` and `text_break_when` match only `end` and `tool_call` | `src/custom_agent.rs` `grok_preset_avoids_stranded_auto_wake` |
| TC-914 | FR-914 | integration | Mock `GROK_FAKE_MODE=linger`: prints text, tool call, text, `end`, then idles while a child holds stdout | Run a turn | The turn completes in under 5 s and the child is gone afterwards | `src/agent/custom.rs` `grok_end_line_finishes_turn_and_reaps_group` |
| TC-915 | FR-914 | integration | Mock `GROK_FAKE_MODE=orphan`: prints `OK` and `end`, leaves a child holding stdout, exits 0 | Run a turn | Reply `OK`, completed in under 2 s, child gone | `src/agent/custom.rs` `grok_orphan_holding_stdout_does_not_block_turn` |
| TC-916 | FR-915 | integration | Mock `GROK_FAKE_MODE=noend`: prints `OK`, no `end`, exits 0 | Run a turn | Reply `OK`, completed | `src/agent/custom.rs` `grok_turn_without_end_line_completes_at_exit` |
| TC-917 | FR-916 | unit | `append_reply_text` with pending breaks | Append words, leading spaces, and text after a newline | One `\n\n` only between two words; none at the start, after whitespace, or before a leading space | `src/custom_agent.rs` `append_reply_text_breaks_paragraphs_once` |
| TC-918 | FR-916 | integration | Mock `linger` mode | Run a turn | Streamed text is `Checking\n\nDone.` | `src/agent/custom.rs` `grok_text_after_tool_call_starts_new_paragraph` |

Mock modes added to `tests/fixtures/fake-grok.sh`: `linger`, `orphan`, and `noend`, as described in the script header.
