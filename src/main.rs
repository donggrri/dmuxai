mod agent;
mod app;
mod process;

use std::{
    error::Error,
    io::{self, IsTerminal, Stdout},
    path::PathBuf,
};

use app::App;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use dmuxai::conversation::ConversationEngine;
use dmuxai::settings::DmuxSettings;
use ratatui::{Terminal, backend::CrosstermBackend};

type DmuxTerminal = Terminal<CrosstermBackend<Stdout>>;

#[derive(Debug, PartialEq, Eq)]
enum CliAction {
    Help,
    Version,
    Run,
    Unknown(String),
}

const USAGE: &str = "\
DmuxAI - One terminal. Multiple AI coding agents.

Usage:
  dmuxai [--help|--version]

Options:
  -h, --help       Print this help message and exit
  -V, --version    Print version information and exit

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
    let mut action = CliAction::Run;
    for arg in args {
        match arg.as_ref() {
            "-h" | "--help" => return CliAction::Help,
            "-V" | "--version" => {
                if action == CliAction::Run {
                    action = CliAction::Version;
                }
            }
            other => return CliAction::Unknown(other.to_owned()),
        }
    }
    action
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
        CliAction::Run => {}
    }

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
    let engine = ConversationEngine::open(&conversation_db_path()?, &workspace_root)?;
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
    let mut app = App::new(engine, workspace_root, settings)?;
    for warning in config_warnings.into_iter().chain(custom_warnings) {
        app.push_startup_notice(warning);
    }
    if !color_enabled {
        app.push_startup_notice(
            "Chat colors are off because NO_COLOR is set. Run with DMUXAI_FORCE_COLOR=1 or set [theme] force_color = true in dmuxai.toml to enable them.".to_owned(),
        );
    }
    if let Some(warning) = startup_warning {
        app.push_startup_notice(warning);
    }
    let mut session = TerminalSession::start()?;
    let result = app.run(session.terminal_mut()).await;
    app.shutdown().await;
    result.map_err(Into::into)
}

fn conversation_db_path() -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        let base = std::env::var_os("LOCALAPPDATA").ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA is not configured")
        })?;
        Ok(PathBuf::from(base)
            .join("DmuxAI")
            .join("conversations.sqlite3"))
    }

    #[cfg(not(windows))]
    {
        if let Some(base) = std::env::var_os("XDG_STATE_HOME") {
            return Ok(PathBuf::from(base)
                .join("dmuxai")
                .join("conversations.sqlite3"));
        }
        let home = std::env::var_os("HOME").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "neither XDG_STATE_HOME nor HOME is configured",
            )
        })?;
        Ok(PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("dmuxai")
            .join("conversations.sqlite3"))
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
}
