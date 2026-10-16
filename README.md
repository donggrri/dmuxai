# DmuxAI

One terminal. Multiple AI coding agents.

Product direction and model policy: see [MISSION.md](MISSION.md) (primary CLIs: **cursor-agent** then **agy**; graph workflows use **composer-2.5** only).

DmuxAI is a Ratatui client for three real, structured, headless coding-agent
CLIs. It does not scrape ANSI terminal screens and does not assemble prompts
into shell commands.

| Agent | Transport | Conversation continuity |
| --- | --- | --- |
| Codex | `codex app-server --stdio` JSONL | durable `thread_id` + `thread/resume` |
| Cursor | `--print --output-format stream-json` NDJSON | durable `session_id` + `--resume` |
| AGY | stream-json input/output NDJSON | durable `conversation_id` + `--conversation` |

## Requirements

The Windows release installer does not require Rust, Cargo, or Git. It installs
the DmuxAI executable only; coding-agent CLIs remain independently installed and
authenticated tools.

- Codex CLI, authenticated with `codex login`
- Cursor Agent CLI, authenticated with `cursor-agent login`
- AGY CLI with a working account/configuration

Installed CLI status can be checked with:

```powershell
codex --version
codex login status
cursor-agent --version
cursor-agent status
agy --version
agy models
opencode --version
claude --version
pi --version
```

## Install and run in any project

### Windows

Install the latest published Windows release for the current user. No
administrator permissions are required. The installer downloads the release,
verifies its SHA-256 checksum, installs `dmuxai.exe` to
`%LOCALAPPDATA%\DmuxAI\bin`, and adds that directory to the user `PATH`.

PowerShell:

```powershell
irm https://raw.githubusercontent.com/donggrri/dmuxai/main/install.ps1 | iex
```

Or, after cloning the repository, run `install.cmd` from Command Prompt or
`install.ps1` from PowerShell. These use the same published release; they do
not build from source.

```cmd
cd C:\path\to\DmuxAI
install.cmd
```

At the end of installation, DmuxAI reports whether Codex, Cursor Agent, AGY,
OpenCode, Claude Code, and Pi are available on `PATH`. Missing tools do not
prevent DmuxAI from installing. To omit that report, run
`./install.ps1 -SkipProviderCheck`.

Then move to any project and run `dmuxai` with no path argument:

```powershell
cd C:\path\to\project
dmuxai
```

DmuxAI uses the folder from which it was started as its workspace, so its agents
operate on that project. To update it, run the installer again; it replaces the
current-user executable with the latest published release.

You can start multiple DmuxAI windows from the same project at the same time.
When another live DmuxAI window already owns that workspace, a new window
starts in its own conversation and provider processes; the existing workspace
conversation list remains available from `/conversations` or `F3`.

### Build from source

Rust and Cargo are required only when building from source:

```powershell
cargo install --path . --locked --force
```

Conversations remain scoped by the canonical project path.

## Run from source

Windows PowerShell:

```powershell
cd C:\Users\tlsfmswls\Desktop\DmuxAI
cargo run

# Open another project without changing directories:
cargo run -- C:\path\to\project
```

Linux shell:

```bash
cd /path/to/DmuxAI
cargo run

# Or: cargo run -- /path/to/project
```

Controls:

- Type or paste: edit the prompt
- `/image <path>` or paste a single image path (`png`, `jpg`, `webp`, max 10 MiB each): attach files for the next prompt; attachments appear in the prompt title bar
- `Backspace`: delete the previous character
- `Enter`: send to the selected agent
- Type `/`: preview matching slash commands; use `Up` / `Down` and `Enter` or `Tab`
- `/agent`: open the active-agent picker, then use `Up` / `Down` and `Enter`
- `/mode`: choose `Ask`, `Plan`, or `Implement` for the active agent; Cursor Implement is unavailable on Windows
- `/effort`: choose a supported Codex reasoning effort for later turns
- `/status`: show Codex model/effort plus available token activity and rate limits
- `/new [title]`: create and switch to a durable conversation
- `/conversations` or `F3`: list and load conversations for the current workspace
- `/resume <id-prefix>`: load one conversation by a unique ID prefix
- `/summarize`: compress the completed shared context for the selected agent
- `/share <codex|cursor|agy>`: optionally replay a context import to one agent; ordinary turns are shared automatically
- `/quit`: stop owned child processes, restore the terminal, and exit
- `F2`: load and open that agent's model picker
- Model picker `Up` / `Down`, `Enter`, `Esc`: move, select, or close; on Codex,
  `Tab` selects the highlighted model and opens its effort picker
