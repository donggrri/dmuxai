//! Agent-facing CLI subcommands (SPEC-agent-api FR-760..FR-767):
//! `run`, `todo add|list|run`, `status`, `events`, `mcp`, `init`.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use dmuxai::agent_api::{self, ApiError, exit};
use dmuxai::conversation::{ConversationEngine, ConversationError};
use dmuxai::events::{EventLog, EventSink};
use dmuxai::mcp_server::{McpBackend, RunArgs};
use dmuxai::settings::DmuxSettings;
use dmuxai::term::proto::{RequestOp, Response};
use dmuxai::term::{self, SessionKind, TermError};
use dmuxai::web::DashboardHub;

use crate::app::{App, HeadlessOutcome, HeadlessRequest};

/// A parsed agent-facing subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentCommand {
    Run(RunOptions),
    TodoAdd {
        title: String,
        body: String,
        json: bool,
    },
    TodoList {
        status: Option<String>,
        json: bool,
    },
    TodoRun {
        id: String,
        run: RunOptions,
    },
    Status {
        json: bool,
    },
    Events {
        since: u64,
        limit: Option<usize>,
        follow: bool,
        json: bool,
    },
    Mcp,
    /// `init [--workspace] [--force]` (SPEC-first-run FR-814).
    Init {
        workspace: bool,
        force: bool,
    },
    /// `trust [--status] [--json] [--revoke] [--yes]`.
    Trust {
        status: bool,
        json: bool,
        revoke: bool,
        yes: bool,
    },
    /// `runs [--limit N] [--failed] [--json]` or `runs show <id> [--json]`.
    Runs {
        show_id: Option<String>,
        limit: usize,
        failed: bool,
        json: bool,
    },
    /// `runners [--json]` (FR-2831).
    Runners {
        json: bool,
    },
    /// `models [--agent ID] [--json]` (FR-2832).
    Models {
        agent: Option<String>,
        json: bool,
    },
    /// Persistent named terminal sessions (SPEC-terminal-sessions).
    Term(TermCommand),
    /// User-level coordinator decision board (SPEC-decision-board).
    Board(BoardCommand),
    /// `fanout clean [<run_id>|--all]` (FR-2934).
    FanoutClean {
        run_id: Option<String>,
        all: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardCommand {
    pub op: BoardOp,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardOp {
    Add {
        title: String,
        summary: String,
        kind: String,
        grade: Option<String>,
        lane: Option<String>,
        bot: Option<String>,
        channel: Option<String>,
        ref_url: Option<String>,
        workspace: Option<PathBuf>,
        recommendation: Option<String>,
        options: Vec<String>,
        plan_hash: Option<String>,
        key: Option<String>,
    },
    List {
        lanes: Vec<String>,
        workspace: Option<PathBuf>,
        all: bool,
    },
    Show {
        id: String,
    },
    Update {
        id: String,
        title: Option<String>,
        summary: Option<String>,
        recommendation: Option<String>,
        options: Vec<String>,
        replace_options: bool,
        grade: Option<String>,
        kind: Option<String>,
        run_id: Option<String>,
        if_version: Option<u32>,
    },
    Move {
        id: String,
        lane: String,
        if_version: Option<u32>,
    },
    Plan {
        id: String,
        hash: Option<String>,
        edit: Option<String>,
        prompt: Option<String>,
    },
    Decide {
        id: String,
        verdict: String,
        text: Option<String>,
        option: Option<usize>,
        plan_hash: Option<String>,
        by: Option<String>,
    },
    Say {
        id: String,
        text: String,
        from: Option<String>,
    },
    Wait {
        id: Option<String>,
        any: bool,
        since_version: Option<u32>,
        since_seq: Option<u64>,
        timeout_secs: u64,
        wait_for: String,
        waiter: Option<String>,
    },
    Events {
        since_seq: u64,
    },
    Prompt {
        id: String,
        hash: Option<String>,
    },
    Rules {
        set: Option<String>,
    },
    Ask {
        id: String,
        text: String,
        revise: bool,
        agent: Option<String>,
        model: Option<String>,
        by: Option<String>,
    },
    Run {
        id: String,
        detach: bool,
        by: Option<String>,
        exec_prepared: bool,
        plan_hash: Option<String>,
    },
    Pending {
        workspace: Option<PathBuf>,
    },
    Check {
        id: String,
        text: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermCommand {
    pub op: TermOp,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermOp {
    Open {
        name: String,
        kind: SessionKind,
        cwd: Option<PathBuf>,
        env: Vec<(String, String)>,
        cols: u16,
        rows: u16,
        argv: Vec<String>,
        serial: Option<term::SerialConfig>,
        no_usbipd_attach: bool,
        wait_for: Option<String>,
        timeout_ms: Option<u64>,
    },
    Send {
        name: String,
        text: String,
        newline: bool,
        secret: bool,
    },
    SendKeys {
        name: String,
        keys: String,
    },
    Read {
        name: String,
        since: u64,
        tail: Option<usize>,
        raw: bool,
    },
    Wait {
        name: String,
        regex: Option<String>,
        idle_ms: Option<u64>,
        timeout_ms: u64,
        since: Option<u64>,
    },
    Exec {
        name: String,
        command: String,
        timeout_ms: Option<u64>,
    },
    Approve {
        id: String,
    },
    List {
        /// FR-2606: `--all` includes closed and exited history sessions.
        all: bool,
    },
    Show {
        name: String,
    },
    Resize {
        name: String,
        cols: u16,
        rows: u16,
    },
    Close {
        name: Option<String>,
        force: bool,
        all: bool,
    },
    Reopen {
        name: String,
    },
    /// Hidden host entry point used by the client to autostart a host.
    Host,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOptions {
    pub prompt: String,
    pub agent: Option<String>,
    pub mode: Option<String>,
    pub read_only: bool,
    pub graph: bool,
    pub approve_plan: bool,
    pub approve_plan_id: Option<String>,
    pub allow_sensitive: bool,
    pub json: bool,
    pub model: Option<String>,
    pub progress: bool,
    pub trust_workspace_config: bool,
    pub agents: Option<String>,
    pub min_ok: Option<usize>,
    pub judge: Option<String>,
    pub write: bool,
    pub keep_worktrees: bool,
}

/// First arguments that select an agent-facing subcommand.
pub const SUBCOMMANDS: [&str; 13] = [
    "run", "todo", "status", "events", "mcp", "init", "trust", "runs", "runners", "models", "term",
    "board", "fanout",
];

pub const BOARD_USAGE: &str = "\
Usage: dmuxai board <subcommand> [args...]

Coordinator decision board (user-level, not per repo).

Subcommands:
  add        Create an item
  list       List items (newest first)
  show       Show one item
  update     Patch fields (optional --if-version)
  move       Set lane
  plan       Attach (--hash) or edit (--edit) a graph plan
  decide     Record approve|reject|request_changes|answer
  say        Append a thread message
  wait       Block until owner/planner activity (--any for any item)
  events     List board events (--since SEQ)
  prompt     Print unmasked plan prompt for piping into run
  rules      Print or replace classification rules (owner only for --set)
  ask        Ask the planner a question or request a plan revision
  run        Execute an approved or auto item's graph plan
  pending    List items waiting for human action
  check      Mark a done or unknown item as checked (owner only)

Run `dmuxai board <subcommand> --help` for details.
";

pub const BOARD_ADD_USAGE: &str = "\
Usage: dmuxai board add --title T [--summary S|-] --kind K [--grade G] [--lane L] \
[--bot NAME] [--channel C] [--ref URL] [-C DIR] [--recommend TEXT] [--option TEXT]... \
[--plan-hash H] [--key K] [--json]
";

pub const BOARD_LIST_USAGE: &str = "\
Usage: dmuxai board list [--lane L]... [--workspace DIR] [--all] [--json]
";

pub const BOARD_SHOW_USAGE: &str = "\
Usage: dmuxai board show <id> [--json]
";

pub const BOARD_UPDATE_USAGE: &str = "\
Usage: dmuxai board update <id> [--title T] [--summary S|-] [--recommend T] \
[--option TEXT]... [--grade G] [--kind K] [--run-id R] [--if-version N] [--json]
";

pub const BOARD_MOVE_USAGE: &str = "\
Usage: dmuxai board move <id> <lane> [--json]
";

pub const BOARD_PLAN_USAGE: &str = "\
Usage: dmuxai board plan <id> [--hash H] [--edit FILE|-] [--json]
";

pub const BOARD_DECIDE_USAGE: &str = "\
Usage: dmuxai board decide <id> approve|reject|request_changes|answer \
[--text T] [--option N] [--plan-hash H] [--by owner|bot:NAME] [--json]
";

pub const BOARD_SAY_USAGE: &str = "\
Usage: dmuxai board say <id> --text T|- [--from owner|bot:NAME|planner] [--json]
";

pub const BOARD_WAIT_USAGE: &str = "\
Usage: dmuxai board wait <id>|--any [--since-version N] [--since-seq S] [--timeout SECS] \
[--for decision|message|any] [--as bot:NAME] [--json]
";

pub const BOARD_EVENTS_USAGE: &str = "\
Usage: dmuxai board events [--since SEQ] [--json]
";

pub const BOARD_PROMPT_USAGE: &str = "\
Usage: dmuxai board prompt <id> [--hash H] [--json]
";

pub const BOARD_RULES_USAGE: &str = "\
Usage: dmuxai board rules [--set FILE|-] [--json]
";

pub const BOARD_ASK_USAGE: &str = "\
Usage: dmuxai board ask <id> --text T|- [--revise] [--agent A] [--model M] [--by ACTOR] [--json]
";

pub const BOARD_RUN_USAGE: &str = "\
Usage: dmuxai board run <id> [--detach] [--by ACTOR] [--json]
";

pub const BOARD_PENDING_USAGE: &str = "\
Usage: dmuxai board pending [--workspace DIR] [--json]
";

pub const BOARD_CHECK_USAGE: &str = "\
Usage: dmuxai board check <id> [--text T] [--json]
";

pub const RUN_USAGE: &str = "\
Usage: dmuxai run <prompt|-> [options]

Run one prompt headless and exit.

Arguments:
  <prompt|->               Prompt text to run, or - to read from stdin

Options:
  --agent ID               Runner id (cursor-agent, agy, codex, or a custom agent; `cursor` is an alias)
  --agents RUNNERS         Comma-separated runners for fan-out (a,b[:model],...)
  --write                  Write fan-out: each leg runs in its own git worktree (needs --agents)
  --keep-worktrees         Keep leg worktrees after a write fan-out (needs --write)
  --judge RUNNER           Read-only judge runner after fan-out (agent[:model]; needs --agents)
  --min-ok N               Minimum successful legs required (default 1; fan-out only)
  --model M                Model id for the runner
  --approve-plan           Approve a pending plan and confirm sensitive gates
  --approve-plan-id HASH   Run a plan saved by a previous exit 7 (requires --graph)
  --allow-sensitive        Confirm every sensitive gate
  --graph                  Use the graph planner
  --progress               Stream one JSON line per agent activity to stderr
  --json                   Print machine-readable JSON on stdout
  --trust-workspace-config Trust workspace configuration without prompting
  -h, --help               Print help information
";

pub const TODO_USAGE: &str = "\
Usage: dmuxai todo <subcommand> [options]

Add, list or run to-dos without the TUI.

Subcommands:
  add <title> [--body TEXT] [--json]       Add a to-do
  list [--status open|running|done] [--json] List to-dos
  run <id> [options]                       Run a to-do

Options for `todo run`:
  --agent ID               Runner id (cannot be used with --graph)
  --model M                Model id for the runner
  --approve-plan           Approve a pending plan and confirm sensitive gates
  --approve-plan-id HASH   Run a plan saved by a previous exit 7 (requires --graph)
  --allow-sensitive        Confirm every sensitive gate
  --graph                  Use the graph planner
  --progress               Stream activity progress events to stderr
  --json                   Print machine-readable JSON on stdout
  --trust-workspace-config Trust workspace configuration without prompting
  -h, --help               Print help information
";

pub const STATUS_USAGE: &str = "\
Usage: dmuxai status [options]

Print workspace status (agents, to-dos, last graph, events).

Options:
  --json                   Print machine-readable JSON on stdout
  -h, --help               Print help information
";

pub const EVENTS_USAGE: &str = "\
Usage: dmuxai events [options]

Print state-change events from .dmuxai/events.jsonl.

Options:
  --since N                Only print events with seq greater than N
  --limit N                Maximum number of events to print
  -f, --follow             Keep waiting for new events to arrive
  --json                   Print machine-readable JSON on stdout
  -h, --help               Print help information
";

pub const MCP_USAGE: &str = "\
Usage: dmuxai mcp [options]

Serve DmuxAI as an MCP server over stdio.

Options:
  -h, --help               Print help information
";

pub const INIT_USAGE: &str = "\
Usage: dmuxai init [options]

Write a commented starter dmuxai.toml to the user config or workspace.

Options:
  --workspace              Write configuration to workspace instead of user config
  --force                  Overwrite existing configuration file
  -h, --help               Print help information
";

pub const TRUST_USAGE: &str = "\
Usage: dmuxai trust [options]

Manage workspace configuration trust for dmuxai.toml.

Options:
  --status                 Show workspace trust status
  --revoke                 Revoke trust for the current workspace
  --yes                    Confirm trusting workspace configuration without prompting
  --json                   Print machine-readable JSON on stdout
  -h, --help               Print help information
";

pub const RUNS_USAGE: &str = "\
Usage: dmuxai runs [options]

List or inspect recorded execution runs.

Subcommands:
  show <id> [--json]       Show details for a recorded run

Options:
  --limit N                Maximum number of runs to list (default 20)
  --failed                 Only list failed runs
  --json                   Print machine-readable JSON on stdout
  -h, --help               Print help information
";

pub const RUNNERS_USAGE: &str = "\
Usage: dmuxai runners [options]

List registered runners (primary Cursor keyword: cursor-agent; `cursor` is an alias), admission, installation and model policy.

Options:
  --json                   Print machine-readable JSON on stdout
  -h, --help               Print help information
";

pub const FANOUT_USAGE: &str = "\
Usage: dmuxai fanout clean [<run_id>|--all]

Remove leftover write fan-out git worktrees under .dmuxai/fanout/.

Subcommands:
  clean [<run_id>|--all]   Remove worktrees for one run or all runs

Options:
  --all                    Clean every run under .dmuxai/fanout/
  -h, --help               Print help information
";

pub const FANOUT_CLEAN_FLAGS: &[FlagSpec] = &[FlagSpec {
    names: &["--all"],
    takes_value: false,
    hidden: false,
}];

pub const MODELS_USAGE: &str = "\
Usage: dmuxai models [options]

List available models and runner model policy.

Options:
  --agent ID               Only list models for the given runner id (cursor-agent; `cursor` is an alias)
  --json                   Print machine-readable JSON on stdout
  -h, --help               Print help information
";

/// Subcommand-local help for agent-facing subcommands (FR-2830).
pub fn subcommand_usage(name: &str) -> Option<&'static str> {
    match name {
        "run" => Some(RUN_USAGE),
        "todo" => Some(TODO_USAGE),
        "status" => Some(STATUS_USAGE),
        "events" => Some(EVENTS_USAGE),
        "mcp" => Some(MCP_USAGE),
        "init" => Some(INIT_USAGE),
        "trust" => Some(TRUST_USAGE),
        "runs" => Some(RUNS_USAGE),
        "runners" => Some(RUNNERS_USAGE),
        "models" => Some(MODELS_USAGE),
        "fanout" => Some(FANOUT_USAGE),
        _ => None,
    }
}

/// Subcommand-local help for `dmuxai term --help` / `-h` (FR-2617). This text
/// replaces the root usage for term; the root `dmuxai --help` is unchanged.
pub const TERM_USAGE: &str = "\
Usage: dmuxai term <subcommand> [args...]

Persistent named terminal sessions.

Subcommands:
  open NAME [options] [-- CMD...]  Start a shell, a --kind cmd command, or a
                                   --serial DEV session and keep it running
  send NAME TEXT | NAME --keys KEYS
                                   Send text or named keys (C-c, Enter, ...)
  read NAME [--since N] [--tail N] [--raw]
                                   Read session output
  wait NAME --match REGEX|--idle DUR --timeout DUR [--since N]
                                   Wait for a regex match or an idle period
                                   (--idle: bare number is ms; --timeout:
                                   bare number is seconds)
  exec NAME COMMAND [--timeout DUR]
                                   Run a command in the session and return
                                   exactly its output
  approve ID    Approve one pending terminal-policy approval (16 lowercase
                hex characters)
  list [--all]  List sessions (empty when no host runs); closed and exited
                sessions are history shown only with --all
  show NAME     Show one session
  resize NAME --cols N --rows N
                Resize a session
  close NAME|--all [--force]
                Close a session (or all sessions with --all)
  reopen NAME   Reopen a lost session with the same spec

Run `dmuxai term open --help` for open options (including --serial, --baud,
--usbipd-busid and the deprecated --no-usbipd-attach). Run `dmuxai --help` for
the root usage.
";

/// Subcommand-local help for `dmuxai term open --help` / `-h` (FR-2617).
pub const TERM_OPEN_USAGE: &str = "\
Usage: dmuxai term open NAME [--kind shell|cmd|serial] [--cwd DIR] [--env K=V]
                [--cols N --rows N] [--json] [-- CMD...]
                [--serial DEV --baud N [--parity none|even|odd]
                 [--data 5-8] [--stop 1|2] [--flow none|rtscts]
                 [--usbipd-busid ID] [--no-usbipd-attach]]
                [--wait-for REGEX --timeout DUR]

Starts a named terminal session; it stays alive under the terminal host after
this command exits.

Session options:
  --kind shell|cmd|serial  Session kind (default shell; --serial implies
                           serial)
  --cwd DIR                Working directory for the session
  --env K=V                Extra session environment (repeatable)
  --cols N --rows N        Terminal size (default 80x24)
  --wait-for REGEX         Return after REGEX appears in the output; requires
  --timeout DUR            --timeout; a bare number is seconds (8 and 8s =
                           8 seconds), suffixes work (500ms = 500ms)
  -- CMD...                Command for --kind cmd; serial and shell sessions
                           take no command

Serial options:
  --serial DEV             Open DEV as a serial port instead of a shell
  --baud N                 Baud rate (required with --serial)
  --parity P               Parity: none, even or odd (default none)
  --data B                 Data bits 5-8 (default 8)
  --stop S                 Stop bits 1 or 2 (default 1)
  --flow F                 Flow control: none or rtscts (default none)
   --usbipd-busid ID        Busid named only in needs_usbipd_attach guidance;
                            never selected from `usbipd list` output
   --no-usbipd-attach       Deprecated no-op; accepted for compatibility,
                            dmuxai never runs usbipd attach

On WSL2 with a missing --serial device, dmuxai exits with error_kind
needs_usbipd_attach and never runs usbipd.exe; its guidance names the bind and
attach commands for an elevated Windows PowerShell. --usbipd-busid (or trusted
[terminal.serial] usbipd_busid) only names the busid in that guidance.
";

/// Subcommand-local help for `dmuxai term send --help` / `-h` (FR-2617).
pub const TERM_SEND_USAGE: &str = "\
Usage: dmuxai term send NAME TEXT [--no-newline] [--secret] [--json]
       dmuxai term send NAME --keys KEYS [--json]

Send TEXT to the session as if typed; a newline is appended unless
--no-newline is set. With --keys, send named keys such as C-c, Enter or
C-c,C-c (separated by ',' or '+') instead of text. --secret masks the
input in the session journal.

  --keys KEYS     Send named keys (C-c, Enter, ...) instead of text
  --no-newline    Do not append a newline after TEXT
  --secret        Mask this input in the session journal
  --json          Print a machine-readable reply
";

/// Subcommand-local help for `dmuxai term read --help` / `-h` (FR-2617).
pub const TERM_READ_USAGE: &str = "\
Usage: dmuxai term read NAME [--since SEQ] [--tail N] [--raw] [--json]

Print the output the session has produced so far.

  --since SEQ     Only output after sequence number SEQ (default 0)
  --tail N        Only the last N bytes of the stripped output
  --raw           Print raw bytes including control characters
  --json          Print a machine-readable reply with seq_from/seq_to
";

/// Subcommand-local help for `dmuxai term wait --help` / `-h` (FR-2617).
pub const TERM_WAIT_USAGE: &str = "\
Usage: dmuxai term wait NAME (--match REGEX | --idle DUR) --timeout DUR
                     [--since SEQ] [--json]

Wait until stripped output after SEQ matches REGEX, or until output has
been quiet for DUR. A bare number for --idle is milliseconds, so --idle 500
is 500ms; suffixes work too (500ms, 1s, 1m). A bare number for --timeout is
seconds, so --timeout 8 and --timeout 8s both mean 8 seconds, and
--timeout 500ms means 500ms. --since applies only to the --match search;
without it the search starts at the current seq, so output already in the
buffer is not matched. After send, pass --since from that send's reply to
match output caused by the send. --idle succeeds when no new output arrives for that
duration: the idle clock starts when the wait starts and resets whenever
new output arrives, so a quiet session returns almost immediately.

  --match REGEX   Wait until REGEX matches new output
  --idle DUR      Succeed after DUR without new output; a bare number is
                  milliseconds (500 = 500ms), suffixes work (500ms, 1s)
  --timeout DUR   Give up after DUR; a bare number is seconds (8 and 8s =
                  8 seconds), suffixes work (500ms = 500ms); exits 1
  --since SEQ     Search for REGEX only in output after SEQ
  --json          Print a machine-readable reply
";

/// Subcommand-local help for `dmuxai term exec --help` / `-h` (FR-2617).
pub const TERM_EXEC_USAGE: &str = "\
Usage: dmuxai term exec NAME COMMAND [--timeout DUR] [--json]

Type COMMAND into the session, press Enter and return exactly the output
the command prints together with its exit code; the internal completion
marker never appears in the reply.

  --timeout DUR   Give up after DUR and exit 1 (default 30s); a bare number
                  is seconds (8 and 8s = 8 seconds), suffixes work
                  (500ms = 500ms)
  --json          Print a machine-readable reply
";

/// Subcommand-local help for `dmuxai term approve --help` / `-h` (FR-2617).
pub const TERM_APPROVE_USAGE: &str = "\
Usage: dmuxai term approve ID [--json]

Approve one pending terminal-policy approval. ID is the 16 lowercase hex
characters from the approval_required error; the approval releases
exactly one identical send or exec and is then consumed.

  --json          Print a machine-readable reply
";

/// Subcommand-local help for `dmuxai term list --help` / `-h` (FR-2617).
pub const TERM_LIST_USAGE: &str = "\
Usage: dmuxai term list [--all] [--json]

List this workspace's terminal sessions. Closed and exited sessions are
intended history: the default list hides them, `show NAME` still reports
them, and `list --all` includes them. Prints nothing (an empty JSON
array with --json) when no host runs; list never starts one.

  --all           Include closed and exited history sessions
  --json          Print a machine-readable reply
";

/// Subcommand-local help for `dmuxai term show --help` / `-h` (FR-2617).
pub const TERM_SHOW_USAGE: &str = "\
Usage: dmuxai term show NAME [--json]

Show one session: kind, state, pid, size, takeover flag and output
sequence.

  --json          Print a machine-readable reply
";

/// Subcommand-local help for `dmuxai term resize --help` / `-h` (FR-2617).
pub const TERM_RESIZE_USAGE: &str = "\
Usage: dmuxai term resize NAME --cols N --rows N [--json]

Resize the session's terminal. N must be between 1 and 1000.

  --cols N        New number of columns (required)
  --rows N        New number of rows (required)
  --json          Print a machine-readable reply
";

/// Subcommand-local help for `dmuxai term close --help` / `-h` (FR-2617).
pub const TERM_CLOSE_USAGE: &str = "\
Usage: dmuxai term close NAME [--force] [--json]
       dmuxai term close --all [--force] [--json]

Close one session, or every session of this workspace with --all.
Children in the session's process group are asked to hang up first.

  --all           Close every session of this workspace
  --force         Skip the 2s grace period and terminate immediately
  --json          Print a machine-readable reply
";

/// Subcommand-local help for `dmuxai term reopen --help` / `-h` (FR-2617).
pub const TERM_REOPEN_USAGE: &str = "\
Usage: dmuxai term reopen NAME [--json]

Reopen a lost session with the same spec: the same name, kind, size,
command (for --kind cmd) or serial settings.

  --json          Print a machine-readable reply
";

/// Per-subcommand term help (FR-2617): `term <sub> --help` / `-h` prints this
/// topic instead of the parent `term` list or the root usage. `open` keeps
/// [`TERM_OPEN_USAGE`]; an unknown subcommand falls back to [`TERM_USAGE`].
pub fn board_subcommand_usage(subcommand: &str) -> &'static str {
    match subcommand {
        "add" => BOARD_ADD_USAGE,
        "list" => BOARD_LIST_USAGE,
        "show" => BOARD_SHOW_USAGE,
        "update" => BOARD_UPDATE_USAGE,
        "move" => BOARD_MOVE_USAGE,
        "plan" => BOARD_PLAN_USAGE,
        "decide" => BOARD_DECIDE_USAGE,
        "say" => BOARD_SAY_USAGE,
        "wait" => BOARD_WAIT_USAGE,
        "events" => BOARD_EVENTS_USAGE,
        "prompt" => BOARD_PROMPT_USAGE,
        "rules" => BOARD_RULES_USAGE,
        "ask" => BOARD_ASK_USAGE,
        "run" => BOARD_RUN_USAGE,
        "pending" => BOARD_PENDING_USAGE,
        "check" => BOARD_CHECK_USAGE,
        _ => BOARD_USAGE,
    }
}

pub fn term_subcommand_usage(subcommand: &str) -> &'static str {
    match subcommand {
        "open" => TERM_OPEN_USAGE,
        "send" => TERM_SEND_USAGE,
        "read" => TERM_READ_USAGE,
        "wait" => TERM_WAIT_USAGE,
        "exec" => TERM_EXEC_USAGE,
        "approve" => TERM_APPROVE_USAGE,
        "list" => TERM_LIST_USAGE,
        "show" => TERM_SHOW_USAGE,
        "resize" => TERM_RESIZE_USAGE,
        "close" => TERM_CLOSE_USAGE,
        "reopen" => TERM_REOPEN_USAGE,
        _ => TERM_USAGE,
    }
}

/// Table-driven flag specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagSpec {
    pub names: &'static [&'static str],
    pub takes_value: bool,
    /// Hidden flags are accepted but not listed in help.
    pub hidden: bool,
}

/// Parsed flags for a subcommand.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedFlags {
    pub flags: std::collections::BTreeMap<String, Option<String>>,
    pub positionals: Vec<String>,
}

