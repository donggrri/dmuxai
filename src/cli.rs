//! Agent-facing CLI subcommands (SPEC-agent-api FR-760..FR-767):
//! `run`, `todo add|list|run`, `status`, `events`, `mcp`, `init`.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

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
    /// Persistent named terminal sessions (SPEC-terminal-sessions).
    Term(TermCommand),
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
        since: u64,
    },
    Exec {
        name: String,
        command: String,
        timeout_ms: Option<u64>,
    },
    List,
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
}

/// First arguments that select an agent-facing subcommand.
pub const SUBCOMMANDS: [&str; 9] = [
    "run", "todo", "status", "events", "mcp", "init", "trust", "runs", "term",
];

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

const RUN_FLAGS: &[FlagSpec] = &[
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

/// Parses run-style flags; positional words become the prompt.
fn parse_run_options(args: &[String], allow_prompt: bool) -> Result<RunOptions, String> {
    let parsed = parse_flags(args, RUN_FLAGS, allow_prompt)?;
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
    };
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
                    const TODO_ADD_FLAGS: &[FlagSpec] = &[
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
                    const TODO_LIST_FLAGS: &[FlagSpec] = &[
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
                    let run = parse_run_options(&rest[1..], false)?;
                    Ok(AgentCommand::TodoRun {
                        id: id.clone(),
                        run,
                    })
                }
                other => Err(format!("unknown todo subcommand '{other}'")),
            }
        }
        "status" => {
            const STATUS_FLAGS: &[FlagSpec] = &[FlagSpec {
                names: &["--json"],
                takes_value: false,
                hidden: false,
            }];
            let parsed = parse_flags(rest, STATUS_FLAGS, false)?;
            Ok(AgentCommand::Status {
                json: parsed.flags.contains_key("--json"),
            })
        }
        "events" => {
            const EVENTS_FLAGS: &[FlagSpec] = &[
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
            const INIT_FLAGS: &[FlagSpec] = &[
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
            let parsed = parse_flags(rest, INIT_FLAGS, false)?;
            Ok(AgentCommand::Init {
                workspace: parsed.flags.contains_key("--workspace"),
                force: parsed.flags.contains_key("--force"),
            })
        }
        "trust" => {
            const TRUST_FLAGS: &[FlagSpec] = &[
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
            let parsed = parse_flags(rest, TRUST_FLAGS, false)?;
            Ok(AgentCommand::Trust {
                status: parsed.flags.contains_key("--status"),
                json: parsed.flags.contains_key("--json"),
                revoke: parsed.flags.contains_key("--revoke"),
                yes: parsed.flags.contains_key("--yes"),
            })
        }
        "runs" => {
            const RUNS_FLAGS: &[FlagSpec] = &[
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

            if rest.first().map(String::as_str) == Some("show") {
                const SHOW_FLAGS: &[FlagSpec] = &[FlagSpec {
                    names: &["--json"],
                    takes_value: false,
                    hidden: false,
                }];
                let parsed = parse_flags(&rest[1..], SHOW_FLAGS, true)?;
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
        "term" => parse_term(rest).map(AgentCommand::Term),
        other => Err(format!("unknown subcommand '{other}'")),
    }
}

fn parse_term(args: &[String]) -> Result<TermCommand, String> {
    let Some(subcommand) = args.first() else {
        return Err(
            "term needs a subcommand: open, send, read, wait, exec, list, show, resize, close or reopen"
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
            let idle_ms = duration_flag(&parsed, "--idle", "--idle")?;
            if regex.is_some() == idle_ms.is_some() {
                return Err("term wait needs exactly one of --match or --idle".to_owned());
            }
            if let Some(pattern) = &regex {
                regex::Regex::new(pattern)
                    .map_err(|error| format!("invalid regex '{pattern}': {error}"))?;
            }
            let timeout_ms = duration_flag(&parsed, "--timeout", "--timeout")?
                .ok_or_else(|| "term wait needs --timeout".to_owned())?;
            let since = optional_u64_flag(&parsed, "--since", "--since")?.unwrap_or(0);
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
        "list" => {
            const LIST_FLAGS: &[FlagSpec] = &[FlagSpec {
                names: &["--json"],
                takes_value: false,
                hidden: false,
            }];
            let parsed = parse_flags(rest, LIST_FLAGS, false)?;
            Ok(TermCommand {
                op: TermOp::List,
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

fn parse_term_open(args: &[String]) -> Result<TermCommand, String> {
    let mut name = None;
    let mut kind = SessionKind::Shell;
    let mut cwd = None;
    let mut env = Vec::new();
    let mut cols = term::DEFAULT_COLS;
    let mut rows = term::DEFAULT_ROWS;
    let mut json = false;
    let mut argv = Vec::new();
    let mut after_separator = false;
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
                kind = match args.get(index).map(String::as_str) {
                    Some("shell") => SessionKind::Shell,
                    Some("cmd") => SessionKind::Cmd,
                    _ => return Err("--kind must be 'shell' or 'cmd'".to_owned()),
                };
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
    if kind == SessionKind::Cmd && argv.is_empty() {
        return Err("--kind cmd needs a command after --".to_owned());
    }
    if kind == SessionKind::Shell && !argv.is_empty() {
        return Err("shell sessions do not take a command after --".to_owned());
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
    parsed
        .flags
        .get(name)
        .cloned()
        .flatten()
        .map(|raw| parse_duration_ms(&raw, label))
        .transpose()
}

fn parse_duration_ms(raw: &str, label: &str) -> Result<u64, String> {
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
        (trimmed, 1_000.0)
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
        AgentCommand::Term(command) => execute_term(&workspace_root, command),
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
    if matches!(&op, TermOp::Host) {
        return match dmuxai::term::host::run(workspace_root) {
            Ok(()) => exit::OK,
            Err(error) => term_fail(error, false),
        };
    }

    let response = match &op {
        TermOp::List => match dmuxai::term::client::try_connect(workspace_root) {
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
fn report_term_response(op: &TermOp, response: Response, json: bool) -> i32 {
    let code = response.exit_code.unwrap_or(exit::OK);
    let data = response.data.unwrap_or(Value::Null);
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
        } => RequestOp::Open {
            name: name.clone(),
            kind: *kind,
            cwd: cwd.as_ref().map(|path| path.display().to_string()),
            env: env.clone(),
            cols: *cols,
            rows: *rows,
            argv: argv.clone(),
        },
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
        TermOp::List => RequestOp::List,
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
        TermOp::List => {
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
    let mut value = json!({
        "ok": outcome.ok,
        "exit_code": outcome.exit_code,
        "conversation_id": outcome.conversation_id,
        "agent": outcome.agent,
        "graph_id": outcome.graph_id,
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

async fn run_headless(
    workspace_root: &Path,
    options: &RunOptions,
    prompt: Option<String>,
    todo_id: Option<String>,
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
    let first_seq = sink.log().last_seq() + 1;
    let workspace = dmuxai::workspace::normalize_workspace_path(workspace_root)
        .unwrap_or_else(|_| workspace_root.display().to_string());
    let hub = headless_dashboard_hub(workspace, sink.clone());

    let mut app = App::new(engine, workspace_root.to_path_buf(), settings)
        .map_err(|error| conversation_open_error(&error))?;
    app.attach_dashboard(hub);
    if options.progress {
        app.set_progress_sink(Box::new(|line: &str| {
            eprintln!("{line}");
        }));
    }
    if todo_id.is_some() {
        let store = agent_api::open_todo_store(workspace_root)?;
        app.attach_todo_store(Arc::new(store));
    }

    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_signal = cancel.clone();
    let finished = Arc::new(AtomicBool::new(false));
    let finished_signal = finished.clone();
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
    let outcome = app.run_headless(request, cancel).await;
    app.shutdown().await;
    finished.store(true, Ordering::SeqCst);
    signal_task.abort();
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
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
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
            fallbacks: Vec::new(),
        };
        let json = outcome_json(&outcome, 1, 2);
        assert_eq!(json["error"], "boom");
        assert_eq!(json["error_kind"], "provider");

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
            fallbacks: Vec::new(),
        };
        let json = outcome_json(&outcome, 0, 0);
        assert!(json["error"].is_null());
        assert!(json["error_kind"].is_null());
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
}