- Mouse wheel: scroll the chat history without changing the active agent
- Drag across Chat text: highlight and copy the selected text to the system clipboard
- Drag the prompt pane's top border: resize the chat/prompt height ratio
- `Ctrl+C`: cancel the selected agent's active turn
- `Esc`: close an open picker or slash preview; otherwise keep the application running

`/agent` changes which adapter receives the next prompt. Within a DmuxAI
conversation, each provider keeps its own native thread/session binding, while
completed turns are automatically projected into the next request for every
agent. Merely switching agents no longer requires `/share`; the command remains
available only for an explicit replay when desired.
The active agent is shown in the bottom status line; the agent list appears only
inside the `/agent` picker instead of occupying a permanent sidebar.

Model catalogs are loaded lazily only when `F2` is pressed. The chosen model is
kept separately for each agent and passed to subsequent turns. A CLI can list a
model that the current account or team policy does not permit; that provider
error is shown in chat so another model can be selected.

For Codex, `/effort` loads the selected model's advertised reasoning-effort
options and applies the selected value to subsequent turns. In the model picker,
press `Tab` to choose effort for the highlighted model. Changing an idle agent's
mode retires only that agent's native session; the next prompt opens a new
mode-scoped session with DmuxAI's completed shared context. During a Codex turn,
DmuxAI shows concise transient progress for commands, MCP calls, file changes,
searches, and other supported tool items. `/status` displays provider-reported
token activity and quota windows when the current Codex authentication supports
those fields; unavailable fields are labeled rather than estimated.

DmuxAI stores the canonical transcript, turns, provider bindings, context
checkpoints, and runtime-session ownership in SQLite. When no other live window
is using the current workspace, restart restores the latest conversation and
resumes a valid provider-native session. A new window started beside a live
peer receives a fresh conversation, while all completed workspace conversations
remain selectable. If no valid native session covers the common conversation
head, DmuxAI injects the shared rolling summary and recent completed turns from
all agents into a fresh session. The full transcript is retained; summarization
only changes the bounded context projection sent to the provider.

The database is stored at `%LOCALAPPDATA%\DmuxAI\conversations.sqlite3` on
Windows, `$XDG_STATE_HOME/dmuxai/conversations.sqlite3` when `XDG_STATE_HOME` is
set, or `~/.local/state/dmuxai/conversations.sqlite3` otherwise. Conversations
are scoped by the canonical current workspace path.

## Safety and platform behavior

Every agent starts in `Ask`. Modes are kept separately per agent and apply to
subsequent turns:

| Mode | Codex | Cursor | AGY |
| --- | --- | --- | --- |
| `Ask` | read-only sandbox | `--mode ask` | plan+sandbox with a Q&A instruction |
| `Plan` | read-only sandbox with a planning instruction | `--mode plan` | `--mode plan --sandbox` |
| `Implement` | workspace-write sandbox, approvals `never` | `--force --sandbox enabled` (macOS/Linux only; unavailable on Windows) | `--mode accept-edits --sandbox` |

`Implement` is the only mode intended to modify project files. It must be
selected explicitly with `/mode`; commands remain constrained to the provider's
workspace/sandbox configuration. DmuxAI never enables Codex danger-full-access,
Cursor's unsandboxed mode, or AGY's `--dangerously-skip-permissions` flag.

Cursor Implement is unavailable on Windows because Cursor sandbox mode requires
macOS or Linux. DmuxAI rejects that mode before changing the selected mode or
starting a provider process; use Cursor Ask or Plan on Windows.