/// Generic table-driven flag parser.
pub fn parse_flags(
    args: &[String],
    specs: &[FlagSpec],
    allow_positional: bool,
) -> Result<ParsedFlags, String> {
    let mut parsed = ParsedFlags::default();
    let mut index = 0;
    let mut rest_is_positional = false;
    while index < args.len() {
        let arg = args[index].clone();
        if rest_is_positional {
            if !allow_positional {
                return Err(format!("unexpected argument '{arg}'"));
            }
            parsed.positionals.push(arg);
            index += 1;
            continue;
        }
        if arg == "--" {
            rest_is_positional = true;
            index += 1;
            continue;
        }
        let matched = specs.iter().find(|spec| spec.names.contains(&arg.as_str()));
        if let Some(spec) = matched {
            if spec.takes_value {
                index += 1;
                let value = args.get(index).map(String::as_str);
                match value {
                    Some(value) if !value.starts_with("--") => {
                        for name in spec.names {
                            parsed
                                .flags
                                .insert((*name).to_owned(), Some(value.to_owned()));
                        }
                    }
                    _ => {
                        return Err(format!("{arg} needs a value"));
                    }
                }
            } else {
                for name in spec.names {
                    parsed.flags.insert((*name).to_owned(), None);
                }
            }
            index += 1;
            continue;
        }
        if arg.starts_with("--") {
            return Err(format!("unexpected argument '{arg}'"));
        }
        if !allow_positional {
            return Err(format!("unexpected argument '{arg}'"));
        }
        parsed.positionals.push(arg);
        index += 1;
    }
    Ok(parsed)
}

pub const RUN_FLAGS: &[FlagSpec] = &[
    FlagSpec {
        names: &["--agent"],
        takes_value: true,
        hidden: false,
    },
    FlagSpec {
        names: &["--agents"],
        takes_value: true,
        hidden: false,
    },
    FlagSpec {
        names: &["--write"],
        takes_value: false,
        hidden: false,
    },
    FlagSpec {
        names: &["--keep-worktrees"],
        takes_value: false,
        hidden: false,
    },
    FlagSpec {
        names: &["--min-ok"],
        takes_value: true,
        hidden: false,
    },
    FlagSpec {
        names: &["--judge"],
        takes_value: true,
        hidden: false,
    },
    FlagSpec {
        names: &["--mode"],
        takes_value: true,
        hidden: true,
    },
    FlagSpec {
        names: &["--graph"],
        takes_value: false,
        hidden: false,
    },
    FlagSpec {
        names: &["--read-only"],
        takes_value: false,
        hidden: true,
    },
    FlagSpec {
        names: &["--approve-plan"],
        takes_value: false,
        hidden: false,
    },
    FlagSpec {
        names: &["--approve-plan-id"],
        takes_value: true,
        hidden: false,
    },
    FlagSpec {
        names: &["--allow-sensitive"],
        takes_value: false,
        hidden: false,
    },
    FlagSpec {
        names: &["--json"],
        takes_value: false,
        hidden: false,
    },
    FlagSpec {
        names: &["--model"],
        takes_value: true,
        hidden: false,
    },
    FlagSpec {
        names: &["--progress"],
        takes_value: false,
        hidden: false,
    },
    FlagSpec {
        names: &["--trust-workspace-config"],
        takes_value: false,
        hidden: false,
    },
];

/// `todo run` flags: same as [`RUN_FLAGS`] except fan-out-only options.
pub const TODO_RUN_FLAGS: &[FlagSpec] = &[
    FlagSpec {
        names: &["--agent"],
        takes_value: true,
        hidden: false,
    },
    FlagSpec {
        names: &["--mode"],
        takes_value: true,
        hidden: true,
    },
    FlagSpec {
        names: &["--graph"],
        takes_value: false,
        hidden: false,
    },
    FlagSpec {
        names: &["--read-only"],
        takes_value: false,
        hidden: true,
    },
    FlagSpec {
        names: &["--approve-plan"],
        takes_value: false,
        hidden: false,
    },
    FlagSpec {
        names: &["--approve-plan-id"],
        takes_value: true,
        hidden: false,
    },
    FlagSpec {
        names: &["--allow-sensitive"],
        takes_value: false,
        hidden: false,
    },
    FlagSpec {
        names: &["--json"],
        takes_value: false,
        hidden: false,
    },
    FlagSpec {
        names: &["--model"],
        takes_value: true,
        hidden: false,
    },
    FlagSpec {
        names: &["--progress"],
        takes_value: false,
        hidden: false,
    },
    FlagSpec {
        names: &["--trust-workspace-config"],
        takes_value: false,
        hidden: false,
    },
];

pub const TODO_ADD_FLAGS: &[FlagSpec] = &[
    FlagSpec {
        names: &["--body"],
        takes_value: true,
        hidden: false,
    },
    FlagSpec {
        names: &["--json"],
        takes_value: false,
        hidden: false,
    },
];

pub const TODO_LIST_FLAGS: &[FlagSpec] = &[
    FlagSpec {
        names: &["--status"],
        takes_value: true,
        hidden: false,
    },
    FlagSpec {
        names: &["--json"],
        takes_value: false,
        hidden: false,
    },
];

pub const STATUS_FLAGS: &[FlagSpec] = &[FlagSpec {
    names: &["--json"],
    takes_value: false,
    hidden: false,
}];

pub const EVENTS_FLAGS: &[FlagSpec] = &[
    FlagSpec {
        names: &["--since"],
        takes_value: true,
        hidden: false,
    },
    FlagSpec {
        names: &["--limit"],
        takes_value: true,
        hidden: false,
    },
    FlagSpec {
        names: &["--follow", "-f"],
        takes_value: false,
        hidden: false,
    },
    FlagSpec {
        names: &["--json"],
        takes_value: false,
        hidden: false,
    },
];

pub const INIT_FLAGS: &[FlagSpec] = &[
    FlagSpec {
        names: &["--workspace"],
        takes_value: false,
        hidden: false,
    },
    FlagSpec {
        names: &["--force"],
        takes_value: false,
        hidden: false,
    },
];

pub const TRUST_FLAGS: &[FlagSpec] = &[
    FlagSpec {
        names: &["--status"],
        takes_value: false,
        hidden: false,
    },
    FlagSpec {
        names: &["--json"],
        takes_value: false,
        hidden: false,
    },
    FlagSpec {
        names: &["--revoke"],
        takes_value: false,
        hidden: false,
    },
    FlagSpec {
        names: &["--yes"],
        takes_value: false,
        hidden: false,
    },
];

pub const RUNS_FLAGS: &[FlagSpec] = &[
    FlagSpec {
        names: &["--limit"],
        takes_value: true,
        hidden: false,
    },
    FlagSpec {
        names: &["--failed"],
        takes_value: false,
        hidden: false,
    },
    FlagSpec {
        names: &["--json"],
        takes_value: false,
        hidden: false,
    },
];

pub const RUNS_SHOW_FLAGS: &[FlagSpec] = &[FlagSpec {
    names: &["--json"],
    takes_value: false,
    hidden: false,
}];

pub const RUNNERS_FLAGS: &[FlagSpec] = &[FlagSpec {
    names: &["--json"],
    takes_value: false,
    hidden: false,
}];

pub const MODELS_FLAGS: &[FlagSpec] = &[
    FlagSpec {
        names: &["--agent"],
        takes_value: true,
        hidden: false,
    },
    FlagSpec {
        names: &["--json"],
        takes_value: false,
        hidden: false,
    },
];

/// Parses run-style flags; positional words become the prompt.
fn parse_run_options(args: &[String], allow_prompt: bool) -> Result<RunOptions, String> {
    parse_run_options_with_flags(args, allow_prompt, RUN_FLAGS)
}

fn parse_run_options_with_flags(
    args: &[String],
    allow_prompt: bool,
    flag_specs: &[FlagSpec],
) -> Result<RunOptions, String> {
    let parsed = parse_flags(args, flag_specs, allow_prompt)?;
    if parsed.flags.contains_key("--mode") {
        return Err(
            "--mode was removed; DmuxAI runs a single agent mode (write-capable). Omit --mode."
                .to_owned(),
        );
    }
    if !allow_prompt
        && !parsed.positionals.is_empty()
        && let Some(word) = parsed.positionals.first()
    {
        return Err(format!("unexpected argument '{word}'"));
    }
    let approve_plan_id = parsed.flags.get("--approve-plan-id").cloned().flatten();
    let agents = parsed.flags.get("--agents").cloned().flatten();
    let min_ok = if let Some(raw) = parsed.flags.get("--min-ok").cloned().flatten() {
        Some(
            raw.trim()
                .parse::<usize>()
                .map_err(|_| "--min-ok must be a positive integer".to_owned())?,
        )
    } else {
        None
    };
    let options = RunOptions {
        prompt: if allow_prompt {
            parsed.positionals.join(" ")
        } else {
            String::new()
        },
        agent: parsed.flags.get("--agent").cloned().flatten(),
        mode: None,
        read_only: parsed.flags.contains_key("--read-only"),
        graph: parsed.flags.contains_key("--graph"),
        approve_plan: parsed.flags.contains_key("--approve-plan") || approve_plan_id.is_some(),
        approve_plan_id,
        allow_sensitive: parsed.flags.contains_key("--allow-sensitive"),
        json: parsed.flags.contains_key("--json"),
        model: parsed.flags.get("--model").cloned().flatten(),
        progress: parsed.flags.contains_key("--progress"),
        trust_workspace_config: parsed.flags.contains_key("--trust-workspace-config"),
        agents,
        min_ok,
        judge: parsed.flags.get("--judge").cloned().flatten(),
        write: parsed.flags.contains_key("--write"),
        keep_worktrees: parsed.flags.contains_key("--keep-worktrees"),
    };
    if options.write && options.agents.is_none() {
        return Err("--write requires --agents".to_owned());
    }
    if options.keep_worktrees && !options.write {
        return Err("--keep-worktrees requires --write".to_owned());
    }
    if options.write && options.read_only {
        return Err("--write cannot be used with --read-only".to_owned());
    }
    if options.judge.is_some() && options.agents.is_none() {
        return Err("--judge requires --agents".to_owned());
    }
    if options.agents.is_some() {
        if options.agent.is_some() {
            return Err("--agents cannot be used with --agent".to_owned());
        }
        if options.graph {
            return Err("--agents cannot be used with --graph".to_owned());
        }
        if options.model.is_some() {
            return Err("--agents cannot be used with --model".to_owned());
        }
        if let Some(raw) = options.agents.as_deref() {
            let legs = dmuxai::fanout::parse_agents_flag(raw)?;
            if let Some(min_ok) = options.min_ok {
                dmuxai::fanout::parse_min_ok(Some(&min_ok.to_string()), legs.len())?;
            }
        }
    } else if options.min_ok.is_some() {
        return Err("--min-ok requires --agents".to_owned());
    }
    if let Some(raw) = options.judge.as_deref() {
        dmuxai::fanout::parse_judge_flag(raw)?;
    }
    if options.graph && options.agent.is_some() {
        return Err("use either --agent or --graph, not both".to_owned());
    }
    // FR-2550: the id is interpolated into the pending-plan path, so a
    // malformed id is usage before any other validation or filesystem access.
    if let Some(plan_id) = &options.approve_plan_id
        && !dmuxai::state_path::valid_pending_plan_hash(plan_id)
    {
        return Err("--approve-plan-id must be 16 lowercase hex characters".to_owned());
    }
    if options.approve_plan_id.is_some() && !options.graph {
        return Err("--approve-plan-id needs --graph".to_owned());
    }
    if options.model.is_some() && options.graph {
        return Err("--model cannot be used with --graph".to_owned());
    }
    if options.model.is_some() && options.agent.is_none() {
        return Err("--model needs --agent".to_owned());
    }
    Ok(options)
}

/// Parses `args` (without the program name) whose first element is one of
/// [`SUBCOMMANDS`]. Errors carry the message for a usage failure (exit 2).
pub fn parse(args: &[String]) -> Result<AgentCommand, String> {
    let Some(command) = args.first() else {
        return Err("missing subcommand".to_owned());
    };
    let rest = &args[1..];
    match command.as_str() {
        "run" => {
            let options = parse_run_options(rest, true)?;
            if options.prompt.trim().is_empty() {
                return Err("run needs a prompt (use - to read it from stdin)".to_owned());
            }
            Ok(AgentCommand::Run(options))
        }
        "todo" => {
            let Some(sub) = rest.first() else {
                return Err("todo needs add, list or run".to_owned());
            };
            let rest = &rest[1..];
            match sub.as_str() {
                "add" => {
                    let parsed = parse_flags(rest, TODO_ADD_FLAGS, true)?;
                    let title = parsed.positionals.join(" ");
                    let body = parsed
                        .flags
                        .get("--body")
                        .cloned()
                        .flatten()
                        .unwrap_or_default();
                    let json = parsed.flags.contains_key("--json");
                    if title.trim().is_empty() {
                        return Err("todo add needs a title".to_owned());
                    }
                    Ok(AgentCommand::TodoAdd { title, body, json })
                }
                "list" => {
                    let parsed = parse_flags(rest, TODO_LIST_FLAGS, false)?;
                    let mut status = None;
                    if let Some(raw) = parsed.flags.get("--status").cloned().flatten() {
                        agent_api::parse_todo_status(&raw).map_err(|e| e.message)?;
                        status = Some(raw.to_ascii_lowercase());
                    }
                    let json = parsed.flags.contains_key("--json");
                    Ok(AgentCommand::TodoList { status, json })
                }
                "run" => {
                    let Some(id) = rest.first().filter(|id| !id.starts_with("--")) else {
                        return Err("todo run needs a to-do id".to_owned());
                    };
                    let run = parse_run_options_with_flags(&rest[1..], false, TODO_RUN_FLAGS)?;
                    Ok(AgentCommand::TodoRun {
                        id: id.clone(),
                        run,
                    })
                }
                other => Err(format!("unknown todo subcommand '{other}'")),
            }
        }
        "status" => {
            let parsed = parse_flags(rest, STATUS_FLAGS, false)?;
            Ok(AgentCommand::Status {
                json: parsed.flags.contains_key("--json"),
            })
        }
        "events" => {
            let parsed = parse_flags(rest, EVENTS_FLAGS, false)?;
            let mut since = 0;
            if let Some(raw) = parsed.flags.get("--since").cloned().flatten() {
                since = raw
                    .parse()
                    .map_err(|_| "--since must be a number".to_owned())?;
            }
            let mut limit = None;
            if let Some(raw) = parsed.flags.get("--limit").cloned().flatten() {
                let parsed_limit: usize = raw
                    .parse()
                    .map_err(|_| "--limit must be a number".to_owned())?;
                if parsed_limit == 0 {
                    return Err("--limit must be at least 1".to_owned());
                }
                limit = Some(parsed_limit);
            }
            let follow = parsed.flags.contains_key("--follow") || parsed.flags.contains_key("-f");
            let json = parsed.flags.contains_key("--json");
            Ok(AgentCommand::Events {
                since,
                limit,
                follow,
                json,
            })
        }
        "mcp" => {
            parse_flags(rest, &[], false)?;
            Ok(AgentCommand::Mcp)
        }
        "init" => {
            let parsed = parse_flags(rest, INIT_FLAGS, false)?;
            Ok(AgentCommand::Init {
                workspace: parsed.flags.contains_key("--workspace"),
                force: parsed.flags.contains_key("--force"),
            })
        }
        "trust" => {
            let parsed = parse_flags(rest, TRUST_FLAGS, false)?;
            Ok(AgentCommand::Trust {
                status: parsed.flags.contains_key("--status"),
                json: parsed.flags.contains_key("--json"),
                revoke: parsed.flags.contains_key("--revoke"),
                yes: parsed.flags.contains_key("--yes"),
            })
        }
        "runs" => {
            if rest.first().map(String::as_str) == Some("show") {
                let parsed = parse_flags(&rest[1..], RUNS_SHOW_FLAGS, true)?;
                if parsed.positionals.is_empty() {
                    return Err("runs show requires a run id".to_owned());
                }
                if parsed.positionals.len() > 1 {
                    return Err(format!("unexpected argument '{}'", parsed.positionals[1]));
                }
                let id = parsed.positionals[0].clone();
                if !dmuxai::run_journal::valid_run_id(&id) {
                    return Err(format!("invalid run id '{id}'"));
                }
                return Ok(AgentCommand::Runs {
                    show_id: Some(id),
                    limit: 20,
                    failed: false,
                    json: parsed.flags.contains_key("--json"),
                });
            }

            let parsed = parse_flags(rest, RUNS_FLAGS, false)?;
            let mut limit = 20usize;
            if let Some(raw) = parsed.flags.get("--limit").cloned().flatten() {
                limit = raw
                    .parse()
                    .map_err(|_| "--limit must be a number".to_owned())?;
                if limit == 0 {
                    return Err("--limit must be at least 1".to_owned());
                }
            }
            let failed = parsed.flags.contains_key("--failed");
            let json = parsed.flags.contains_key("--json");

            Ok(AgentCommand::Runs {
                show_id: None,
                limit,
                failed,
                json,
            })
        }
        "runners" => {
            let parsed = parse_flags(rest, RUNNERS_FLAGS, false)?;
            Ok(AgentCommand::Runners {
                json: parsed.flags.contains_key("--json"),
            })
        }
        "models" => {
            let parsed = parse_flags(rest, MODELS_FLAGS, false)?;
            let agent = parsed.flags.get("--agent").cloned().flatten();
            Ok(AgentCommand::Models {
                agent,
                json: parsed.flags.contains_key("--json"),
            })
        }
        "term" => parse_term(rest).map(AgentCommand::Term),
        "board" => parse_board(rest).map(AgentCommand::Board),
        "fanout" => {
            let Some(sub) = rest.first() else {
                return Err("fanout needs clean".to_owned());
            };
            if sub != "clean" {
                return Err(format!("unknown fanout subcommand '{sub}'"));
            }
            let parsed = parse_flags(&rest[1..], FANOUT_CLEAN_FLAGS, true)?;
            let all = parsed.flags.contains_key("--all");
            let run_id = parsed.positionals.first().cloned();
            if all && run_id.is_some() {
                return Err("fanout clean: use either a run id or --all, not both".to_owned());
            }
            if !all && run_id.is_none() {
                return Err("fanout clean needs a run id or --all".to_owned());
            }
            Ok(AgentCommand::FanoutClean { run_id, all })
        }
        other => Err(format!("unknown subcommand '{other}'")),
    }
}

const BOARD_JSON_FLAG: &[FlagSpec] = &[FlagSpec {
    names: &["--json"],
    takes_value: false,
    hidden: false,
}];

