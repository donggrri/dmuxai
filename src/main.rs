mod agent;
mod app;
mod cli;
mod process;

use std::{
    error::Error,
    io::{self, IsTerminal, Stdout, Write},
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use app::App;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use dmuxai::conversation::ConversationEngine;
use dmuxai::settings::DmuxSettings;
use dmuxai::todo::TodoStore;
use dmuxai::web::commands::DashboardCommand;
use dmuxai::web::{
    DEFAULT_PORT, DashboardHub, DashboardServer, DashboardServices, FileGraphSource, ServerConfig,
    SnapshotSource, check_bind, generate_token, start_server,
};
use ratatui::{Terminal, backend::CrosstermBackend};

type DmuxTerminal = Terminal<CrosstermBackend<Stdout>>;

/// CLI options for the dashboard server (FR-520, FR-521, FR-524).
#[derive(Debug, Clone, PartialEq, Eq)]
struct WebOptions {
    port: u16,
    bind: IpAddr,
    allow_remote: bool,
}

impl Default for WebOptions {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            allow_remote: false,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum CliAction {
    Help,
    Version,
    Run,
    Serve(WebOptions),
    RunWeb(WebOptions),
    McpMemory(MemoryOptions),
    MockMcp,
    /// Agent-facing subcommand (SPEC-agent-api FR-760).
    Agent(cli::AgentCommand),
    /// A usage error in an agent-facing subcommand (exit 2).
    Usage(String),
    Unknown(String),
}

/// CLI options for `dmuxai mcp-memory` (INT-729).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct MemoryOptions {
    workspace: Option<PathBuf>,
    vault: Option<PathBuf>,
    dir: Option<String>,
    project: Option<String>,
}

const USAGE: &str = "\
DmuxAI - One terminal. Multiple AI coding agents.

Usage:
  dmuxai [--help|--version]
  dmuxai [-C DIR | --workspace-dir DIR] [subcommand...]
  dmuxai serve [--port N] [--bind ADDR] [--allow-remote]
  dmuxai --web [--port N] [--bind ADDR] [--allow-remote]
  dmuxai mcp-memory [--workspace DIR] [--vault PATH] [--dir DIR] [--project NAME]
  dmuxai run <prompt|-> [--agent ID] [--mode ask|plan|implement] [--graph] [--json]
  dmuxai todo add <title> [--body TEXT] [--json]
  dmuxai todo list [--status open|running|done] [--json]
  dmuxai todo run <id> [--agent ID | --graph] [--mode M] [--json]
  dmuxai status [--json]
  dmuxai events [--since N] [--limit N] [--follow] [--json]
  dmuxai mcp
  dmuxai init [--workspace] [--force]

Options:
  -h, --help             Print this help message and exit
  -V, --version          Print version information and exit
  -C, --workspace-dir DIR  Run in DIR instead of the current directory
  serve            Run the local dashboard server without the TUI
  --web            Run the TUI and the local dashboard server
  --port N         Dashboard port (default 7420)
  --bind ADDR      Dashboard bind address (default 127.0.0.1)
  --allow-remote   Allow binding to a non-loopback address
  mcp-memory       Run the Obsidian memory MCP server on stdio
  --workspace DIR  Workspace directory for mcp-memory (default: current dir)
  --vault PATH     Obsidian vault for mcp-memory (overrides dmuxai.toml)
  --dir DIR        Memory directory inside the vault
  --project NAME   Project name for mcp-memory
  run              Run one prompt headless and exit (exit codes: 0 ok, 1 failed,
                   2 usage, 3 not found, 4 busy, 5 runner not allowed, 6 internal,
                   130 cancelled)
  todo             Add, list or run to-dos without the TUI
  status           Print workspace status (agents, to-dos, last graph, events)
  events           Print state-change events from .dmuxai/events.jsonl
  mcp              Serve DmuxAI as an MCP server over stdio
  init             Write a commented starter dmuxai.toml to the user config
                   (or the workspace with --workspace); --force overwrites
  --json           Print machine-readable JSON on stdout

Configuration:
  dmuxai.toml      Optional configuration file in the workspace root or parent directories

Environment variables:
  DMUXAI_FORCE_COLOR  Force color output on (1) or off (0)
  NO_COLOR            Disable color output unless forced
  XDG_STATE_HOME      Directory for application state and conversation history
  DMUXAI_WEB_TOKEN    Fixed dashboard token (32-128 chars of A-Z a-z 0-9 _ -)
";

fn parse_cli<I, S>(args: I) -> CliAction
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args: Vec<String> = args
        .into_iter()
        .map(|arg| arg.as_ref().to_owned())
        .collect();

    if args.first().map(String::as_str) == Some("__mock-mcp") {
        if args.len() == 1 {
            return CliAction::MockMcp;
        }
        return CliAction::Unknown(args[1].clone());
    }
    // Agent-facing subcommands (FR-760). `--help` anywhere prints usage.
    if args
        .first()
        .is_some_and(|first| cli::SUBCOMMANDS.contains(&first.as_str()))
    {
        // Stop at `--`: anything after it is prompt text.
        if args
            .iter()
            .take_while(|arg| arg.as_str() != "--")
            .any(|arg| arg == "-h" || arg == "--help")
        {
            return CliAction::Help;
        }
        return match cli::parse(&args) {
            Ok(command) => CliAction::Agent(command),
            Err(message) => CliAction::Usage(message),
        };
    }

    let serve = args.first().map(String::as_str) == Some("serve");
    let web = args.iter().any(|arg| arg == "--web");
    let mcp = args.first().map(String::as_str) == Some("mcp-memory");

    if !serve && !web && !mcp {
        // Preserve the original single-action semantics exactly: `--help`
        // returns immediately, `--version` is honored only while the action is
        // still `Run`, and any other argument returns `Unknown` at that point.
        let mut action = CliAction::Run;
        for arg in &args {
            match arg.as_str() {
                "-h" | "--help" => return CliAction::Help,
                "-V" | "--version" => {
                    if action == CliAction::Run {
                        action = CliAction::Version;
                    }
                }
                other => return CliAction::Unknown(other.to_owned()),
            }
        }
        return action;
    }

    // With `serve`/`--web`/`mcp-memory` present, `--help` and `--version` win
    // from any position (FR-520).
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        return CliAction::Help;
    }
    if args.iter().any(|arg| arg == "-V" || arg == "--version") {
        return CliAction::Version;
    }

    if mcp {
        return parse_mcp_memory_cli(&args);
    }

    // `serve` and `--web` are mutually exclusive, and `serve` is only valid as
    // the first argument; anything else is an unknown argument.
    if serve && web {
        return CliAction::Unknown("--web".to_owned());
    }

    let mut options = WebOptions::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "serve" => {
                if index != 0 {
                    return CliAction::Unknown("serve".to_owned());
                }
            }
            "--web" => {}
            "--port" => {
                index += 1;
                match args.get(index).and_then(|value| value.parse::<u16>().ok()) {
                    Some(port) => options.port = port,
                    None => return CliAction::Unknown("--port".to_owned()),
                }
            }
            "--bind" => {
                index += 1;
                let bind = args
                    .get(index)
                    .map(|value| {
                        let inner = value
                            .strip_prefix('[')
                            .and_then(|rest| rest.strip_suffix(']'))
                            .unwrap_or(value.as_str());
                        inner.parse::<IpAddr>()
                    })
                    .and_then(Result::ok);
                match bind {
                    Some(bind) => options.bind = bind,
                    None => return CliAction::Unknown("--bind".to_owned()),
                }
            }
            "--allow-remote" => options.allow_remote = true,
            other => return CliAction::Unknown(other.to_owned()),
        }
        index += 1;
    }

    if serve {
        CliAction::Serve(options)
    } else {
        CliAction::RunWeb(options)
    }
}

