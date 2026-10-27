//! One workspace's runtime; a later change opens several.

use std::{
    collections::HashMap,
    error::Error,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use dmuxai::conversation::ConversationEngine;
use dmuxai::settings::DmuxSettings;
use dmuxai::todo::TodoStore;
use dmuxai::web::commands::DashboardCommand;
use dmuxai::web::directory::{
    DirectoryError, MAX_WORKSPACES, OpenError, OpenRequest, WorkspaceDirectory, WorkspaceEntry,
};
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

/// Shared dashboard state passed to a headless host so it can open folders
/// in-process without re-executing `dmuxai`.
pub(crate) struct HostContext {
    pub directory: Arc<WorkspaceDirectory>,
    pub token: String,
    pub base_url: String,
}

/// The apps and result returned when a headless host ends.
pub(crate) struct HostOutcome {
    /// The app hosting the initial workspace ("default").
    pub primary: Option<App>,
    /// Apps opened later through the dashboard.
    pub others: Vec<App>,
    /// The primary app's result, or `Ok(())` when ended by a signal.
    pub result: std::io::Result<()>,
}

/// Runs a headless host for one or more Apps.
///
/// The first app always hosts the "default" workspace. When a [`HostContext`]
/// is supplied, dashboard switch requests are served by spawning additional
/// apps in this same local set and task loop.
pub(crate) async fn run_headless_host(
    mut app: App,
    ctx: Option<HostContext>,
    mut shutdown: Option<&mut crate::ShutdownSignals>,
) -> HostOutcome {
    let mut tasks = tokio::task::JoinSet::new();
    let mut stop_flags: HashMap<String, Arc<AtomicBool>> = HashMap::new();
    let mut next_id = 2u32;
    let mut rx = match ctx.as_ref() {
        Some(ctx) => {
            let (tx, rx) = tokio::sync::mpsc::channel::<OpenRequest>(4);
            ctx.directory.set_opener(tx);
            Some(rx)
        }
        None => None,
    };

    let stop = app.stop_handle();
    stop_flags.insert("default".to_owned(), stop);
    tasks.spawn_local(async move {
        let backend = ratatui::backend::CrosstermBackend::new(std::io::sink());
        let mut terminal = ratatui::Terminal::with_options(
            backend,
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Fixed(ratatui::layout::Rect::new(0, 0, 120, 40)),
            },
        )?;
        let result = app.run(&mut terminal).await;
        Ok::<_, std::io::Error>(("default".to_owned(), app, result))
    });

    let mut collected: Vec<(String, App, std::io::Result<()>)> = Vec::new();
    let mut join_error: Option<std::io::Error> = None;
    let mut host_ended = false;

    while !host_ended {
        tokio::select! {
            joined = tasks.join_next() => {
                match joined {
                    Some(Ok(Ok(item))) => collected.push(item),
                    Some(Ok(Err(error))) => join_error = Some(error),
                    Some(Err(error)) => join_error = Some(std::io::Error::other(error)),
                    None => {}
                }
                host_ended = true;
            }
            _ = async {
                match shutdown.as_deref_mut() {
                    Some(signals) => signals.recv().await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                host_ended = true;
            }
            received = async {
                match rx.as_mut() {
                    Some(receiver) => receiver.recv().await,
                    None => std::future::pending::<Option<OpenRequest>>().await,
                }
            } => {
                match received {
                    Some(OpenRequest { path, reply }) => {
                        let Some(ctx) = ctx.as_ref() else {
                            continue;
                        };
                        match open_hosted_workspace(ctx, path, &mut next_id).await {
                            Ok((mut new_app, entry)) => {
                                let id = entry.id.clone();
                                let stop = new_app.stop_handle();
                                stop_flags.insert(id.clone(), stop);
                                tasks.spawn_local(async move {
                                    let backend =
                                        ratatui::backend::CrosstermBackend::new(std::io::sink());
                                    let mut terminal = ratatui::Terminal::with_options(
                                        backend,
                                        ratatui::TerminalOptions {
                                            viewport: ratatui::Viewport::Fixed(
                                                ratatui::layout::Rect::new(0, 0, 120, 40),
                                            ),
                                        },
                                    )?;
                                    let result = new_app.run(&mut terminal).await;
                                    Ok::<_, std::io::Error>((id, new_app, result))
                                });
                                let _ = reply.send(Ok(entry));
                            }
                            Err(error) => {
                                let _ = reply.send(Err(error));
                            }
                        }
                    }
                    None => {
                        rx = None;
                    }
                }
            }
        }
    }

    for stop in stop_flags.values() {
        stop.store(true, Ordering::Relaxed);
    }

    let remaining = async {
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok(Ok(item)) => collected.push(item),
                Ok(Err(error)) => join_error = Some(error),
                Err(error) => join_error = Some(std::io::Error::other(error)),
            }
        }
    };
    if tokio::time::timeout(std::time::Duration::from_secs(5), remaining)
        .await
        .is_err()
    {
        tasks.abort_all();
    }

    let mut primary = None;
    let mut others = Vec::new();
    let mut primary_result = None;
    for (id, app, result) in collected {
        if id == "default" {
            primary = Some(app);
            primary_result = Some(result);
        } else {
            others.push(app);
        }
    }
    let result = match primary_result {
        Some(result) => result,
        None => match join_error {
            Some(error) => Err(error),
            None => Ok(()),
        },
    };
    HostOutcome {
        primary,
        others,
        result,
    }
}