fn parse_board(args: &[String]) -> Result<BoardCommand, String> {
    let Some(sub) = args.first() else {
        return Err("board needs a subcommand".to_owned());
    };
    let rest = &args[1..];
    match sub.as_str() {
        "add" => {
            const FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--title"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--summary"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--kind"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--grade"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--lane"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--bot"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--channel"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--ref"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["-C", "--workspace-dir"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--recommend"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--option"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--plan-hash"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--key"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, FLAGS, false)?;
            let title = parsed
                .flags
                .get("--title")
                .and_then(|v| v.clone())
                .ok_or_else(|| "--title is required".to_owned())?;
            let summary = parsed
                .flags
                .get("--summary")
                .cloned()
                .flatten()
                .unwrap_or_default();
            let kind = parsed
                .flags
                .get("--kind")
                .and_then(|v| v.clone())
                .ok_or_else(|| "--kind is required".to_owned())?;
            let options = collect_repeat_flag(rest, "--option");
            Ok(BoardCommand {
                json: parsed.flags.contains_key("--json"),
                op: BoardOp::Add {
                    title,
                    summary,
                    kind,
                    grade: parsed.flags.get("--grade").cloned().flatten(),
                    lane: parsed.flags.get("--lane").cloned().flatten(),
                    bot: parsed.flags.get("--bot").cloned().flatten(),
                    channel: parsed.flags.get("--channel").cloned().flatten(),
                    ref_url: parsed.flags.get("--ref").cloned().flatten(),
                    workspace: parsed
                        .flags
                        .get("-C")
                        .or(parsed.flags.get("--workspace-dir"))
                        .and_then(|v| v.clone())
                        .map(PathBuf::from),
                    recommendation: parsed.flags.get("--recommend").cloned().flatten(),
                    options,
                    plan_hash: parsed.flags.get("--plan-hash").cloned().flatten(),
                    key: parsed.flags.get("--key").cloned().flatten(),
                },
            })
        }
        "list" => {
            const FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--lane"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--workspace"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--all"],
                    takes_value: false,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, FLAGS, false)?;
            let mut lanes = Vec::new();
            for (key, value) in &parsed.flags {
                if key == "--lane"
                    && let Some(l) = value
                {
                    lanes.push(l.clone());
                }
            }
            Ok(BoardCommand {
                json: parsed.flags.contains_key("--json"),
                op: BoardOp::List {
                    lanes,
                    workspace: parsed
                        .flags
                        .get("--workspace")
                        .and_then(|v| v.clone())
                        .map(PathBuf::from),
                    all: parsed.flags.contains_key("--all"),
                },
            })
        }
        "show" => {
            let parsed = parse_flags(rest, BOARD_JSON_FLAG, true)?;
            let id = parsed
                .positionals
                .first()
                .cloned()
                .ok_or_else(|| "board show needs an id".to_owned())?;
            Ok(BoardCommand {
                json: parsed.flags.contains_key("--json"),
                op: BoardOp::Show { id },
            })
        }
        "update" => {
            const FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--title"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--summary"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--recommend"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--option"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--grade"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--kind"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--run-id"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--if-version"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, FLAGS, true)?;
            let id = parsed
                .positionals
                .first()
                .cloned()
                .ok_or_else(|| "board update needs an id".to_owned())?;
            let options = collect_repeat_flag(rest, "--option");
            let if_version = parsed
                .flags
                .get("--if-version")
                .and_then(|v| v.as_deref())
                .map(|raw| raw.parse::<u32>())
                .transpose()
                .map_err(|_| "--if-version must be a number".to_owned())?;
            Ok(BoardCommand {
                json: parsed.flags.contains_key("--json"),
                op: BoardOp::Update {
                    id,
                    title: parsed.flags.get("--title").cloned().flatten(),
                    summary: parsed.flags.get("--summary").cloned().flatten(),
                    recommendation: parsed.flags.get("--recommend").cloned().flatten(),
                    options,
                    replace_options: parsed.flags.contains_key("--option"),
                    grade: parsed.flags.get("--grade").cloned().flatten(),
                    kind: parsed.flags.get("--kind").cloned().flatten(),
                    run_id: parsed.flags.get("--run-id").cloned().flatten(),
                    if_version,
                },
            })
        }
        "move" => {
            const FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--if-version"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, FLAGS, true)?;
            let id = parsed
                .positionals
                .first()
                .cloned()
                .ok_or_else(|| "board move needs an id".to_owned())?;
            let lane = parsed
                .positionals
                .get(1)
                .cloned()
                .ok_or_else(|| "board move needs a lane".to_owned())?;
            let if_version = parsed
                .flags
                .get("--if-version")
                .and_then(|v| v.as_deref())
                .map(|raw| raw.parse::<u32>())
                .transpose()
                .map_err(|_| "--if-version must be a number".to_owned())?;
            Ok(BoardCommand {
                json: parsed.flags.contains_key("--json"),
                op: BoardOp::Move {
                    id,
                    lane,
                    if_version,
                },
            })
        }
        "plan" => {
            const FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--hash"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--edit"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--prompt"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, FLAGS, true)?;
            let id = parsed
                .positionals
                .first()
                .cloned()
                .ok_or_else(|| "board plan needs an id".to_owned())?;
            Ok(BoardCommand {
                json: parsed.flags.contains_key("--json"),
                op: BoardOp::Plan {
                    id,
                    hash: parsed.flags.get("--hash").cloned().flatten(),
                    edit: parsed.flags.get("--edit").cloned().flatten(),
                    prompt: parsed.flags.get("--prompt").cloned().flatten(),
                },
            })
        }
        "decide" => {
            const FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--text"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--option"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--plan-hash"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--by"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, FLAGS, true)?;
            let id = parsed
                .positionals
                .first()
                .cloned()
                .ok_or_else(|| "board decide needs an id".to_owned())?;
            let verdict = parsed
                .positionals
                .get(1)
                .cloned()
                .ok_or_else(|| "board decide needs a verdict".to_owned())?;
            let option = parsed
                .flags
                .get("--option")
                .and_then(|v| v.as_deref())
                .map(|raw| raw.parse::<usize>())
                .transpose()
                .map_err(|_| "--option must be a number".to_owned())?;
            Ok(BoardCommand {
                json: parsed.flags.contains_key("--json"),
                op: BoardOp::Decide {
                    id,
                    verdict,
                    text: parsed.flags.get("--text").cloned().flatten(),
                    option,
                    plan_hash: parsed.flags.get("--plan-hash").cloned().flatten(),
                    by: parsed.flags.get("--by").cloned().flatten(),
                },
            })
        }
        "say" => {
            const FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--text"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--from"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, FLAGS, true)?;
            let id = parsed
                .positionals
                .first()
                .cloned()
                .ok_or_else(|| "board say needs an id".to_owned())?;
            let text = parsed
                .flags
                .get("--text")
                .and_then(|v| v.clone())
                .ok_or_else(|| "--text is required".to_owned())?;
            Ok(BoardCommand {
                json: parsed.flags.contains_key("--json"),
                op: BoardOp::Say {
                    id,
                    text,
                    from: parsed.flags.get("--from").cloned().flatten(),
                },
            })
        }
        "wait" => {
            const FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--since-version"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--timeout"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--for"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--any"],
                    takes_value: false,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--since-seq"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--as"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, FLAGS, true)?;
            let any = parsed.flags.contains_key("--any");
            let id = parsed.positionals.first().cloned();
            if !any && id.is_none() {
                return Err("board wait needs an id or --any".to_owned());
            }
            let since_seq = parsed
                .flags
                .get("--since-seq")
                .and_then(|v| v.as_deref())
                .map(|raw| raw.parse::<u64>())
                .transpose()
                .map_err(|_| "--since-seq must be a number".to_owned())?;
            let since_version = parsed
                .flags
                .get("--since-version")
                .and_then(|v| v.as_deref())
                .map(|raw| raw.parse::<u32>())
                .transpose()
                .map_err(|_| "--since-version must be a number".to_owned())?;
            let timeout_secs = parsed
                .flags
                .get("--timeout")
                .and_then(|v| v.as_deref())
                .map(|raw| raw.parse::<u64>())
                .transpose()
                .map_err(|_| "--timeout must be a number".to_owned())?
                .unwrap_or(600)
                .min(3600);
            let wait_for = parsed
                .flags
                .get("--for")
                .and_then(|v| v.clone())
                .unwrap_or_else(|| "any".to_owned());
            Ok(BoardCommand {
                json: parsed.flags.contains_key("--json"),
                op: BoardOp::Wait {
                    id,
                    any,
                    since_version,
                    since_seq,
                    timeout_secs,
                    wait_for,
                    waiter: parsed.flags.get("--as").cloned().flatten(),
                },
            })
        }
        "events" => {
            const FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--since"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, FLAGS, false)?;
            let since_seq = parsed
                .flags
                .get("--since")
                .and_then(|v| v.as_deref())
                .map(|raw| raw.parse::<u64>())
                .transpose()
                .map_err(|_| "--since must be a number".to_owned())?
                .unwrap_or(0);
            Ok(BoardCommand {
                json: parsed.flags.contains_key("--json"),
                op: BoardOp::Events { since_seq },
            })
        }
        "prompt" => {
            const FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--hash"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, FLAGS, true)?;
            let id = parsed
                .positionals
                .first()
                .cloned()
                .ok_or_else(|| "board prompt needs an id".to_owned())?;
            Ok(BoardCommand {
                json: parsed.flags.contains_key("--json"),
                op: BoardOp::Prompt {
                    id,
                    hash: parsed.flags.get("--hash").cloned().flatten(),
                },
            })
        }
        "rules" => {
            const FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--set"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, FLAGS, false)?;
            Ok(BoardCommand {
                json: parsed.flags.contains_key("--json"),
                op: BoardOp::Rules {
                    set: parsed.flags.get("--set").cloned().flatten(),
                },
            })
        }
        "pending" => {
            const FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--workspace"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, FLAGS, false)?;
            Ok(BoardCommand {
                json: parsed.flags.contains_key("--json"),
                op: BoardOp::Pending {
                    workspace: parsed
                        .flags
                        .get("--workspace")
                        .and_then(|v| v.clone())
                        .map(PathBuf::from),
                },
            })
        }
        "check" => {
            const FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--text"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, FLAGS, true)?;
            let id = parsed
                .positionals
                .first()
                .cloned()
                .ok_or_else(|| "board check needs an id".to_owned())?;
            Ok(BoardCommand {
                json: parsed.flags.contains_key("--json"),
                op: BoardOp::Check {
                    id,
                    text: parsed.flags.get("--text").cloned().flatten(),
                },
            })
        }
        "ask" => {
            const FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--text"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--revise"],
                    takes_value: false,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--agent"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--model"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--by"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, FLAGS, true)?;
            let id = parsed
                .positionals
                .first()
                .cloned()
                .ok_or_else(|| "board ask needs an id".to_owned())?;
            let text = parsed
                .flags
                .get("--text")
                .and_then(|v| v.clone())
                .ok_or_else(|| "--text is required".to_owned())?;
            Ok(BoardCommand {
                json: parsed.flags.contains_key("--json"),
                op: BoardOp::Ask {
                    id,
                    text,
                    revise: parsed.flags.contains_key("--revise"),
                    agent: parsed.flags.get("--agent").cloned().flatten(),
                    model: parsed.flags.get("--model").cloned().flatten(),
                    by: parsed.flags.get("--by").cloned().flatten(),
                },
            })
        }
        "run" => {
            const FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--detach"],
                    takes_value: false,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--by"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--exec-prepared"],
                    takes_value: false,
                    hidden: true,
                },
                FlagSpec {
                    names: &["--plan-hash"],
                    takes_value: true,
                    hidden: true,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, FLAGS, true)?;
            let id = parsed
                .positionals
                .first()
                .cloned()
                .ok_or_else(|| "board run needs an id".to_owned())?;
            Ok(BoardCommand {
                json: parsed.flags.contains_key("--json"),
                op: BoardOp::Run {
                    id,
                    detach: parsed.flags.contains_key("--detach"),
                    by: parsed.flags.get("--by").cloned().flatten(),
                    exec_prepared: parsed.flags.contains_key("--exec-prepared"),
                    plan_hash: parsed.flags.get("--plan-hash").cloned().flatten(),
                },
            })
        }
        other => Err(format!("unknown board subcommand '{other}'")),
    }
}

fn collect_repeat_flag(args: &[String], name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == name {
            index += 1;
            if let Some(value) = args.get(index)
                && !value.starts_with("--")
            {
                out.push(value.clone());
                index += 1;
            }
            continue;
        }
        index += 1;
    }
    out
}

fn parse_term(args: &[String]) -> Result<TermCommand, String> {
    let Some(subcommand) = args.first() else {
        return Err(
            "term needs a subcommand: open, send, read, wait, exec, approve, list, show, resize, close or reopen"
                .to_owned(),
        );
    };
    let rest = &args[1..];
    match subcommand.as_str() {
        "open" => parse_term_open(rest),
        "send" => {
            const SEND_FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--keys"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--no-newline"],
                    takes_value: false,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--secret"],
                    takes_value: false,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, SEND_FLAGS, true)?;
            let json = parsed.flags.contains_key("--json");
            let Some(name) = parsed.positionals.first() else {
                return Err("term send needs a session name".to_owned());
            };
            term::validate_name(name).map_err(|error| error.message)?;
            let keys = parsed.flags.get("--keys").cloned().flatten();
            let text = parsed.positionals[1..].join(" ");
            if keys.is_some() {
                if !text.is_empty() {
                    return Err("term send takes text or --keys, not both".to_owned());
                }
                let keys = keys.unwrap_or_default();
                term::parse_key_list(&keys).map_err(|error| error.message)?;
                return Ok(TermCommand {
                    op: TermOp::SendKeys {
                        name: name.clone(),
                        keys,
                    },
                    json,
                });
            }
            if parsed.positionals.len() < 2 {
                return Err("term send needs text or --keys".to_owned());
            }
            Ok(TermCommand {
                op: TermOp::Send {
                    name: name.clone(),
                    text,
                    newline: !parsed.flags.contains_key("--no-newline"),
                    secret: parsed.flags.contains_key("--secret"),
                },
                json,
            })
        }
        "read" => {
            const READ_FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--since"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--tail"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--raw"],
                    takes_value: false,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, READ_FLAGS, true)?;
            let name = single_term_name(&parsed, "term read")?;
            let since = optional_u64_flag(&parsed, "--since", "--since")?.unwrap_or(0);
            let tail = optional_usize_flag(&parsed, "--tail", "--tail")?;
            Ok(TermCommand {
                op: TermOp::Read {
                    name,
                    since,
                    tail,
                    raw: parsed.flags.contains_key("--raw"),
                },
                json: parsed.flags.contains_key("--json"),
            })
        }
        "wait" => {
            const WAIT_FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--match"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--idle"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--timeout"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--since"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, WAIT_FLAGS, true)?;
            let name = single_term_name(&parsed, "term wait")?;
            let regex = parsed.flags.get("--match").cloned().flatten();
            let idle_ms = idle_duration_flag(&parsed, "--idle", "--idle")?;
            if regex.is_some() == idle_ms.is_some() {
                return Err("term wait needs exactly one of --match or --idle".to_owned());
            }
            if let Some(pattern) = &regex {
                regex::Regex::new(pattern)
                    .map_err(|error| format!("invalid regex '{pattern}': {error}"))?;
            }
            let timeout_ms = duration_flag(&parsed, "--timeout", "--timeout")?
                .ok_or_else(|| "term wait needs --timeout".to_owned())?;
            let since = optional_u64_flag(&parsed, "--since", "--since")?;
            Ok(TermCommand {
                op: TermOp::Wait {
                    name,
                    regex,
                    idle_ms,
                    timeout_ms,
                    since,
                },
                json: parsed.flags.contains_key("--json"),
            })
        }
        "exec" => {
            const EXEC_FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--timeout"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, EXEC_FLAGS, true)?;
            let json = parsed.flags.contains_key("--json");
            let Some(name) = parsed.positionals.first() else {
                return Err("term exec needs a session name".to_owned());
            };
            term::validate_name(name).map_err(|error| error.message)?;
            if parsed.positionals.len() < 2 {
                return Err("term exec needs a command".to_owned());
            }
            let command = parsed.positionals[1..].join(" ");
            let timeout_ms = duration_flag(&parsed, "--timeout", "--timeout")?;
            Ok(TermCommand {
                op: TermOp::Exec {
                    name: name.clone(),
                    command,
                    timeout_ms,
                },
                json,
            })
        }
        "approve" => {
            const APPROVE_FLAGS: &[FlagSpec] = &[FlagSpec {
                names: &["--json"],
                takes_value: false,
                hidden: false,
            }];
            let parsed = parse_flags(rest, APPROVE_FLAGS, true)?;
            if parsed.positionals.is_empty() {
                return Err("term approve needs an approval id".to_owned());
            }
            if parsed.positionals.len() > 1 {
                return Err(format!("unexpected argument '{}'", parsed.positionals[1]));
            }
            let id = parsed.positionals[0].clone();
            if !valid_approval_id(&id) {
                return Err(format!(
                    "invalid approval id '{id}' (must be 16 lowercase hex characters)"
                ));
            }
            Ok(TermCommand {
                op: TermOp::Approve { id },
                json: parsed.flags.contains_key("--json"),
            })
        }
        "list" => {
            const LIST_FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--all"],
                    takes_value: false,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, LIST_FLAGS, false)?;
            Ok(TermCommand {
                op: TermOp::List {
                    all: parsed.flags.contains_key("--all"),
                },
                json: parsed.flags.contains_key("--json"),
            })
        }
        "show" => {
            const SHOW_FLAGS: &[FlagSpec] = &[FlagSpec {
                names: &["--json"],
                takes_value: false,
                hidden: false,
            }];
            let parsed = parse_flags(rest, SHOW_FLAGS, true)?;
            let name = single_term_name(&parsed, "term show")?;
            Ok(TermCommand {
                op: TermOp::Show { name },
                json: parsed.flags.contains_key("--json"),
            })
        }
        "resize" => {
            const RESIZE_FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--cols"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--rows"],
                    takes_value: true,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, RESIZE_FLAGS, true)?;
            let name = single_term_name(&parsed, "term resize")?;
            let cols = required_u16_flag(&parsed, "--cols", "--cols")?;
            let rows = required_u16_flag(&parsed, "--rows", "--rows")?;
            term::validate_dimensions(cols, rows).map_err(|error| error.message)?;
            Ok(TermCommand {
                op: TermOp::Resize { name, cols, rows },
                json: parsed.flags.contains_key("--json"),
            })
        }
        "close" => {
            const CLOSE_FLAGS: &[FlagSpec] = &[
                FlagSpec {
                    names: &["--force"],
                    takes_value: false,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--all"],
                    takes_value: false,
                    hidden: false,
                },
                FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                },
            ];
            let parsed = parse_flags(rest, CLOSE_FLAGS, true)?;
            if parsed.positionals.len() > 1 {
                return Err(format!("unexpected argument '{}'", parsed.positionals[1]));
            }
            let all = parsed.flags.contains_key("--all");
            let name = parsed.positionals.first().cloned();
            if all && name.is_some() {
                return Err("term close takes a session name or --all, not both".to_owned());
            }
            if !all && name.is_none() {
                return Err("term close needs a session name or --all".to_owned());
            }
            if let Some(name) = &name {
                term::validate_name(name).map_err(|error| error.message)?;
            }
            Ok(TermCommand {
                op: TermOp::Close {
                    name,
                    force: parsed.flags.contains_key("--force"),
                    all,
                },
                json: parsed.flags.contains_key("--json"),
            })
        }
        "reopen" => {
            const REOPEN_FLAGS: &[FlagSpec] = &[FlagSpec {
                names: &["--json"],
                takes_value: false,
                hidden: false,
            }];
            let parsed = parse_flags(rest, REOPEN_FLAGS, true)?;
            let name = single_term_name(&parsed, "term reopen")?;
            Ok(TermCommand {
                op: TermOp::Reopen { name },
                json: parsed.flags.contains_key("--json"),
            })
        }
        "__host" => {
            if let Some(argument) = rest.first() {
                return Err(format!("unexpected argument '{argument}'"));
            }
            Ok(TermCommand {
                op: TermOp::Host,
                json: false,
            })
        }
        other => Err(format!("unknown term subcommand '{other}'")),
    }
}

fn valid_approval_id(id: &str) -> bool {
    id.len() == 16
        && id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

fn parse_term_open(args: &[String]) -> Result<TermCommand, String> {
    let mut name = None;
    let mut kind = None;
    let mut cwd = None;
    let mut env = Vec::new();
    let mut cols = term::DEFAULT_COLS;
    let mut rows = term::DEFAULT_ROWS;
    let mut json = false;
    let mut argv = Vec::new();
    let mut after_separator = false;
    let mut serial_device = None;
    let mut baud = None;
    let mut parity = term::SerialParity::None;
    let mut data = 8u8;
    let mut stop = 1u8;
    let mut flow = term::SerialFlow::None;
    let mut usbipd_busid = None;
    let mut no_usbipd_attach = false;
    let mut wait_for = None;
    let mut timeout_ms = None;
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if after_separator {
            argv.push(argument.clone());
            index += 1;
            continue;
        }
        match argument.as_str() {
            "--" => after_separator = true,
            "--json" => json = true,
            "--kind" => {
                index += 1;
                kind = Some(match args.get(index).map(String::as_str) {
                    Some("shell") => SessionKind::Shell,
                    Some("cmd") => SessionKind::Cmd,
                    Some("serial") => SessionKind::Serial,
                    _ => return Err("--kind must be 'shell', 'cmd' or 'serial'".to_owned()),
                });
            }
            "--cwd" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--cwd needs a value".to_owned())?;
                cwd = Some(PathBuf::from(value));
            }
            "--env" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--env needs K=V".to_owned())?;
                let (key, value) = value
                    .split_once('=')
                    .ok_or_else(|| "--env needs K=V".to_owned())?;
                if key.is_empty() {
                    return Err("--env needs a non-empty name before '='".to_owned());
                }
                env.push((key.to_owned(), value.to_owned()));
            }
            "--cols" => {
                index += 1;
                cols = parse_u16_value(args.get(index), "--cols")?;
            }
            "--rows" => {
                index += 1;
                rows = parse_u16_value(args.get(index), "--rows")?;
            }
            "--serial" => {
                index += 1;
                serial_device = Some(
                    args.get(index)
                        .ok_or_else(|| "--serial needs a device path".to_owned())?
                        .clone(),
                );
            }
            "--baud" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--baud needs a value".to_owned())?;
                baud = Some(
                    value
                        .parse::<u32>()
                        .map_err(|_| "--baud must be a positive integer".to_owned())?,
                );
            }
            "--parity" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--parity needs a value".to_owned())?;
                parity = value.parse().map_err(|error: String| error)?;
            }
            "--data" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--data needs a value".to_owned())?;
                data = value
                    .parse::<u8>()
                    .map_err(|_| "--data must be 5, 6, 7 or 8".to_owned())?;
            }
            "--stop" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--stop needs a value".to_owned())?;
                stop = value
                    .parse::<u8>()
                    .map_err(|_| "--stop must be 1 or 2".to_owned())?;
            }
            "--flow" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--flow needs a value".to_owned())?;
                flow = value.parse().map_err(|error: String| error)?;
            }
            "--usbipd-busid" => {
                index += 1;
                usbipd_busid = Some(
                    args.get(index)
                        .ok_or_else(|| "--usbipd-busid needs a value".to_owned())?
                        .clone(),
                );
            }
            "--no-usbipd-attach" => no_usbipd_attach = true,
            "--wait-for" => {
                index += 1;
                wait_for = Some(
                    args.get(index)
                        .ok_or_else(|| "--wait-for needs a regex".to_owned())?
                        .clone(),
                );
            }
            "--timeout" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--timeout needs a value".to_owned())?;
                timeout_ms = Some(parse_duration_ms(value, "--timeout")?);
            }
            other if other.starts_with('-') => {
                return Err(format!("unexpected argument '{other}'"));
            }
            other => {
                if name.is_some() {
                    return Err(format!("unexpected argument '{other}'"));
                }
                name = Some(other.to_owned());
            }
        }
        index += 1;
    }
    let name = name.ok_or_else(|| "term open needs a session name".to_owned())?;
    term::validate_name(&name).map_err(|error| error.message)?;
    term::validate_dimensions(cols, rows).map_err(|error| error.message)?;

    let serial = match serial_device {
        Some(device) => {
            if matches!(kind, Some(kind) if kind != SessionKind::Serial) {
                return Err(
                    "--serial cannot be combined with --kind shell or --kind cmd".to_owned(),
                );
            }
            let baud = baud.ok_or_else(|| "term open --serial needs --baud".to_owned())?;
            let mut config = term::SerialConfig::new(device, baud);
            config.parity = parity;
            config.data = data;
            config.stop = stop;
            config.flow = flow;
            config.usbipd_busid = usbipd_busid.filter(|value| !value.trim().is_empty());
            // Deprecated flag: still carried in the open request so a warm
            // host's saved session spec keeps its shape; it no longer changes
            // any behavior there.
            config.no_usbipd_attach = no_usbipd_attach;
            config.validate().map_err(|error| error.message)?;
            Some(config)
        }
        None => {
            if kind == Some(SessionKind::Serial) {
                return Err("--kind serial needs --serial DEV".to_owned());
            }
            if baud.is_some() {
                return Err("--baud needs --serial".to_owned());
            }
            if flow != term::SerialFlow::None
                || data != 8
                || stop != 1
                || usbipd_busid.is_some()
                || no_usbipd_attach
            {
                return Err("serial settings need --serial".to_owned());
            }
            None
        }
    };
    let kind = if serial.is_some() {
        SessionKind::Serial
    } else {
        kind.unwrap_or(SessionKind::Shell)
    };
    if kind == SessionKind::Cmd && argv.is_empty() {
        return Err("--kind cmd needs a command after --".to_owned());
    }
    if kind == SessionKind::Serial && !argv.is_empty() {
        return Err("serial sessions do not take a command after --".to_owned());
    }
    if kind == SessionKind::Shell && !argv.is_empty() {
        return Err("shell sessions do not take a command after --".to_owned());
    }
    if let Some(pattern) = &wait_for {
        regex::Regex::new(pattern)
            .map_err(|error| format!("invalid regex '{pattern}': {error}"))?;
        if timeout_ms.is_none() {
            return Err("--wait-for needs --timeout".to_owned());
        }
    } else if timeout_ms.is_some() {
        return Err("--timeout needs --wait-for".to_owned());
    }
    if no_usbipd_attach {
        // Deprecated: the flag parses and travels with the request, but dmuxai
        // never runs usbipd attach from WSL, so it has no effect.
        eprintln!(
            "warning: --no-usbipd-attach is deprecated and has no effect; dmuxai never runs usbipd attach from WSL"
        );
    }
    Ok(TermCommand {
        op: TermOp::Open {
            name,
            kind,
            cwd,
            env,
            cols,
            rows,
            argv,
            serial,
            no_usbipd_attach,
            wait_for,
            timeout_ms,
        },
        json,
    })
}

fn single_term_name(parsed: &ParsedFlags, command: &str) -> Result<String, String> {
    let Some(name) = parsed.positionals.first() else {
        return Err(format!("{command} needs a session name"));
    };
    if parsed.positionals.len() > 1 {
        return Err(format!("unexpected argument '{}'", parsed.positionals[1]));
    }
    term::validate_name(name).map_err(|error| error.message)?;
    Ok(name.clone())
}

fn required_u16_flag(parsed: &ParsedFlags, name: &str, label: &str) -> Result<u16, String> {
    let raw = parsed
        .flags
        .get(name)
        .cloned()
        .flatten()
        .ok_or_else(|| format!("{label} needs a value"))?;
    parse_u16_value(Some(&raw), label)
}

fn parse_u16_value(raw: Option<&String>, label: &str) -> Result<u16, String> {
    let raw = raw.ok_or_else(|| format!("{label} needs a value"))?;
    raw.parse::<u16>()
        .map_err(|_| format!("{label} must be a number between 1 and 1000"))
}

fn optional_u64_flag(parsed: &ParsedFlags, name: &str, label: &str) -> Result<Option<u64>, String> {
    parsed
        .flags
        .get(name)
        .cloned()
        .flatten()
        .map(|raw| {
            raw.parse::<u64>()
                .map_err(|_| format!("{label} must be a non-negative integer"))
        })
        .transpose()
}

fn optional_usize_flag(
    parsed: &ParsedFlags,
    name: &str,
    label: &str,
) -> Result<Option<usize>, String> {
    parsed
        .flags
        .get(name)
        .cloned()
        .flatten()
        .map(|raw| {
            raw.parse::<usize>()
                .map_err(|_| format!("{label} must be a non-negative integer"))
        })
        .transpose()
}

fn duration_flag(parsed: &ParsedFlags, name: &str, label: &str) -> Result<Option<u64>, String> {
    duration_flag_with_unitless(parsed, name, label, 1_000.0)
}

fn idle_duration_flag(
    parsed: &ParsedFlags,
    name: &str,
    label: &str,
) -> Result<Option<u64>, String> {
    duration_flag_with_unitless(parsed, name, label, 1.0)
}

fn duration_flag_with_unitless(
    parsed: &ParsedFlags,
    name: &str,
    label: &str,
    unitless_multiplier: f64,
) -> Result<Option<u64>, String> {
    parsed
        .flags
        .get(name)
        .cloned()
        .flatten()
        .map(|raw| parse_duration_ms_with_unitless(&raw, label, unitless_multiplier))
        .transpose()
}

fn parse_duration_ms(raw: &str, label: &str) -> Result<u64, String> {
    parse_duration_ms_with_unitless(raw, label, 1_000.0)
}

fn parse_duration_ms_with_unitless(
    raw: &str,
    label: &str,
    unitless_multiplier: f64,
) -> Result<u64, String> {
    let trimmed = raw.trim();
    let (number, multiplier) = if let Some(number) = trimmed.strip_suffix("ms") {
        (number, 1.0)
    } else if let Some(number) = trimmed.strip_suffix('s') {
        (number, 1_000.0)
    } else if let Some(number) = trimmed.strip_suffix('m') {
        (number, 60_000.0)
    } else if let Some(number) = trimmed.strip_suffix('h') {
        (number, 3_600_000.0)
    } else {
        (trimmed, unitless_multiplier)
    };
    let value = number
        .parse::<f64>()
        .map_err(|_| format!("{label} must be a duration like 500ms, 2s or 1m"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!("{label} must be a non-negative duration"));
    }
    let milliseconds = value * multiplier;
    if milliseconds > u64::MAX as f64 {
        return Err(format!("{label} is too large"));
    }
    Ok(milliseconds.round() as u64)
}