/// Parse `dmuxai mcp-memory [--workspace DIR] [--vault PATH] [--dir DIR] [--project NAME]`
/// (INT-729). The subcommand is only valid as the first argument.
fn parse_mcp_memory_cli(args: &[String]) -> CliAction {
    let mut options = MemoryOptions::default();
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--workspace" => {
                index += 1;
                match args.get(index) {
                    Some(value) => options.workspace = Some(PathBuf::from(value)),
                    None => return CliAction::Unknown("--workspace".to_owned()),
                }
            }
            "--vault" => {
                index += 1;
                match args.get(index) {
                    Some(value) => options.vault = Some(PathBuf::from(value)),
                    None => return CliAction::Unknown("--vault".to_owned()),
                }
            }
            "--dir" => {
                index += 1;
                match args.get(index) {
                    Some(value) => options.dir = Some(value.clone()),
                    None => return CliAction::Unknown("--dir".to_owned()),
                }
            }
            "--project" => {
                index += 1;
                match args.get(index) {
                    Some(value) => options.project = Some(value.clone()),
                    None => return CliAction::Unknown("--project".to_owned()),
                }
            }
            other => return CliAction::Unknown(other.to_owned()),
        }
        index += 1;
    }
    CliAction::McpMemory(options)
}

fn main() -> Result<(), Box<dyn Error>> {
    let (workspace_flag, remaining_args) =
        match dmuxai::workspace::extract_workspace_dir_flag(std::env::args().skip(1)) {
            Ok(res) => res,
            Err(raw) => {
                eprintln!("error: -C {raw}: invalid path");
                std::process::exit(2);
            }
        };
    if let Some(raw_path) = workspace_flag {
        match dmuxai::workspace::validate_workspace_dir(Path::new(&raw_path)) {
            Ok(canonical) => {
                // SAFETY: called at the very start of main before the tokio
                // runtime or any background threads are spawned.
                unsafe {
                    std::env::set_var("DMUXAI_WORKSPACE", &canonical);
                }
            }
            Err(reason) => {
                eprintln!("error: -C {raw_path}: {reason}");
                std::process::exit(2);
            }
        }
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async_main(remaining_args))
}

