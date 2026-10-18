//! Agent-facing CLI subcommands (SPEC-agent-api FR-760..FR-767):
//! `run`, `todo add|list|run`, `status`, `events`, `mcp`, `init`.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use dmuxai::agent_api::{self, ApiError, exit};
use dmuxai::events::{EventLog, EventSink};
use dmuxai::mcp_server::{McpBackend, RunArgs};
use dmuxai::settings::DmuxSettings;

use crate::agent::ExecutionMode;
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
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOptions {
    pub prompt: String,
    pub agent: Option<String>,
    pub mode: Option<String>,
    pub graph: bool,
    pub json: bool,
}

/// First arguments that select an agent-facing subcommand.
pub const SUBCOMMANDS: [&str; 6] = ["run", "todo", "status", "events", "mcp", "init"];

fn value<'a>(args: &'a [String], index: &mut usize, flag: &str) -> Result<&'a str, String> {
    *index += 1;
    args.get(*index)
        .map(String::as_str)
        .filter(|value| !value.starts_with("--"))
        .ok_or_else(|| format!("{flag} needs a value"))
}

fn parse_mode(raw: &str) -> Result<String, String> {
    match raw.to_ascii_lowercase().as_str() {
        m @ ("ask" | "plan" | "implement") => Ok(m.to_owned()),
        _ => Err("--mode must be ask, plan or implement".to_owned()),
    }
}