/// Builds the per-workspace event sink (FR-768, FR-770).
pub fn build_event_sink(workspace_root: &Path, settings: &DmuxSettings) -> Arc<EventSink> {
    let label = dmuxai::workspace::normalize_workspace_path(workspace_root)
        .unwrap_or_else(|_| workspace_root.display().to_string());
    let notifier = dmuxai::notify::Notifier::spawn(&settings.notify, workspace_root);
    let sink = EventSink::new(
        EventLog::open(workspace_root, label),
        settings.notify.stall_after(),
        notifier,
    );
    sink.spawn_ticker();
    sink
}

fn print_json(value: &Value) {
    let mut stdout = io::stdout().lock();
    let _ = writeln!(stdout, "{value}");
    let _ = stdout.flush();
}

/// FR-3102: reject malformed ids, enums, text and option indexes before the
/// board store is opened (which creates `<state base>/board/`).
fn prevalidate_board_command(command: &BoardCommand) -> Result<(), ApiError> {
    use dmuxai::board::{
        Grade, Kind, Lane, OPTIONS_MAX, TEXT_MAX, Verdict, valid_item_id, validate_message,
        validate_new_text, validate_options, validate_ref_url, validate_text_limit,
    };

    fn usage(message: impl Into<String>) -> ApiError {
        ApiError::new(exit::USAGE, message.into())
    }
    fn check_id(id: &str) -> Result<(), ApiError> {
        if valid_item_id(id) {
            Ok(())
        } else {
            Err(usage("invalid board item id"))
        }
    }
    fn check_hash(hash: &str) -> Result<(), ApiError> {
        if dmuxai::state_path::valid_pending_plan_hash(hash) {
            Ok(())
        } else {
            Err(usage("plan hash must be 16 lowercase hex characters"))
        }
    }

    match &command.op {
        BoardOp::Add {
            title,
            summary,
            kind,
            grade,
            lane,
            ref_url,
            recommendation,
            options,
            plan_hash,
            ..
        } => {
            if Kind::parse(kind).is_none() {
                return Err(usage("invalid --kind"));
            }
            if let Some(grade) = grade
                && Grade::parse(grade).is_none()
            {
                return Err(usage("invalid --grade"));
            }
            if let Some(lane) = lane
                && Lane::parse(lane).is_none()
            {
                return Err(usage("invalid --lane"));
            }
            let summary = if summary.trim() == "-" { "" } else { summary };
            validate_new_text(title, summary, recommendation.as_deref()).map_err(ApiError::from)?;
            validate_options(options).map_err(ApiError::from)?;
            validate_ref_url(ref_url.as_deref()).map_err(ApiError::from)?;
            if let Some(hash) = plan_hash {
                check_hash(hash)?;
            }
        }
        BoardOp::List { lanes, .. } => {
            for lane in lanes {
                if Lane::parse(lane).is_none() {
                    return Err(usage(format!("invalid lane '{lane}'")));
                }
            }
        }
        BoardOp::Show { id } => check_id(id)?,
        BoardOp::Update {
            id,
            title,
            summary,
            recommendation,
            options,
            replace_options,
            grade,
            kind,
            ..
        } => {
            check_id(id)?;
            if let Some(title) = title {
                validate_new_text(title, "", None).map_err(ApiError::from)?;
            }
            if let Some(summary) = summary
                && summary.trim() != "-"
            {
                validate_text_limit("summary", summary, TEXT_MAX).map_err(ApiError::from)?;
            }
            if let Some(recommendation) = recommendation {
                validate_text_limit("recommendation", recommendation, TEXT_MAX)
                    .map_err(ApiError::from)?;
            }
            if *replace_options {
                validate_options(options).map_err(ApiError::from)?;
            }
            if let Some(grade) = grade
                && !grade.is_empty()
                && Grade::parse(grade).is_none()
            {
                return Err(usage("invalid --grade"));
            }
            if let Some(kind) = kind
                && Kind::parse(kind).is_none()
            {
                return Err(usage("invalid --kind"));
            }
        }
        BoardOp::Move { id, lane, .. } => {
            check_id(id)?;
            if Lane::parse(lane).is_none() {
                return Err(usage("invalid lane"));
            }
        }
        BoardOp::Plan { id, hash, .. } => {
            check_id(id)?;
            if let Some(hash) = hash {
                check_hash(hash)?;
            }
        }
        BoardOp::Decide {
            id,
            verdict,
            text,
            option,
            plan_hash,
            ..
        } => {
            check_id(id)?;
            let verdict = Verdict::parse(verdict).ok_or_else(|| usage("invalid verdict"))?;
            match verdict {
                Verdict::RequestChanges => {
                    let text = text
                        .as_deref()
                        .filter(|t| !t.trim().is_empty())
                        .ok_or_else(|| usage("request_changes needs --text"))?;
                    validate_message(text).map_err(ApiError::from)?;
                }
                Verdict::Answer => {
                    if text.as_deref().is_none_or(|t| t.trim().is_empty()) && option.is_none() {
                        return Err(usage("answer needs --text or --option"));
                    }
                    if let Some(text) = text.as_deref() {
                        validate_message(text).map_err(ApiError::from)?;
                    }
                    if let Some(index) = option
                        && *index >= OPTIONS_MAX
                    {
                        return Err(usage("option index out of range"));
                    }
                }
                Verdict::Approve | Verdict::Reject => {}
            }
            if let Some(hash) = plan_hash {
                check_hash(hash)?;
            }
        }
        BoardOp::Say { id, text, .. } => {
            check_id(id)?;
            if text.trim() != "-" {
                validate_message(text).map_err(ApiError::from)?;
            }
        }
        BoardOp::Events { .. } => {}
        BoardOp::Prompt { id, hash } => {
            check_id(id)?;
            if let Some(hash) = hash {
                check_hash(hash)?;
            }
        }
        BoardOp::Wait {
            id, any, wait_for, ..
        } => {
            match wait_for.as_str() {
                "decision" | "message" | "any" => {}
                other => return Err(usage(format!("invalid --for '{other}'"))),
            }
            if !any {
                let id = id
                    .as_deref()
                    .ok_or_else(|| usage("board wait needs an id"))?;
                check_id(id)?;
            }
        }
        BoardOp::Rules { .. } => {}
        BoardOp::Ask { id, text, .. } => {
            check_id(id)?;
            if text.trim() != "-" {
                validate_message(text).map_err(ApiError::from)?;
            }
        }
        BoardOp::Run { id, plan_hash, .. } => {
            check_id(id)?;
            if let Some(hash) = plan_hash {
                check_hash(hash)?;
            }
        }
        BoardOp::Pending { .. } => {}
        BoardOp::Check { id, text } => {
            check_id(id)?;
            if let Some(t) = text {
                validate_text_limit("text", t, TEXT_MAX).map_err(ApiError::from)?;
            }
        }
    }
    Ok(())
}

fn prompt_tty_confirmation(prompt: &str) -> Result<String, dmuxai::board::BoardError> {
    use dmuxai::board::BoardError;
    use std::io::{BufRead, BufReader, Write};
    let mut tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|e| {
            BoardError::Forbidden(format!("cannot open /dev/tty for confirmation: {e}"))
        })?;
    write!(tty, "{prompt} ")
        .and_then(|_| tty.flush())
        .map_err(|e| BoardError::Forbidden(format!("failed to write /dev/tty: {e}")))?;
    let mut line = String::new();
    BufReader::new(tty)
        .read_line(&mut line)
        .map_err(|e| BoardError::Forbidden(format!("failed to read /dev/tty: {e}")))?;
    Ok(line.trim().to_owned())
}

fn confirm_owner_action_cli(id: &str, verb: &str, title: &str) -> Result<(), ApiError> {
    dmuxai::board::confirm_owner_action(id, verb, title, prompt_tty_confirmation)
        .map_err(ApiError::from)
}

fn run_board_move(
    store: &dmuxai::board::BoardStore,
    id: &str,
    lane: dmuxai::board::Lane,
    by: &str,
    if_version: Option<u32>,
    confirm: impl FnOnce(&str, &str, &str) -> Result<(), ApiError>,
) -> Result<dmuxai::board::BoardItem, ApiError> {
    if by == "planner" {
        return Err(ApiError::new(
            exit::FORBIDDEN,
            "planner cannot move items".to_owned(),
        ));
    }
    if lane == dmuxai::board::Lane::Auto && by == "owner" {
        let existing = store.get(id).map_err(ApiError::from)?;
        confirm(id, "move to auto", &existing.title)?;
    }
    store
        .move_lane(id, lane, by, if_version)
        .map_err(ApiError::from)
}

fn format_pending_age(age_ms: u64) -> String {
    let mins = age_ms / 60_000;
    if mins < 60 {
        return format!("{mins}m");
    }
    let hrs = mins / 60;
    if hrs < 48 {
        return format!("{hrs}h");
    }
    format!("{}d", hrs / 24)
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    text.chars().take(max).collect()
}

fn execute_board(workspace_root: &Path, command: BoardCommand) -> i32 {
    use std::io::IsTerminal;

    let is_tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let caller = if is_tty {
        dmuxai::board::BoardCaller::CliTty
    } else {
        dmuxai::board::BoardCaller::CliNonTty
    };
    execute_board_with_caller(workspace_root, command, caller)
}

fn execute_board_with_caller(
    workspace_root: &Path,
    command: BoardCommand,
    caller: dmuxai::board::BoardCaller,
) -> i32 {
    use dmuxai::board::{
        AddItemRequest, BoardCaller, DecideRequest, Grade, Kind, Lane, SayRequest,
        UpdateItemRequest, Verdict, WaitFilter, WaitFor, WaitOutcome, resolve_actor,
        wait_ready_json,
    };
    use dmuxai::board::{RulesStore, compile_rules_from_set_text, edit_plan_from_json};

    let json_out = command.json;
    let state_root = match dmuxai::state_path::state_base() {
        Ok(path) => path,
        Err(error) => {
            return fail(ApiError::new(exit::INTERNAL, error.to_string()), json_out);
        }
    };

    let result: Result<(), ApiError> = (|| {
        prevalidate_board_command(&command)?;
        let store = agent_api::open_board_store()?;
        match command.op {
            BoardOp::Add {
                title,
                summary,
                kind,
                grade,
                lane,
                bot,
                channel,
                ref_url,
                workspace,
                recommendation,
                options,
                plan_hash,
                key,
            } => {
                let kind = Kind::parse(&kind)
                    .ok_or_else(|| ApiError::new(exit::USAGE, "invalid --kind".to_owned()))?;
                let grade =
                    match grade.as_deref() {
                        None => None,
                        Some(g) => Some(Grade::parse(g).ok_or_else(|| {
                            ApiError::new(exit::USAGE, "invalid --grade".to_owned())
                        })?),
                    };
                let lane =
                    match lane.as_deref() {
                        None => None,
                        Some(l) => Some(Lane::parse(l).ok_or_else(|| {
                            ApiError::new(exit::USAGE, "invalid --lane".to_owned())
                        })?),
                    };
                let summary = if summary.trim() == "-" {
                    read_prompt(&summary)?
                } else if summary.is_empty() {
                    String::new()
                } else {
                    summary
                };
                let ws = workspace.unwrap_or_else(|| workspace_root.to_path_buf());
                let by = resolve_actor(None, caller).map_err(ApiError::from)?;
                if lane == Some(Lane::Auto) && by == "owner" {
                    confirm_owner_action_cli("auto", "add to auto lane", &title)?;
                }
                let item = store
                    .add(AddItemRequest {
                        title,
                        summary,
                        kind,
                        grade,
                        lane,
                        source_bot: bot,
                        source_channel: channel,
                        source_ref: ref_url,
                        workspace: Some(ws),
                        recommendation,
                        options,
                        plan_hash,
                        key,
                        by,
                        state_root: state_root.clone(),
                    })
                    .map_err(ApiError::from)?;
                if json_out {
                    print_json(&serde_json::to_value(&item).unwrap_or(Value::Null));
                } else {
                    println!("{}", item.id);
                }
                Ok(())
            }
            BoardOp::List {
                lanes,
                workspace,
                all,
            } => {
                let lanes: Result<Vec<Lane>, ApiError> = lanes
                    .iter()
                    .map(|l| {
                        Lane::parse(l).ok_or_else(|| {
                            ApiError::new(exit::USAGE, format!("invalid lane '{l}'"))
                        })
                    })
                    .collect();
                let lanes = lanes?;
                let items = store
                    .list(&dmuxai::board::ListFilter {
                        lanes,
                        workspace,
                        include_all: all,
                    })
                    .map_err(ApiError::from)?;
                if json_out {
                    print_json(&serde_json::to_value(&items).unwrap_or(Value::Null));
                } else {
                    for item in items {
                        println!(
                            "{}\t{}\t{}\t{}",
                            item.id,
                            item.lane.as_str(),
                            item.kind.as_str(),
                            item.title
                        );
                    }
                }
                Ok(())
            }
            BoardOp::Show { id } => {
                let item = store.get(&id).map_err(ApiError::from)?;
                if json_out {
                    print_json(&serde_json::to_value(&item).unwrap_or(Value::Null));
                } else {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&item).unwrap_or_default()
                    );
                }
                Ok(())
            }
            BoardOp::Update {
                id,
                title,
                summary,
                recommendation,
                options,
                replace_options,
                grade,
                kind,
                run_id,
                if_version,
            } => {
                let by = resolve_actor(None, caller).map_err(ApiError::from)?;
                let summary = summary.map(|s| read_prompt(&s)).transpose()?;
                let grade = grade
                    .map(|g| {
                        if g.is_empty() {
                            Ok(None)
                        } else {
                            Grade::parse(&g)
                                .ok_or_else(|| {
                                    ApiError::new(exit::USAGE, "invalid --grade".to_owned())
                                })
                                .map(Some)
                        }
                    })
                    .transpose()?;
                let kind = kind
                    .map(|k| {
                        Kind::parse(&k)
                            .ok_or_else(|| ApiError::new(exit::USAGE, "invalid --kind".to_owned()))
                    })
                    .transpose()?;
                let item = store
                    .update(
                        &id,
                        UpdateItemRequest {
                            title,
                            summary,
                            recommendation,
                            options: if replace_options { Some(options) } else { None },
                            grade,
                            kind,
                            run_id,
                            if_version,
                            by,
                        },
                    )
                    .map_err(ApiError::from)?;
                if json_out {
                    print_json(&serde_json::to_value(&item).unwrap_or(Value::Null));
                }
                Ok(())
            }
            BoardOp::Move {
                id,
                lane,
                if_version,
            } => {
                dmuxai::board::validate_item_id_early(&id).map_err(ApiError::from)?;
                let lane = Lane::parse(&lane)
                    .ok_or_else(|| ApiError::new(exit::USAGE, "invalid lane".to_owned()))?;
                let by = resolve_actor(None, caller).map_err(ApiError::from)?;
                let item =
                    run_board_move(&store, &id, lane, &by, if_version, confirm_owner_action_cli)?;
                if json_out {
                    print_json(&serde_json::to_value(&item).unwrap_or(Value::Null));
                }
                Ok(())
            }
            BoardOp::Plan {
                id,
                hash,
                edit,
                prompt,
            } => {
                dmuxai::board::validate_item_id_early(&id).map_err(ApiError::from)?;
                let by = resolve_actor(None, caller).map_err(ApiError::from)?;
                let item = match (hash, edit) {
                    (Some(hash), _) => store
                        .attach_plan(&id, &hash, &state_root, &by)
                        .map_err(ApiError::from)?,
                    (None, Some(edit)) => {
                        let text =
                            dmuxai::board::read_path_or_stdin(&edit).map_err(ApiError::from)?;
                        let item = store.get(&id).map_err(ApiError::from)?;
                        let ws = item
                            .workspace
                            .path
                            .as_deref()
                            .map(PathBuf::from)
                            .unwrap_or_else(|| workspace_root.to_path_buf());
                        let stored_prompt = if let Some(prompt_path) = prompt {
                            dmuxai::board::read_path_or_stdin(&prompt_path)
                                .map_err(ApiError::from)?
                        } else {
                            dmuxai::board::unmasked_prompt_from_item_plan(
                                &state_root,
                                store.base(),
                                item.plan.as_ref(),
                            )
                            .map_err(ApiError::from)?
                            .unwrap_or_default()
                        };
                        if stored_prompt.trim().is_empty() {
                            return Err(ApiError::new(
                                exit::USAGE,
                                "board plan --edit needs --prompt when the item has no plan"
                                    .to_owned(),
                            ));
                        }
                        let inherit = item.plan.as_ref().map(|p| p.tasks.clone());
                        let (_pending, hash) =
                            edit_plan_from_json(&ws, &state_root, &stored_prompt, &text, inherit)
                                .map_err(ApiError::from)?;
                        store
                            .edit_plan(&id, &hash, &ws, &state_root, &by)
                            .map_err(ApiError::from)?
                    }
                    _ => {
                        return Err(ApiError::new(
                            exit::USAGE,
                            "board plan needs --hash or --edit".to_owned(),
                        ));
                    }
                };
                if json_out {
                    print_json(&serde_json::to_value(&item).unwrap_or(Value::Null));
                }
                Ok(())
            }
            BoardOp::Decide {
                id,
                verdict,
                text,
                option,
                plan_hash,
                by,
            } => {
                let verdict = Verdict::parse(&verdict)
                    .ok_or_else(|| ApiError::new(exit::USAGE, "invalid verdict".to_owned()))?;
                dmuxai::board::validate_item_id_early(&id).map_err(ApiError::from)?;
                let by = resolve_actor(by.as_deref(), caller).map_err(ApiError::from)?;
                if by == "planner" {
                    return Err(ApiError::new(
                        exit::FORBIDDEN,
                        "planner cannot record decisions".to_owned(),
                    ));
                }
                let mut plan_hash = plan_hash;
                if by == "owner" {
                    let existing = store.get(&id).map_err(ApiError::from)?;
                    if matches!(verdict, Verdict::Approve)
                        && plan_hash.is_none()
                        && caller == BoardCaller::CliTty
                        && let Some(plan) = existing.plan.as_ref()
                    {
                        plan_hash = Some(plan.hash.clone());
                    }
                    confirm_owner_action_cli(&id, verdict.as_str(), &existing.title)?;
                }
                let item = store
                    .decide(
                        &id,
                        DecideRequest {
                            verdict,
                            text,
                            option,
                            plan_hash,
                            by,
                        },
                    )
                    .map_err(ApiError::from)?;
                if json_out {
                    print_json(&serde_json::to_value(&item).unwrap_or(Value::Null));
                }
                Ok(())
            }
            BoardOp::Say { id, text, from } => {
                let text = read_prompt(&text)?;
                let from = resolve_actor(from.as_deref(), caller).map_err(ApiError::from)?;
                let item = store
                    .say(
                        &id,
                        SayRequest {
                            text,
                            from: from.clone(),
                        },
                    )
                    .map_err(ApiError::from)?;
                if json_out {
                    print_json(&serde_json::to_value(&item).unwrap_or(Value::Null));
                }
                Ok(())
            }
            BoardOp::Events { since_seq } => {
                let events = store.list_events(since_seq).map_err(ApiError::from)?;
                if json_out {
                    print_json(&serde_json::to_value(&events).unwrap_or(Value::Null));
                } else {
                    for event in events {
                        println!(
                            "{}\t{}\t{}\t{}\t{}",
                            event.seq, event.item_id, event.kind, event.by, event.version
                        );
                    }
                }
                Ok(())
            }
            BoardOp::Prompt { id, hash } => {
                dmuxai::board::validate_item_id_early(&id).map_err(ApiError::from)?;
                let prompt = store
                    .unmasked_prompt(&id, hash.as_deref())
                    .map_err(ApiError::from)?;
                if json_out {
                    print_json(&json!({ "prompt": prompt }));
                } else {
                    println!("{prompt}");
                }
                Ok(())
            }
            BoardOp::Wait {
                id,
                any,
                since_version,
                since_seq,
                timeout_secs,
                wait_for,
                waiter,
            } => {
                if any {
                    let since_seq = since_seq.unwrap_or(0);
                    let wait_for = match wait_for.as_str() {
                        "decision" => WaitFor::Decision,
                        "message" => WaitFor::Message,
                        "any" => WaitFor::Any,
                        other => {
                            return Err(ApiError::new(
                                exit::USAGE,
                                format!("invalid --for '{other}'"),
                            ));
                        }
                    };
                    let waiter = waiter.unwrap_or_else(|| "bot:coordinator".to_owned());
                    let outcome = store
                        .wait_any(
                            since_seq,
                            WaitFilter {
                                since_version: since_version.unwrap_or(0),
                                timeout_secs,
                                wait_for,
                                waiter,
                            },
                        )
                        .map_err(ApiError::from)?;
                    if let Some((event, item)) = outcome {
                        print_json(&dmuxai::board::wait_ready_json_any(&event, &item));
                    } else {
                        let val =
                            json!({ "timed_out": true, "version": since_version.unwrap_or(0) });
                        print_json(&val);
                    }
                    return Ok(());
                }
                let id = id.ok_or_else(|| {
                    ApiError::new(exit::USAGE, "board wait needs an id".to_owned())
                })?;
                dmuxai::board::validate_item_id_early(&id).map_err(ApiError::from)?;
                let since =
                    since_version.unwrap_or_else(|| store.get(&id).map(|i| i.version).unwrap_or(0));
                let wait_for = match wait_for.as_str() {
                    "decision" => WaitFor::Decision,
                    "message" => WaitFor::Message,
                    "any" => WaitFor::Any,
                    other => {
                        return Err(ApiError::new(
                            exit::USAGE,
                            format!("invalid --for '{other}'"),
                        ));
                    }
                };
                let waiter = waiter.unwrap_or_else(|| "bot:coordinator".to_owned());
                let outcome = store
                    .wait(
                        &id,
                        WaitFilter {
                            since_version: since,
                            timeout_secs,
                            wait_for,
                            waiter,
                        },
                    )
                    .map_err(ApiError::from)?;
                match outcome {
                    WaitOutcome::Ready { item, event } => {
                        print_json(&wait_ready_json(&event, &item));
                    }
                    WaitOutcome::TimedOut { version } => {
                        let val = json!({ "timed_out": true, "version": version });
                        print_json(&val);
                    }
                }
                Ok(())
            }
            BoardOp::Rules { set } => {
                let rules_store = RulesStore::new(store.base());
                if let Some(source) = set {
                    resolve_actor(Some("owner"), caller).map_err(ApiError::from)?;
                    let text =
                        dmuxai::board::read_path_or_stdin(&source).map_err(ApiError::from)?;
                    let rules = compile_rules_from_set_text(&text).map_err(ApiError::from)?;
                    confirm_owner_action_cli("rules", "set board rules", "rules")?;
                    store
                        .with_rules_lock(|| rules_store.save(&rules))
                        .map_err(ApiError::from)?;
                    if json_out {
                        print_json(&serde_json::to_value(&rules).unwrap_or(Value::Null));
                    }
                } else {
                    let rules = rules_store.load();
                    if json_out {
                        print_json(&serde_json::to_value(&rules).unwrap_or(Value::Null));
                    } else {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&rules).unwrap_or_default()
                        );
                    }
                }
                Ok(())
            }
            BoardOp::Ask {
                id,
                text,
                revise,
                agent,
                model,
                by,
            } => {
                let text = read_prompt(&text)?;
                let by = resolve_actor(by.as_deref(), caller).map_err(ApiError::from)?;
                let item = dmuxai::board::runner::ask_item(
                    &store,
                    dmuxai::board::runner::AskParams {
                        id,
                        text,
                        revise,
                        agent,
                        model,
                        by,
                        state_root,
                    },
                )
                .map_err(ApiError::from)?;
                if json_out {
                    print_json(&serde_json::to_value(&item).unwrap_or(Value::Null));
                } else if let Some(last) = item.thread.last() {
                    println!("{}", last.text);
                }
                Ok(())
            }
            BoardOp::Run {
                id,
                detach,
                by,
                exec_prepared,
                plan_hash,
            } => {
                let by = if exec_prepared {
                    let actor = by
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .unwrap_or("bot:coordinator")
                        .to_owned();
                    if actor == "planner" {
                        return Err(ApiError::new(
                            exit::FORBIDDEN,
                            "planner cannot run items".to_owned(),
                        ));
                    }
                    if actor != "owner" && !actor.starts_with("bot:") {
                        return Err(ApiError::new(
                            exit::USAGE,
                            format!("invalid --by '{actor}' (expected owner or bot:NAME)"),
                        ));
                    }
                    actor
                } else {
                    resolve_actor(by.as_deref(), caller).map_err(ApiError::from)?
                };
                let item = if exec_prepared {
                    let hash = plan_hash.ok_or_else(|| {
                        ApiError::new(
                            exit::USAGE,
                            "--exec-prepared requires --plan-hash".to_owned(),
                        )
                    })?;
                    dmuxai::board::runner::run_item_execute_prepared(
                        &store,
                        &id,
                        &hash,
                        &by,
                        &state_root,
                    )
                    .map_err(ApiError::from)?
                } else {
                    let prepared = dmuxai::board::runner::run_item_prepare(
                        &store,
                        dmuxai::board::runner::RunParams {
                            id,
                            expected_plan_hash: None,
                            by,
                            state_root,
                        },
                    )
                    .map_err(ApiError::from)?;
                    if detach {
                        dmuxai::board::runner::launch_detached_board_run(&store, prepared)
                            .map_err(ApiError::from)?
                    } else {
                        dmuxai::board::runner::run_item_execute(&store, prepared)
                            .map_err(ApiError::from)?
                    }
                };
                if json_out {
                    print_json(&serde_json::to_value(&item).unwrap_or(Value::Null));
                } else {
                    println!("{}\t{}", item.id, item.lane.as_str());
                }
                Ok(())
            }
            BoardOp::Pending { workspace } => {
                let pending = store
                    .pending(workspace.as_deref())
                    .map_err(ApiError::from)?;
                if json_out {
                    print_json(&serde_json::to_value(&pending).unwrap_or(Value::Null));
                } else {
                    for item in &pending.items {
                        let ws_base = item
                            .workspace_path
                            .as_deref()
                            .and_then(|p| Path::new(p).file_name())
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "-".to_owned());
                        let title = truncate_chars(&item.title, 80);
                        let age = format_pending_age(item.age_ms);
                        println!(
                            "{}\t{}\t{}\t{}\t{}",
                            item.id, item.reason, ws_base, title, age
                        );
                    }
                    println!("{} pending", pending.count);
                }
                Ok(())
            }
            BoardOp::Check { id, text } => {
                dmuxai::board::validate_item_id_early(&id).map_err(ApiError::from)?;
                let by = resolve_actor(Some("owner"), caller).map_err(ApiError::from)?;
                let existing = store.get(&id).map_err(ApiError::from)?;
                confirm_owner_action_cli(&id, "check", &existing.title)?;
                let item = store.check(&id, text, &by).map_err(ApiError::from)?;
                if json_out {
                    print_json(&serde_json::to_value(&item).unwrap_or(Value::Null));
                }
                Ok(())
            }
        }
    })();

    dmuxai::board::notify::wait_for_pending(std::time::Duration::from_secs(10));
    match result {
        Ok(()) => exit::OK,
        Err(error) => fail(error, json_out),
    }
}