The active mode is shown in the bottom status bar and in the `/agent` picker.
Mode changes are rejected by the UI while that agent has an active turn.

Windows uses `codex.exe` and `agy.exe`. Cursor's Windows installer exposes shell
wrappers, so DmuxAI resolves the latest installed Cursor version and launches
its bundled `node.exe index.js` directly. Linux uses `codex`, `cursor-agent`,
and `agy`. Windows children receive `CREATE_NO_WINDOW`; Cursor Implement remains
disabled on Windows because its sandbox is unavailable there.

Executable overrides:

- `DMUXAI_CODEX_BIN`
- `DMUXAI_CURSOR_BIN`
- `DMUXAI_AGY_BIN`

Prompts are written to child stdin. Neither Windows nor Linux routes prompt text
through `cmd.exe`, PowerShell command strings, or `/bin/sh`.

## Agent graph orchestration (default)

Multi-step prompts are planned as an agent-graph-engineering DAG before any CLI
turn runs. **Every graph node uses `composer-2.5` only** (repository model policy).
Model ids containing `fast`, or any non–composer-2.5 id, are rejected. Short 1–2
step prompts skip the graph and run on the **currently selected** agent with
`composer-2.5` applied.

- Status bar shows the active graph assignment.
- `/graph` — show the current plan
- `/graph continue` — approve a merge/DB/auth gate
- `/graph override <node> <cursor\|agy\|codex> <model-id>` — override runner/model

Graph state is stored under `.dmuxai/graphs/` for resume. Skills are loaded from
`.agents/skills/*/SKILL.md` and prefixed to node prompts when bound.

Disable auto-planning in `dmuxai.toml`:

```toml
[graph]
enabled = false
max_parallel = 2
max_retries = 2
```

## Custom headless agents

Any headless agent CLI (opencode, hermes, pi, ...) can be added via `[[agents.custom]]`
in `dmuxai.toml` — no Rust code. Custom runners appear in `/agent` and F2, keep their own
chat color, and can act as graph runners. See
[docs/agents/custom-agents.md](docs/agents/custom-agents.md) for the fields and
[docs/agents/presets.toml](docs/agents/presets.toml) for ready-made presets.

`NO_COLOR` disables chat colors unless `DMUXAI_FORCE_COLOR=1` is set.

## Image attachments

Attach `png`, `jpg`, or `webp` files (10 MiB per file) before sending a prompt:

- `/image path/to/screenshot.png`
- Paste a single filesystem path into the prompt area
- Mention supported paths in the prompt text (for example `see ./ui.png`)
- Press Ctrl+V or Alt+V with a PNG image in the clipboard

Attached files are copied under `.dmuxai/attachments/` in the workspace. Clipboard
images are saved there as `clip-<uuid>.png`. The prompt title shows `[Image #1]`
chips until the turn is sent successfully.

Cursor and AGY have no `--image` CLI flag, so their prompts carry `@<path>`
tokens appended to the prompt text; Codex keeps `@path` references in the turn
text (the app-server JSON has no separate image field in DmuxAI today).

## Verify

```powershell
cargo fmt --check
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
rustup target add x86_64-unknown-linux-gnu
cargo clippy --target x86_64-unknown-linux-gnu --all-targets --all-features -- -D warnings
```

## Delivery workflow

All behavior changes follow `Spec -> Plan -> Execute -> Review -> Test`.
The current product contract and test traceability matrix are in
[docs/specs/DMUXAI-BASELINE.md](docs/specs/DMUXAI-BASELINE.md); start a new
feature from [docs/templates/FEATURE-SPEC.md](docs/templates/FEATURE-SPEC.md)
and follow [docs/WORKFLOW.md](docs/WORKFLOW.md).

Real-provider smoke tests are ignored in the ordinary suite because they need
installed, authenticated CLIs and make real model requests:

```powershell
cargo test real_codex_app_server_handshake -- --ignored --nocapture
cargo test real_codex_turn_streams_to_completion -- --ignored --nocapture
cargo test real_codex_turn_can_be_interrupted -- --ignored --nocapture
cargo test real_cursor_turn_streams_to_completion -- --ignored --nocapture
cargo test real_agy_turn_streams_to_completion -- --ignored --nocapture
```

The Cursor smoke test currently names a model allowed by this development
machine's team policy. If that catalog changes, update the ignored smoke-test
model without hard-coding it into production selection behavior.

## Architecture

```text
TUI / keyboard
    -> ConversationEngine
    -> SQLite WAL: runtime_sessions / conversations / turns / messages
        -> per-process session heartbeat and turn ownership
        -> provider bindings by (conversation, agent)
        -> agent-specific summary checkpoint + recent tail
    -> bounded command mpsc (8 per agent)
        -> Codex actor
            -> persistent App Server JSONL process
        -> Cursor actor
            -> one headless NDJSON process per turn
            -> resume with provider session_id
        -> AGY actor
            -> one headless NDJSON process per turn
            -> resume with provider conversation_id
    -> bounded AgentOutput mpsc (64 shared)
    -> provider-independent AgentEvent
        -> correlated conversation/turn persistence
        -> text/model/completion/error state
        -> coalesced rendering (at most every 25 ms)
```

Each adapter owns its process and protocol-specific state. The UI sees only
normalized events. Cursor and AGY close stdin after writing one prompt so their
one-shot processes receive EOF; cancelling aborts the owning task and Tokio's
`kill_on_drop(true)` terminates the provider child. App shutdown closes all
command senders and awaits every adapter actor.

Chat autoscroll uses Ratatui's rendered-line count, so wrapped lines are included
when calculating the bottom of the viewport.

## Rust concepts for a C++ developer

### `async` / `await`

An `async fn` produces a `Future`, conceptually similar to a C++ coroutine state
machine. At `.await`, the task can suspend without blocking an OS thread. This
lets keyboard handling continue while one or more agents stream output.

### `tokio::spawn`

`tokio::spawn` schedules a lightweight task on the runtime. It resembles moving
an owning coroutine onto an executor more than creating one `std::thread` for
every operation. Its `JoinHandle` can be awaited or aborted.

### Bounded `mpsc`

Tokio's multi-producer/single-consumer channel resembles a typed, thread-safe
producer/consumer queue. Fixed capacities make backpressure explicit instead of
allowing memory to grow without limit. UI-facing methods use `try_send`, so an
overloaded runtime reports an error rather than blocking keyboard input.

### `Send`

`Send` proves that ownership of a value may safely move to another thread. It
does not mean “has a `send()` method.” Tokio may move spawned futures between
runtime workers, so values held across `.await` points normally need `Send`.

### `'static`

For `tokio::spawn`, `'static` means the future does not borrow references that
could expire while the task is alive. It does not mean the data lives forever.
The task owns its prompt, path, channel sender, selected model, and session ID,
similar to move-capturing owning C++ values in a task lambda.

## Code map

- `src/main.rs`: Tokio entry point and terminal RAII guard
- `src/app.rs`: event loop, chat rendering, routing, model picker, shutdown
- `src/conversation.rs`: durable SQLite conversations, runtime sessions, and context projection
- `src/agent/mod.rs`: provider-independent events, models, adapter interface
- `src/agent/codex.rs`: Codex App Server client
- `src/agent/cursor.rs`: Cursor NDJSON process and session adapter
- `src/agent/agy.rs`: AGY NDJSON process and conversation adapter
- `src/process/mod.rs`: Windows/Linux executable and headless-process boundary

Protocol references:

- [OpenAI Codex App Server](https://developers.openai.com/codex/app-server)
- [Cursor headless CLI](https://docs.cursor.com/en/cli/headless)
- [Cursor stream-json output](https://docs.cursor.com/en/cli/reference/output-format)
- [Google Antigravity headless mode](https://antigravity.google/docs/cli/headless/)

The next major step is tool/permission event normalization and richer structured
message parts beyond the current normalized text transcript.