async fn async_main(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    let action = parse_cli(args);
    match action {
        CliAction::Help => {
            print!("{USAGE}");
            return Ok(());
        }
        CliAction::Version => {
            println!("dmuxai {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        CliAction::Unknown(flag) => {
            eprintln!("error: unexpected argument '{flag}'\n\n{USAGE}");
            std::process::exit(2);
        }
        CliAction::MockMcp => {
            dmuxai::mcp::mock::serve_stdio().await;
            return Ok(());
        }
        CliAction::Usage(message) => {
            eprintln!("error: {message}\n\n{USAGE}");
            std::process::exit(dmuxai::agent_api::exit::USAGE);
        }
        CliAction::Agent(command) => {
            let code = cli::execute(command).await;
            std::process::exit(code);
        }
        CliAction::Serve(options) => return run_serve(options).await,
        CliAction::RunWeb(options) => return run_tui(Some(options)).await,
        CliAction::McpMemory(options) => return run_mcp_memory(options),
        CliAction::Run => return run_tui(None).await,
    }
}

/// `dmuxai mcp-memory`: run the Obsidian memory MCP server on stdio (INT-729).
///
/// Never starts the TUI; stdin/stdout carry only JSON-RPC. Config warnings go to
/// stderr. A `--vault` that is not an existing directory, a `--dir` that escapes
/// the vault, or an invalid `--project` all exit 2.
fn run_mcp_memory(options: MemoryOptions) -> Result<(), Box<dyn Error>> {
    let workspace = match &options.workspace {
        Some(path) => path.clone(),
        None => dmuxai::workspace::resolve_workspace_root()?,
    };
    // `--project` overrides apply to both the `--vault` and settings path, so
    // validate once up front (FR-720 name rules).
    if let Some(project) = options.project.as_deref()
        && !dmuxai::memory::is_valid_name(project)
    {
        eprintln!("error: --project \"{project}\" must match [a-z0-9][a-z0-9_-]{{0,47}}");
        std::process::exit(2);
    }
    let config = match &options.vault {
        Some(vault) => {
            let vault = dmuxai::settings::expand_home(&vault.to_string_lossy());
            let (root, dir) = match dmuxai::memory::resolve_root(&vault, options.dir.as_deref()) {
                Ok(resolved) => resolved,
                Err(message) => {
                    eprintln!("error: --vault/--dir: {message}");
                    std::process::exit(2);
                }
            };
            Some(dmuxai::memory::MemoryConfig {
                root,
                dir,
                project: options.project.clone(),
                inject: true,
            })
        }
        None => {
            let settings = DmuxSettings::load(&workspace);
            for warning in &settings.config_warnings {
                eprintln!("warning: {warning}");
            }
            let mut config = settings.memory;
            // Apply the `--project` override on top of `[memory]` (if enabled).
            match (options.project.as_ref(), config.as_mut()) {
                (Some(project), Some(config)) => config.project = Some(project.clone()),
                (Some(_), None) => {
                    eprintln!("warning: --project ignored: memory is not configured");
                }
                _ => {}
            }
            config
        }
    };
    let memory = config
        .as_ref()
        .map(|config| dmuxai::memory::MemoryHandle::from_config(config, &workspace));
    dmuxai::memory_mcp::serve(memory, io::stdin().lock(), io::stdout().lock())?;
    Ok(())
}

/// Opens the per-repo to-do store (FR-540). Callers report the error as a
/// warning (`serve`) or a startup notice (`--web`).
fn open_todo_store(workspace_root: &Path) -> Result<Arc<TodoStore>, String> {
    let db_path = dmuxai::state_path::repo_db_path(workspace_root).map_err(|e| e.to_string())?;
    TodoStore::open(&db_path)
        .map(Arc::new)
        .map_err(|e| e.to_string())
}

/// Publishes the persisted to-do list into the hub once at startup so `serve`
/// (which never runs the TUI's `publish_todos`) shows existing to-dos.
fn seed_hub_todos(hub: &DashboardHub, store: &TodoStore) {
    if let Ok(todos) = store.list() {
        hub.set_todos(todos);
    }
}

/// `dmuxai serve`: read-only dashboard over persisted graph runs (FR-520, FR-524).
async fn run_serve(options: WebOptions) -> Result<(), Box<dyn Error>> {
    let workspace_root = match dmuxai::workspace::resolve_workspace_root() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(2);
        }
    };
    let _ = dmuxai::workspace::record_recent_workspace(&workspace_root);
    let workspace = dmuxai::workspace::normalize_workspace_path(&workspace_root)
        .unwrap_or_else(|_| workspace_root.display().to_string());

    if let Err(error) = check_bind(options.bind, options.allow_remote) {
        eprintln!("error: {error}\n\n{USAGE}");
        std::process::exit(2);
    }

    let settings = DmuxSettings::load(&workspace_root);
    let memory = settings
        .memory
        .as_ref()
        .map(|config| dmuxai::memory::MemoryHandle::from_config(config, &workspace_root));
    let ws = dmuxai::docs::confine::Workspace::new(&workspace_root).ok();
    let token = dashboard_token(|warning| eprintln!("warning: {warning}"));
    let open_tickets = Arc::new(std::sync::Mutex::new(dmuxai::web::OpenTickets::new()));
    let docs = ws.map(|workspace| {
        let changes = dmuxai::docs::changes::ChangeLog::default();
        let soffice = dmuxai::docs::office::find_soffice();
        Arc::new(dmuxai::web::docs_api::DocsService::new(
            workspace,
            true, // read_only
            changes,
            soffice,
            open_tickets.clone(),
            token.clone(),
        ))
    });

    for warning in &settings.config_warnings {
        eprintln!("warning: {warning}");
    }
    let mcp_registry = Arc::new(dmuxai::mcp::registry::McpRegistry::from_specs(
        &settings.mcp_servers,
        |k| std::env::var(k).ok(),
    ));
    for warning in mcp_registry.warnings() {
        eprintln!("warning: {warning}");
    }

    let services = match open_todo_store(&workspace_root) {
        Ok(store) => {
            store.set_jira_base_url(settings.todo_jira_base_url.clone());
            DashboardServices {
                todos: Some(store),
                commands: None,
                mcp: Some(mcp_registry),
                web_attached: false,
                memory: memory.clone(),
                events_path: Some(dmuxai::events::events_path(&workspace_root)),
                docs,
                switch_config: None,
            }
        }
        Err(error) => {
            eprintln!("warning: to-do board disabled: {error}");
            DashboardServices {
                todos: None,
                commands: None,
                mcp: Some(mcp_registry),
                web_attached: false,
                memory,
                // The event log is independent of the to-do database (FR-766).
                events_path: Some(dmuxai::events::events_path(&workspace_root)),
                docs,
                switch_config: None,
            }
        }
    };
    // `serve` never runs agents, so it does not write events itself (the
    // processes that run agents do); it only streams the shared log (FR-766).
    let hub = DashboardHub::new(SnapshotSource::Files, workspace);
    hub.set_ui(dmuxai::web::UiTheme {
        theme: settings.web_theme.theme,
        accent: settings.web_theme.accent,
        reduced_motion: Some(settings.reduced_motion || settings.theme.reduced_motion),
    });
    hub.set_jira_base_url(settings.todo_jira_base_url.clone());
    // Seed once so `GET /api/state` / SSE show persisted to-dos before the
    // first mutation (FR-540/FR-541).
    if let Some(store) = services.todos.as_ref() {
        seed_hub_todos(&hub, store);
    }
    let config = ServerConfig {
        bind: options.bind,
        port: options.port,
        allow_remote: options.allow_remote,
        token: token.clone(),
        services,
    };
    let token = config.token.clone();
    let server = match start_server(config, hub.clone()).await {
        Ok(server) => server,
        Err(error) => {
            eprintln!("error: could not start the dashboard server: {error}");
            std::process::exit(2);
        }
    };

    println!("dmuxai dashboard: {}", server.url);
    io::stdout().flush()?;
    if let Some(warning) = publish_discovery(&workspace_root, &server, &token) {
        eprintln!("warning: {warning}");
    }
    if let Some(warning) = &server.warning {
        eprintln!("{warning}");
    }

    let mut source = FileGraphSource::new(&workspace_root);
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let graph = source.poll();
                hub.publish(Vec::new(), graph);
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }

    dmuxai::web::discovery::remove_discovery(&workspace_root);
    Ok(())
}