fn fail(error: ApiError, json_out: bool) -> i32 {
    if json_out {
        print_json(&error.to_json());
    } else {
        eprintln!("error: {}", error.message);
    }
    error.code
}

/// Runs an agent subcommand and returns the process exit code.
pub async fn execute(command: AgentCommand) -> i32 {
    let workspace_root = match dmuxai::workspace::resolve_workspace_root() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("error: {error}");
            return if error.kind() == io::ErrorKind::InvalidInput {
                exit::USAGE
            } else {
                exit::INTERNAL
            };
        }
    };
    match command {
        AgentCommand::Run(options) => {
            let json_out = options.json;
            match read_prompt(&options.prompt) {
                Ok(prompt) => run_and_report(&workspace_root, options, Some(prompt), None).await,
                Err(error) => fail(error, json_out),
            }
        }
        AgentCommand::TodoRun { id, run } => {
            run_and_report(&workspace_root, run, None, Some(id)).await
        }
        AgentCommand::TodoAdd { title, body, json } => {
            let result = agent_api::open_todo_store(&workspace_root)
                .and_then(|store| agent_api::todo_add(&store, &title, &body));
            match result {
                Ok(todo) => {
                    if json {
                        print_json(&serde_json::to_value(&todo).unwrap_or(Value::Null));
                    } else {
                        println!("{}\t{}", todo.id, todo.title);
                    }
                    exit::OK
                }
                Err(error) => fail(error, json),
            }
        }
        AgentCommand::TodoList { status, json } => {
            let filter = status
                .as_deref()
                .map(agent_api::parse_todo_status)
                .transpose();
            let result = filter.and_then(|filter| {
                agent_api::open_todo_store(&workspace_root)
                    .and_then(|store| agent_api::todo_list(&store, filter))
            });
            match result {
                Ok(todos) => {
                    if json {
                        print_json(&serde_json::to_value(&todos).unwrap_or(Value::Null));
                    } else {
                        for todo in todos {
                            let status = serde_json::to_value(todo.status)
                                .ok()
                                .and_then(|v| v.as_str().map(str::to_owned))
                                .unwrap_or_default();
                            println!("{}\t{}\t{}", todo.id, status, todo.title);
                        }
                    }
                    exit::OK
                }
                Err(error) => fail(error, json),
            }
        }
        AgentCommand::Status { json } => {
            let status = agent_api::status_json(&workspace_root);
            if json {
                print_json(&status);
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&status).unwrap_or_default()
                );
            }
            exit::OK
        }
        AgentCommand::Events {
            since,
            limit,
            follow,
            json,
        } => {
            print_events(&workspace_root, since, limit, follow, json).await;
            exit::OK
        }
        AgentCommand::Init { workspace, force } => init_config(&workspace_root, workspace, force),
        AgentCommand::Trust {
            status,
            json,
            revoke,
            yes,
        } => trust_workspace(&workspace_root, status, json, revoke, yes),
        AgentCommand::Runs {
            show_id,
            limit,
            failed,
            json,
        } => execute_runs(&workspace_root, show_id, limit, failed, json),
        AgentCommand::Runners { json } => {
            let settings = DmuxSettings::load(&workspace_root);
            let val = agent_api::runners_json(&settings);
            if json {
                print_json(&val);
            } else if let Some(runners) = val["runners"].as_array() {
                for runner in runners {
                    let id = runner["id"].as_str().unwrap_or_default();
                    let label = runner["label"].as_str().unwrap_or_default();
                    let kind = runner["kind"].as_str().unwrap_or_default();
                    let allowed = runner["allowed"].as_bool().unwrap_or(false);
                    let graph_allowed = runner["graph_allowed"].as_bool().unwrap_or(false);
                    let installed = match runner.get("installed") {
                        Some(Value::Bool(true)) => "installed",
                        Some(Value::Bool(false)) => "not installed",
                        _ => "unknown",
                    };
                    let default_model = runner
                        .get("default_model")
                        .and_then(Value::as_str)
                        .unwrap_or("-");
                    let policy = runner
                        .get("model_policy")
                        .and_then(Value::as_str)
                        .unwrap_or("-");
                    println!(
                        "{id}\t{label} ({kind})\tallowed: {allowed}\tgraph_allowed: {graph_allowed}\tinstalled: {installed}\tdefault: {default_model}\tpolicy: {policy}"
                    );
                }
            }
            exit::OK
        }
        AgentCommand::Models { agent, json } => {
            let settings = DmuxSettings::load(&workspace_root);
            let _ = dmuxai::custom_agent::install_registry(settings.custom_agents.clone());
            match agent_api::collect_models_json(&workspace_root, &settings, agent.as_deref()).await
            {
                Ok(val) => {
                    if json {
                        print_json(&val);
                    } else if let Some(runners) = val["runners"].as_array() {
                        for runner in runners {
                            let id = runner["id"].as_str().unwrap_or_default();
                            let source = runner["source"].as_str().unwrap_or_default();
                            println!("{id} (source: {source}):");
                            if let Some(err) = runner.get("error").and_then(Value::as_str) {
                                println!("  error: {err}");
                            }
                            if let Some(models) = runner.get("models").and_then(Value::as_array) {
                                for m in models {
                                    let mid = m["id"].as_str().unwrap_or_default();
                                    let allowed = m["allowed"].as_bool().unwrap_or(false);
                                    let marker = if allowed { "" } else { " [disallowed]" };
                                    if let Some(label) = m.get("label").and_then(Value::as_str) {
                                        println!("  {mid} ({label}){marker}");
                                    } else {
                                        println!("  {mid}{marker}");
                                    }
                                }
                            }
                        }
                    }
                    exit::OK
                }
                Err(error) => fail(error, json),
            }
        }
        AgentCommand::Term(command) => execute_term(&workspace_root, command),
        AgentCommand::Board(command) => execute_board(&workspace_root, command),
        AgentCommand::FanoutClean { run_id, all } => {
            match dmuxai::fanout_worktree::fanout_clean(&workspace_root, run_id.as_deref(), all) {
                Ok(()) => exit::OK,
                Err(error) => fail(ApiError::new(exit::USAGE, error.to_string()), false),
            }
        }
        AgentCommand::Mcp => {
            let backend = CliMcpBackend {
                workspace_root: workspace_root.clone(),
            };
            let result = tokio::task::spawn_blocking(move || {
                let stdin = io::stdin();
                let stdout = io::stdout();
                dmuxai::mcp_server::serve(&backend, stdin.lock(), stdout.lock())
            })
            .await;
            match result {
                Ok(Ok(())) => exit::OK,
                Ok(Err(error)) => {
                    eprintln!("error: {error}");
                    exit::INTERNAL
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    exit::INTERNAL
                }
            }
        }
    }
}

#[cfg(not(unix))]
fn execute_term(_workspace_root: &Path, command: TermCommand) -> i32 {
    term_fail(
        TermError::usage("terminal sessions are not supported on this platform yet"),
        command.json,
    )
}

#[cfg(unix)]
fn execute_term(workspace_root: &Path, command: TermCommand) -> i32 {
    let TermCommand { op, json } = command;
    // FR-2614 as amended: dmuxai never runs usbipd attach from WSL, so
    // `--no-usbipd-attach` is accepted only for compatibility; the deprecated
    // flag no longer changes any behavior on the host side.
    if matches!(&op, TermOp::Host) {
        return match dmuxai::term::host::run(workspace_root) {
            Ok(()) => exit::OK,
            Err(error) => term_fail(error, false),
        };
    }

    if let TermOp::Approve { .. } = &op {
        let run_id = std::env::var(dmuxai::term::policy::RUN_ID_ENV).unwrap_or_default();
        if !run_id.trim().is_empty() {
            return term_fail(
                TermError::new(
                    exit::FORBIDDEN,
                    "term approve is refused while a dmuxai run is active (DMUXAI_RUN_ID is set); ask a human to approve",
                ),
                json,
            );
        }
    }

    let response = match &op {
        TermOp::List { .. } => match dmuxai::term::client::try_connect(workspace_root) {
            Ok(mut client) => client.call(RequestOp::List),
            Err(_) => {
                if json {
                    print_json(&json!([]));
                } else {
                    println!("No terminal sessions.");
                }
                return exit::OK;
            }
        },
        TermOp::Open {
            name,
            wait_for: Some(regex),
            timeout_ms,
            ..
        } => {
            let open = {
                let mut client = match dmuxai::term::client::connect(workspace_root) {
                    Ok(client) => client,
                    Err(error) => return term_fail(error, json),
                };
                match client.call(term_request_op(&op)) {
                    Ok(response) => response,
                    Err(error) => return term_fail(error, json),
                }
            };
            let mut client = match dmuxai::term::client::connect(workspace_root) {
                Ok(client) => client,
                Err(error) => return term_fail(error, json),
            };
            let wait = match client.call(RequestOp::Wait {
                name: name.clone(),
                regex: Some(regex.clone()),
                idle_ms: None,
                timeout_ms: Some(timeout_ms.unwrap_or(0)),
                since: Some(0),
            }) {
                Ok(response) => response,
                Err(error) => return term_fail(error, json),
            };
            Ok(merge_open_wait(open, wait))
        }
        TermOp::Close { name, all, .. } => {
            match dmuxai::term::client::try_connect(workspace_root) {
                Ok(mut client) => client.call(term_request_op(&op)),
                Err(_) if *all => {
                    if json {
                        print_json(&json!({"closed": []}));
                    } else {
                        println!("closed 0 session(s)");
                    }
                    return exit::OK;
                }
                Err(_) => {
                    return term_fail(
                        TermError::not_found(name.as_deref().unwrap_or_default()),
                        json,
                    );
                }
            }
        }
        _ => match dmuxai::term::client::connect(workspace_root) {
            Ok(mut client) => client.call(term_request_op(&op)),
            Err(error) => return term_fail(error, json),
        },
    };
    let response = match response {
        Ok(response) => response,
        Err(error) => return term_fail(error, json),
    };
    report_term_response(&op, response, json)
}

#[cfg(unix)]
fn merge_open_wait(open: Response, wait: Response) -> Response {
    let mut data = match open.data {
        Some(Value::Object(map)) => Value::Object(map),
        Some(other) => other,
        None => Value::Object(serde_json::Map::new()),
    };
    if let (Some(map), Some(Value::Object(wait_data))) = (data.as_object_mut(), wait.data) {
        map.extend(wait_data);
    }
    Response {
        ok: wait.ok,
        error_kind: wait.error_kind,
        error: wait.error,
        exit_code: wait.exit_code,
        data: Some(data),
    }
}

#[cfg(unix)]
fn report_term_response(op: &TermOp, response: Response, json: bool) -> i32 {
    let code = response.exit_code.unwrap_or(exit::OK);
    let mut data = response.data.unwrap_or(Value::Null);
    // FR-2606 / FR-2607: closed and exited sessions are intended history; the
    // default list hides them while `show` and `--all` still cover them.
    if matches!(op, TermOp::List { all: false })
        && let Some(sessions) = data.as_array().cloned()
    {
        data = Value::Array(
            sessions
                .into_iter()
                .filter(|session| {
                    !matches!(
                        session.get("state").and_then(Value::as_str),
                        Some("closed") | Some("exited")
                    )
                })
                .collect(),
        );
    }
    if json {
        print_json(&data);
    } else {
        print_term_human(op, &data);
    }
    code
}

#[cfg(unix)]
fn term_request_op(op: &TermOp) -> RequestOp {
    match op {
        TermOp::Open {
            name,
            kind,
            cwd,
            env,
            cols,
            rows,
            argv,
            serial,
            ..
        } => {
            let mut env = env.clone();
            if let Ok(run_id) = std::env::var(dmuxai::term::policy::RUN_ID_ENV)
                && !run_id.trim().is_empty()
            {
                env.retain(|(k, _)| k != dmuxai::term::policy::RUN_ID_ENV);
                env.push((dmuxai::term::policy::RUN_ID_ENV.to_owned(), run_id));
            }
            RequestOp::Open {
                name: name.clone(),
                kind: *kind,
                cwd: cwd.as_ref().map(|path| path.display().to_string()),
                env,
                cols: *cols,
                rows: *rows,
                argv: argv.clone(),
                serial: serial.clone(),
            }
        }
        TermOp::Send {
            name,
            text,
            newline,
            secret,
        } => RequestOp::Send {
            name: name.clone(),
            text: text.clone(),
            newline: *newline,
            secret: *secret,
        },
        TermOp::SendKeys { name, keys } => RequestOp::SendKeys {
            name: name.clone(),
            keys: keys.clone(),
        },
        TermOp::Read {
            name,
            since,
            tail,
            raw,
        } => RequestOp::Read {
            name: name.clone(),
            since: *since,
            tail: *tail,
            raw: *raw,
        },
        TermOp::Wait {
            name,
            regex,
            idle_ms,
            timeout_ms,
            since,
        } => RequestOp::Wait {
            name: name.clone(),
            regex: regex.clone(),
            idle_ms: *idle_ms,
            timeout_ms: Some(*timeout_ms),
            since: *since,
        },
        TermOp::Exec {
            name,
            command,
            timeout_ms,
        } => RequestOp::Exec {
            name: name.clone(),
            command: command.clone(),
            timeout_ms: *timeout_ms,
        },
        TermOp::Approve { id } => RequestOp::Approve { id: id.clone() },
        TermOp::List { .. } => RequestOp::List,
        TermOp::Show { name } => RequestOp::Show { name: name.clone() },
        TermOp::Resize { name, cols, rows } => RequestOp::Resize {
            name: name.clone(),
            cols: *cols,
            rows: *rows,
        },
        TermOp::Close { name, force, all } => RequestOp::Close {
            name: name.clone(),
            force: *force,
            all: *all,
        },
        TermOp::Reopen { name } => RequestOp::Reopen { name: name.clone() },
        TermOp::Host => unreachable!("the host entry point is handled before requests"),
    }
}

#[cfg(unix)]
fn print_term_human(op: &TermOp, data: &Value) {
    match op {
        TermOp::Open { .. } | TermOp::Reopen { .. } => {
            println!(
                "{}\t{}\t{}\t{}",
                text_field(data, "name"),
                text_field(data, "kind"),
                text_field(data, "state"),
                data.get("pid")
                    .and_then(Value::as_u64)
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
            );
        }
        TermOp::Send { .. } | TermOp::SendKeys { .. } => {
            println!(
                "seq {}",
                data.get("seq")
                    .and_then(Value::as_u64)
                    .map(|seq| seq.to_string())
                    .unwrap_or_else(|| "-".to_owned())
            );
        }
        TermOp::Read { .. } | TermOp::Wait { .. } | TermOp::Exec { .. } => {
            if let Some(text) = data.get("text").and_then(Value::as_str) {
                print!("{text}");
                let _ = io::stdout().flush();
            }
            if data.get("reason").and_then(Value::as_str) == Some("timeout") {
                eprintln!("timeout");
            }
        }
        TermOp::List { .. } => {
            if let Some(sessions) = data.as_array() {
                for session in sessions {
                    println!(
                        "{}\t{}\t{}\t{}",
                        text_field(session, "name"),
                        text_field(session, "kind"),
                        text_field(session, "state"),
                        session
                            .get("pid")
                            .and_then(Value::as_u64)
                            .map(|pid| pid.to_string())
                            .unwrap_or_else(|| "-".to_owned()),
                    );
                }
            }
        }
        TermOp::Show { .. } => {
            println!(
                "{}",
                serde_json::to_string_pretty(data).unwrap_or_else(|_| data.to_string())
            );
        }
        TermOp::Resize { name, .. } => {
            println!(
                "resized {} to {}x{}",
                name,
                text_field(data, "cols"),
                text_field(data, "rows")
            );
        }
        TermOp::Close { .. } => {
            if let Some(closed) = data.get("closed").and_then(Value::as_array) {
                println!("closed {} session(s)", closed.len());
            } else {
                println!("closed {}", text_field(data, "name"));
            }
        }
        TermOp::Approve { id } => {
            println!("approved {id}");
        }
        TermOp::Host => {}
    }
}

#[cfg(unix)]
fn text_field(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Null) | None => "-".to_owned(),
        Some(other) => other.to_string(),
    }
}

fn term_fail(error: TermError, json: bool) -> i32 {
    if json {
        print_json(&error.to_json());
    } else {
        eprintln!("error: {}", error.message);
    }
    error.code
}

/// Reports a term usage error; `--json` still emits exactly one JSON object.
pub fn term_usage_error(message: impl Into<String>, json: bool) -> i32 {
    term_fail(TermError::usage(message), json)
}

/// Executes `dmuxai runs` or `dmuxai runs show <id>` (SPEC-run-journal FR-2562).
fn execute_runs(
    workspace_root: &Path,
    show_id: Option<String>,
    limit: usize,
    failed: bool,
    json: bool,
) -> i32 {
    let runs_dir = match dmuxai::run_journal::repo_runs_dir(workspace_root) {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::INTERNAL;
        }
    };

    let store = dmuxai::run_journal::JournalStore::new(runs_dir.clone());

    if let Some(id) = show_id {
        if !dmuxai::run_journal::valid_run_id(&id) {
            eprintln!("error: invalid run id '{id}'");
            return exit::USAGE;
        }
        let run_dir = runs_dir.join(&id);
        if let Ok(true) = dmuxai::run_journal::is_symlink(&run_dir) {
            eprintln!("error: symlink rejected at {}", run_dir.display());
            return exit::INTERNAL;
        }
        if !run_dir.exists() {
            eprintln!("error: run '{id}' not found");
            return exit::NOT_FOUND;
        }
        match store.load(&id) {
            Ok(manifest) => {
                if json {
                    match serde_json::to_value(&manifest) {
                        Ok(val) => print_json(&val),
                        Err(e) => {
                            eprintln!("error: {e}");
                            return exit::INTERNAL;
                        }
                    }
                } else {
                    println!("Run ID:    {}", manifest.run_id);
                    println!("Status:    {}", manifest.status);
                    println!("Created:   {}", manifest.created_at_ms);
                    if let Some(runner) = &manifest.requested_runner {
                        println!("Runner:    {runner}");
                    }
                    if let Some(err) = &manifest.error_summary {
                        println!("Error:     {err}");
                    }
                }
                exit::OK
            }
            Err(dmuxai::run_journal::JournalError::Io(e)) => {
                eprintln!("error: {e}");
                exit::INTERNAL
            }
            Err(dmuxai::run_journal::JournalError::SymlinkDetected(p)) => {
                eprintln!("error: symlink rejected at {}", p.display());
                exit::INTERNAL
            }
            Err(e) => {
                eprintln!("error: {e}");
                exit::INTERNAL
            }
        }
    } else {
        // List runs
        if !runs_dir.exists() {
            if json {
                print_json(&serde_json::Value::Array(Vec::new()));
            }
            return exit::OK;
        }

        if let Ok(true) = dmuxai::run_journal::is_symlink(&runs_dir) {
            eprintln!("error: symlink rejected at {}", runs_dir.display());
            return exit::INTERNAL;
        }

        let entries = match std::fs::read_dir(&runs_dir) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("error: {e}");
                return exit::INTERNAL;
            }
        };

        let mut manifests = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(true) = dmuxai::run_journal::is_symlink(&path) {
                continue;
            }
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !dmuxai::run_journal::valid_run_id(name) {
                continue;
            }

            match store.load(name) {
                Ok(m) => {
                    if failed && m.status != "failed" {
                        continue;
                    }
                    manifests.push(m);
                }
                Err(dmuxai::run_journal::JournalError::UnknownSchemaVersion(v)) => {
                    eprintln!("warning: unknown schema version {v} in run '{name}', skipping");
                }
                Err(dmuxai::run_journal::JournalError::CorruptManifest(msg)) => {
                    eprintln!("warning: corrupt manifest in run '{name}': {msg}, skipping");
                }
                Err(e) => {
                    eprintln!("warning: could not load run '{name}': {e}, skipping");
                }
            }
        }

        // Sort newest first by created_at_ms
        manifests.sort_by_key(|m| std::cmp::Reverse(m.created_at_ms));

        if manifests.len() > limit {
            manifests.truncate(limit);
        }

        if json {
            let val =
                serde_json::to_value(&manifests).unwrap_or(serde_json::Value::Array(Vec::new()));
            print_json(&val);
        } else {
            for m in manifests {
                println!("{}\t{}\t{}", m.run_id, m.status, m.created_at_ms);
            }
        }

        exit::OK
    }
}

/// `dmuxai init` (FR-814): writes the starter config and reports the runners
/// found on `PATH`. An existing file is kept unless `--force` is given.
fn init_config(workspace_root: &Path, workspace: bool, force: bool) -> i32 {
    let path = match dmuxai::init::target_path(workspace, workspace_root) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("error: {error}");
            return exit::FAILED;
        }
    };
    let detected = dmuxai::init::detected_runners();
    let opencode = detected.contains(&"opencode");
    let grok = detected.contains(&"grok");
    if let Err(error) = dmuxai::init::write_starter_config(&path, opencode, grok, force) {
        eprintln!("error: {error}");
        return exit::FAILED;
    }
    println!("Wrote {}", path.display());
    if detected.is_empty() {
        println!("Detected runners on PATH: none");
    } else {
        println!("Detected runners on PATH: {}", detected.join(", "));
    }
    exit::OK
}

fn read_prompt(raw: &str) -> Result<String, ApiError> {
    if raw.trim() != "-" {
        return Ok(raw.to_owned());
    }
    let mut text = String::new();
    io::stdin().read_to_string(&mut text).map_err(|e| {
        ApiError::new(
            exit::USAGE,
            format!("could not read the prompt from stdin: {e}"),
        )
    })?;
    if text.trim().is_empty() {
        return Err(ApiError::new(exit::USAGE, "the prompt from stdin is empty"));
    }
    Ok(text.trim_end_matches(['\r', '\n']).to_owned())
}

