# DmuxAI

One terminal. Multiple AI coding agents.

DmuxAI is a Ratatui client for three real, structured, headless coding-agent
CLIs. It does not scrape ANSI terminal screens and does not assemble prompts
into shell commands.

| Agent | Transport | Conversation continuity |
| --- | --- | --- |
| Codex | `codex app-server --stdio` JSONL | One App Server thread per DmuxAI run |
| Cursor | `--print --output-format stream-json` NDJSON | `session_id` + `--resume` |
| AGY | stream-json input/output NDJSON | `conversation_id` + `--conversation` |

## Requirements

- Rust and Cargo
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
```

## Run

Windows PowerShell:

```powershell
cd C:\Users\tlsfmswls\Desktop\DmuxAI
cargo run
```

Linux shell:

```bash
cd /path/to/DmuxAI
cargo run
```

Controls:

- Type or paste: edit the prompt
- `Backspace`: delete the previous character
- `Enter`: send to the selected agent
- Type `/`: preview matching slash commands; use `Up` / `Down` and `Enter` or `Tab`
- `/agent`: open the active-agent picker, then use `Up` / `Down` and `Enter`
- `/mode`: choose `Ask`, `Plan`, or `Implement` for the active agent
- `F2`: load and open that agent's model picker
- Model picker `Up` / `Down`, `Enter`, `Esc`: move, select, or close
- Mouse wheel: scroll the chat history without changing the active agent
- Drag across Chat text: highlight and copy the selected text to the system clipboard
- Drag the prompt pane's top border: resize the chat/prompt height ratio
- `Ctrl+C`: cancel the selected agent's active turn
- `Esc`: stop all owned child processes, restore the terminal, and exit

`/agent` changes only which adapter receives the next prompt. Each provider's
thread, session, or conversation remains alive while DmuxAI is running, so
switching away and back continues that agent's existing conversation.
The active agent is shown in the bottom status line; the agent list appears only
inside the `/agent` picker instead of occupying a permanent sidebar.

Model catalogs are loaded lazily only when `F2` is pressed. The chosen model is
kept separately for each agent and passed to subsequent turns. A CLI can list a
model that the current account or team policy does not permit; that provider
error is shown in chat so another model can be selected.

Provider thread/session IDs are reused while DmuxAI remains open. Persisting and
resuming them after a DmuxAI restart remains future session-management work.

## Safety and platform behavior

Every agent starts in `Ask`. Modes are kept separately per agent and apply to
subsequent turns:

| Mode | Codex | Cursor | AGY |
| --- | --- | --- | --- |
| `Ask` | read-only sandbox | `--mode ask` | plan+sandbox with a Q&A instruction |
| `Plan` | read-only sandbox with a planning instruction | `--mode plan` | `--mode plan --sandbox` |
| `Implement` | workspace-write sandbox, approvals `never` | `--force --sandbox enabled` | `--mode accept-edits --sandbox` |

`Implement` is the only mode intended to modify project files. It must be
selected explicitly with `/mode`; commands remain constrained to the provider's
workspace/sandbox configuration. DmuxAI never enables Codex danger-full-access,
Cursor's unsandboxed mode, or AGY's `--dangerously-skip-permissions` flag.

The active mode is shown in the bottom status bar and in the `/agent` picker.
Mode changes are rejected by the UI while that agent has an active turn.

Windows uses `codex.exe` and `agy.exe`. Cursor's Windows installer exposes shell
wrappers, so DmuxAI resolves the latest installed Cursor version and launches
its bundled `node.exe index.js` directly. Linux uses `codex`, `cursor-agent`,
and `agy`. Windows children receive `CREATE_NO_WINDOW`.

Executable overrides:

- `DMUXAI_CODEX_BIN`
- `DMUXAI_CURSOR_BIN`
- `DMUXAI_AGY_BIN`

Prompts are written to child stdin. Neither Windows nor Linux routes prompt text
through `cmd.exe`, PowerShell command strings, or `/bin/sh`.

## Verify

```powershell
cargo fmt --check
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
rustup target add x86_64-unknown-linux-gnu
cargo clippy --target x86_64-unknown-linux-gnu --all-targets --all-features -- -D warnings
```

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

The next major step is tool/permission event normalization followed by durable
session persistence across DmuxAI restarts.
