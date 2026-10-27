//! One workspace's runtime; a later change opens several.

use std::{error::Error, path::Path, sync::Arc};

use dmuxai::conversation::ConversationEngine;
use dmuxai::settings::DmuxSettings;
use dmuxai::todo::TodoStore;
use dmuxai::web::commands::DashboardCommand;
use dmuxai::web::{DashboardHub, DashboardServices, SnapshotSource};
use tokio::sync::mpsc::Receiver;

use crate::app::App;
use crate::cli;
use crate::open_todo_store;

/// Opens an interactive workspace app, setting up its conversation engine and repository state directory.
///
/// One workspace's runtime; a later change opens several.
pub(crate) fn open_workspace_app(
    workspace_root: &Path,
    settings: &DmuxSettings,
) -> Result<(App, Option<String>), Box<dyn Error>> {
    let (db_path, migration_notice) = dmuxai::state_path::prepare_repo_db(workspace_root)?;
    let engine = ConversationEngine::open(&db_path, workspace_root)?;
    let mut app = App::new_interactive(engine, workspace_root.to_path_buf(), settings.clone())?;
    app.set_state_repos_dir(
        dmuxai::state_path::state_base()
            .ok()
            .map(|b| b.join("repos")),
    );
    Ok((app, migration_notice))
}

/// The web services and resources constructed for a workspace dashboard.
///
/// One workspace's runtime; a later change opens several.
pub(crate) struct WebWorkspaceParts {
    pub hub: DashboardHub,
    pub services: DashboardServices,
    pub todos: Option<(
        Arc<TodoStore>,
        tokio::sync::mpsc::Receiver<DashboardCommand>,
    )>,
    pub doc_changes: dmuxai::docs::changes::ChangeLog,
    pub open_tickets: Arc<std::sync::Mutex<dmuxai::web::OpenTickets>>,
    pub notices: Vec<String>,
}

/// Builds dashboard services, the hub, and associated resources for a workspace.
///
/// One workspace's runtime; a later change opens several.
pub(crate) fn build_web_workspace(
    app: &App,
    workspace_root: &Path,
    settings: &DmuxSettings,
    token: &str,
) -> WebWorkspaceParts {
    let mut notices = Vec::new();
    let workspace = dmuxai::workspace::normalize_workspace_path(workspace_root)
        .unwrap_or_else(|_| workspace_root.display().to_string());

    let (todos_pair, receiver) = match open_todo_store(workspace_root) {
        Ok(store) => {
            store.set_jira_base_url(settings.todo_jira_base_url.clone());
            let (sender, receiver) = tokio::sync::mpsc::channel::<DashboardCommand>(16);
            (Some((store, sender)), Some(receiver))
        }
        Err(error) => {
            notices.push(format!("to-do board unavailable: {error}"));
            (None, None)
        }
    };
    let ws = dmuxai::docs::confine::Workspace::new(workspace_root).ok();
    let doc_changes = dmuxai::docs::changes::ChangeLog::default();
    let open_tickets = Arc::new(std::sync::Mutex::new(dmuxai::web::OpenTickets::new()));
    let docs = ws.map(|workspace| {
        let soffice = dmuxai::docs::office::find_soffice();
        Arc::new(dmuxai::web::docs_api::DocsService::new(
            workspace,
            false, // read_only
            doc_changes.clone(),
            soffice,
            open_tickets.clone(),
            token.to_string(),
        ))
    });

    let services = DashboardServices {
        todos: todos_pair.as_ref().map(|(store, _)| store.clone()),
        commands: todos_pair.as_ref().map(|(_, sender)| sender.clone()),
        mcp: Some(app.mcp_registry()),
        web_attached: true,
        memory: DmuxSettings::load(workspace_root)
            .memory
            .as_ref()
            .map(|config| dmuxai::memory::MemoryHandle::from_config(config, workspace_root)),
        events_path: Some(dmuxai::events::events_path(workspace_root)),
        docs,
        trust_env: None,
    };

    let sink = cli::build_event_sink(workspace_root, settings);
    let hub = DashboardHub::new(SnapshotSource::App, workspace).with_events(sink);
    hub.set_jira_base_url(settings.todo_jira_base_url.clone());

    let todos = match (todos_pair, receiver) {
        (Some((store, _)), Some(rx)) => Some((store, rx)),
        _ => None,
    };

    WebWorkspaceParts {
        hub,
        services,
        todos,
        doc_changes,
        open_tickets,
        notices,
    }
}

/// Attaches web workspace components (hub, doc changes, open tickets, base URL, and services) to the app.
///
/// One workspace's runtime; a later change opens several.
pub(crate) fn attach_web_workspace(
    app: &mut App,
    parts_hub: DashboardHub,
    doc_changes: dmuxai::docs::changes::ChangeLog,
    open_tickets: Arc<std::sync::Mutex<dmuxai::web::OpenTickets>>,
    todos: Option<(Arc<TodoStore>, Receiver<DashboardCommand>)>,
    base_url: String,
) {
    app.attach_dashboard(parts_hub);
    app.attach_doc_changes(doc_changes);
    app.attach_open_tickets(open_tickets);
    app.set_dashboard_base_url(base_url);
    if let Some((store, receiver)) = todos {
        app.attach_dashboard_services(store, receiver);
    }
}

/// Runs a headless host for an App.
///
/// Hosts one workspace today; a later change hosts several.
pub(crate) async fn run_headless_host(
    mut app: App,
    shutdown: Option<&mut crate::ShutdownSignals>,
) -> (Option<App>, std::io::Result<()>) {
    let mut tasks = tokio::task::JoinSet::new();
    let stop = app.stop_handle();

    tasks.spawn_local(async move {
        let backend = ratatui::backend::CrosstermBackend::new(std::io::sink());
        let mut terminal = ratatui::Terminal::with_options(
            backend,
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Fixed(ratatui::layout::Rect::new(0, 0, 120, 40)),
            },
        )?;
        let result = app.run(&mut terminal).await;
        Ok::<_, std::io::Error>((app, result))
    });

    let shutdown_fut = async {
        match shutdown {
            Some(s) => s.recv().await,
            None => std::future::pending::<()>().await,
        }
    };

    tokio::select! {
        joined = tasks.join_next() => {
            match joined {
                Some(Ok(Ok((app, result)))) => (Some(app), result),
                Some(Ok(Err(io_err))) => (None, Err(io_err)),
                Some(Err(join_err)) => (None, Err(std::io::Error::other(join_err))),
                None => (None, Ok(())),
            }
        }
        _ = shutdown_fut => {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            match tokio::time::timeout(std::time::Duration::from_secs(5), tasks.join_next()).await {
                Ok(Some(Ok(Ok((app, result))))) => (Some(app), result),
                Ok(Some(Ok(Err(io_err)))) => (None, Err(io_err)),
                Ok(Some(Err(join_err))) => (None, Err(std::io::Error::other(join_err))),
                _ => {
                    tasks.abort_all();
                    (None, Ok(()))
                }
            }
        }
    }
}