fn format_event_line(event: &dmuxai::events::EventRecord) -> String {
    let subject = [
        event.runner.as_deref(),
        event.node.as_deref(),
        event.todo.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    let message = event
        .message
        .as_deref()
        .map(|m| format!(" — {m}"))
        .unwrap_or_default();
    if event.kind == "activity" {
        format!(
            "#{} {} {} {} {}{}",
            event.seq, event.ts, event.kind, subject, event.to_state, message
        )
    } else {
        let from = event.from_state.as_deref().unwrap_or("-");
        format!(
            "#{} {} {} {} {} -> {}{}",
            event.seq, event.ts, event.kind, subject, from, event.to_state, message
        )
    }
}

async fn print_events(root: &Path, since: u64, limit: Option<usize>, follow: bool, json: bool) {
    let path = dmuxai::events::events_path(root);
    let mut last = since;
    let mut remaining = limit;
    loop {
        let batch = dmuxai::events::read_since(&path, last, remaining);
        {
            let mut stdout = io::stdout().lock();
            for event in &batch {
                let line = if json {
                    serde_json::to_string(event).unwrap_or_default()
                } else {
                    format_event_line(event)
                };
                if writeln!(stdout, "{line}").is_err() {
                    return;
                }
                last = last.max(event.seq);
            }
            let _ = stdout.flush();
        }
        if let Some(left) = remaining.as_mut() {
            *left = left.saturating_sub(batch.len());
            if *left == 0 {
                return;
            }
        }
        if !follow {
            return;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            _ = tokio::signal::ctrl_c() => return,
        }
    }
}

fn outcome_json(outcome: &HeadlessOutcome, first_seq: u64, last_seq: u64) -> Value {
    if let Some(report) = &outcome.fanout {
        return dmuxai::fanout::fanout_json_value(dmuxai::fanout::FanoutOutcomeJson {
            ok: outcome.ok,
            min_ok: report.min_ok,
            legs: &report.legs,
            judge_enabled: report.judge_enabled,
            aggregate: outcome.aggregate.as_ref(),
            aggregate_error: outcome.aggregate_error.as_deref(),
            aggregate_skipped: outcome.aggregate_skipped.as_deref(),
            judge: outcome.fanout_judge.as_ref(),
            error: outcome.error.as_deref(),
            error_kind: outcome.error_kind.as_deref(),
        });
    }
    let mut value = json!({
        "ok": outcome.ok,
        "exit_code": outcome.exit_code,
        "conversation_id": outcome.conversation_id,
        "agent": outcome.agent,
        "graph_id": outcome.graph_id,
        "run_id": outcome.run_id,
        "text": outcome.text,
        "messages": outcome.messages,
        "error": outcome.error,
        "error_kind": outcome.error_kind,
        "todo": outcome.todo,
        "triage": outcome.triage,
        "plan": outcome.plan,
        "system_prompt": outcome.system_prompt,
        // Both `null` when the run wrote no events (e.g. rejected before start).
        "events": if last_seq >= first_seq {
            json!({ "first_seq": first_seq, "last_seq": last_seq })
        } else {
            json!({ "first_seq": null, "last_seq": null })
        },
    });
    // FR-2545: the saved pending graph plan hash, omitted when there is none.
    if let Some(plan_hash) = &outcome.plan_hash {
        value["plan_hash"] = json!(plan_hash);
    }
    // FR-2119: the runner fallbacks of this run, omitted when there were none.
    if !outcome.fallbacks.is_empty() {
        value["fallbacks"] = json!(outcome.fallbacks);
    }
    value
}

/// Headless `run` / `todo run` (FR-761, FR-762).
async fn run_and_report(
    workspace_root: &Path,
    options: RunOptions,
    prompt: Option<String>,
    todo_id: Option<String>,
) -> i32 {
    let json_out = options.json;
    match run_headless(workspace_root, &options, prompt, todo_id).await {
        Ok((outcome, first_seq, last_seq)) => {
            if json_out {
                print_json(&outcome_json(&outcome, first_seq, last_seq));
            } else {
                if !outcome.text.is_empty() {
                    println!("{}", outcome.text);
                }
                if let Some(error) = &outcome.error
                    && !outcome.ok
                {
                    eprintln!("error: {error}");
                }
            }
            outcome.exit_code
        }
        Err(error) => fail(error, json_out),
    }
}

fn headless_dashboard_hub(workspace: String, sink: Arc<EventSink>) -> DashboardHub {
    dmuxai::web::DashboardHub::new(dmuxai::web::SnapshotSource::App, workspace).with_events(sink)
}

/// Maps a conversation-database open failure to a headless exit code
/// (FR-2120). A database another process still holds is a conflict (4);
/// everything else stays an internal error (6).
fn conversation_open_error(error: &ConversationError) -> ApiError {
    match error {
        ConversationError::Busy(message) => ApiError::new(exit::CONFLICT, message.clone()),
        other => ApiError::new(exit::INTERNAL, other.to_string()),
    }
}

fn start_headless_journal(
    workspace_root: &Path,
    options: &RunOptions,
    run_id: &str,
) -> Option<(
    dmuxai::run_journal::JournalStore,
    dmuxai::run_journal::RunManifest,
)> {
    let runs_dir = match dmuxai::run_journal::repo_runs_dir(workspace_root) {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("warning: run journal unavailable: {e}");
            return None;
        }
    };
    let store = dmuxai::run_journal::JournalStore::new(runs_dir);
    let origin = if std::env::var("DMUXAI_RUN_ORIGIN").as_deref() == Ok("mcp") {
        "mcp".to_string()
    } else {
        "cli_headless".to_string()
    };
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let manifest = dmuxai::run_journal::RunManifest {
        schema_version: 1,
        run_id: run_id.to_string(),
        origin,
        status: "active".to_string(),
        created_at_ms: now_ms,
        completed_at_ms: None,
        duration_ms: None,
        requested_runner: options
            .agent
            .as_deref()
            .map(dmuxai::runner_id::canonical_runner_id),
        resolved_runner: None,
        requested_model: options.model.clone(),
        resolved_model: None,
        dmuxai_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        build_commit: None,
        pid: Some(std::process::id()),
        dmuxai_exit_code: None,
        child_exit_code: None,
        child_signal: None,
        error_kind: None,
        error_summary: None,
        links: dmuxai::run_journal::RunLinks::default(),
        diagnostics: dmuxai::run_journal::RunDiagnosticsRef::default(),
        end_summary: None,
    };
    if let Err(e) = store.save_relaxed(&manifest) {
        eprintln!("warning: run journal unavailable: {e}");
        return None;
    }
    Some((store, manifest))
}

async fn run_fanout_headless(
    workspace_root: &Path,
    options: &RunOptions,
    prompt: Option<String>,
) -> Result<(HeadlessOutcome, u64, u64), ApiError> {
    let internal = |e: &dyn std::fmt::Display| ApiError::new(exit::INTERNAL, e.to_string());
    dmuxai::workspace::ensure_directory_writable(workspace_root).map_err(|e| internal(&e))?;
    let mut env = dmuxai::runtime_env::RuntimeEnv::from_process();
    if options.trust_workspace_config {
        env.trust_workspace_config = true;
    }
    let settings = DmuxSettings::load_with_env(workspace_root, &env);
    for warning in &settings.config_warnings {
        eprintln!("warning: {warning}");
    }
    let company_workspace = settings.fanout_company_workspace;
    let agents_raw = options.agents.as_deref().unwrap_or_default();
    let all_specs = dmuxai::fanout::parse_agents_flag(agents_raw)
        .map_err(|message| ApiError::new(dmuxai::agent_api::exit::USAGE, message))?;
    let cli_order: Vec<String> = all_specs.iter().map(|leg| leg.runner.clone()).collect();
    let mut specs = Vec::new();
    let mut skipped_reports: Vec<dmuxai::fanout::FanoutLegReport> = Vec::new();
    for leg in all_specs {
        if company_workspace && dmuxai::fanout::fanout_runner_is_free(&leg, &settings) {
            eprintln!(
                "warning: skipping free runner '{}' in company workspace",
                leg.runner
            );
            skipped_reports.push(dmuxai::fanout::FanoutLegReport {
                agent: leg.runner.clone(),
                model: dmuxai::fanout::resolved_fanout_leg_model(&leg, &settings),
                status: dmuxai::fanout::LegStatus::Skipped,
                text: String::new(),
                error: None,
                error_kind: None,
                blocked: false,
                duration_ms: 0,
                ..Default::default()
            });
        } else {
            specs.push(leg);
        }
    }
    if specs.len() < 2 {
        return Err(ApiError::new(
            dmuxai::agent_api::exit::USAGE,
            "fan-out needs at least two runners after filtering".to_owned(),
        ));
    }
    let judge = if let Some(raw) = options.judge.as_deref() {
        let judge_spec = dmuxai::fanout::parse_judge_flag(raw)
            .map_err(|message| ApiError::new(dmuxai::agent_api::exit::USAGE, message))?;
        if let Err(message) = dmuxai::fanout::reject_free_judge_in_company_workspace(
            &judge_spec,
            company_workspace,
            &settings,
        ) {
            eprintln!(
                "{}",
                dmuxai::fanout::free_judge_company_workspace_warning(&judge_spec.runner)
            );
            return Err(ApiError::new(dmuxai::agent_api::exit::USAGE, message));
        }
        Some(judge_spec)
    } else {
        None
    };
    let min_ok = dmuxai::fanout::parse_min_ok(
        options.min_ok.map(|n| n.to_string()).as_deref(),
        specs.len(),
    )
    .map_err(|message| ApiError::new(dmuxai::agent_api::exit::USAGE, message))?;

    let user_prompt = prompt.unwrap_or_default();
    if options.write
        && dmuxai::graph::sensitive_keyword_match(&user_prompt)
        && !options.allow_sensitive
        && !options.approve_plan
    {
        return Err(ApiError::new(
            exit::APPROVAL_REQUIRED,
            "sensitive step needs approval: rerun with --allow-sensitive (or --agent <name> --approve-plan)"
                .to_owned(),
        ));
    }
    let write = if options.write {
        dmuxai::fanout_worktree::validate_write_fanout_workspace(workspace_root)
            .map_err(|e| ApiError::new(dmuxai::agent_api::exit::USAGE, e.to_string()))?;
        dmuxai::fanout_worktree::disk_guard_check(
            workspace_root,
            specs.len(),
            settings.fanout_min_free_gb,
        )
        .map_err(|e| ApiError::new(dmuxai::agent_api::exit::USAGE, e.to_string()))?;
        Some(dmuxai::fanout_worktree::FanoutWriteRunConfig {
            run_id: dmuxai::run_journal::generate_run_id(),
            keep_worktrees: options.keep_worktrees,
        })
    } else {
        None
    };

    let (db_path, _) =
        dmuxai::state_path::prepare_repo_db(workspace_root).map_err(|e| internal(&e))?;
    let engine = ConversationEngine::open(&db_path, workspace_root)
        .map_err(|error| conversation_open_error(&error))?;
    for warning in dmuxai::custom_agent::install_registry(settings.custom_agents.clone()) {
        eprintln!("warning: {warning}");
    }
    let sink = build_event_sink(workspace_root, &settings);
    let first_seq = sink.log().last_seq() + 1;
    let workspace = dmuxai::workspace::normalize_workspace_path(workspace_root)
        .unwrap_or_else(|_| workspace_root.display().to_string());
    let hub = headless_dashboard_hub(workspace, sink.clone());
    let mut app = App::new(engine, workspace_root.to_path_buf(), settings.clone())
        .map_err(|error| conversation_open_error(&error))?;
    app.attach_dashboard(hub);
    app.set_headless(true);

    let write_cleanup = Arc::new(std::sync::Mutex::new(None::<(PathBuf, String)>));
    if let Some(ref cfg) = write {
        *write_cleanup.lock().unwrap() = Some((workspace_root.to_path_buf(), cfg.run_id.clone()));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_signal = cancel.clone();
    let finished = Arc::new(AtomicBool::new(false));
    let finished_signal = finished.clone();
    let write_cleanup_signal = write_cleanup.clone();
    let signal_task = tokio::spawn(async move {
        wait_for_stop_signal().await;
        cancel_signal.store(true, Ordering::SeqCst);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(10)) => {}
            _ = wait_for_stop_signal() => {}
        }
        if !finished_signal.load(Ordering::SeqCst) {
            if let Some((ws, run_id)) = write_cleanup_signal.lock().unwrap().take() {
                let _ = dmuxai::fanout_worktree::fanout_clean(&ws, Some(&run_id), false);
            }
            dmuxai::process::supervisor::stop_live_groups_of(
                dmuxai::process::supervisor::GroupOwner::DEFAULT,
                Duration::from_secs(2),
            );
            eprintln!("error: stopped after signal");
            std::process::exit(130);
        }
    });
    let outcome = app
        .run_fanout_headless(
            user_prompt,
            specs,
            min_ok,
            skipped_reports,
            cli_order,
            judge,
            cancel,
            write,
        )
        .await;
    app.shutdown().await;
    finished.store(true, Ordering::SeqCst);
    write_cleanup.lock().unwrap().take();
    signal_task.abort();
    let last_seq = sink.log().last_seq();
    Ok((outcome, first_seq, last_seq))
}

async fn run_headless(
    workspace_root: &Path,
    options: &RunOptions,
    prompt: Option<String>,
    todo_id: Option<String>,
) -> Result<(HeadlessOutcome, u64, u64), ApiError> {
    let internal = |e: &dyn std::fmt::Display| ApiError::new(exit::INTERNAL, e.to_string());
    if options.agents.is_some() {
        if todo_id.is_some() {
            return Err(ApiError::new(
                exit::USAGE,
                "todo run does not support --agents".to_owned(),
            ));
        }
        return run_fanout_headless(workspace_root, options, prompt).await;
    }
    dmuxai::workspace::ensure_directory_writable(workspace_root).map_err(|e| internal(&e))?;
    let mut env = dmuxai::runtime_env::RuntimeEnv::from_process();
    if options.trust_workspace_config {
        env.trust_workspace_config = true;
    }
    let settings = DmuxSettings::load_with_env(workspace_root, &env);
    for warning in &settings.config_warnings {
        eprintln!("warning: {warning}");
    }
    let (db_path, notice) =
        dmuxai::state_path::prepare_repo_db(workspace_root).map_err(|e| internal(&e))?;
    if let Some(notice) = notice {
        eprintln!("warning: {notice}");
    }
    let engine = ConversationEngine::open(&db_path, workspace_root)
        .map_err(|error| conversation_open_error(&error))?;
    for warning in dmuxai::custom_agent::install_registry(settings.custom_agents.clone()) {
        eprintln!("warning: {warning}");
    }
    let sink = build_event_sink(workspace_root, &settings);
    let run_id = dmuxai::run_journal::generate_run_id();
    struct RunIdEnvGuard(Option<std::ffi::OsString>);
    impl Drop for RunIdEnvGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(prev) => unsafe { std::env::set_var(dmuxai::term::policy::RUN_ID_ENV, prev) },
                None => unsafe { std::env::remove_var(dmuxai::term::policy::RUN_ID_ENV) },
            }
        }
    }
    let _run_id_env_guard = RunIdEnvGuard(std::env::var_os(dmuxai::term::policy::RUN_ID_ENV));
    unsafe { std::env::set_var(dmuxai::term::policy::RUN_ID_ENV, &run_id) };
    sink.log().set_run_id(Some(run_id.clone()));
    let first_seq = sink.log().last_seq() + 1;
    let workspace = dmuxai::workspace::normalize_workspace_path(workspace_root)
        .unwrap_or_else(|_| workspace_root.display().to_string());
    let hub = headless_dashboard_hub(workspace, sink.clone());

    let mut app = App::new(engine, workspace_root.to_path_buf(), settings)
        .map_err(|error| conversation_open_error(&error))?;
    app.attach_dashboard(hub);
    if todo_id.is_some() {
        let store = agent_api::open_todo_store(workspace_root)?;
        app.attach_todo_store(Arc::new(store));
    }

    let journal_pair = start_headless_journal(workspace_root, options, &run_id);
    let progress_warned = Arc::new(AtomicBool::new(false));
    let progress_copy = journal_pair.as_ref().and_then(|(store, manifest)| {
        match store.open_progress_copy(&manifest.run_id) {
            Ok(copy) => Some(copy),
            Err(err) => {
                if !progress_warned.swap(true, Ordering::SeqCst) {
                    eprintln!("warning: run journal unavailable: {err}");
                }
                None
            }
        }
    });
    let progress_copy = std::sync::Mutex::new(progress_copy);
    let journal_handle = Arc::new(std::sync::Mutex::new(journal_pair));

    let progress_opt = options.progress;
    let progress_warned_sink = Arc::clone(&progress_warned);
    app.set_progress_sink(Box::new(move |line: &str| {
        if progress_opt {
            eprintln!("{line}");
        }
        let mut guard = progress_copy.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(writer) = guard.as_mut()
            && let Err(err) = writer.append_line(line)
            && !progress_warned_sink.swap(true, Ordering::SeqCst)
        {
            eprintln!("warning: run journal unavailable: {err}");
        }
    }));

    let meta_journal = Arc::clone(&journal_handle);
    let meta_warned = Arc::clone(&progress_warned);
    // FR-2816 / TC-2816: supersede older `awaiting_approval` manifests with the
    // same plan hash exactly once, and only once the approved plan actually
    // starts executing (the meta sink reports a graph id for this run). Runs
    // without `--approve-plan-id` never trigger it (FR-2810/#301).
    let meta_approve_plan_id = options.approve_plan_id.clone();
    let meta_superseded = Arc::new(AtomicBool::new(false));
    let meta_supersede_flag = Arc::clone(&meta_superseded);
    app.set_headless_meta_sink(Box::new(move |meta| {
        if let Some(plan_hash) = meta_approve_plan_id.as_deref()
            && meta.graph_id.is_some()
            && !meta_supersede_flag.load(Ordering::SeqCst)
        {
            let pair = {
                let guard = meta_journal.lock().unwrap_or_else(|e| e.into_inner());
                guard
                    .as_ref()
                    .map(|(store, manifest)| (store.clone(), manifest.run_id.clone()))
            };
            let result = match pair {
                Some((store, run_id)) => store.supersede_awaiting(plan_hash, &run_id, 200),
                None => Ok(0),
            };
            match result {
                Ok(_) => meta_supersede_flag.store(true, Ordering::SeqCst),
                Err(err) => {
                    if !meta_warned.swap(true, Ordering::SeqCst) {
                        eprintln!("warning: run journal unavailable: {err}");
                    }
                }
            }
        }
        let mut guard = meta_journal.lock().unwrap_or_else(|e| e.into_inner());
        let Some((store, manifest)) = guard.as_mut() else {
            return;
        };
        let mut changed = false;
        if meta.graph_id.is_some() && manifest.links.graph_id != meta.graph_id {
            manifest.links.graph_id = meta.graph_id;
            changed = true;
        }
        if meta.runner.is_some() && manifest.resolved_runner != meta.runner {
            manifest.resolved_runner = meta.runner;
            changed = true;
        }
        if meta.model.is_some() && manifest.resolved_model != meta.model {
            manifest.resolved_model = meta.model;
            changed = true;
        }
        if changed
            && let Err(err) = store.save_relaxed(manifest)
            && !meta_warned.swap(true, Ordering::SeqCst)
        {
            eprintln!("warning: run journal unavailable: {err}");
        }
    }));

    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_signal = cancel.clone();
    let finished = Arc::new(AtomicBool::new(false));
    let finished_signal = finished.clone();
    let signal_journal = Arc::clone(&journal_handle);
    let signal_task = tokio::spawn(async move {
        wait_for_stop_signal().await;
        cancel_signal.store(true, Ordering::SeqCst);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(10)) => {}
            _ = wait_for_stop_signal() => {}
        }
        if !finished_signal.load(Ordering::SeqCst) {
            dmuxai::process::supervisor::stop_live_groups_of(
                dmuxai::process::supervisor::GroupOwner::DEFAULT,
                Duration::from_secs(2),
            );
            let taken = {
                let mut guard = signal_journal.lock().unwrap_or_else(|e| e.into_inner());
                guard.take()
            };
            if let Some((store, mut manifest)) = taken {
                let now_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(manifest.created_at_ms);
                manifest.status = "cancelled".to_string();
                manifest.completed_at_ms = Some(now_ms);
                manifest.duration_ms = Some(now_ms.saturating_sub(manifest.created_at_ms));
                manifest.dmuxai_exit_code = Some(130);
                if let Err(err) = store.save_relaxed(&manifest) {
                    eprintln!("warning: run journal unavailable: {err}");
                }
            }
            eprintln!("error: stopped after signal");
            std::process::exit(130);
        }
    });
    let mode = if options.read_only {
        Some("ask".to_owned())
    } else {
        options.mode.clone()
    };
    let request = HeadlessRequest {
        prompt: prompt.unwrap_or_default(),
        agent: options.agent.clone(),
        mode,
        graph: options.graph,
        approve_plan: options.approve_plan,
        approve_plan_id: options.approve_plan_id.clone(),
        allow_sensitive: options.allow_sensitive,
        todo_id,
        model: options.model.clone(),
    };
    let mut outcome = app.run_headless(request, cancel).await;
    app.shutdown().await;
    finished.store(true, Ordering::SeqCst);
    signal_task.abort();

    outcome.run_id = Some(run_id.clone());

    let taken = {
        let mut guard = journal_handle.lock().unwrap_or_else(|e| e.into_inner());
        guard.take()
    };
    if let Some((store, mut manifest)) = taken {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(manifest.created_at_ms);
        manifest.status = match outcome.exit_code {
            0 => "completed".to_string(),
            7 => "awaiting_approval".to_string(),
            130 => "cancelled".to_string(),
            _ => "failed".to_string(),
        };
        manifest.completed_at_ms = Some(now_ms);
        manifest.duration_ms = Some(now_ms.saturating_sub(manifest.created_at_ms));
        manifest.dmuxai_exit_code = Some(outcome.exit_code);
        manifest.error_kind = outcome.error_kind.clone();
        manifest.error_summary = outcome.error.as_deref().map(|err| {
            let masked = dmuxai::run_journal::mask_sensitive_tokens(err);
            dmuxai::run_journal::truncate_tail_utf8(&masked, 2048).to_string()
        });
        manifest.resolved_runner = outcome.agent.clone();
        manifest.resolved_model = outcome.triage.as_ref().map(|t| t.model.clone());
        manifest.links.graph_id = outcome.graph_id.clone();
        manifest.links.conversation_id = Some(outcome.conversation_id.clone());
        if let Some(plan_hash) = &outcome.plan_hash {
            manifest.end_summary = Some(json!({ "plan_hash": plan_hash }));
        }
        if let Err(err) = store.save_relaxed(&manifest) {
            eprintln!("warning: run journal unavailable: {err}");
        }
    }

    let last_seq = sink.log().last_seq();
    Ok((outcome, first_seq, last_seq))
}

/// MCP backend for `dmuxai mcp`: in-process for reads, a child
/// `dmuxai run --json` process for runs so each run is isolated (FR-767).
struct CliMcpBackend {
    workspace_root: PathBuf,
}

/// Upper bound for one MCP `run` call; `DMUXAI_MCP_RUN_TIMEOUT_SECS` overrides it.
const MCP_RUN_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);

fn mcp_run_timeout() -> Duration {
    std::env::var("DMUXAI_MCP_RUN_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map(Duration::from_secs)
        .unwrap_or(MCP_RUN_TIMEOUT)
}

/// Builds the child argv for an MCP `run` call (FR-2509..FR-2511).
fn mcp_run_argv(args: &RunArgs) -> Vec<String> {
    let mut argv: Vec<String> = match &args.todo_id {
        Some(id) => vec!["todo".into(), "run".into(), id.clone()],
        None => vec!["run".into()],
    };
    if let Some(agent) = &args.agent {
        argv.extend(["--agent".into(), agent.clone()]);
    }
    if let Some(model) = &args.model {
        argv.extend(["--model".into(), model.clone()]);
    }
    if args.approve_plan {
        argv.push("--approve-plan".into());
    }
    if let Some(approve_plan_id) = &args.approve_plan_id {
        argv.extend(["--approve-plan-id".into(), approve_plan_id.clone()]);
    }
    if args.allow_sensitive {
        argv.push("--allow-sensitive".into());
    }
    if args.graph {
        argv.push("--graph".into());
    }
    argv.push("--json".into());
    if args.todo_id.is_none() {
        argv.push("--".into());
        argv.push(args.prompt.clone().unwrap_or_default());
    }
    argv
}

impl CliMcpBackend {
    fn run_child(&self, args: Vec<String>) -> Result<Value, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut child = std::process::Command::new(exe)
            .args(&args)
            .current_dir(&self.workspace_root)
            .env("DMUXAI_RUN_ORIGIN", "mcp")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start dmuxai: {e}"))?;
        // Drain both pipes on threads so a chatty child never blocks.
        let mut out_pipe = child.stdout.take();
        let mut err_pipe = child.stderr.take();
        let out_reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(pipe) = out_pipe.as_mut() {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        });
        let err_reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(pipe) = err_pipe.as_mut() {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        });
        let deadline = std::time::Instant::now() + mcp_run_timeout();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if std::time::Instant::now() >= deadline => {
                    // SIGKILL skips the child's own cancel path; acceptable for a hung run.
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "dmuxai run timed out after {}s and was stopped",
                        mcp_run_timeout().as_secs()
                    ));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(error) => return Err(format!("could not wait for dmuxai: {error}")),
            }
        };
        let output = std::process::Output {
            status,
            stdout: out_reader.join().unwrap_or_default(),
            stderr: err_reader.join().unwrap_or_default(),
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        serde_json::from_str::<Value>(line).map_err(|_| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            format!(
                "dmuxai exited with {} and no JSON result: {}",
                output.status.code().unwrap_or(-1),
                stderr.trim()
            )
        })
    }
}

impl McpBackend for CliMcpBackend {
    fn todo_add(&self, title: &str, body: &str) -> Result<Value, String> {
        agent_api::open_todo_store(&self.workspace_root)
            .and_then(|store| agent_api::todo_add(&store, title, body))
            .map(|todo| serde_json::to_value(todo).unwrap_or(Value::Null))
            .map_err(|e| e.message)
    }

    fn todo_list(&self, status: Option<&str>) -> Result<Value, String> {
        let filter = status
            .map(agent_api::parse_todo_status)
            .transpose()
            .map_err(|e| e.message)?;
        agent_api::open_todo_store(&self.workspace_root)
            .and_then(|store| agent_api::todo_list(&store, filter))
            .map(|todos| serde_json::to_value(todos).unwrap_or(Value::Null))
            .map_err(|e| e.message)
    }

    fn run(&self, args: RunArgs) -> Result<Value, String> {
        self.run_child(mcp_run_argv(&args))
    }

    fn status(&self) -> Result<Value, String> {
        Ok(agent_api::status_json(&self.workspace_root))
    }

    fn events_since(&self, since: u64, limit: Option<usize>) -> Result<Value, String> {
        let path = dmuxai::events::events_path(&self.workspace_root);
        let events = dmuxai::events::read_since(&path, since, Some(limit.unwrap_or(200)));
        let last_seq = events.last().map(|e| e.seq).unwrap_or(since);
        Ok(json!({ "events": events, "last_seq": last_seq }))
    }

    fn board_tool(&self, name: &str, args: &Value) -> Result<Value, String> {
        Ok(dmuxai::board::mcp_invoke(&self.workspace_root, name, args))
    }