/// The dashboard token: `DMUXAI_WEB_TOKEN` when valid, else random (FR-773).
fn dashboard_token(mut warn: impl FnMut(String)) -> String {
    let env = std::env::var("DMUXAI_WEB_TOKEN").ok();
    match dmuxai::web::discovery::token_from_env(env.as_deref()) {
        Ok(Some(token)) => token,
        Ok(None) => generate_token(),
        Err(warning) => {
            warn(warning);
            generate_token()
        }
    }
}

/// Writes `.dmuxai/web.json` so local agents can find the dashboard (FR-773).
fn publish_discovery(
    workspace_root: &Path,
    server: &DashboardServer,
    token: &str,
) -> Option<String> {
    let host = if server.addr.ip().is_unspecified() {
        "127.0.0.1".to_owned()
    } else if server.addr.is_ipv6() {
        format!("[{}]", server.addr.ip())
    } else {
        server.addr.ip().to_string()
    };
    let discovery = dmuxai::web::discovery::Discovery {
        base_url: format!("http://{host}:{}", server.addr.port()),
        port: server.addr.port(),
        pid: std::process::id(),
        token: token.to_owned(),
    };
    dmuxai::web::discovery::write_discovery(workspace_root, &discovery)
        .err()
        .map(|error| format!("could not write the dashboard discovery file: {error}"))
}