/// Parses run-style flags; positional words become the prompt.
fn parse_run_options(args: &[String], allow_prompt: bool) -> Result<RunOptions, String> {
    let mut options = RunOptions::default();
    let mut words: Vec<String> = Vec::new();
    let mut index = 0;
    let mut rest_is_prompt = false;
    while index < args.len() {
        let arg = args[index].as_str();
        if rest_is_prompt {
            words.push(arg.to_owned());
        } else {
            match arg {
                "--" => rest_is_prompt = true,
                "--agent" => options.agent = Some(value(args, &mut index, "--agent")?.to_owned()),
                "--mode" => options.mode = Some(parse_mode(value(args, &mut index, "--mode")?)?),
                "--graph" => options.graph = true,
                "--json" => options.json = true,
                flag if flag.starts_with("--") => {
                    return Err(format!("unexpected argument '{flag}'"));
                }
                word => words.push(word.to_owned()),
            }
        }
        index += 1;
    }
    if options.graph && options.agent.is_some() {
        return Err("use either --agent or --graph, not both".to_owned());
    }
    if allow_prompt {
        options.prompt = words.join(" ");
    } else if let Some(word) = words.first() {
        return Err(format!("unexpected argument '{word}'"));
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
                    let mut title = Vec::new();
                    let mut body = String::new();
                    let mut json = false;
                    let mut index = 0;
                    while index < rest.len() {
                        match rest[index].as_str() {
                            "--body" => body = value(rest, &mut index, "--body")?.to_owned(),
                            "--json" => json = true,
                            flag if flag.starts_with("--") => {
                                return Err(format!("unexpected argument '{flag}'"));
                            }
                            word => title.push(word.to_owned()),
                        }
                        index += 1;
                    }
                    let title = title.join(" ");
                    if title.trim().is_empty() {
                        return Err("todo add needs a title".to_owned());
                    }
                    Ok(AgentCommand::TodoAdd { title, body, json })
                }
                "list" => {
                    let mut status = None;
                    let mut json = false;
                    let mut index = 0;
                    while index < rest.len() {
                        match rest[index].as_str() {
                            "--status" => {
                                let raw = value(rest, &mut index, "--status")?;
                                agent_api::parse_todo_status(raw).map_err(|e| e.message)?;
                                status = Some(raw.to_ascii_lowercase());
                            }
                            "--json" => json = true,
                            other => return Err(format!("unexpected argument '{other}'")),
                        }
                        index += 1;
                    }
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
        "status" => match rest {
            [] => Ok(AgentCommand::Status { json: false }),
            [flag] if flag == "--json" => Ok(AgentCommand::Status { json: true }),
            [other, ..] => Err(format!("unexpected argument '{other}'")),
        },
        "events" => {
            let mut since = 0;
            let mut limit = None;
            let mut follow = false;
            let mut json = false;
            let mut index = 0;
            while index < rest.len() {
                match rest[index].as_str() {
                    "--since" => {
                        since = value(rest, &mut index, "--since")?
                            .parse()
                            .map_err(|_| "--since must be a number".to_owned())?;
                    }
                    "--limit" => {
                        let parsed: usize = value(rest, &mut index, "--limit")?
                            .parse()
                            .map_err(|_| "--limit must be a number".to_owned())?;
                        if parsed == 0 {
                            return Err("--limit must be at least 1".to_owned());
                        }
                        limit = Some(parsed);
                    }
                    "--follow" | "-f" => follow = true,
                    "--json" => json = true,
                    other => return Err(format!("unexpected argument '{other}'")),
                }
                index += 1;
            }
            Ok(AgentCommand::Events {
                since,
                limit,
                follow,
                json,
            })
        }
        "mcp" => match rest {
            [] => Ok(AgentCommand::Mcp),
            [other, ..] => Err(format!("unexpected argument '{other}'")),
        },
        "init" => {
            let mut workspace = false;
            let mut force = false;
            for arg in rest {
                match arg.as_str() {
                    "--workspace" => workspace = true,
                    "--force" => force = true,
                    other => return Err(format!("unexpected argument '{other}'")),
                }
            }
            Ok(AgentCommand::Init { workspace, force })
        }
        other => Err(format!("unknown subcommand '{other}'")),
    }
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
    let from = event.from_state.as_deref().unwrap_or("-");
    let message = event
        .message
        .as_deref()
        .map(|m| format!(" — {m}"))
        .unwrap_or_default();
    format!(
        "#{} {} {} {} {} -> {}{}",
        event.seq, event.ts, event.kind, subject, from, event.to_state, message
    )
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

fn parse_execution_mode(mode: &str) -> Option<ExecutionMode> {
    match mode {
        "ask" => Some(ExecutionMode::Ask),
        "plan" => Some(ExecutionMode::Plan),
        "implement" => Some(ExecutionMode::Implement),
        _ => None,
    }
}

fn outcome_json(outcome: &HeadlessOutcome, first_seq: u64, last_seq: u64) -> Value {
    json!({
        "ok": outcome.ok,
        "exit_code": outcome.exit_code,
        "conversation_id": outcome.conversation_id,
        "agent": outcome.agent,
        "graph_id": outcome.graph_id,
        "text": outcome.text,
        "messages": outcome.messages,
        "error": outcome.error,
        "todo": outcome.todo,
        // Both `null` when the run wrote no events (e.g. rejected before start).
        "events": if last_seq >= first_seq {
            json!({ "first_seq": first_seq, "last_seq": last_seq })
        } else {
            json!({ "first_seq": null, "last_seq": null })
        },
    })
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

async fn run_headless(
    workspace_root: &Path,
    options: &RunOptions,
    prompt: Option<String>,
    todo_id: Option<String>,
) -> Result<(HeadlessOutcome, u64, u64), ApiError> {
    let internal = |e: &dyn std::fmt::Display| ApiError::new(exit::INTERNAL, e.to_string());
    dmuxai::workspace::ensure_directory_writable(workspace_root).map_err(|e| internal(&e))?;
    let settings = DmuxSettings::load(workspace_root);
    for warning in &settings.config_warnings {
        eprintln!("warning: {warning}");
    }
    let mode = options
        .mode
        .as_deref()
        .map(|m| parse_execution_mode(m).ok_or_else(|| ApiError::new(exit::USAGE, "invalid mode")));
    let mode = mode.transpose()?;
    let (db_path, notice) =
        dmuxai::state_path::prepare_repo_db(workspace_root).map_err(|e| internal(&e))?;
    if let Some(notice) = notice {
        eprintln!("warning: {notice}");
    }
    let engine = dmuxai::conversation::ConversationEngine::open(&db_path, workspace_root)
        .map_err(|e| internal(&e))?;
    for warning in dmuxai::custom_agent::install_registry(settings.custom_agents.clone()) {
        eprintln!("warning: {warning}");
    }
    let sink = build_event_sink(workspace_root, &settings);
    let first_seq = sink.log().last_seq() + 1;
    let workspace = dmuxai::workspace::normalize_workspace_path(workspace_root)
        .unwrap_or_else(|_| workspace_root.display().to_string());
    let hub = dmuxai::web::DashboardHub::new(dmuxai::web::SnapshotSource::Tui, workspace)
        .with_events(sink.clone());

    let mut app =
        App::new(engine, workspace_root.to_path_buf(), settings).map_err(|e| internal(&e))?;
    app.attach_dashboard(hub);
    if todo_id.is_some() {
        let store = agent_api::open_todo_store(workspace_root)?;
        app.attach_todo_store(Arc::new(store));
    }

    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_signal = cancel.clone();
    let signal_task = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancel_signal.store(true, Ordering::SeqCst);
        }
    });
    let request = HeadlessRequest {
        prompt: prompt.unwrap_or_default(),
        agent: options.agent.clone(),
        mode,
        graph: options.graph,
        todo_id,
    };
    let outcome = app.run_headless(request, cancel).await;
    app.shutdown().await;
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
        let mut argv: Vec<String> = match &args.todo_id {
            Some(id) => vec!["todo".into(), "run".into(), id.clone()],
            None => vec!["run".into()],
        };
        if let Some(agent) = args.agent {
            argv.extend(["--agent".into(), agent]);
        }
        if let Some(mode) = args.mode {
            argv.extend(["--mode".into(), mode]);
        }
        if args.graph {
            argv.push("--graph".into());
        }
        argv.push("--json".into());
        if args.todo_id.is_none() {
            argv.push("--".into());
            argv.push(args.prompt.unwrap_or_default());
        }
        self.run_child(argv)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    // TC-760 / FR-760: agent-facing subcommands and flags.
    #[test]
    fn parse_cli_agent_api_subcommands() {
        assert_eq!(
            parse(&args(&[
                "run", "fix", "the", "bug", "--agent", "agy", "--mode", "Plan", "--json"
            ])),
            Ok(AgentCommand::Run(RunOptions {
                prompt: "fix the bug".into(),
                agent: Some("agy".into()),
                mode: Some("plan".into()),
                graph: false,
                json: true,
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
}