    fn models(&self, agent: Option<&str>) -> Result<Value, String> {
        let settings = DmuxSettings::load(&self.workspace_root);
        let _ = dmuxai::custom_agent::install_registry(settings.custom_agents.clone());
        let fut = agent_api::collect_models_json(&self.workspace_root, &settings, agent);
        let value = match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
            Err(_) => tokio::runtime::Runtime::new()
                .map_err(|e| e.to_string())?
                .block_on(fut),
        };
        match value {
            Ok(v) => Ok(v),
            Err(err) => Ok(err.to_json()),
        }
    }
}

/// Waits for any of Ctrl+C, SIGTERM or SIGHUP (Unix); Ctrl+C only elsewhere.
async fn wait_for_stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM handler");
        let mut sighup = signal(SignalKind::hangup()).expect("SIGHUP handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
            _ = sighup.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn trust_workspace(
    workspace_root: &Path,
    status: bool,
    json: bool,
    revoke: bool,
    yes: bool,
) -> i32 {
    use dmuxai::runtime_env::RuntimeEnv;
    use dmuxai::workspace_trust::TrustStore;
    use std::io::{IsTerminal, Write};

    let env = RuntimeEnv::from_process();

    if revoke {
        let mut store = TrustStore::load(&env);
        if store.path().is_none() {
            eprintln!(
                "error: trust store path could not be resolved (neither DMUXAI_STATE_DIR, XDG_STATE_HOME, nor HOME is set)"
            );
            return dmuxai::agent_api::exit::FAILED;
        }
        let warning = store.corrupt_warning().map(str::to_owned);
        if !json && let Some(warning) = &warning {
            eprintln!(
                "warning: {}",
                dmuxai::workspace_trust::escape_control(warning)
            );
        }
        let removed = store.revoke(workspace_root);
        if let Err(error) = store.save() {
            eprintln!("error: {error}");
            return dmuxai::agent_api::exit::INTERNAL;
        }
        if json {
            let mut value = serde_json::json!({
                "status": "revoked",
                "removed": removed,
            });
            if let Some(warning) = &warning {
                value["warning"] = serde_json::json!(warning);
            }
            print_json(&value);
        } else if removed {
            println!("revoked");
        } else {
            println!("not recorded");
        }
        return dmuxai::agent_api::exit::OK;
    }

    let (exists, sha, restricted, details) =
        dmuxai::settings::inspect_workspace_trust_full(workspace_root, &env);

    if !exists {
        if status {
            if json {
                print_json(&serde_json::json!({"status": "no_workspace_config"}));
            } else {
                println!("no workspace config");
            }
        } else {
            println!("no workspace config");
        }
        return dmuxai::agent_api::exit::OK;
    }

    if restricted.is_empty() {
        if status {
            if json {
                print_json(&serde_json::json!({"status": "not_needed"}));
            } else {
                println!("not needed");
            }
        } else {
            println!("not needed");
        }
        return dmuxai::agent_api::exit::OK;
    }

    let mut store = TrustStore::load(&env);
    let warning = store.corrupt_warning().map(str::to_owned);
    if !(status && json)
        && let Some(warning) = &warning
    {
        eprintln!(
            "warning: {}",
            dmuxai::workspace_trust::escape_control(warning)
        );
    }
    let already = store.is_trusted(workspace_root, &sha);

    if status {
        let state = if already { "trusted" } else { "untrusted" };
        if json {
            let details_json: Vec<serde_json::Value> = details
                .iter()
                .map(|(k, v)| serde_json::json!({ "key": k, "value": v }))
                .collect();
            let mut value = serde_json::json!({
                "status": state,
                "sha256": sha,
                "ignored": restricted,
                "details": details_json,
            });
            if let Some(warning) = &warning {
                value["warning"] = serde_json::json!(warning);
            }
            print_json(&value);
        } else {
            println!("{state}");
        }
        return dmuxai::agent_api::exit::OK;
    }

    println!(
        "workspace: {}",
        dmuxai::workspace_trust::escape_control(&workspace_root.display().to_string())
    );
    println!("sha256: {sha}");
    println!(
        "restricted keys: {}",
        dmuxai::workspace_trust::escape_control(&restricted.join(", "))
    );
    if !details.is_empty() {
        println!("restricted values:");
        for (k, v) in &details {
            println!("  {k} = {v}");
        }
    }

    if already {
        println!("already trusted");
        return dmuxai::agent_api::exit::OK;
    }

    let should_trust = if yes {
        true
    } else if std::io::stdin().is_terminal() && std::io::stderr().is_terminal() {
        eprint!("Trust this workspace config? [y/N] ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(_) => {
                let answer = line.trim().to_ascii_lowercase();
                answer == "y" || answer == "yes"
            }
            Err(_) => false,
        }
    } else {
        eprintln!("not a TTY; pass --yes to trust");
        return dmuxai::agent_api::exit::FAILED;
    };

    if !should_trust {
        eprintln!("not trusted");
        return dmuxai::agent_api::exit::FAILED;
    }

    if store.path().is_none() {
        eprintln!(
            "error: trust store path could not be resolved (neither DMUXAI_STATE_DIR, XDG_STATE_HOME, nor HOME is set)"
        );
        return dmuxai::agent_api::exit::FAILED;
    }

    store.trust(workspace_root, &sha);
    if let Err(error) = store.save() {
        eprintln!("error: {error}");
        return dmuxai::agent_api::exit::INTERNAL;
    }
    println!("trusted");
    dmuxai::agent_api::exit::OK
}

