mod agent;
mod app;
mod process;

use std::{
    error::Error,
    io::{self, IsTerminal, Stdout, Write},
    net::{IpAddr, Ipv4Addr},
    path::Path,
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
    Unknown(String),
}

const USAGE: &str = "\
DmuxAI - One terminal. Multiple AI coding agents.

Usage:
  dmuxai [--help|--version]
  dmuxai serve [--port N] [--bind ADDR] [--allow-remote]
  dmuxai --web [--port N] [--bind ADDR] [--allow-remote]

Options:
  -h, --help       Print this help message and exit
  -V, --version    Print version information and exit
  serve            Run the local dashboard server without the TUI
  --web            Run the TUI and the local dashboard server
  --port N         Dashboard port (default 7420)
  --bind ADDR      Dashboard bind address (default 127.0.0.1)
  --allow-remote   Allow binding to a non-loopback address

Configuration:
  dmuxai.toml      Optional configuration file in the workspace root or parent directories

Environment variables:
  DMUXAI_FORCE_COLOR  Force color output on (1) or off (0)
  NO_COLOR            Disable color output unless forced
  XDG_STATE_HOME      Directory for application state and conversation history
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

    let serve = args.first().map(String::as_str) == Some("serve");
    let web = args.iter().any(|arg| arg == "--web");

    if !serve && !web {
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

    // With `serve`/`--web` present, `--help` and `--version` win from any
    // position (FR-520).
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        return CliAction::Help;
    }
    if args.iter().any(|arg| arg == "-V" || arg == "--version") {
        return CliAction::Version;
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let action = parse_cli(std::env::args().skip(1));
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
        CliAction::Serve(options) => return run_serve(options).await,
        CliAction::RunWeb(options) => return run_tui(Some(options)).await,
        CliAction::Run => return run_tui(None).await,
    }
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
    let workspace_root = dmuxai::workspace::resolve_workspace_root()?;
    let workspace = dmuxai::workspace::normalize_workspace_path(&workspace_root)
        .unwrap_or_else(|_| workspace_root.display().to_string());

    if let Err(error) = check_bind(options.bind, options.allow_remote) {
        eprintln!("error: {error}\n\n{USAGE}");
        std::process::exit(2);
    }

    let services = match open_todo_store(&workspace_root) {
        Ok(store) => DashboardServices {
            todos: Some(store),
            commands: None,
            web_attached: false,
        },
        Err(error) => {
            eprintln!("warning: to-do board disabled: {error}");
            DashboardServices::default()
        }
    };
    let settings = DmuxSettings::load(&workspace_root);
    for warning in &settings.config_warnings {
        eprintln!("warning: {warning}");
    }
    let hub = DashboardHub::new(SnapshotSource::Files, workspace);
    hub.set_ui(dmuxai::web::UiTheme {
        theme: settings.web_theme.theme,
        accent: settings.web_theme.accent,
    });
    // Seed once so `GET /api/state` / SSE show persisted to-dos before the
    // first mutation (FR-540/FR-541).
    if let Some(store) = services.todos.as_ref() {
        seed_hub_todos(&hub, store);
    }
    let config = ServerConfig {
        bind: options.bind,
        port: options.port,
        allow_remote: options.allow_remote,
        token: generate_token(),
        services,
    };
    let server = match start_server(config, hub.clone()).await {
        Ok(server) => server,
        Err(error) => {
            eprintln!("error: could not start the dashboard server: {error}");
            std::process::exit(2);
        }
    };

    println!("dmuxai dashboard: {}", server.url);
    io::stdout().flush()?;
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

    Ok(())
}

/// Run the TUI, optionally alongside the in-process dashboard (FR-520, FR-530).
async fn run_tui(web: Option<WebOptions>) -> Result<(), Box<dyn Error>> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        eprintln!(
            "dmuxai needs an interactive terminal (TTY); run it in a terminal, or see dmuxai --help"
        );
        std::process::exit(1);
    }
    let workspace_root = dmuxai::workspace::resolve_workspace_root()?;
    dmuxai::workspace::ensure_directory_writable(&workspace_root)?;
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
    let mut app = App::new(engine, workspace_root.clone(), settings)?;
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
        attach_web_dashboard(&mut app, &workspace_root, options).await
    } else {
        None
    };
    let mut session = TerminalSession::start()?;
    let result = app.run(session.terminal_mut()).await;
    app.shutdown().await;
    if let Some(server) = dashboard {
        server.abort();
    }
    result.map_err(Into::into)
}

/// Start the TUI-source dashboard and attach it to `app` (FR-521, FR-524, FR-530).
///
/// Returns the running server so the caller can keep it alive for the whole
/// TUI run and abort it afterwards.
async fn attach_web_dashboard(
    app: &mut App,
    workspace_root: &Path,
    options: WebOptions,
) -> Option<DashboardServer> {
    let workspace = dmuxai::workspace::normalize_workspace_path(workspace_root)
        .unwrap_or_else(|_| workspace_root.display().to_string());

    let (todos, receiver) = match open_todo_store(workspace_root) {
        Ok(store) => {
            let (sender, receiver) = tokio::sync::mpsc::channel::<DashboardCommand>(16);
            (Some((store, sender)), Some(receiver))
        }
        Err(error) => {
            app.push_startup_notice(format!("to-do board unavailable: {error}"));
            (None, None)
        }
    };
    let services = DashboardServices {
        todos: todos.as_ref().map(|(store, _)| store.clone()),
        commands: todos.as_ref().map(|(_, sender)| sender.clone()),
        web_attached: true,
    };

    let hub = DashboardHub::new(SnapshotSource::Tui, workspace);
    let config = ServerConfig {
        bind: options.bind,
        port: options.port,
        allow_remote: options.allow_remote,
        token: generate_token(),
        services,
    };

    match start_server(config, hub.clone()).await {
        Ok(server) => {
            app.attach_dashboard(hub);
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
}