/// Opens a workspace inside an existing headless dashboard host.
///
/// The dashboard directory already focuses folders that are open; this
/// function closes the remaining race and performs the validation, app
/// creation, and registration needed to serve a new folder.
async fn open_hosted_workspace(
    ctx: &HostContext,
    path: PathBuf,
    next_id: &mut u32,
) -> Result<(App, Arc<WorkspaceEntry>), OpenError> {
    let key = dmuxai::workspace::normalize_workspace_path(&path)
        .unwrap_or_else(|_| path.display().to_string());
    if ctx.directory.find_by_path(&key).is_some() {
        return Err(OpenError {
            status: 409,
            message: "folder is already open".to_owned(),
        });
    }
    if ctx.directory.len() >= MAX_WORKSPACES {
        return Err(OpenError {
            status: 409,
            message: "too many open folders (max 4); close one first".to_owned(),
        });
    }
    if let Err(error) = dmuxai::workspace::ensure_directory_writable(&path) {
        return Err(OpenError {
            status: 400,
            message: error.to_string(),
        });
    }

    let settings = DmuxSettings::load(&path);
    let (mut app, migration_notice) =
        open_workspace_app(&path, &settings).map_err(|error| OpenError {
            status: 500,
            message: error.to_string(),
        })?;
    if !settings.custom_agents.is_empty() {
        app.push_startup_notice(
            "Custom agents from this folder's dmuxai.toml are not loaded when it is opened in a dashboard that already serves another folder; start a separate `dmuxai --web` here to use them.".to_owned(),
        );
    }
    for warning in &settings.config_warnings {
        app.push_startup_notice(warning.clone());
    }
    if let Some(notice) = migration_notice {
        app.push_startup_notice(notice);
    }
    app.set_headless(true);

    let mut parts = build_web_workspace(&app, &path, &settings, &ctx.token);
    for notice in parts.notices.drain(..) {
        app.push_startup_notice(notice);
    }
    if let Some(first) = ctx.directory.get("default")
        && let Some(config) = first.hub.switch_config()
    {
        parts.hub.set_switch_config(config);
    }

    let id = format!("w{next_id}");
    *next_id += 1;
    let entry = ctx
        .directory
        .insert(WorkspaceEntry {
            id,
            path: key.clone(),
            hub: parts.hub.clone(),
            services: parts.services,
        })
        .map_err(|error| {
            let status = match error {
                DirectoryError::Full | DirectoryError::Duplicate => 409,
                _ => 500,
            };
            OpenError {
                status,
                message: error.to_string(),
            }
        })?;

    attach_web_workspace(
        &mut app,
        parts.hub,
        parts.doc_changes,
        parts.open_tickets,
        parts.todos,
        ctx.base_url.clone(),
    );
    app.push_local_notice(format!("Opened {key} in this dashboard."));
    let _ = dmuxai::workspace::record_recent_workspace(&path);
    Ok((app, entry))
}