#[cfg(test)]
mod tests {
    fn resolve_cli_builtin_runner(agent: &str) -> Option<&'static str> {
        match dmuxai::runner_id::canonical_runner_id(agent).as_str() {
            dmuxai::runner_id::CURSOR_RUNNER_ID => Some(dmuxai::runner_id::CURSOR_RUNNER_ID),
            "agy" => Some("agy"),
            "codex" => Some("codex"),
            _ => None,
        }
    }

    fn parse_cli_agents_csv(raw: &str) -> Vec<String> {
        raw.split(',')
            .map(dmuxai::runner_id::canonical_runner_id)
            .filter(|id| !id.is_empty())
            .collect()
    }
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    // TC-2910 / FR-2910
    #[test]
    fn tc_2910_agents_flag_parse_and_conflicts() {
        let legs = dmuxai::fanout::parse_agents_flag("cursor,codex").expect("parse");
        assert_eq!(legs.len(), 2);
        assert!(dmuxai::fanout::parse_agents_flag("cursor,cursor").is_err());
        assert!(dmuxai::fanout::parse_agents_flag("cursor").is_err());
        assert!(parse(&args(&["run", "x", "--agents", "cursor,codex"])).is_ok());
        assert!(
            parse(&args(&[
                "run",
                "x",
                "--agents",
                "cursor,codex",
                "--agent",
                "agy"
            ]))
            .is_err()
        );
        assert!(parse(&args(&["run", "x", "--agents", "cursor,codex", "--graph"])).is_err());
        assert!(
            parse(&args(&[
                "run",
                "x",
                "--agents",
                "cursor,codex",
                "--model",
                "m"
            ]))
            .is_err()
        );
        assert!(parse(&args(&["run", "x", "--min-ok", "2"])).is_err());
        assert!(parse(&args(&["todo", "run", "abc", "--agents", "a,b"])).is_err());
    }

    // TC-1492 / FR-1318: --mode is rejected with a parse error, not silently ignored.
    #[test]
    fn tc_1492_mode_flag_is_rejected_with_error() {
        let err = parse(&args(&["run", "x", "--mode", "ask"])).unwrap_err();
        assert!(err.contains("--mode was removed"), "{err}");

        let err_todo = parse(&args(&["todo", "run", "abc", "--mode", "ask"])).unwrap_err();
        assert!(err_todo.contains("--mode was removed"), "{err_todo}");

        assert!(parse(&args(&["run", "x", "--mode", "fast"])).is_err());
    }

    // TC-760 / FR-760, TC-1816 / FR-1816, TC-1817 / FR-1817, TC-1821 / FR-1821: agent-facing subcommands and flags.
    #[test]
    fn tc_760_1816_1817_1821_parse_cli_agent_api_subcommands() {
        assert_eq!(
            parse(&args(&[
                "run", "fix", "the", "bug", "--agent", "agy", "--json"
            ])),
            Ok(AgentCommand::Run(RunOptions {
                prompt: "fix the bug".into(),
                agent: Some("agy".into()),
                mode: None,
                read_only: false,
                graph: false,
                approve_plan: false,
                approve_plan_id: None,
                allow_sensitive: false,
                json: true,
                model: None,
                progress: false,
                trust_workspace_config: false,
                agents: None,
                min_ok: None,
                judge: None,
                ..Default::default()
            }))
        );
        assert_eq!(
            parse(&args(&["run", "--graph", "--", "--literal prompt"])),
            Ok(AgentCommand::Run(RunOptions {
                prompt: "--literal prompt".into(),
                graph: true,
                ..RunOptions::default()
            }))
        );
        assert!(parse(&args(&["run"])).is_err());
        assert!(parse(&args(&["run", "x", "--agent", "a", "--graph"])).is_err());
        assert!(parse(&args(&["run", "x", "--mode", "fast"])).is_err());
        assert!(parse(&args(&["run", "x", "--nope"])).is_err());
        assert_eq!(
            parse(&args(&["run", "deploy", "--allow-sensitive", "--json"])),
            Ok(AgentCommand::Run(RunOptions {
                prompt: "deploy".into(),
                allow_sensitive: true,
                json: true,
                ..RunOptions::default()
            }))
        );
        // A flag is never consumed as another flag's value.
        assert!(parse(&args(&["run", "x", "--agent", "--json"])).is_err());
        assert_eq!(
            parse(&args(&[
                "todo", "add", "Write", "docs", "--body", "B", "--json"
            ])),
            Ok(AgentCommand::TodoAdd {
                title: "Write docs".into(),
                body: "B".into(),
                json: true
            })
        );
        assert!(parse(&args(&["todo", "add"])).is_err());
        assert_eq!(
            parse(&args(&["todo", "list", "--status", "done"])),
            Ok(AgentCommand::TodoList {
                status: Some("done".into()),
                json: false
            })
        );
        assert!(parse(&args(&["todo", "list", "--status", "x"])).is_err());
        assert_eq!(
            parse(&args(&["todo", "run", "abc", "--graph", "--json"])),
            Ok(AgentCommand::TodoRun {
                id: "abc".into(),
                run: RunOptions {
                    graph: true,
                    json: true,
                    ..RunOptions::default()
                }
            })
        );
        assert!(parse(&args(&["todo", "run"])).is_err());
        assert!(parse(&args(&["todo", "run", "abc", "extra"])).is_err());
        assert!(parse(&args(&["todo", "zap"])).is_err());
        assert_eq!(
            parse(&args(&["status", "--json"])),
            Ok(AgentCommand::Status { json: true })
        );
        assert!(parse(&args(&["status", "--x"])).is_err());
        assert_eq!(
            parse(&args(&[
                "events", "--since", "7", "--follow", "--json", "--limit", "3"
            ])),
            Ok(AgentCommand::Events {
                since: 7,
                limit: Some(3),
                follow: true,
                json: true
            })
        );
        assert!(parse(&args(&["events", "--since", "x"])).is_err());
        assert!(parse(&args(&["events", "--limit", "0"])).is_err());
        assert_eq!(parse(&args(&["mcp"])), Ok(AgentCommand::Mcp));
        assert!(parse(&args(&["mcp", "--stdio"])).is_err());
    }

    // FR-814: `init` takes only --workspace and --force.
    #[test]
    fn parse_init_subcommand() {
        assert_eq!(
            parse(&args(&["init"])),
            Ok(AgentCommand::Init {
                workspace: false,
                force: false
            })
        );
        assert_eq!(
            parse(&args(&["init", "--workspace", "--force"])),
            Ok(AgentCommand::Init {
                workspace: true,
                force: true
            })
        );
        assert!(parse(&args(&["init", "--global"])).is_err());
        assert!(parse(&args(&["init", "extra"])).is_err());
    }

    // TC-2915 / FR-2915: fan-out JSON leg order and stable leg fields via outcome_json.
    #[test]
    fn tc_2915_json_output_shape_and_leg_order() {
        let outcome = HeadlessOutcome {
            ok: false,
            exit_code: 1,
            conversation_id: "c".to_owned(),
            agent: None,
            graph_id: None,
            text: String::new(),
            messages: Vec::new(),
            error: Some("fan-out quorum not met".to_owned()),
            error_kind: Some("policy".to_owned()),
            todo: None,
            triage: None,
            plan: None,
            plan_hash: None,
            system_prompt: None,
            run_id: None,
            fallbacks: Vec::new(),
            aggregate: None,
            aggregate_error: None,
            aggregate_skipped: None,
            fanout_judge: None,
            fanout: Some(dmuxai::fanout::FanoutReport {
                min_ok: 2,
                judge_enabled: false,
                legs: vec![
                    dmuxai::fanout::FanoutLegReport {
                        agent: "cursor".into(),
                        model: Some("m1".into()),
                        status: dmuxai::fanout::LegStatus::Ok,
                        text: "a".into(),
                        error: None,
                        error_kind: None,
                        blocked: false,
                        duration_ms: 1,
                        ..Default::default()
                    },
                    dmuxai::fanout::FanoutLegReport {
                        agent: "codex".into(),
                        model: None,
                        status: dmuxai::fanout::LegStatus::Failed,
                        text: String::new(),
                        error: Some("err".into()),
                        error_kind: Some("provider".into()),
                        blocked: false,
                        duration_ms: 2,
                        ..Default::default()
                    },
                ],
            }),
        };
        let json = outcome_json(&outcome, 0, 0);
        assert_eq!(json["error_kind"], "policy");
        let legs = json["fanout"]["legs"].as_array().expect("legs");
        assert_eq!(legs[0]["agent"], "cursor");
        assert_eq!(legs[1]["agent"], "codex");
        assert!(legs[1].get("text").is_some());
        assert!(legs[1].get("error").is_some());
        assert!(legs[1].get("error_kind").is_some());
    }

    // TC-2916 / FR-2916: resolved-model free detection for company workspace.
    #[test]
    fn tc_2916_company_workspace_skips_free_runners_with_warning() {
        let mut settings = DmuxSettings::default();
        settings
            .runner_models
            .insert("codex".into(), "gpt-x-free".into());
        let leg = dmuxai::fanout::FanoutLegSpec {
            runner: "codex".into(),
            model: None,
        };
        assert!(dmuxai::fanout::fanout_runner_is_free(&leg, &settings));
        let leg_explicit = dmuxai::fanout::FanoutLegSpec {
            runner: "cursor".into(),
            model: Some("composer-2.5".into()),
        };
        assert!(!dmuxai::fanout::fanout_runner_is_free(
            &leg_explicit,
            &settings
        ));
    }

    // TC-2924 / FR-2924
    #[test]
    fn tc_2924_json_output_has_aggregate_fields() {
        let aggregate = dmuxai::fanout::FanoutAggregate {
            agreements: vec!["both".into()],
            disagreements: Vec::new(),
            unique_findings: Default::default(),
            recommendation: "go".into(),
        };
        let outcome = HeadlessOutcome {
            ok: true,
            exit_code: 0,
            conversation_id: "c".into(),
            agent: None,
            graph_id: None,
            text: String::new(),
            messages: Vec::new(),
            error: None,
            error_kind: None,
            todo: None,
            triage: None,
            plan: None,
            plan_hash: None,
            system_prompt: None,
            run_id: None,
            fallbacks: Vec::new(),
            aggregate: Some(aggregate),
            aggregate_error: None,
            aggregate_skipped: None,
            fanout_judge: Some(dmuxai::fanout::FanoutJudgeMeta {
                agent: "codex".into(),
                model: None,
                duration_ms: 12,
            }),
            fanout: Some(dmuxai::fanout::FanoutReport {
                min_ok: 1,
                judge_enabled: true,
                legs: vec![
                    dmuxai::fanout::FanoutLegReport {
                        agent: "cursor".into(),
                        model: None,
                        status: dmuxai::fanout::LegStatus::Ok,
                        text: "a".into(),
                        error: None,
                        error_kind: None,
                        blocked: false,
                        duration_ms: 1,
                        ..Default::default()
                    },
                    dmuxai::fanout::FanoutLegReport {
                        agent: "codex".into(),
                        model: None,
                        status: dmuxai::fanout::LegStatus::Ok,
                        text: "b".into(),
                        error: None,
                        error_kind: None,
                        blocked: false,
                        duration_ms: 2,
                        ..Default::default()
                    },
                ],
            }),
        };
        let json = outcome_json(&outcome, 0, 0);
        assert_eq!(json["aggregate"]["recommendation"], "go");
        assert_eq!(json["judge"]["agent"], "codex");
        assert!(json["aggregate"].is_object());
        assert!(json["aggregate_skipped"].is_null());
        assert!(json["aggregate_error"].is_null());

        let no_judge = HeadlessOutcome {
            fanout: Some(dmuxai::fanout::FanoutReport {
                min_ok: 1,
                judge_enabled: false,
                legs: outcome.fanout.as_ref().unwrap().legs.clone(),
            }),
            ..outcome.clone()
        };
        let plain = outcome_json(&no_judge, 0, 0);
        assert!(plain.get("aggregate").is_none());
        assert!(plain.get("judge").is_none());
    }

    // TC-2926 / FR-2926
    #[test]
    fn tc_2935_sensitive_prompt_requires_approve_plan() {
        assert!(
            parse(&[
                "run".into(),
                "rotate password".into(),
                "--agents".into(),
                "cursor,codex".into(),
                "--write".into(),
            ])
            .is_ok()
        );
        assert!(parse(&["run".into(), "hi".into(), "--write".into(),]).is_err());
    }

    // TC-2936 / FR-2936
    #[test]
    fn tc_2936_company_workspace_skips_free_runners_in_write_mode() {
        let mut settings = DmuxSettings::default();
        settings.fanout_company_workspace = true;
        let free = dmuxai::fanout::FanoutLegSpec {
            runner: "codex".into(),
            model: Some("kimi-k2.5-free".into()),
        };
        assert!(dmuxai::fanout::fanout_runner_is_free(&free, &settings));
        let paid = dmuxai::fanout::FanoutLegSpec {
            runner: "cursor".into(),
            model: None,
        };
        assert!(!dmuxai::fanout::fanout_runner_is_free(&paid, &settings));
    }

    #[test]
    fn tc_2926_company_workspace_rejects_free_judge() {
        let mut settings = DmuxSettings::default();
        settings.fanout_company_workspace = true;
        let judge = dmuxai::fanout::parse_judge_flag("codex:kimi-k2.5-free").unwrap();
        assert!(dmuxai::fanout::fanout_runner_is_free(&judge, &settings));
        let err = dmuxai::fanout::reject_free_judge_in_company_workspace(&judge, true, &settings)
            .expect_err("free judge");
        assert!(err.contains("company workspace"));
        let warning = dmuxai::fanout::free_judge_company_workspace_warning(&judge.runner);
        assert!(warning.contains("warning:"));
        assert!(warning.contains("codex"));
        assert_eq!(dmuxai::agent_api::exit::USAGE, 2);
        assert!(
            crate::cli::parse(&[
                "run".into(),
                "x".into(),
                "--agents".into(),
                "cursor,codex".into(),
                "--judge".into(),
                "codex:kimi-k2.5-free".into(),
            ])
            .is_ok()
        );
        assert!(
            crate::cli::parse(&[
                "run".into(),
                "x".into(),
                "--agents".into(),
                "cursor,codex".into(),
                "--judge".into(),
                ":bad".into(),
            ])
            .is_err()
        );
    }

    // TC-1695 / FR-1682: headless JSON includes error_kind next to error.
    #[test]
    fn tc_1695_outcome_json_includes_error_kind() {
        let outcome = HeadlessOutcome {
            ok: false,
            exit_code: 1,
            conversation_id: "c".to_owned(),
            agent: Some("codex".to_owned()),
            graph_id: None,
            text: String::new(),
            messages: Vec::new(),
            error: Some("boom".to_owned()),
            error_kind: Some("provider".to_owned()),
            todo: None,
            triage: None,
            plan: None,
            plan_hash: None,
            system_prompt: None,
            run_id: Some("0123456789abcdef".to_owned()),
            fallbacks: Vec::new(),
            fanout: None,
            aggregate: None,
            aggregate_error: None,
            aggregate_skipped: None,
            fanout_judge: None,
        };
        let json = outcome_json(&outcome, 1, 2);
        assert_eq!(json["error"], "boom");
        assert_eq!(json["error_kind"], "provider");
        assert_eq!(json["run_id"], "0123456789abcdef");

        let outcome = HeadlessOutcome {
            ok: true,
            exit_code: 0,
            conversation_id: "c".to_owned(),
            agent: Some("codex".to_owned()),
            graph_id: None,
            text: String::new(),
            messages: Vec::new(),
            error: None,
            error_kind: None,
            todo: None,
            triage: None,
            plan: None,
            plan_hash: None,
            system_prompt: None,
            run_id: None,
            fallbacks: Vec::new(),
            fanout: None,
            aggregate: None,
            aggregate_error: None,
            aggregate_skipped: None,
            fanout_judge: None,
        };
        let json = outcome_json(&outcome, 0, 0);
        assert!(json["error"].is_null());
        assert!(json["error_kind"].is_null());
        assert!(json["run_id"].is_null());
    }

    // TC-1750 / FR-1730: headless --model parses for run and todo run
    #[test]
    fn tc_1750_headless_model_parses() {
        assert_eq!(
            parse(&args(&[
                "run",
                "x",
                "--agent",
                "agy",
                "--model",
                "gemini-3.8-flash-high",
                "--json"
            ])),
            Ok(AgentCommand::Run(RunOptions {
                prompt: "x".into(),
                agent: Some("agy".into()),
                mode: None,
                read_only: false,
                graph: false,
                approve_plan: false,
                approve_plan_id: None,
                allow_sensitive: false,
                json: true,
                model: Some("gemini-3.8-flash-high".into()),
                progress: false,
                trust_workspace_config: false,
                agents: None,
                min_ok: None,
                judge: None,
                ..Default::default()
            }))
        );
        assert!(parse(&args(&["run", "x", "--agent", "agy", "--model"])).is_err());
        assert!(parse(&args(&["run", "x", "--model", "m"])).is_err());
        assert!(parse(&args(&["run", "x", "--graph", "--model", "m"])).is_err());
        assert!(
            parse(&args(&[
                "run", "x", "--agent", "agy", "--graph", "--model", "m"
            ]))
            .is_err()
        );
        assert_eq!(
            parse(&args(&[
                "todo", "run", "abc", "--agent", "a", "--model", "m"
            ])),
            Ok(AgentCommand::TodoRun {
                id: "abc".into(),
                run: RunOptions {
                    agent: Some("a".into()),
                    model: Some("m".into()),
                    ..RunOptions::default()
                }
            })
        );
    }

    // TC-1763 / FR-1763: headless-style dashboard hub reports SnapshotSource::App
    #[test]
    fn tc_1763_headless_dashboard_source_is_app() {
        let dir = tempfile::tempdir().unwrap();
        let sink = dmuxai::events::EventSink::new(
            dmuxai::events::EventLog::open(dir.path(), "/test".to_owned()),
            std::time::Duration::from_secs(60),
            None,
        );
        let hub = headless_dashboard_hub("/test".to_owned(), sink);
        let snap = hub.snapshot();
        assert_eq!(snap.source, dmuxai::web::SnapshotSource::App);
        assert_eq!(
            serde_json::to_value(snap.source).unwrap(),
            serde_json::json!("app")
        );
    }

    // TC-1769: table-driven parser handles run flags and --progress.
    #[test]
    fn tc_1769_flag_table_parses_run_flags_and_progress() {
        assert_eq!(
            parse(&args(&[
                "run",
                "x",
                "--agent",
                "a",
                "--model",
                "m",
                "--progress",
                "--json"
            ])),
            Ok(AgentCommand::Run(RunOptions {
                prompt: "x".into(),
                agent: Some("a".into()),
                mode: None,
                read_only: false,
                graph: false,
                approve_plan: false,
                approve_plan_id: None,
                allow_sensitive: false,
                json: true,
                model: Some("m".into()),
                progress: true,
                trust_workspace_config: false,
                agents: None,
                min_ok: None,
                judge: None,
                ..Default::default()
            }))
        );
        assert_eq!(
            parse(&args(&["todo", "run", "abc", "--agent", "a", "--progress"])),
            Ok(AgentCommand::TodoRun {
                id: "abc".into(),
                run: RunOptions {
                    agent: Some("a".into()),
                    progress: true,
                    ..RunOptions::default()
                }
            })
        );
        match parse(&args(&["todo", "run", "abc", "--agent", "a"])) {
            Ok(AgentCommand::TodoRun { run, .. }) => assert!(!run.progress),
            other => panic!("unexpected {other:?}"),
        }
    }

    // TC-2509 / FR-2509, FR-2510: the MCP backend forwards the parity fields
    // to the child CLI as the same flags accepted by `run` / `todo run`.
    #[test]
    fn tc_2509_mcp_run_argv_forwards_parity_flags() {
        let args = RunArgs {
            prompt: Some("ship it".into()),
            agent: Some("agy".into()),
            model: Some("m".into()),
            approve_plan: true,
            approve_plan_id: None,
            allow_sensitive: true,
            graph: false,
            todo_id: None,
        };
        assert_eq!(
            mcp_run_argv(&args),
            vec![
                "run",
                "--agent",
                "agy",
                "--model",
                "m",
                "--approve-plan",
                "--allow-sensitive",
                "--json",
                "--",
                "ship it"
            ]
        );

        let todo = RunArgs {
            prompt: None,
            agent: Some("agy".into()),
            model: Some("m".into()),
            approve_plan: true,
            approve_plan_id: None,
            allow_sensitive: true,
            graph: false,
            todo_id: Some("t1".into()),
        };
        assert_eq!(
            mcp_run_argv(&todo),
            vec![
                "todo",
                "run",
                "t1",
                "--agent",
                "agy",
                "--model",
                "m",
                "--approve-plan",
                "--allow-sensitive",
                "--json"
            ]
        );

        let graph = RunArgs {
            prompt: Some("plan it".into()),
            graph: true,
            ..RunArgs::default()
        };
        assert_eq!(
            mcp_run_argv(&graph),
            vec!["run", "--graph", "--json", "--", "plan it"]
        );
    }

    // TC-2549 / FR-2546, FR-2548: CLI parses --approve-plan-id (which implies
    // plan approval and requires --graph) and MCP forwards it.
    #[test]
    fn tc_2549_approve_plan_id_cli_and_mcp_forwarding() {
        match parse(&args(&[
            "run",
            "--graph",
            "--approve-plan-id",
            "deadbeefdeadbeef",
            "do it",
        ])) {
            Ok(AgentCommand::Run(run)) => {
                assert!(run.graph);
                assert!(run.approve_plan, "--approve-plan-id implies approval");
                assert_eq!(run.approve_plan_id.as_deref(), Some("deadbeefdeadbeef"));
                assert_eq!(run.prompt, "do it");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            parse(&args(&[
                "run",
                "--approve-plan-id",
                "deadbeefdeadbeef",
                "do it"
            ])),
            Err("--approve-plan-id needs --graph".to_owned())
        );
        assert!(parse(&args(&["run", "--graph", "--approve-plan-id"])).is_err());

        let forwarded = RunArgs {
            prompt: Some("plan it".into()),
            approve_plan_id: Some("deadbeefdeadbeef".into()),
            graph: true,
            ..RunArgs::default()
        };
        assert_eq!(
            mcp_run_argv(&forwarded),
            vec![
                "run",
                "--approve-plan-id",
                "deadbeefdeadbeef",
                "--graph",
                "--json",
                "--",
                "plan it"
            ]
        );
    }

    // TC-2550 / FR-2550: --approve-plan-id must be exactly 16 lowercase hex
    // characters; anything else is a usage error before any filesystem access.
    #[test]
    fn tc_2550_approve_plan_id_rejects_non_hex() {
        for bad in [
            "../x",
            "ABC",
            "ABCDEF0123456789",
            "deadbeefdeadbee",
            "deadbeefdeadbeef1",
            "g000000000000000",
        ] {
            assert_eq!(
                parse(&args(&[
                    "run",
                    "--graph",
                    "--approve-plan-id",
                    bad,
                    "do it"
                ])),
                Err("--approve-plan-id must be 16 lowercase hex characters".to_owned())
            );
            assert_eq!(
                parse(&args(&["run", "--approve-plan-id", bad, "do it"])),
                Err("--approve-plan-id must be 16 lowercase hex characters".to_owned()),
                "format check precedes the graph check for {bad}"
            );
        }
        assert!(
            parse(&args(&[
                "run",
                "--graph",
                "--approve-plan-id",
                "deadbeefdeadbeef",
                "do it"
            ]))
            .is_ok()
        );
    }

    // TC-1770: error messages unchanged by table-driven parser.
    #[test]
    fn tc_1770_flag_table_errors_unchanged() {
        assert_eq!(
            parse(&args(&["run", "x", "--nope"])),
            Err("unexpected argument '--nope'".to_owned())
        );
        assert_eq!(
            parse(&args(&["run", "x", "--agent"])),
            Err("--agent needs a value".to_owned())
        );
        assert_eq!(
            parse(&args(&["run", "x", "--agent", "--json"])),
            Err("--agent needs a value".to_owned())
        );
        assert_eq!(
            parse(&args(&["run", "x", "--agent", "a", "--graph"])),
            Err("use either --agent or --graph, not both".to_owned())
        );
        assert_eq!(
            parse(&args(&["run", "x", "--graph", "--model", "m"])),
            Err("--model cannot be used with --graph".to_owned())
        );
        assert_eq!(
            parse(&args(&["run", "x", "--model", "m"])),
            Err("--model needs --agent".to_owned())
        );
        assert_eq!(
            parse(&args(&["run", "--json"])),
            Err("run needs a prompt (use - to read it from stdin)".to_owned())
        );
        assert_eq!(
            parse(&args(&["todo", "add", "--json"])),
            Err("todo add needs a title".to_owned())
        );
        assert_eq!(
            parse(&args(&["events", "--since", "x"])),
            Err("--since must be a number".to_owned())
        );
        assert_eq!(
            parse(&args(&["events", "--limit", "0"])),
            Err("--limit must be at least 1".to_owned())
        );
        assert_eq!(
            parse(&args(&["run", "--graph", "--", "--literal prompt"])),
            Ok(AgentCommand::Run(RunOptions {
                prompt: "--literal prompt".into(),
                graph: true,
                ..RunOptions::default()
            }))
        );
    }

    // TC-1815 / FR-1815: empty prompt or whitespace-only prompt fails with exit 2 message.
    #[test]
    fn tc_1815_empty_prompt_rejected_at_parse_and_stdin() {
        assert_eq!(
            parse(&args(&["run", "   "])),
            Err("run needs a prompt (use - to read it from stdin)".to_owned())
        );
        assert_eq!(
            parse(&args(&["run", ""])),
            Err("run needs a prompt (use - to read it from stdin)".to_owned())
        );
    }

    // TC-1771: help lists --progress, not --read-only; hidden flags.
    #[test]
    fn tc_1771_help_lists_progress_not_read_only() {
        let usage = crate::USAGE;
        assert!(usage.contains("--progress"));
        assert!(!usage.contains("--read-only"));
        let run_line = usage
            .lines()
            .find(|l| l.trim_start().starts_with("dmuxai run "))
            .expect("dmuxai run line");
        let normalized = run_line.replace(['[', ']', '(', ')'], " ");
        let tokens: Vec<&str> = normalized.split_whitespace().collect();
        for spec in RUN_FLAGS {
            let name = spec.names[0];
            let present = tokens.contains(&name);
            if spec.hidden {
                assert!(!present, "hidden flag {name} should not appear in run line");
            } else {
                assert!(present, "non-hidden flag {name} should appear in run line");
            }
        }
    }

    // TC-2120: a busy conversation database is a conflict (4), not an
    // internal error (6).
    #[test]
    fn tc_2120_busy_conversation_open_maps_to_exit_4() {
        let busy = ConversationError::Busy(
            "another dmuxai process is using this workspace's conversation database \
             (database is locked); retry when it finishes"
                .to_owned(),
        );
        let error = conversation_open_error(&busy);
        assert_eq!(error.code, exit::CONFLICT);
        assert_eq!(error.code, 4);
        assert!(error.message.contains("retry"));
        // FR-2140: the busy open carries the "conflict" error kind in JSON.
        assert_eq!(error.to_json()["error_kind"], "conflict");

        let other =
            conversation_open_error(&ConversationError::Sqlite(rusqlite::Error::InvalidQuery));
        assert_eq!(other.code, exit::INTERNAL);
        assert_eq!(other.code, 6);
        assert_eq!(other.to_json()["error_kind"], "internal");
    }

    // TC-2630 / FR-2630: `term approve <id> [--json]` validates the 16-hex id.
    #[test]
    fn tc_2630_parse_term_approve() {
        match parse(&args(&["term", "approve", "deadbeefdeadbeef"])) {
            Ok(AgentCommand::Term(TermCommand {
                op: TermOp::Approve { id },
                json,
            })) => {
                assert_eq!(id, "deadbeefdeadbeef");
                assert!(!json);
            }
            other => panic!("unexpected {other:?}"),
        }
        match parse(&args(&["term", "approve", "0123456789abcdef", "--json"])) {
            Ok(AgentCommand::Term(command)) => assert!(command.json),
            other => panic!("unexpected {other:?}"),
        }
        assert!(parse(&args(&["term", "approve"])).is_err());
        assert!(parse(&args(&["term", "approve", "nope"])).is_err());
        assert!(parse(&args(&["term", "approve", "DEADBEEFDEADBEEF"])).is_err());
        assert!(parse(&args(&["term", "approve", "deadbeefdeadbeef1"])).is_err());
        assert!(parse(&args(&["term", "approve", "extra", "deadbeefdeadbeef"])).is_err());
    }

    // TC-2615 / FR-2615: `term wait` parses `--since` as optional; absent
    // means the host starts the wait at the current sequence (no old match).
    #[test]
    fn tc_2615_parse_term_wait_since_is_optional() {
        match parse(&args(&[
            "term",
            "wait",
            "a",
            "--match",
            "r",
            "--timeout",
            "1s",
        ])) {
            Ok(AgentCommand::Term(command)) => match command.op {
                TermOp::Wait { since, .. } => assert_eq!(since, None),
                other => panic!("unexpected {other:?}"),
            },
            other => panic!("unexpected {other:?}"),
        }
        match parse(&args(&[
            "term",
            "wait",
            "a",
            "--match",
            "r",
            "--since",
            "7",
            "--timeout",
            "1s",
        ])) {
            Ok(AgentCommand::Term(command)) => match command.op {
                TermOp::Wait { since, .. } => assert_eq!(since, Some(7)),
                other => panic!("unexpected {other:?}"),
            },
            other => panic!("unexpected {other:?}"),
        }
    }

    // TC-2614 / FR-2614: `--no-usbipd-attach` lands both on the open op and
    // inside the serial config, so the request carries the flag to a warm host
    // without relying on env-var inheritance.
    #[test]
    fn tc_2614_parse_no_usbipd_attach_sets_serial_config() {
        let open_args = [
            "term",
            "open",
            "board",
            "--serial",
            "/dev/ttyMissing",
            "--baud",
            "115200",
            "--usbipd-busid",
            "9-99",
            "--no-usbipd-attach",
        ];
        match parse(&args(&open_args)) {
            Ok(AgentCommand::Term(TermCommand {
                op:
                    TermOp::Open {
                        serial,
                        no_usbipd_attach,
                        ..
                    },
                ..
            })) => {
                assert!(no_usbipd_attach, "op flag must be set");
                let serial = serial.expect("serial config present");
                assert!(
                    serial.no_usbipd_attach,
                    "config flag must travel to the host"
                );
                assert_eq!(serial.usbipd_busid.as_deref(), Some("9-99"));
            }
            other => panic!("unexpected {other:?}"),
        }
        // Without the flag both stay false.
        match parse(&args(&[
            "term",
            "open",
            "board",
            "--serial",
            "/dev/ttyMissing",
            "--baud",
            "115200",
            "--usbipd-busid",
            "9-99",
        ])) {
            Ok(AgentCommand::Term(TermCommand {
                op:
                    TermOp::Open {
                        serial,
                        no_usbipd_attach,
                        ..
                    },
                ..
            })) => {
                assert!(!no_usbipd_attach);
                assert!(!serial.expect("serial config present").no_usbipd_attach);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    // TC-2617 / FR-2617: every term subcommand has its own help topic; only
    // `term` itself (no subcommand) and unknown subcommands fall back to the
    // parent term list.
    #[test]
    fn tc_2617_term_subcommand_usage_topics() {
        for subcommand in [
            "open", "send", "read", "wait", "exec", "approve", "list", "show", "resize", "close",
            "reopen",
        ] {
            let text = term_subcommand_usage(subcommand);
            assert!(
                text.contains(&format!("Usage: dmuxai term {subcommand}")),
                "term {subcommand} help has its own usage line"
            );
            assert_ne!(text, TERM_USAGE);
            assert!(!text.contains("Subcommands:"));
        }
        assert_eq!(term_subcommand_usage("open"), TERM_OPEN_USAGE);
        assert_eq!(term_subcommand_usage("nope"), TERM_USAGE);
        for flag in ["--match", "--idle", "--timeout", "--since"] {
            assert!(
                term_subcommand_usage("wait").contains(flag),
                "term wait help documents {flag}"
            );
        }
    }

    // TC-2619 / FR-2604: a bare `--idle` value is milliseconds, while
    // suffixes keep their usual meaning.
    #[test]
    fn tc_2619_parse_term_wait_idle_bare_number_is_milliseconds() {
        for (raw, expected) in [("500", 500), ("500ms", 500), ("1s", 1_000)] {
            match parse(&args(&[
                "term",
                "wait",
                "a",
                "--idle",
                raw,
                "--timeout",
                "8s",
            ])) {
                Ok(AgentCommand::Term(command)) => match command.op {
                    TermOp::Wait { idle_ms, .. } => assert_eq!(idle_ms, Some(expected), "{raw}"),
                    other => panic!("unexpected {other:?}"),
                },
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    // TC-2619 / FR-2604: `--timeout` keeps seconds for a bare number.
    #[test]
    fn tc_2619_parse_term_wait_timeout_bare_number_is_seconds() {
        for raw in ["8", "8s"] {
            match parse(&args(&[
                "term",
                "wait",
                "a",
                "--match",
                "r",
                "--timeout",
                raw,
            ])) {
                Ok(AgentCommand::Term(command)) => match command.op {
                    TermOp::Wait { timeout_ms, .. } => {
                        assert_eq!(timeout_ms, 8_000, "{raw}")
                    }
                    other => panic!("unexpected {other:?}"),
                },
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    // TC-2832 / FR-2832: parsing runners and models commands
    #[test]
    fn tc_2832_parse_runners_and_models() {
        let cmd = parse(&["runners".into(), "--json".into()]).expect("parse runners");
        assert_eq!(cmd, AgentCommand::Runners { json: true });

        let cmd = parse(&[
            "models".into(),
            "--agent".into(),
            "agy".into(),
            "--json".into(),
        ])
        .expect("parse models");
        assert_eq!(
            cmd,
            AgentCommand::Models {
                agent: Some("agy".into()),
                json: true,
            }
        );

        let err = parse(&["runners".into(), "--bogus".into()]);
        assert!(err.is_err());

        let err = parse(&["models".into(), "--bogus".into()]);
        assert!(err.is_err());
    }

    // TC-2830 / FR-2830: subcommand usage texts list every non-hidden flag
    #[test]
    fn tc_2830_help_lists_parser_flags() {
        let cases: &[(&str, &[FlagSpec])] = &[
            (RUN_USAGE, RUN_FLAGS),
            (TODO_USAGE, TODO_RUN_FLAGS),
            (STATUS_USAGE, STATUS_FLAGS),
            (EVENTS_USAGE, EVENTS_FLAGS),
            (INIT_USAGE, INIT_FLAGS),
            (TRUST_USAGE, TRUST_FLAGS),
            (RUNS_USAGE, RUNS_FLAGS),
            (RUNNERS_USAGE, RUNNERS_FLAGS),
            (MODELS_USAGE, MODELS_FLAGS),
        ];

        for (usage, specs) in cases {
            for spec in *specs {
                if !spec.hidden {
                    for name in spec.names {
                        assert!(
                            usage.contains(name),
                            "usage text missing flag {name}:\n{usage}"
                        );
                    }
                }
            }
        }

        for spec in TODO_ADD_FLAGS {
            if !spec.hidden {
                for name in spec.names {
                    assert!(TODO_USAGE.contains(name), "TODO_USAGE missing {name}");
                }
            }
        }
        for spec in TODO_LIST_FLAGS {
            if !spec.hidden {
                for name in spec.names {
                    assert!(TODO_USAGE.contains(name), "TODO_USAGE missing {name}");
                }
            }
        }
        for spec in RUNS_SHOW_FLAGS {
            if !spec.hidden {
                for name in spec.names {
                    assert!(RUNS_USAGE.contains(name), "RUNS_USAGE missing {name}");
                }
            }
        }
    }

    // TC-3102 / FR-3102: the CLI executor validates ids and enums before it
    // opens (and creates) the board store.
    #[test]
    fn tc_3102_invalid_ids_and_limits_rejected_before_fs_access() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let _guard = dmuxai::state_path::test_env_lock();
        // SAFETY: test-only mutation of the process environment.
        unsafe { std::env::set_var("DMUXAI_STATE_DIR", &state) };
        let bad_id = execute_board(
            Path::new("/tmp"),
            BoardCommand {
                op: BoardOp::Show {
                    id: "../items/x".to_owned(),
                },
                json: true,
            },
        );
        let bad_lane = execute_board(
            Path::new("/tmp"),
            BoardCommand {
                op: BoardOp::Move {
                    id: "b000000000000".to_owned(),
                    lane: "not-a-lane".to_owned(),
                    if_version: None,
                },
                json: true,
            },
        );
        let bad_add_lane = execute_board(
            Path::new("/tmp"),
            BoardCommand {
                op: BoardOp::Add {
                    title: "t".to_owned(),
                    summary: String::new(),
                    kind: "debug".to_owned(),
                    grade: None,
                    lane: Some("not-a-lane".to_owned()),
                    bot: None,
                    channel: None,
                    ref_url: None,
                    workspace: None,
                    recommendation: None,
                    options: vec![],
                    plan_hash: None,
                    key: None,
                },
                json: true,
            },
        );
        unsafe { std::env::remove_var("DMUXAI_STATE_DIR") };
        assert_eq!(bad_id, 2);
        assert_eq!(bad_lane, 2);
        assert_eq!(bad_add_lane, 2);
        assert!(
            !state.join("board").exists(),
            "invalid input must not create the board directory"
        );
    }

    // TC-3105 / FR-3105: board add parses workspace flag.
    #[test]
    fn tc_3105_board_add_cli_records_workspace_and_prints_id() {
        let cmd = parse(&args(&[
            "board",
            "add",
            "--title",
            "Ship",
            "--kind",
            "feature",
            "-C",
            "/tmp/repo",
            "--json",
        ]))
        .unwrap();
        match cmd {
            AgentCommand::Board(BoardCommand {
                op: BoardOp::Add {
                    title, workspace, ..
                },
                json: true,
            }) => {
                assert_eq!(title, "Ship");
                assert_eq!(workspace, Some(PathBuf::from("/tmp/repo")));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    // TC-3114 / FR-3114: board help lists subcommands.
    #[test]
    fn tc_3114_board_help_lists_subcommands() {
        for sub in [
            "add", "list", "show", "update", "move", "plan", "decide", "say", "wait", "events",
            "prompt", "rules", "ask", "run", "pending", "check",
        ] {
            assert!(BOARD_USAGE.contains(sub), "BOARD_USAGE missing {sub}");
        }
        assert!(board_subcommand_usage("add").contains("--title"));
        assert!(board_subcommand_usage("wait").contains("--timeout"));
        assert!(board_subcommand_usage("pending").contains("--workspace"));
        assert!(board_subcommand_usage("check").contains("--text"));
    }

    // TC-3138 / FR-3105, FR-3110, FR-3133, FR-3138: CLI non-TTY add --lane auto -> 5,
    // TTY move reaches confirmation, and exec-prepared works without TTY.
    #[test]
    fn tc_3138_cli_add_auto_move_confirm_and_exec_prepared_non_tty() {
        let _guard = dmuxai::state_path::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        unsafe { std::env::set_var("DMUXAI_STATE_DIR", &state) };

        // 1. Non-TTY board add --lane auto -> exit 5 (Forbidden).
        let add_auto_exit = execute_board_with_caller(
            &ws,
            BoardCommand {
                op: BoardOp::Add {
                    title: "Auto from non-tty".to_owned(),
                    summary: String::new(),
                    kind: "feature".to_owned(),
                    grade: None,
                    lane: Some("auto".to_owned()),
                    bot: None,
                    channel: None,
                    ref_url: None,
                    workspace: Some(ws.clone()),
                    recommendation: None,
                    options: vec![],
                    plan_hash: None,
                    key: None,
                },
                json: true,
            },
            dmuxai::board::BoardCaller::CliNonTty,
        );
        assert_eq!(add_auto_exit, exit::FORBIDDEN);

        // 2. TTY move to auto reaches confirmation prompt (injectable prompt).
        let store = dmuxai::board::BoardStore::open(state.join("board")).unwrap();
        let item = store
            .add(dmuxai::board::AddItemRequest {
                title: "Move me".to_owned(),
                summary: String::new(),
                kind: dmuxai::board::Kind::Feature,
                grade: None,
                lane: None,
                source_bot: None,
                source_channel: None,
                source_ref: None,
                workspace: Some(ws.clone()),
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:test".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        let mut prompted = false;
        let mismatch = run_board_move(
            &store,
            &item.id,
            dmuxai::board::Lane::Auto,
            "owner",
            None,
            |id, verb, title| {
                prompted = true;
                dmuxai::board::confirm_owner_action(id, verb, title, |_| Ok("wrong".to_owned()))
                    .map_err(ApiError::from)
            },
        )
        .unwrap_err();
        assert!(prompted, "TTY move to auto must reach confirmation");
        assert_eq!(mismatch.code, exit::FORBIDDEN);

        let moved = run_board_move(
            &store,
            &item.id,
            dmuxai::board::Lane::Auto,
            "owner",
            None,
            |id, verb, title| {
                dmuxai::board::confirm_owner_action(id, verb, title, |_| Ok(id.to_owned()))
                    .map_err(ApiError::from)
            },
        )
        .unwrap();
        assert_eq!(moved.lane, dmuxai::board::Lane::Auto);

        // 3. --exec-prepared --by owner works without TTY even when DMUXAI_RUN_ID is set.
        let plan_json = r#"{"tasks":[{"id":"task-1","title":"t","done_check":"d","size":"small","depends_on":[]}],"steps":[{"id":"s1","role":"implement","job":"j","runner":"cursor","depends_on":[],"task":"task-1"}]}"#;
        let (_, hash) =
            dmuxai::board::edit_plan_from_json(&ws, &state, "ship", plan_json, None).unwrap();
        store
            .attach_plan(&item.id, &hash, &state, "bot:test")
            .unwrap();
        let prepared = dmuxai::board::runner::run_item_prepare(
            &store,
            dmuxai::board::runner::RunParams {
                id: item.id.clone(),
                expected_plan_hash: Some(hash.clone()),
                by: "owner".to_owned(),
                state_root: state.clone(),
            },
        )
        .unwrap();
        prepared.guard.disarm();

        let fake_bin = dir.path().join("fake-exec.sh");
        std::fs::write(
            &fake_bin,
            "#!/bin/sh\ncat > /dev/null\nprintf '{\"ok\":true,\"exit_code\":0,\"run_id\":\"0123456789abcdef\",\"text\":\"done\"}\\n'\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake_bin).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake_bin, perms).unwrap();
        }
        unsafe {
            std::env::set_var("DMUXAI_BOARD_CHILD_BIN", &fake_bin);
            std::env::set_var(dmuxai::term::policy::RUN_ID_ENV, "run-123");
        }
        let exec_exit = execute_board_with_caller(
            &ws,
            BoardCommand {
                op: BoardOp::Run {
                    id: item.id.clone(),
                    detach: false,
                    by: Some("owner".to_owned()),
                    exec_prepared: true,
                    plan_hash: Some(hash),
                },
                json: true,
            },
            dmuxai::board::BoardCaller::CliNonTty,
        );
        unsafe {
            std::env::remove_var("DMUXAI_BOARD_CHILD_BIN");
            std::env::remove_var(dmuxai::term::policy::RUN_ID_ENV);
            std::env::remove_var("DMUXAI_STATE_DIR");
        }
        assert_eq!(exec_exit, exit::OK);
        assert_eq!(store.get(&item.id).unwrap().lane, dmuxai::board::Lane::Done);
    }

    // TC-3201 / FR-3201
    #[test]
    fn tc_3201_cli_accepts_cursor_agent_keyword() {
        let run = parse(&args(&["run", "hi", "--agent", "cursor-agent"])).unwrap();
        match run {
            AgentCommand::Run(opts) => assert_eq!(opts.agent.as_deref(), Some("cursor-agent")),
            other => panic!("unexpected {other:?}"),
        }
        let run_legacy = parse(&args(&["run", "hi", "--agent", "cursor"])).unwrap();
        match run_legacy {
            AgentCommand::Run(opts) => assert_eq!(opts.agent.as_deref(), Some("cursor")),
            other => panic!("unexpected {other:?}"),
        }
        let models = parse(&args(&["models", "--agent", "cursor-agent"])).unwrap();
        match models {
            AgentCommand::Models { agent, .. } => {
                assert_eq!(agent.as_deref(), Some("cursor-agent"))
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            parse_cli_agents_csv("cursor-agent,agy"),
            vec![
                dmuxai::runner_id::CURSOR_RUNNER_ID.to_owned(),
                "agy".to_owned()
            ]
        );
        assert_eq!(
            parse_cli_agents_csv("cursor,agy"),
            vec![
                dmuxai::runner_id::CURSOR_RUNNER_ID.to_owned(),
                "agy".to_owned()
            ]
        );
        assert_eq!(
            resolve_cli_builtin_runner("cursor-agent"),
            Some(dmuxai::runner_id::CURSOR_RUNNER_ID)
        );
        assert_eq!(
            resolve_cli_builtin_runner("cursor"),
            Some(dmuxai::runner_id::CURSOR_RUNNER_ID)
        );
        let msg = dmuxai::runner_id::format_unknown_agent_message("nope");
        assert!(msg.contains("cursor-agent"));
        assert!(msg.contains("nope"));
    }
}