/// Run the TUI, optionally alongside the in-process dashboard (FR-520, FR-530).
async fn run_tui(web: Option<WebOptions>) -> Result<(), Box<dyn Error>> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        eprintln!(
            "dmuxai needs an interactive terminal (TTY); run it in a terminal, or see dmuxai --help"
        );
        std::process::exit(1);
    }
    let workspace_root = match dmuxai::workspace::resolve_workspace_root() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(2);
        }
    };
    dmuxai::workspace::ensure_directory_writable(&workspace_root)?;
    let _ = dmuxai::workspace::record_recent_workspace(&workspace_root);
    let settings = DmuxSettings::load(&workspace_root);
    let startup_warning = dmuxai::workspace::normalize_workspace_path(&workspace_root)
        .ok()
        .and_then(|normalized| dmuxai::workspace::wsl_mnt_warning(&normalized));
    let (db_path, migration_notice) = dmuxai::state_path::prepare_repo_db(&workspace_root)?;
    let engine = ConversationEngine::open(&db_path, &workspace_root)?;
    let no_color = std::env::var("NO_COLOR").ok();
    let force_env = std::env::var("DMUXAI_FORCE_COLOR").ok();
    let color_enabled = dmuxai::theme::resolve_color_output(
        no_color.as_deref(),
        force_env.as_deref(),
        settings.theme_force_color,
    );
    crossterm::style::force_color_output(color_enabled);
    let custom_warnings = dmuxai::custom_agent::install_registry(settings.custom_agents.clone());
    let config_warnings = settings.config_warnings.clone();
    let mut app = App::new_interactive(engine, workspace_root.clone(), settings.clone())?;
    for warning in config_warnings.into_iter().chain(custom_warnings) {
        app.push_startup_notice(warning);
    }
    if let Some(notice) = migration_notice {
        app.push_startup_notice(notice);
    }
    if !color_enabled {
        app.push_startup_notice(
            "Chat colors are off because NO_COLOR is set. Run with DMUXAI_FORCE_COLOR=1 or set [theme] force_color = true in dmuxai.toml to enable them.".to_owned(),
        );
    }
    if let Some(warning) = startup_warning {
        app.push_startup_notice(warning);
    }
    let dashboard = if let Some(options) = web {
        attach_web_dashboard(&mut app, &workspace_root, &settings, options).await
    } else {
        // No server, but a hub still feeds the event log (FR-768).
        let workspace = dmuxai::workspace::normalize_workspace_path(&workspace_root)
            .unwrap_or_else(|_| workspace_root.display().to_string());
        let sink = cli::build_event_sink(&workspace_root, &settings);
        app.attach_dashboard(DashboardHub::new(SnapshotSource::Tui, workspace).with_events(sink));
        None
    };
    let mut session = TerminalSession::start()?;
    let result = app.run(session.terminal_mut()).await;
    let switch_target = app.take_switch_ready();
    app.shutdown().await;
    drop(session);
    let mut bound_port = None;
    let mut dashboard_tok = None;
    if let Some(server) = dashboard {
        bound_port = Some(server.addr.port());
        dashboard_tok = server.url.split("/?token=").nth(1).map(str::to_owned);
        server.stop().await;
        dmuxai::web::discovery::remove_discovery(&workspace_root);
    }
    if let Some(target_workspace) = switch_target {
        exec_workspace_switch(&target_workspace, bound_port, dashboard_tok.as_deref());
    }
    result.map_err(Into::into)
}

