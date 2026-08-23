mod agent;
mod app;
mod process;

use std::{
    error::Error,
    io::{self, Stdout},
    path::PathBuf,
};

use app::App;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use dmuxai::conversation::ConversationEngine;
use ratatui::{Terminal, backend::CrosstermBackend};

type DmuxTerminal = Terminal<CrosstermBackend<Stdout>>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let workspace_root = std::env::current_dir()?;
    let engine = ConversationEngine::open(&conversation_db_path()?, &workspace_root)?;
    let mut app = App::new(engine, workspace_root)?;
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