#[cfg(unix)]
fn exec_workspace_switch(
    target_workspace: &Path,
    bound_port: Option<u16>,
    token: Option<&str>,
) -> ! {
    use std::os::unix::process::CommandExt;

    let orig_args: Vec<String> = std::env::args().skip(1).collect();
    let new_args = dmuxai::workspace::build_reexec_args(&orig_args, target_workspace, bound_port);
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            let msg = dmuxai::web::format_reexec_failure(Path::new("dmuxai"), &new_args, &err);
            eprint!("{msg}");
            std::process::exit(1);
        }
    };
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(&new_args);
    if let Some(tok) = token {
        cmd.env("DMUXAI_WEB_TOKEN", tok);
    }
    cmd.env("DMUXAI_WORKSPACE", target_workspace);
    let err = cmd.exec();
    let msg = dmuxai::web::format_reexec_failure(&exe, &new_args, &err);
    eprint!("{msg}");
    std::process::exit(1);
}

#[cfg(not(unix))]
fn exec_workspace_switch(
    target_workspace: &Path,
    _bound_port: Option<u16>,
    _token: Option<&str>,
) -> ! {
    eprintln!(
        "error: workspace switch is only supported on Unix\nrestart command: dmuxai -C {}",
        target_workspace.display()
    );
    std::process::exit(1);
}

/// Start the TUI-source dashboard and attach it to `app` (FR-521, FR-524, FR-530).
///
/// Returns the running server so the caller can keep it alive for the whole
/// TUI run and abort it afterwards.
async fn attach_web_dashboard(
    app: &mut App,
    workspace_root: &Path,
    settings: &DmuxSettings,
    options: WebOptions,
) -> Option<DashboardServer> {
    let workspace = dmuxai::workspace::normalize_workspace_path(workspace_root)
        .unwrap_or_else(|_| workspace_root.display().to_string());

    let (todos, receiver) = match open_todo_store(workspace_root) {
        Ok(store) => {
            store.set_jira_base_url(settings.todo_jira_base_url.clone());
            let (sender, receiver) = tokio::sync::mpsc::channel::<DashboardCommand>(16);
            (Some((store, sender)), Some(receiver))
        }
        Err(error) => {
            app.push_startup_notice(format!("to-do board unavailable: {error}"));
            (None, None)
        }
    };
    let ws = dmuxai::docs::confine::Workspace::new(workspace_root).ok();
    let doc_changes = dmuxai::docs::changes::ChangeLog::default();
    let doc_changes_for_app = doc_changes.clone();
    let open_tickets = Arc::new(std::sync::Mutex::new(dmuxai::web::OpenTickets::new()));
    let mut token_warning = None;
    let token = dashboard_token(|warning| token_warning = Some(warning));
    let docs = ws.map(|workspace| {
        let soffice = dmuxai::docs::office::find_soffice();
        Arc::new(dmuxai::web::docs_api::DocsService::new(
            workspace,
            false, // read_only
            doc_changes,
            soffice,
            open_tickets.clone(),
            token.clone(),
        ))
    });

    let services = DashboardServices {
        todos: todos.as_ref().map(|(store, _)| store.clone()),
        commands: todos.as_ref().map(|(_, sender)| sender.clone()),
        mcp: Some(app.mcp_registry()),
        web_attached: true,
        memory: DmuxSettings::load(workspace_root)
            .memory
            .as_ref()
            .map(|config| dmuxai::memory::MemoryHandle::from_config(config, workspace_root)),
        events_path: Some(dmuxai::events::events_path(workspace_root)),
        docs,
        switch_config: None,
    };

    let sink = cli::build_event_sink(workspace_root, settings);
    let hub = DashboardHub::new(SnapshotSource::Tui, workspace).with_events(sink);
    hub.set_jira_base_url(settings.todo_jira_base_url.clone());
    let config = ServerConfig {
        bind: options.bind,
        port: options.port,
        allow_remote: options.allow_remote,
        token: token.clone(),
        services,
    };
    if let Some(warning) = token_warning {
        app.push_startup_notice(warning);
    }
    let token = config.token.clone();

    match start_server(config, hub.clone()).await {
        Ok(server) => {
            if let Some(warning) = publish_discovery(workspace_root, &server, &token) {
                app.push_startup_notice(warning);
            }
            app.attach_dashboard(hub);
            app.attach_doc_changes(doc_changes_for_app);
            app.attach_open_tickets(open_tickets);
            let base_url = server
                .url
                .split("/?token=")
                .next()
                .unwrap_or(&server.url)
                .to_string();
            app.set_dashboard_base_url(base_url);
            if let (Some((store, _)), Some(receiver)) = (todos, receiver) {
                app.attach_dashboard_services(store.clone(), receiver);
            }
            app.push_startup_notice(format!("dmuxai dashboard: {}", server.url));
            if let Some(warning) = server.warning.clone() {
                app.push_startup_notice(warning);
            }
            Some(server)
        }
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => {
            eprintln!("error: {error}");
            std::process::exit(2);
        }
        Err(error) => {
            app.push_startup_notice(format!("dmuxai dashboard failed to start: {error}"));
            None
        }
    }
}

/// Owns terminal raw/alternate-screen state and restores it on every return path.
struct TerminalSession {
    terminal: DmuxTerminal,
}

impl TerminalSession {
    fn start() -> io::Result<Self> {
        enable_raw_mode()?;

        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
            let _ = disable_raw_mode();
            return Err(error);
        }

        match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(mut terminal) => {
                if let Err(error) = terminal.clear() {
                    let _ = disable_raw_mode();
                    let _ = execute!(
                        terminal.backend_mut(),
                        DisableMouseCapture,
                        LeaveAlternateScreen
                    );
                    return Err(error);
                }
                Ok(Self { terminal })
            }
            Err(error) => {
                let _ = disable_raw_mode();
                let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
                Err(error)
            }
        }
    }

    fn terminal_mut(&mut self) -> &mut DmuxTerminal {
        &mut self.terminal
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cli_empty_args_defaults_to_run() {
        assert_eq!(parse_cli(std::iter::empty::<&str>()), CliAction::Run);
    }

    #[test]
    fn parse_cli_help_flags() {
        assert_eq!(parse_cli(["--help"]), CliAction::Help);
        assert_eq!(parse_cli(["-h"]), CliAction::Help);
    }

    #[test]
    fn parse_cli_version_flags() {
        assert_eq!(parse_cli(["--version"]), CliAction::Version);
        assert_eq!(parse_cli(["-V"]), CliAction::Version);
    }

    #[test]
    fn parse_cli_unknown_flags() {
        assert_eq!(
            parse_cli(["--unknown"]),
            CliAction::Unknown("--unknown".to_owned())
        );
        assert_eq!(parse_cli(["-x"]), CliAction::Unknown("-x".to_owned()));
        assert_eq!(
            parse_cli(["positional"]),
            CliAction::Unknown("positional".to_owned())
        );
    }

    #[test]
    fn parse_cli_help_takes_precedence_over_version() {
        assert_eq!(parse_cli(["--version", "--help"]), CliAction::Help);
    }

    fn web_default() -> WebOptions {
        WebOptions::default()
    }

    // TC-520 / FR-520: serve and --web flag parsing.
    #[test]
    fn parse_cli_serve_and_web_flags() {
        assert_eq!(parse_cli(["serve"]), CliAction::Serve(web_default()));
        assert_eq!(
            parse_cli(["serve", "--port", "9000"]),
            CliAction::Serve(WebOptions {
                port: 9000,
                ..web_default()
            })
        );
        assert_eq!(
            parse_cli(["serve", "--bind", "127.0.0.1"]),
            CliAction::Serve(WebOptions {
                bind: "127.0.0.1".parse().unwrap(),
                ..web_default()
            })
        );
        assert_eq!(
            parse_cli(["serve", "--bind", "[::1]"]),
            CliAction::Serve(WebOptions {
                bind: "::1".parse().unwrap(),
                ..web_default()
            })
        );
        assert_eq!(
            parse_cli(["serve", "--bind", "::1"]),
            CliAction::Serve(WebOptions {
                bind: "::1".parse().unwrap(),
                ..web_default()
            })
        );
        assert_eq!(
            parse_cli(["serve", "--bind", "0.0.0.0", "--allow-remote"]),
            CliAction::Serve(WebOptions {
                bind: "0.0.0.0".parse().unwrap(),
                allow_remote: true,
                ..web_default()
            })
        );
        assert_eq!(parse_cli(["--web"]), CliAction::RunWeb(web_default()));
        assert_eq!(
            parse_cli(["--web", "--port", "0"]),
            CliAction::RunWeb(WebOptions {
                port: 0,
                ..web_default()
            })
        );
        assert!(matches!(
            parse_cli(["serve", "--port"]),
            CliAction::Unknown(_)
        ));
        assert!(matches!(
            parse_cli(["serve", "--port", "x"]),
            CliAction::Unknown(_)
        ));
        assert!(matches!(
            parse_cli(["serve", "--nope"]),
            CliAction::Unknown(_)
        ));
        assert!(matches!(
            parse_cli(["--port", "9000"]),
            CliAction::Unknown(_)
        ));
        // `serve` is only valid as the first argument; `--web` excludes `serve`.
        assert!(matches!(
            parse_cli(["--web", "serve"]),
            CliAction::Unknown(_)
        ));
        assert!(matches!(
            parse_cli(["serve", "serve"]),
            CliAction::Unknown(_)
        ));
        assert!(matches!(
            parse_cli(["serve", "--web"]),
            CliAction::Unknown(_)
        ));
        // Original single-action semantics are preserved without serve/--web.
        assert_eq!(
            parse_cli(["foo", "--version"]),
            CliAction::Unknown("foo".to_owned())
        );
        // With --web present, --version wins.
        assert_eq!(parse_cli(["--version", "--web"]), CliAction::Version);
    }

    // TC-729 (INT-729): `mcp-memory` and its options parse like the other
    // subcommands; it is only valid as the first argument.
    #[test]
    fn parse_cli_mcp_memory_flags() {
        assert_eq!(
            parse_cli(["mcp-memory"]),
            CliAction::McpMemory(MemoryOptions::default())
        );
        assert_eq!(
            parse_cli(["mcp-memory", "--workspace", "/tmp/ws"]),
            CliAction::McpMemory(MemoryOptions {
                workspace: Some(PathBuf::from("/tmp/ws")),
                ..MemoryOptions::default()
            })
        );
        assert_eq!(
            parse_cli([
                "mcp-memory",
                "--workspace",
                "/tmp/ws",
                "--vault",
                "/tmp/vault",
                "--dir",
                "notes/memory",
                "--project",
                "demo",
            ]),
            CliAction::McpMemory(MemoryOptions {
                workspace: Some(PathBuf::from("/tmp/ws")),
                vault: Some(PathBuf::from("/tmp/vault")),
                dir: Some("notes/memory".to_owned()),
                project: Some("demo".to_owned()),
            })
        );
        assert_eq!(parse_cli(["mcp-memory", "--help"]), CliAction::Help);
        assert_eq!(parse_cli(["mcp-memory", "-V"]), CliAction::Version);
        assert!(matches!(
            parse_cli(["mcp-memory", "--vault"]),
            CliAction::Unknown(_)
        ));
        assert!(matches!(
            parse_cli(["mcp-memory", "--nope"]),
            CliAction::Unknown(_)
        ));
        assert!(matches!(
            parse_cli(["mcp-memory", "--web"]),
            CliAction::Unknown(_)
        ));
    }

    // TC-760 / FR-760: agent subcommands route through `cli::parse`; `--help`
    // before `--` prints usage, after `--` it is prompt text.
    #[test]
    fn parse_cli_routes_agent_subcommands() {
        assert!(matches!(
            parse_cli(["status", "--json"]),
            CliAction::Agent(_)
        ));
        assert_eq!(parse_cli(["run", "--help"]), CliAction::Help);
        match parse_cli(["run", "--", "--help"]) {
            CliAction::Agent(cli::AgentCommand::Run(options)) => {
                assert_eq!(options.prompt, "--help")
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(parse_cli(["todo"]), CliAction::Usage(_)));
        assert!(matches!(parse_cli(["mcp", "x"]), CliAction::Usage(_)));
    }

    // FR-540/FR-541: `serve` seeds the hub with persisted to-dos at startup so
    // `GET /api/state` shows them before any mutation.
    #[test]
    fn serve_seeds_hub_with_persisted_todos() {
        let dir = tempfile::tempdir().unwrap();
        let store = TodoStore::open(&dir.path().join("conversations.sqlite3")).unwrap();
        store
            .create(dmuxai::todo::NewTodo {
                title: "seeded to-do".to_owned(),
                body: "from a previous session".to_owned(),
                source: dmuxai::todo::TodoSource::Manual,
                source_ref: None,
            })
            .unwrap();

        let hub = DashboardHub::new(SnapshotSource::Files, "/workspace".to_owned());
        assert!(hub.snapshot().todos.is_empty());

        seed_hub_todos(&hub, &store);

        let todos = hub.snapshot().todos;
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].title, "seeded to-do");
    }

    #[test]
    fn parse_cli_mock_mcp_hidden_subcommand() {
        assert_eq!(parse_cli(["__mock-mcp"]), CliAction::MockMcp);
        assert!(matches!(
            parse_cli(["__mock-mcp", "unexpected"]),
            CliAction::Unknown(_)
        ));
    }
}
