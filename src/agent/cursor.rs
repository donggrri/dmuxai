use std::{
    collections::VecDeque,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Instant,
};

use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStderr, ChildStdout},
    sync::mpsc,
    task::JoinHandle,
};

use super::{
    AdapterError, Agent, AgentAdapter, AgentError, AgentEvent, AgentOutput, AgentRequest,
    ModelInfo, OutputContext, SpawnedAdapter, TurnPolicy, implement_outcome::ImplementTurnTracker,
    prompt_with_image_tokens,
};
use dmuxai::process::cursor_command;
use dmuxai::runner_protocol::RunnerProtocol;
use dmuxai::settings::DmuxSettings;

use super::acp::PermissionContext;
use super::acp_runner;

const COMMAND_CHANNEL_CAPACITY: usize = 8;
const STDERR_HISTORY_CAPACITY: usize = 8;
/// How long cursor-agent may keep running after its terminal `result` line
/// before its whole process group is stopped. The turn is already finished,
/// and a prompt queued behind it must not wait on a lingering CLI (or on a
/// child that keeps its pipes open).
const TERMINAL_EXIT_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

enum CursorCommand {
    Send {
        request: AgentRequest,
        is_graph: bool,
    },
    Cancel,
    ListModels,
    SelectModel(String),
    Prewarm,
    Reset,
    Tick,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorAdapterConfig {
    pub prewarm: bool,
    pub prewarm_idle_secs: u64,
    pub protocol: RunnerProtocol,
}

impl Default for CursorAdapterConfig {
    fn default() -> Self {
        Self {
            prewarm: false,
            prewarm_idle_secs: 300,
            protocol: RunnerProtocol::Acp,
        }
    }
}

impl From<DmuxSettings> for CursorAdapterConfig {
    fn from(settings: DmuxSettings) -> Self {
        Self {
            prewarm: settings.cursor_prewarm,
            prewarm_idle_secs: settings.cursor_prewarm_idle_secs,
            protocol: settings.cursor_protocol,
        }
    }
}

impl From<&DmuxSettings> for CursorAdapterConfig {
    fn from(settings: &DmuxSettings) -> Self {
        Self {
            prewarm: settings.cursor_prewarm,
            prewarm_idle_secs: settings.cursor_prewarm_idle_secs,
            protocol: settings.cursor_protocol,
        }
    }
}

impl CursorAdapterConfig {
    pub fn from_settings(settings: &DmuxSettings, prewarm: bool) -> Self {
        Self {
            prewarm: prewarm && settings.cursor_prewarm,
            prewarm_idle_secs: settings.cursor_prewarm_idle_secs,
            protocol: settings.cursor_protocol,
        }
    }
}

pub struct CursorAdapter {
    command_tx: mpsc::Sender<CursorCommand>,
    #[allow(dead_code)]
    /// Bumped synchronously by `reset()`. The runtime applies a pending reset
    /// before handling any later command, so a reset is never lost or
    /// reordered behind a prompt sent after `/new` (FR-807).
    reset_requests: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl AgentAdapter for CursorAdapter {
    fn agent(&self) -> Agent {
        Agent::Cursor
    }

    fn send(&self, request: AgentRequest) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(CursorCommand::Send {
                request,
                is_graph: false,
            })
            .map_err(map_send_error)
    }

    fn send_graph_node(&self, request: AgentRequest) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(CursorCommand::Send {
                request,
                is_graph: true,
            })
            .map_err(map_send_error)
    }

    fn cancel(&self) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(CursorCommand::Cancel)
            .map_err(map_send_error)
    }

    fn request_models(&self) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(CursorCommand::ListModels)
            .map_err(map_send_error)
    }

    fn select_model(&self, model: String) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(CursorCommand::SelectModel(model))
            .map_err(map_send_error)
    }

    fn prewarm(&self) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(CursorCommand::Prewarm)
            .map_err(map_send_error)
    }

    fn reset(&self) -> Result<(), AdapterError> {
        self.reset_requests
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Only a wake-up: a full queue is fine because the runtime checks
        // `reset_requests` before every command it handles.
        match self.command_tx.try_send(CursorCommand::Reset) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => Ok(()),
            Err(error) => Err(map_send_error(error)),
        }
    }

    fn tick(&self) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(CursorCommand::Tick)
            .map_err(map_send_error)
    }
}

fn map_send_error(error: mpsc::error::TrySendError<CursorCommand>) -> AdapterError {
    match error {
        mpsc::error::TrySendError::Full(_) => AdapterError::CommandQueueFull,
        mpsc::error::TrySendError::Closed(_) => AdapterError::RuntimeStopped,
    }
}

pub fn spawn_cursor_adapter(
    event_tx: mpsc::Sender<AgentOutput>,
    cwd: PathBuf,
    config: impl Into<CursorAdapterConfig>,
    mcp_servers: Vec<dmuxai::mcp::ResolvedMcpServer>,
) -> SpawnedAdapter {
    let config = config.into();
    let (command_tx, command_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
    let reset_requests = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let worker = tokio::spawn(runtime_loop(
        command_rx,
        event_tx,
        cwd,
        config,
        mcp_servers,
        reset_requests.clone(),
    ));
    (
        Box::new(CursorAdapter {
            command_tx,
            reset_requests,
        }),
        worker,
    )
}

struct WarmProcess {
    program: OsString,
    argv: Vec<OsString>,
    cwd: PathBuf,
    env_bin: Option<OsString>,
    child: Option<tokio::process::Child>,
    spawned_at: Instant,
}

impl Drop for WarmProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            #[cfg(unix)]
            if let Some(id) = child.id() {
                let _ = std::process::Command::new("kill")
                    .args(["-KILL", "--", &format!("-{id}")])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            let _ = child.start_kill();
        }
    }
}

/// Sends `signal` (e.g. `-TERM`) to the whole process group `pgid` (Unix).
#[cfg(unix)]
async fn signal_group(pgid: u32, signal: &str) -> bool {
    tokio::process::Command::new("kill")
        .args([signal, "--", &format!("-{pgid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|status| status.success())
}

/// SIGTERM the group, wait up to 500 ms for it to go away, then SIGKILL
/// whatever is left (descendants that ignore SIGTERM included).
#[cfg(unix)]
async fn terminate_group(pgid: u32) {
    if !signal_group(pgid, "-TERM").await {
        return; // group already gone
    }
    let deadline = Instant::now() + std::time::Duration::from_millis(500);
    while Instant::now() < deadline {
        if !signal_group(pgid, "-0").await {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let _ = signal_group(pgid, "-KILL").await;
}

async fn reap_warm_process(mut warm: WarmProcess) {
    if let Some(mut child) = warm.child.take() {
        #[cfg(unix)]
        if let Some(id) = child.id() {
            let _ = signal_group(id, "-TERM").await;
            let _ = tokio::time::timeout(std::time::Duration::from_millis(500), child.wait()).await;
            // Always clean up the rest of the group, even if the leader exited
            // in time: children may ignore SIGTERM.
            let _ = signal_group(id, "-KILL").await;
        }
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}

fn spawn_warm_process(
    resume_session: Option<&str>,
    selected_model: Option<&str>,
    mode: TurnPolicy,
    cwd: &Path,
) -> io::Result<WarmProcess> {
    let mut command = cursor_command()?;
    configure_command(&mut command, resume_session, selected_model, mode, cwd);
    let program = command.as_std().get_program().to_os_string();
    let argv = command
        .as_std()
        .get_args()
        .map(|s| s.to_os_string())
        .collect::<Vec<_>>();
    let env_bin = std::env::var_os("DMUXAI_CURSOR_BIN");
    #[cfg(unix)]
    command.process_group(0);
    command
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = command.spawn()?;
    Ok(WarmProcess {
        program,
        argv,
        cwd: cwd.to_path_buf(),
        env_bin,
        child: Some(child),
        spawned_at: Instant::now(),
    })
}

async fn runtime_loop(
    mut command_rx: mpsc::Receiver<CursorCommand>,
    event_tx: mpsc::Sender<AgentOutput>,
    cwd: PathBuf,
    config: CursorAdapterConfig,
    mcp_servers: Vec<dmuxai::mcp::ResolvedMcpServer>,
    reset_requests: std::sync::Arc<std::sync::atomic::AtomicU64>,
) {
    let protocol = config.protocol;
    let idle_limit = std::time::Duration::from_secs(config.prewarm_idle_secs);
    let applicable: Vec<dmuxai::mcp::ResolvedMcpServer> = mcp_servers
        .into_iter()
        .filter(|s| s.applies_to("cursor"))
        .collect();
    let can_prewarm = config.prewarm && applicable.is_empty() && protocol == RunnerProtocol::Legacy;
    let cursor_notice_emitted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut active: Option<ActiveTurn> = None;
    let mut selected_model: Option<String> = Some(dmuxai::model_policy::ALLOWED_MODEL.to_owned());
    let mut latest_session_id: Option<String> = None;
    let mut reset_epoch: u64 = 0;
    let mut warm_process: Option<WarmProcess> = None;
    let mut resets_seen: u64 = 0;
    // A prompt that arrived after the active turn already reported its result
    // but before the CLI process exited. It starts as soon as that turn ends,
    // instead of being refused as "already running a turn" (the app already
    // considers the agent idle once it saw `Completed`).
    let mut queued_send: Option<CursorCommand> = None;

    // Applies resets requested via `CursorAdapter::reset` since the last check:
    // forget the --resume session, bump the epoch, and discard the warm process.
    macro_rules! apply_pending_reset {
        () => {
            let requested = reset_requests.load(std::sync::atomic::Ordering::SeqCst);
            if requested != resets_seen {
                resets_seen = requested;
                latest_session_id = None;
                reset_epoch = reset_epoch.wrapping_add(1);
                if let Some(warm) = warm_process.take() {
                    reap_warm_process(warm).await;
                }
            }
        };
    }

    loop {
        apply_pending_reset!();
        if let Some(warm) = &warm_process
            && warm.spawned_at.elapsed() >= idle_limit
        {
            let warm = warm_process.take().unwrap();
            reap_warm_process(warm).await;
        }

        if active.is_none() {
            let sleep_duration = match &warm_process {
                Some(warm) => idle_limit.saturating_sub(warm.spawned_at.elapsed()),
                None => std::time::Duration::from_secs(3600),
            };

            let command = if let Some(queued) = queued_send.take() {
                queued
            } else {
                tokio::select! {
                    _ = tokio::time::sleep(sleep_duration), if warm_process.is_some() => {
                        if let Some(warm) = warm_process.take() {
                            reap_warm_process(warm).await;
                        }
                        continue;
                    }
                    cmd = command_rx.recv() => match cmd {
                        Some(c) => c,
                        None => {
                            if let Some(warm) = warm_process.take() {
                                reap_warm_process(warm).await;
                            }
                            break;
                        }
                    }
                }
            };
            apply_pending_reset!();

            match command {
                CursorCommand::Send { request, is_graph } => {
                    if is_graph {
                        if let Some(warm) = warm_process.take() {
                            reap_warm_process(warm).await;
                        }
                        let policy = request.policy;
                        active = Some(spawn_turn(
                            request,
                            None,
                            selected_model.clone(),
                            policy,
                            cwd.clone(),
                            event_tx.clone(),
                            protocol,
                            applicable.clone(),
                            cursor_notice_emitted.clone(),
                            true,
                            reset_epoch,
                        ));
                    } else {
                        let mut check_cmd = cursor_command().ok();
                        if let Some(cmd) = &mut check_cmd {
                            configure_command(
                                cmd,
                                request.resume_session.as_deref(),
                                selected_model.as_deref(),
                                request.policy,
                                &cwd,
                            );
                        }
                        let turn_program = check_cmd
                            .as_ref()
                            .map(|cmd| cmd.as_std().get_program().to_os_string());
                        let turn_argv = check_cmd
                            .as_ref()
                            .map(|cmd| {
                                cmd.as_std()
                                    .get_args()
                                    .map(|s| s.to_os_string())
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        let turn_env_bin = std::env::var_os("DMUXAI_CURSOR_BIN");

                        let warm_child = match warm_process.take() {
                            Some(mut warm) => {
                                let mut child = warm.child.take().unwrap();
                                match child.try_wait() {
                                    Ok(Some(_)) => {
                                        let _ = child.wait().await;
                                        None
                                    }
                                    Ok(None) => {
                                        let matches = Some(&warm.program) == turn_program.as_ref()
                                            && warm.argv == turn_argv
                                            && warm.cwd == cwd
                                            && warm.env_bin == turn_env_bin;
                                        if matches {
                                            Some(child)
                                        } else {
                                            warm.child = Some(child);
                                            reap_warm_process(warm).await;
                                            None
                                        }
                                    }
                                    Err(_) => {
                                        warm.child = Some(child);
                                        reap_warm_process(warm).await;
                                        None
                                    }
                                }
                            }
                            None => None,
                        };

                        let policy = request.policy;
                        active = Some(spawn_turn(
                            request,
                            warm_child,
                            selected_model.clone(),
                            policy,
                            cwd.clone(),
                            event_tx.clone(),
                            protocol,
                            applicable.clone(),
                            cursor_notice_emitted.clone(),
                            false,
                            reset_epoch,
                        ));
                    }
                }
                CursorCommand::Cancel => {}
                CursorCommand::ListModels => match list_models(&cwd).await {
                    Ok((models, _advertised_default)) => {
                        let selected = match &selected_model {
                            Some(model)
                                if dmuxai::model_policy::validate_model_for_runner(
                                    "cursor", model,
                                )
                                .is_ok() =>
                            {
                                selected_model.clone()
                            }
                            _ => Some(dmuxai::model_policy::ALLOWED_MODEL.to_owned()),
                        };
                        send_event(
                            &event_tx,
                            None,
                            AgentEvent::ModelsListed {
                                models,
                                selected,
                                selected_effort: None,
                            },
                        )
                        .await;
                    }
                    Err(error) => {
                        send_event(
                            &event_tx,
                            None,
                            AgentEvent::Error(AgentError::model_unavailable(error.to_string())),
                        )
                        .await;
                    }
                },
                CursorCommand::SelectModel(model) => {
                    let changed = selected_model.as_ref() != Some(&model);
                    selected_model = Some(model.clone());
                    if changed && let Some(warm) = warm_process.take() {
                        reap_warm_process(warm).await;
                        if can_prewarm {
                            warm_process = spawn_warm_process(
                                latest_session_id.as_deref(),
                                selected_model.as_deref(),
                                TurnPolicy::Agent,
                                &cwd,
                            )
                            .ok();
                        }
                    }
                    send_event(&event_tx, None, AgentEvent::ModelSelected(model)).await;
                }
                CursorCommand::Prewarm => {
                    if warm_process.is_none() && can_prewarm {
                        warm_process = spawn_warm_process(
                            latest_session_id.as_deref(),
                            selected_model.as_deref(),
                            TurnPolicy::Agent,
                            &cwd,
                        )
                        .ok();
                    }
                }
                // Wake-up only; handled by apply_pending_reset! above.
                CursorCommand::Reset => {}
                CursorCommand::Tick => {}
            }
            continue;
        }

        let turn = &mut active.as_mut().expect("checked above").handle;
        tokio::select! {
            res = turn => {
                apply_pending_reset!();
                let completed_turn = active.take().expect("turn was active");
                if completed_turn.reset_epoch == reset_epoch
                    && let Ok(Ok(Some(session_id))) = &res
                {
                    latest_session_id = Some(session_id.clone());
                }
                if can_prewarm && !completed_turn.is_graph {
                    warm_process = spawn_warm_process(
                        latest_session_id.as_deref(),
                        selected_model.as_deref(),
                        TurnPolicy::Agent,
                        &cwd,
                    )
                    .ok();
                }
            }
            command = command_rx.recv() => {
                apply_pending_reset!();
                match command {
                    Some(CursorCommand::Cancel) if queued_send.is_some() => {
                        // The app's current turn is the queued one. The draining
                        // turn already reported its result, so stop its process
                        // group without a second terminal event, then drop the
                        // queued prompt.
                        let draining = active.take();
                        let was_graph = draining.as_ref().is_some_and(|t| t.is_graph);
                        kill_turn(draining).await;
                        cancel_queued(&mut queued_send, &event_tx).await;
                        if can_prewarm && !was_graph {
                            warm_process = spawn_warm_process(
                                latest_session_id.as_deref(),
                                selected_model.as_deref(),
                                TurnPolicy::Agent,
                                &cwd,
                            )
                            .ok();
                        }
                    }
                    Some(CursorCommand::Cancel) => {
                        let cancelled_turn = active.take();
                        let was_graph = cancelled_turn.as_ref().is_some_and(|t| t.is_graph);
                        stop_turn(cancelled_turn, &event_tx).await;
                        if can_prewarm && !was_graph {
                            warm_process = spawn_warm_process(
                                latest_session_id.as_deref(),
                                selected_model.as_deref(),
                                TurnPolicy::Agent,
                                &cwd,
                            )
                            .ok();
                        }
                    }
                    Some(CursorCommand::Send { request, is_graph })
                        if queued_send.is_none()
                            && active
                                .as_ref()
                                .is_some_and(|turn| turn.terminal_seen.load(std::sync::atomic::Ordering::SeqCst)) =>
                    {
                        queued_send = Some(CursorCommand::Send { request, is_graph });
                    }
                    Some(CursorCommand::Send { request, .. }) => {
                        let context = OutputContext::from(&request);
                        send_event(
                            &event_tx,
                            Some(&context),
                            AgentEvent::Error(AgentError::provider(
                                "Cursor is already running a turn; cancel it before sending another prompt",
                            )),
                        ).await;
                    }
                    Some(CursorCommand::ListModels) => {
                        send_event(
                            &event_tx,
                            None,
                            AgentEvent::Notice(
                                "finish or cancel the Cursor turn before loading models".to_owned(),
                            ),
                        ).await;
                    }
                    Some(CursorCommand::SelectModel(_)) => {
                        send_event(
                            &event_tx,
                            None,
                            AgentEvent::Notice(
                                "finish or cancel the Cursor turn before changing models".to_owned(),
                            ),
                        ).await;
                    }
                    Some(CursorCommand::Prewarm | CursorCommand::Tick) => {}
                    // Wake-up only; handled by apply_pending_reset! above.
                    Some(CursorCommand::Reset) => {}
                    None => {
                        if queued_send.is_some() {
                            // The draining turn already finished for the app;
                            // the queued prompt never starts, so say so.
                            kill_turn(active.take()).await;
                            cancel_queued(&mut queued_send, &event_tx).await;
                        } else {
                            stop_turn(active.take(), &event_tx).await;
                        }
                        if let Some(warm) = warm_process.take() {
                            reap_warm_process(warm).await;
                        }
                        break;
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_turn(
    request: AgentRequest,
    warm_child: Option<tokio::process::Child>,
    selected_model: Option<String>,
    mode: TurnPolicy,
    cwd: PathBuf,
    event_tx: mpsc::Sender<AgentOutput>,
    protocol: RunnerProtocol,
    mcp_servers: Vec<dmuxai::mcp::ResolvedMcpServer>,
    cursor_notice_emitted: std::sync::Arc<std::sync::atomic::AtomicBool>,
    is_graph: bool,
    reset_epoch: u64,
) -> ActiveTurn {
    let pgid = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(
        warm_child.as_ref().and_then(|c| c.id()).unwrap_or(0),
    ));
    let turn_pgid = pgid.clone();
    let context = OutputContext::from(&request);
    let error_context = context.clone();
    let terminal_seen = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let turn_terminal_seen = terminal_seen.clone();
    let acp_cancel = (protocol == RunnerProtocol::Acp).then(acp_runner::acp_cancel_for_turn);
    let cancel_for_turn = acp_cancel.clone();
    let handle = tokio::spawn(async move {
        match run_turn_with_child(
            &request,
            warm_child,
            selected_model,
            mode,
            &cwd,
            &event_tx,
            protocol,
            &mcp_servers,
            &cursor_notice_emitted,
            &turn_terminal_seen,
            &turn_pgid,
            cancel_for_turn,
        )
        .await
        {
            Ok(session_id) => Ok(session_id),
            Err(error) => {
                send_event(
                    &event_tx,
                    Some(&error_context),
                    AgentEvent::Error(error.clone()),
                )
                .await;
                Err(error)
            }
        }
    });
    ActiveTurn {
        handle,
        context,
        is_graph,
        reset_epoch,
        pgid,
        terminal_seen,
        acp_cancel,
    }
}

struct ActiveTurn {
    handle: JoinHandle<Result<Option<String>, AgentError>>,
    context: OutputContext,
    is_graph: bool,
    reset_epoch: u64,
    /// Process group of the turn's CLI (its pid; 0 until spawned). Only used
    /// for group kills on Unix.
    #[cfg_attr(not(unix), allow(dead_code))]
    pgid: std::sync::Arc<std::sync::atomic::AtomicU32>,
    /// Set once the stream reported its terminal `result` line.
    terminal_seen: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// ACP turns: cooperative cancel flag (session/cancel before kill).
    acp_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

async fn stop_turn(turn: Option<ActiveTurn>, event_tx: &mpsc::Sender<AgentOutput>) {
    if let Some(turn) = turn {
        let context = turn.context.clone();
        if let Some(cancel) = turn.acp_cancel.clone() {
            cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = tokio::time::timeout(std::time::Duration::from_millis(1600), turn.handle).await;
        } else {
            kill_turn(Some(turn)).await;
        }
        send_event(event_tx, Some(&context), AgentEvent::Cancelled).await;
    }
}

/// Stops a turn's whole process group and its task, without reporting it.
async fn kill_turn(turn: Option<ActiveTurn>) {
    if let Some(turn) = turn {
        #[cfg(unix)]
        {
            let pgid = turn.pgid.load(std::sync::atomic::Ordering::SeqCst);
            if pgid > 0 {
                terminate_group(pgid).await;
            }
        }
        turn.handle.abort();
        let _ = turn.handle.await;
    }
}

/// Drops the queued prompt and tells the app it will not run.
async fn cancel_queued(queued: &mut Option<CursorCommand>, event_tx: &mpsc::Sender<AgentOutput>) {
    if let Some(CursorCommand::Send { request, .. }) = queued.take() {
        let context = OutputContext::from(&request);
        send_event(event_tx, Some(&context), AgentEvent::Cancelled).await;
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_turn(
    request: &AgentRequest,
    selected_model: Option<String>,
    mode: TurnPolicy,
    cwd: &Path,
    event_tx: &mpsc::Sender<AgentOutput>,
    protocol: RunnerProtocol,
    mcp_servers: &[dmuxai::mcp::ResolvedMcpServer],
    cursor_notice_emitted: &std::sync::atomic::AtomicBool,
) -> Result<Option<String>, AgentError> {
    run_turn_with_child(
        request,
        None,
        selected_model,
        mode,
        cwd,
        event_tx,
        protocol,
        mcp_servers,
        cursor_notice_emitted,
        &std::sync::atomic::AtomicBool::new(false),
        &std::sync::atomic::AtomicU32::new(0),
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_turn_with_child(
    request: &AgentRequest,
    warm_child: Option<tokio::process::Child>,
    selected_model: Option<String>,
    mode: TurnPolicy,
    cwd: &Path,
    event_tx: &mpsc::Sender<AgentOutput>,
    protocol: RunnerProtocol,
    mcp_servers: &[dmuxai::mcp::ResolvedMcpServer],
    cursor_notice_emitted: &std::sync::atomic::AtomicBool,
    terminal_seen: &std::sync::atomic::AtomicBool,
    pgid: &std::sync::atomic::AtomicU32,
    acp_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<Option<String>, AgentError> {
    let context = OutputContext::from(request);
    let _guard = if protocol != RunnerProtocol::Acp && !mcp_servers.is_empty() {
        match dmuxai::mcp::propagate::cursor_mcp_guard(cwd, mcp_servers) {
            Ok(Some(guard)) => Some(guard),
            Ok(None) => {
                if !cursor_notice_emitted.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    send_event(
                        event_tx,
                        Some(&context),
                        AgentEvent::Notice(
                            "Workspace .cursor/mcp.json already exists; shared MCP servers were not written for this Cursor turn.".to_owned(),
                        ),
                    )
                    .await;
                }
                None
            }
            Err(e) => {
                send_event(
                    event_tx,
                    Some(&context),
                    AgentEvent::Notice(format!("Failed to write .cursor/mcp.json: {e}")),
                )
                .await;
                None
            }
        }
    } else {
        None
    };

    if protocol == RunnerProtocol::Acp {
        if let Some(mut child) = warm_child {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        let program = PathBuf::from(
            cursor_command()
                .map_err(|e| AgentError::process_exit(format!("could not start Cursor: {e}")))?
                .as_std()
                .get_program()
                .to_os_string(),
        );
        let args = acp_runner::cursor_acp_argv(selected_model.as_deref(), cwd)
            .map_err(|e| AgentError::provider(e.to_string()))?;
        let permission_ctx = PermissionContext {
            workspace: cwd.to_string_lossy().into_owned(),
            sensitive_gate_active: false,
            // FR-1413: read-only turns deny writes client-side even after set_mode.
            read_only_fail_closed: mode == TurnPolicy::ReadOnly,
        };
        let cancel = acp_cancel
            .clone()
            .unwrap_or_else(acp_runner::acp_cancel_for_turn);
        return acp_runner::run_agent_acp_turn(
            &program,
            &args,
            cwd,
            request,
            Agent::Cursor,
            event_tx,
            permission_ctx,
            super::acp::AcpRunnerKind::Cursor,
            super::acp::AcpLifecycle::Managed,
            cancel,
            selected_model.clone(),
            None,
            None,
            dmuxai::mcp::propagate::acp_mcp_servers(mcp_servers, "cursor"),
        )
        .await
        .map(|outcome| outcome.session_id)
        .map_err(|e| AgentError::provider(e.to_string()));
    }

    let mut child = match warm_child {
        Some(child) => child,
        None => {
            let mut command = cursor_command()
                .map_err(|e| AgentError::process_exit(format!("could not start Cursor: {e}")))?;
            configure_command(
                &mut command,
                request.resume_session.as_deref(),
                selected_model.as_deref(),
                mode,
                cwd,
            );
            command
                .current_dir(cwd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            // Own process group, so a lingering CLI and its children can be
            // stopped together (cancel, or after the terminal result).
            #[cfg(unix)]
            command.process_group(0);
            command
                .spawn()
                .map_err(|e| AgentError::process_exit(format!("could not start Cursor: {e}")))?
        }
    };
    if let Some(id) = child.id() {
        pgid.store(id, std::sync::atomic::Ordering::SeqCst);
    }

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| AgentError::process_exit("Cursor stdin was not piped"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AgentError::process_exit("Cursor stdout was not piped"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AgentError::process_exit("Cursor stderr was not piped"))?;

    let context = OutputContext::from(request);
    send_event(event_tx, Some(&context), AgentEvent::Started).await;
    let prompt = prompt_with_image_tokens(&request.prompt, &request.image_paths);
    stdin
        .write_all(prompt.as_bytes())
        .await
        .map_err(|e| AgentError::provider(e.to_string()))?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|e| AgentError::provider(e.to_string()))?;
    stdin
        .shutdown()
        .await
        .map_err(|e| AgentError::provider(e.to_string()))?;
    drop(stdin);

    stream_child(
        &mut child,
        stdout,
        stderr,
        event_tx,
        &context,
        mode,
        terminal_seen,
    )
    .await
}

fn configure_command(
    command: &mut tokio::process::Command,
    resume_session: Option<&str>,
    selected_model: Option<&str>,
    mode: TurnPolicy,
    cwd: &Path,
) {
    command.arg("--print");
    match mode {
        TurnPolicy::ReadOnly => {
            command.args(["--mode", "ask"]);
        }
        TurnPolicy::Agent => {
            command.args(["--force", "--sandbox", "enabled"]);
        }
    }
    command
        .args([
            "--output-format",
            "stream-json",
            "--stream-partial-output",
            "--trust",
            "--workspace",
        ])
        .arg(cwd);
    let model = match selected_model {
        Some(model) if dmuxai::model_policy::validate_model_for_runner("cursor", model).is_ok() => {
            model
        }
        _ => dmuxai::model_policy::ALLOWED_MODEL,
    };
    command.arg("--model").arg(model);
    if let Some(session_id) = resume_session {
        command.arg("--resume").arg(session_id);
    }
}

async fn stream_child(
    child: &mut tokio::process::Child,
    stdout: ChildStdout,
    stderr: ChildStderr,
    event_tx: &mpsc::Sender<AgentOutput>,
    context: &OutputContext,
    mode: TurnPolicy,
    terminal_seen: &std::sync::atomic::AtomicBool,
) -> Result<Option<String>, AgentError> {
    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut stderr_lines = BufReader::new(stderr).lines();
    let mut stderr_history = VecDeque::with_capacity(STDERR_HISTORY_CAPACITY);
    let mut stdout_open = true;
    let mut stderr_open = true;
    let mut exit_status = None;
    let mut state = CursorStreamState::default();
    let mut implement_tracker = ImplementTurnTracker::new(mode);
    // The CLI's process group (it is the group leader); kept for after reaping.
    #[cfg_attr(not(unix), allow(unused_variables))]
    let pgid = child.id();
    // Set when the terminal `result` line arrives: past it the process gets
    // TERMINAL_EXIT_GRACE to exit and close its pipes, then it is stopped.
    let mut exit_deadline: Option<tokio::time::Instant> = None;
    let mut forced_after_result = false;

    while stdout_open || stderr_open || exit_status.is_none() {
        let deadline = exit_deadline
            .unwrap_or_else(|| tokio::time::Instant::now() + std::time::Duration::from_secs(3600));
        tokio::select! {
            line = stdout_lines.next_line(), if stdout_open => {
                match line.map_err(|e| AgentError::process_exit(e.to_string()))? {
                    Some(line) => {
                        implement_tracker.observe_provider_line(&line);
                        let message: Value = serde_json::from_str(&line).map_err(|error| {
                            AgentError::provider(format!("invalid Cursor NDJSON: {error}"))
                        })?;
                        // Mark the turn terminal before its `Completed` reaches the
                        // app, so a follow-up prompt is queued, not refused.
                        let events = normalize_event(&message, &mut state);
                        if state.terminal {
                            terminal_seen.store(true, std::sync::atomic::Ordering::SeqCst);
                            exit_deadline.get_or_insert_with(|| {
                                tokio::time::Instant::now() + TERMINAL_EXIT_GRACE
                            });
                        }
                        for event in events {
                            send_event(event_tx, Some(context), event).await;
                        }
                    }
                    None => stdout_open = false,
                }
            }
            line = stderr_lines.next_line(), if stderr_open => {
                match line.map_err(|e| AgentError::process_exit(e.to_string()))? {
                    Some(line) => push_stderr(&mut stderr_history, line),
                    None => stderr_open = false,
                }
            }
            status = child.wait(), if exit_status.is_none() => {
                exit_status = Some(status.map_err(|e| AgentError::process_exit(e.to_string()))?);
            }
            _ = tokio::time::sleep_until(deadline), if exit_deadline.is_some() => {
                // Still running (or a child holds its pipes) after the grace
                // period: stop the whole group and finish the turn.
                #[cfg(unix)]
                if let Some(pgid) = pgid {
                    terminate_group(pgid).await;
                }
                if exit_status.is_none() {
                    let _ = child.start_kill();
                    exit_status = Some(child.wait().await.map_err(|e| AgentError::process_exit(e.to_string()))?);
                }
                forced_after_result = true;
                break;
            }
        }
    }
    // A finished turn leaves nothing behind: reap any child of the CLI that
    // detached its pipes (no-op when the group is already gone).
    #[cfg(unix)]
    if state.terminal
        && !forced_after_result
        && let Some(pgid) = pgid
    {
        let _ = signal_group(pgid, "-KILL").await;
    }

    let status = exit_status.expect("child.wait runs before the loop can finish");
    let diagnostic = stderr_history.into_iter().collect::<Vec<_>>().join(" | ");
    implement_tracker.finish(
        &status,
        &diagnostic,
        state.terminal,
        false,
        forced_after_result,
    )?;
    if !state.terminal {
        let msg = if diagnostic.is_empty() {
            format!("Cursor exited with {status} without a terminal result")
        } else {
            format!("Cursor exited with {status}: {diagnostic}")
        };
        let err = if !status.success() {
            AgentError::process_exit(msg)
        } else {
            AgentError::provider(msg)
        };
        return Err(err);
    }
    Ok(state.session_id)
}

#[derive(Default)]
struct CursorStreamState {
    session_id: Option<String>,
    emitted_text: bool,
    terminal: bool,
    /// Text streamed as partials since the last segment aggregate or tool
    /// call (FR-800).
    segment_text: String,
    /// Every text delta shown so far in this turn (FR-800).
    turn_text: String,
}

impl CursorStreamState {
    fn emit(&mut self, events: &mut Vec<AgentEvent>, text: &str) {
        if text.is_empty() {
            return;
        }
        self.emitted_text = true;
        self.turn_text.push_str(text);
        events.push(AgentEvent::TextDelta(text.to_owned()));
    }

    /// The part of an aggregate that is not on screen yet
    /// (SPEC-cursor-stream-dedupe FR-800, FR-801).
    fn aggregate_missing<'a>(&mut self, text: &'a str, is_final: bool) -> &'a str {
        let segment = std::mem::take(&mut self.segment_text);
        if is_final {
            // A whole-turn aggregate: add the tail beyond everything shown.
            if let Some(tail) = text.strip_prefix(self.turn_text.as_str()) {
                return tail;
            }
            // Otherwise it repeats the last segment: add only what that
            // segment's partials did not stream.
            if !segment.is_empty() {
                return text.strip_prefix(segment.as_str()).unwrap_or("");
            }
            return if self.turn_text.ends_with(text) {
                ""
            } else {
                text
            };
        }
        if segment.is_empty() {
            // No partials for this segment: the aggregate is its only copy.
            return text;
        }
        // Partials were streamed: add only an unstreamed tail. An aggregate
        // that does not extend them (e.g. re-formatted) adds nothing, since the
        // segment is already visible.
        text.strip_prefix(segment.as_str()).unwrap_or("")
    }
}

fn normalize_event(message: &Value, state: &mut CursorStreamState) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    if let Some(session_id) = message.get("session_id").and_then(Value::as_str)
        && state.session_id.as_deref() != Some(session_id)
    {
        state.session_id = Some(session_id.to_owned());
        events.push(AgentEvent::ProviderSessionBound(session_id.to_owned()));
    }

    match message.get("type").and_then(Value::as_str) {
        Some("assistant") => {
            let text = message
                .pointer("/message/content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<String>();
            // With `--stream-partial-output`, cursor-agent sends three kinds of
            // `assistant` events (SPEC-cursor-stream-dedupe FR-800):
            // - partial deltas (`timestamp_ms`, no `model_call_id`);
            // - a segment aggregate before each tool call (`model_call_id`,
            //   usually also `timestamp_ms`) repeating the segment's text;
            // - a final aggregate (neither key) repeating the last segment.
            // Deltas are shown as they arrive; an aggregate contributes only
            // text that is not on screen yet.
            let has_model_call = message.get("model_call_id").is_some();
            if has_model_call || message.get("timestamp_ms").is_none() {
                let missing = state.aggregate_missing(&text, !has_model_call);
                state.emit(&mut events, missing);
            } else {
                state.segment_text.push_str(&text);
                state.emit(&mut events, &text);
            }
        }
        Some("tool_call") => {
            // A tool call ends the segment; the next partials start fresh.
            state.segment_text.clear();
        }
        Some("result") => {
            state.terminal = true;
            if message
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || message.get("subtype").and_then(Value::as_str) != Some("success")
            {
                events.push(AgentEvent::Error(AgentError::provider(
                    message
                        .get("result")
                        .and_then(Value::as_str)
                        .unwrap_or("Cursor turn failed")
                        .to_owned(),
                )));
            } else {
                if !state.emitted_text
                    && let Some(result) = message.get("result").and_then(Value::as_str)
                    && !result.is_empty()
                {
                    events.push(AgentEvent::TextDelta(result.to_owned()));
                }
                events.push(AgentEvent::Completed);
            }
        }
        _ => {}
    }
    events
}

async fn list_models(cwd: &Path) -> io::Result<(Vec<ModelInfo>, Option<String>)> {
    let mut command = cursor_command()?;
    command
        .arg("--list-models")
        .current_dir(cwd)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = command.output().await?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "Cursor model discovery failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(parse_model_list(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_model_list(output: &str) -> (Vec<ModelInfo>, Option<String>) {
    let mut selected = None;
    let models = output
        .lines()
        .filter_map(|line| {
            let (id, label) = line.split_once(" - ")?;
            let id = id.trim();
            if id.is_empty() {
                return None;
            }
            let label = label.trim();
            let is_default = label.ends_with(" (default)");
            let label = label.strip_suffix(" (default)").unwrap_or(label);
            if is_default {
                selected = Some(id.to_owned());
            }
            Some(ModelInfo::new(id, label))
        })
        .collect();
    (models, selected)
}

fn push_stderr(history: &mut VecDeque<String>, line: String) {
    if history.len() == STDERR_HISTORY_CAPACITY {
        history.pop_front();
    }
    history.push_back(line);
}

async fn send_event(
    event_tx: &mpsc::Sender<AgentOutput>,
    context: Option<&OutputContext>,
    event: AgentEvent,
) {
    let _ = event_tx
        .send(AgentOutput {
            agent: Agent::Cursor,
            conversation_id: context.map(|context| context.conversation_id.clone()),
            turn_id: context.map(|context| context.turn_id.clone()),
            event,
        })
        .await;
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::json;
    use tokio::time::{Duration, timeout};

    use super::super::AgentErrorKind;
    use super::*;

    #[test]
    fn parses_cursor_models_and_default() {
        let (models, selected) = parse_model_list(
            "Available models\nauto - Auto (default)\ngpt-5 - GPT-5\n\nTip: ignored",
        );
        assert_eq!(
            models,
            vec![
                ModelInfo::new("auto", "Auto"),
                ModelInfo::new("gpt-5", "GPT-5")
            ]
        );
        assert_eq!(selected.as_deref(), Some("auto"));
    }

    #[test]
    fn partial_cursor_text_does_not_duplicate_the_final_aggregate() {
        let mut state = CursorStreamState::default();
        let partial = json!({
            "type": "assistant",
            "timestamp_ms": 1,
            "session_id": "session",
            "message": { "content": [{ "type": "text", "text": "CURSOR_OK" }] }
        });
        let aggregate = json!({
            "type": "assistant",
            "session_id": "session",
            "message": { "content": [{ "type": "text", "text": "CURSOR_OK" }] }
        });
        let result = json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "result": "CURSOR_OK",
            "session_id": "session"
        });

        assert_eq!(
            normalize_event(&partial, &mut state),
            vec![
                AgentEvent::ProviderSessionBound("session".to_owned()),
                AgentEvent::TextDelta("CURSOR_OK".to_owned()),
            ]
        );
        assert!(normalize_event(&aggregate, &mut state).is_empty());
        assert_eq!(
            normalize_event(&result, &mut state),
            vec![AgentEvent::Completed]
        );
        assert_eq!(state.session_id.as_deref(), Some("session"));
    }

    // TC-800 / FR-800: a recorded cursor-agent 2026.10.16 stream (plan mode,
    // tool calls between text segments) yields each text exactly once.
    #[test]
    fn recorded_cursor_stream_text_appears_once() {
        let fixture =
            include_str!("../../tests/fixtures/cursor/stream-partial-tool-segments.jsonl");
        let mut state = CursorStreamState::default();
        let mut text = String::new();
        let mut completed = 0;
        let mut result_text = None;
        for line in fixture.lines().filter(|line| !line.trim().is_empty()) {
            let message: Value = serde_json::from_str(line).unwrap();
            if message.get("type").and_then(Value::as_str) == Some("result") {
                result_text = message
                    .get("result")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            for event in normalize_event(&message, &mut state) {
                match event {
                    AgentEvent::TextDelta(delta) => text.push_str(&delta),
                    AgentEvent::Completed => completed += 1,
                    _ => {}
                }
            }
        }
        let first = "README와 Cargo.toml만 읽어 프로젝트를 세 문장으로 요약합니다.";
        assert_eq!(text.matches(first).count(), 1, "segment duplicated: {text}");
        assert_eq!(Some(text.clone()), result_text);
        assert_eq!(completed, 1);
    }

    // TC-801 / FR-800, FR-801: segment aggregates add only unstreamed text;
    // without partials they carry the whole segment; the final aggregate is
    // used only when nothing was streamed.
    #[test]
    fn cursor_segment_aggregates_add_only_missing_text() {
        let partial = |text: &str| json!({"type": "assistant", "timestamp_ms": 1, "message": {"content": [{"type": "text", "text": text}]}});
        let segment = |text: &str| json!({"type": "assistant", "timestamp_ms": 2, "model_call_id": "m", "message": {"content": [{"type": "text", "text": text}]}});
        let deltas = |events: Vec<AgentEvent>| {
            events
                .into_iter()
                .filter_map(|event| match event {
                    AgentEvent::TextDelta(text) => Some(text),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        let mut state = CursorStreamState::default();
        assert_eq!(
            deltas(normalize_event(&partial("Look"), &mut state)),
            ["Look"]
        );
        assert_eq!(
            deltas(normalize_event(&partial("ing."), &mut state)),
            ["ing."]
        );
        assert!(deltas(normalize_event(&segment("Looking."), &mut state)).is_empty());
        // A segment aggregate longer than the streamed text adds the tail only.
        assert_eq!(
            deltas(normalize_event(&partial("Done"), &mut state)),
            ["Done"]
        );
        assert_eq!(
            deltas(normalize_event(&segment("Done!"), &mut state)),
            ["!"]
        );

        // No partials (older cursor-agent or no --stream-partial-output).
        let mut state = CursorStreamState::default();
        assert_eq!(
            deltas(normalize_event(&segment("Only segment."), &mut state)),
            ["Only segment."]
        );
        let final_aggregate = json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Only segment."}]}});
        assert!(deltas(normalize_event(&final_aggregate, &mut state)).is_empty());
    }

    #[test]
    fn cursor_aggregate_edge_cases() {
        let text_event = |extra: Value, text: &str| {
            let mut event = json!({"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}});
            for (key, value) in extra.as_object().unwrap() {
                event[key] = value.clone();
            }
            event
        };
        let partial = |text: &str| text_event(json!({"timestamp_ms": 1}), text);
        let segment =
            |text: &str| text_event(json!({"timestamp_ms": 2, "model_call_id": "m"}), text);
        let final_aggregate = |text: &str| text_event(json!({}), text);
        let tool_call = json!({"type": "tool_call", "subtype": "started"});
        let run = |events: &[Value]| {
            let mut state = CursorStreamState::default();
            events
                .iter()
                .flat_map(|event| normalize_event(event, &mut state))
                .filter_map(|event| match event {
                    AgentEvent::TextDelta(text) => Some(text),
                    _ => None,
                })
                .collect::<String>()
        };

        // A segment streamed without an aggregate, then a tool call: the next
        // segment's aggregate is compared with that segment only.
        assert_eq!(
            run(&[
                partial("A. "),
                tool_call.clone(),
                partial("B"),
                segment("BC")
            ]),
            "A. BC"
        );
        // Several segments without partials, then a final aggregate that
        // repeats the last one.
        assert_eq!(
            run(&[
                segment("One. "),
                tool_call.clone(),
                segment("Two."),
                final_aggregate("Two.")
            ]),
            "One. Two."
        );
        // A whole-turn final aggregate adds only its unseen tail.
        assert_eq!(
            run(&[
                segment("One. "),
                segment("Two."),
                final_aggregate("One. Two. Three.")
            ]),
            "One. Two. Three."
        );
        // The last segment was never streamed: the final aggregate shows it.
        assert_eq!(
            run(&[
                segment("One. "),
                tool_call.clone(),
                final_aggregate("Last.")
            ]),
            "One. Last."
        );
        // Identical consecutive segments without partials are both shown.
        assert_eq!(
            run(&[segment("Done. "), tool_call.clone(), segment("Done. ")]),
            "Done. Done. "
        );
        // A whole-turn final aggregate after a streamed last segment adds its tail.
        assert_eq!(
            run(&[
                segment("One. "),
                tool_call.clone(),
                partial("Two."),
                final_aggregate("One. Two. Three.")
            ]),
            "One. Two. Three."
        );
        // Korean (multi-byte) tails split on character boundaries.
        assert_eq!(
            run(&[partial("프로젝트 "), segment("프로젝트 구조를 봅니다.")]),
            "프로젝트 구조를 봅니다."
        );
        // An aggregate that does not extend the streamed text adds nothing.
        assert_eq!(
            run(&[partial("Hello there"), segment("Hello  there")]),
            "Hello there"
        );

        // A failed turn after partials reports the error without repeating text.
        let mut state = CursorStreamState::default();
        let mut events = normalize_event(&partial("Partial"), &mut state);
        events.extend(normalize_event(
            &json!({"type": "result", "subtype": "error", "is_error": true, "result": "boom"}),
            &mut state,
        ));
        assert!(
            matches!(events.as_slice(), [AgentEvent::TextDelta(text), AgentEvent::Error(error)] if text == "Partial" && error.message == "boom" && error.kind == AgentErrorKind::Provider)
        );
    }

    #[test]
    fn cursor_resume_session_is_passed_to_the_command() {
        let cwd = Path::new("workspace");
        let request = AgentRequest {
            conversation_id: super::super::ConversationId::new("conversation"),
            turn_id: super::super::TurnId::new("turn"),
            prompt: "hello".to_owned(),
            resume_session: Some("cursor-session".to_owned()),
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        };
        let mut command = tokio::process::Command::new("cursor-agent");

        configure_command(
            &mut command,
            request.resume_session.as_deref(),
            Some("composer-2.5"),
            TurnPolicy::ReadOnly,
            cwd,
        );

        let args = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            args.windows(2)
                .any(|args| args == ["--resume", "cursor-session"])
        );
        assert!(
            args.windows(2)
                .any(|args| args == ["--model", "composer-2.5"])
        );
    }

    #[test]
    // TC-410 / INT-410: cursor argv always contains --model composer-2.5 by default.
    fn cursor_argv_always_contains_composer_2_5_by_default() {
        let cwd = Path::new("workspace");
        let request = AgentRequest {
            conversation_id: super::super::ConversationId::new("conversation"),
            turn_id: super::super::TurnId::new("turn"),
            prompt: "hello".to_owned(),
            resume_session: None,
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        };
        let mut command = tokio::process::Command::new("cursor-agent");
        configure_command(
            &mut command,
            request.resume_session.as_deref(),
            None,
            TurnPolicy::ReadOnly,
            cwd,
        );

        let args = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            args.windows(2)
                .any(|args| args == ["--model", "composer-2.5"]),
            "cursor argv should include --model composer-2.5 by default: {args:?}"
        );
    }

    #[test]
    // TC-411 / FR-411, INT-410: a fast model is rejected and replaced with composer-2.5 in argv.
    fn cursor_argv_rejects_and_replaces_fast_model() {
        let cwd = Path::new("workspace");
        let request = AgentRequest {
            conversation_id: super::super::ConversationId::new("conversation"),
            turn_id: super::super::TurnId::new("turn"),
            prompt: "hello".to_owned(),
            resume_session: None,
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        };

        for disallowed in ["composer-2.5-fast", "claude-3-5-sonnet-fast", "gpt-5", ""] {
            let mut command = tokio::process::Command::new("cursor-agent");
            configure_command(
                &mut command,
                request.resume_session.as_deref(),
                Some(disallowed),
                TurnPolicy::ReadOnly,
                cwd,
            );

            let args = command
                .as_std()
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            assert!(
                args.windows(2)
                    .any(|args| args == ["--model", "composer-2.5"]),
                "disallowed model {disallowed:?} should be replaced by composer-2.5 in argv: {args:?}"
            );
            if !disallowed.is_empty() {
                assert!(
                    !args.iter().any(|arg| arg == disallowed),
                    "disallowed model {disallowed:?} should not appear in argv: {args:?}"
                );
            }
        }
    }

    #[test]
    // FR-IMG-5: Cursor has no `--image` flag; image paths become `@path` tokens
    // appended to the prompt written to stdin.
    fn cursor_passes_image_paths_to_cli() {
        let cwd = Path::new("workspace");
        let request = AgentRequest {
            conversation_id: super::super::ConversationId::new("conversation"),
            turn_id: super::super::TurnId::new("turn"),
            prompt: "describe".to_owned(),
            resume_session: None,
            image_paths: vec![
                PathBuf::from("/tmp/shot.png"),
                PathBuf::from("/tmp/other.png"),
            ],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        };
        let mut command = tokio::process::Command::new("cursor-agent");
        configure_command(
            &mut command,
            request.resume_session.as_deref(),
            None,
            TurnPolicy::ReadOnly,
            cwd,
        );
        let args = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(!args.iter().any(|arg| arg == "--image"));
        assert_eq!(
            prompt_with_image_tokens(&request.prompt, &request.image_paths),
            "describe @/tmp/shot.png @/tmp/other.png"
        );
    }

    #[test]
    fn cursor_execution_modes_use_native_cli_flags() {
        let request = AgentRequest {
            conversation_id: super::super::ConversationId::new("conversation"),
            turn_id: super::super::TurnId::new("turn"),
            prompt: "hello".to_owned(),
            resume_session: None,
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        };
        let cwd = Path::new("workspace");

        let args_for = |mode| {
            let mut command = tokio::process::Command::new("cursor-agent");
            configure_command(
                &mut command,
                request.resume_session.as_deref(),
                None,
                mode,
                cwd,
            );
            command
                .as_std()
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };

        let read_only = args_for(TurnPolicy::ReadOnly);
        assert!(read_only.windows(2).any(|args| args == ["--mode", "ask"]));
        let implement = args_for(TurnPolicy::Agent);
        assert!(implement.iter().any(|arg| arg == "--force"));
        assert!(
            implement
                .windows(2)
                .any(|args| args == ["--sandbox", "enabled"])
        );
        assert!(!implement.iter().any(|arg| arg == "--mode"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a locally installed and authenticated Cursor Agent CLI"]
    async fn real_cursor_turn_streams_to_completion() {
        let (event_tx, mut event_rx) = mpsc::channel(128);
        let cwd = std::env::current_dir().unwrap();
        let request = AgentRequest {
            conversation_id: super::super::ConversationId::new("real-conversation"),
            turn_id: super::super::TurnId::new("real-turn"),
            prompt: "Reply with exactly CURSOR_OK. Do not use tools or modify files.".to_owned(),
            resume_session: None,
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        };
        let turn = tokio::spawn(async move {
            run_turn(
                &request,
                Some("composer-2.5".to_owned()),
                TurnPolicy::ReadOnly,
                &cwd,
                &event_tx,
                RunnerProtocol::Legacy,
                &[],
                &std::sync::atomic::AtomicBool::new(false),
            )
            .await
        });

        let answer = timeout(Duration::from_secs(90), async {
            let mut answer = String::new();
            while let Some(output) = event_rx.recv().await {
                match output.event {
                    AgentEvent::TextDelta(delta) => answer.push_str(&delta),
                    AgentEvent::Completed => return answer,
                    AgentEvent::Error(error) => panic!("Cursor turn failed: {error}"),
                    AgentEvent::Started
                    | AgentEvent::ProviderSessionBound(_)
                    | AgentEvent::Notice(_)
                    | AgentEvent::ModelsListed { .. }
                    | AgentEvent::ModelSelected(_)
                    | AgentEvent::EffortSelected(_)
                    | AgentEvent::ToolProgress { .. }
                    | AgentEvent::Activity(_)
                    | AgentEvent::Status(_)
                    | AgentEvent::Cancelled => {}
                }
            }
            answer
        })
        .await
        .expect("Cursor should complete within 90 seconds");

        let session = turn
            .await
            .unwrap()
            .expect("real Cursor turn should not return an I/O error")
            .expect("real Cursor turn should return a session ID");
        assert!(!session.is_empty());
        assert!(answer.contains("CURSOR_OK"), "unexpected answer: {answer}");
    }

    #[cfg(unix)]
    pub(crate) static TEST_CURSOR_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn cursor_mcp_notice_once_per_session() {
        // TC-578, INT-561: Cursor runner with an existing user .cursor/mcp.json; two turns.
        // The "not propagated" notice is emitted once per session; the user file is untouched.
        use dmuxai::mcp::config::{ResolvedMcpServer, ServerKind, ToolNames, Transport};
        use std::collections::BTreeMap;
        use std::sync::atomic::AtomicBool;

        #[cfg(unix)]
        let _guard = TEST_CURSOR_MUTEX.lock().await;
        let _env_guard = dmuxai::state_path::test_env_lock();

        let temp = tempfile::tempdir().unwrap();
        let cursor_dir = temp.path().join(".cursor");
        std::fs::create_dir_all(&cursor_dir).unwrap();
        let user_file = cursor_dir.join("mcp.json");
        std::fs::write(&user_file, r#"{"user": "pre-existing"}"#).unwrap();

        let server = ResolvedMcpServer {
            name: "test-server".to_owned(),
            kind: ServerKind::Generic,
            transport: Transport::Stdio {
                command: "echo".to_owned(),
                args: vec![],
                env: BTreeMap::new(),
            },
            runners: None,
            tools: ToolNames::default(),
        };

        let (event_tx, mut event_rx) = mpsc::channel(32);
        let notice_shown = AtomicBool::new(false);

        let req1 = AgentRequest {
            conversation_id: super::super::ConversationId::new("conv-1"),
            turn_id: super::super::TurnId::new("turn-1"),
            prompt: "prompt 1".to_owned(),
            resume_session: None,
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        };

        // Override cursor binary so the test does not spawn a live external cursor-agent CLI
        unsafe {
            std::env::set_var("DMUXAI_CURSOR_BIN", "nonexistent-cursor-bin");
        }

        // Turn 1
        let _ = run_turn(
            &req1,
            None,
            TurnPolicy::ReadOnly,
            temp.path(),
            &event_tx,
            RunnerProtocol::Legacy,
            std::slice::from_ref(&server),
            &notice_shown,
        )
        .await;

        let mut turn1_notices = Vec::new();
        while let Ok(output) = event_rx.try_recv() {
            if let AgentEvent::Notice(msg) = output.event
                && msg.contains(".cursor/mcp.json already exists")
            {
                turn1_notices.push(msg);
            }
        }
        assert_eq!(
            turn1_notices.len(),
            1,
            "turn 1 should emit exactly one notice"
        );

        let req2 = AgentRequest {
            conversation_id: super::super::ConversationId::new("conv-1"),
            turn_id: super::super::TurnId::new("turn-2"),
            prompt: "prompt 2".to_owned(),
            resume_session: None,
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        };

        // Turn 2
        let _ = run_turn(
            &req2,
            None,
            TurnPolicy::ReadOnly,
            temp.path(),
            &event_tx,
            RunnerProtocol::Legacy,
            std::slice::from_ref(&server),
            &notice_shown,
        )
        .await;

        unsafe {
            std::env::remove_var("DMUXAI_CURSOR_BIN");
        }

        let mut turn2_notices = Vec::new();
        while let Ok(output) = event_rx.try_recv() {
            if let AgentEvent::Notice(msg) = output.event
                && msg.contains(".cursor/mcp.json already exists")
            {
                turn2_notices.push(msg);
            }
        }
        assert_eq!(
            turn2_notices.len(),
            0,
            "turn 2 must not emit the notice again (once per session)"
        );

        // User file remains untouched
        let content = std::fs::read_to_string(&user_file).unwrap();
        assert_eq!(content, r#"{"user": "pre-existing"}"#);
    }

    #[cfg(unix)]
    pub(crate) struct FakeCursorFixture {
        pub(crate) _guard: tokio::sync::MutexGuard<'static, ()>,
        pub(crate) _env_guard: std::sync::MutexGuard<'static, ()>,
        pub(crate) _dir: tempfile::TempDir,
        pub(crate) log_path: PathBuf,
        pub(crate) exit_once_marker: PathBuf,
    }

    #[cfg(unix)]
    #[derive(Debug, Clone)]
    pub(crate) struct SpawnEntry {
        pub(crate) pid: u32,
        pub(crate) argv: Vec<String>,
    }

    #[cfg(unix)]
    impl FakeCursorFixture {
        pub(crate) async fn new() -> Self {
            use std::os::unix::fs::PermissionsExt;
            let guard = TEST_CURSOR_MUTEX.lock().await;
            let env_guard = dmuxai::state_path::test_env_lock();
            let dir = tempfile::tempdir().expect("tempdir");
            let log_path = dir.path().join("fake_cursor.log");
            let exit_once_marker = dir.path().join("exit_once.marker");
            let script_path = dir.path().join("cursor-agent");
            let script = format!(
                "#!/bin/sh\n\
                 PID=$$\n\
                 LOG_FILE=\"{}\"\n\
                 EXIT_MARKER=\"{}\"\n\
                 echo \"SPAWN $PID $@\" >> \"$LOG_FILE\"\n\
                 if [ -f \"$EXIT_MARKER\" ]; then\n\
                     rm -f \"$EXIT_MARKER\"\n\
                     echo \"EXIT_EARLY $PID\" >> \"$LOG_FILE\"\n\
                     exit 1\n\
                 fi\n\
                 if read -r line; then\n\
                     echo \"PROMPT $PID $line\" >> \"$LOG_FILE\"\n\
                     echo '{{\"type\":\"assistant\",\"session_id\":\"sess-1\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"OK\"}}]}}}}'\n\
                     echo '{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"OK\",\"session_id\":\"sess-1\"}}'\n\
                     exit 0\n\
                 else\n\
                     echo \"EOF $PID\" >> \"$LOG_FILE\"\n\
                     echo \"Error: No prompt provided for print mode\" >&2\n\
                     exit 1\n\
                 fi\n",
                log_path.display(),
                exit_once_marker.display()
            );
            std::fs::write(&script_path, script).expect("write fake cursor");
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
            unsafe {
                std::env::set_var("DMUXAI_CURSOR_BIN", &script_path);
            }
            Self {
                _guard: guard,
                _env_guard: env_guard,
                _dir: dir,
                log_path,
                exit_once_marker,
            }
        }

        pub(crate) fn read_log_lines(&self) -> Vec<String> {
            if let Ok(content) = std::fs::read_to_string(&self.log_path) {
                content
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            } else {
                Vec::new()
            }
        }

        pub(crate) fn read_spawns(&self) -> Vec<SpawnEntry> {
            self.read_log_lines()
                .into_iter()
                .filter_map(|line| {
                    if let Some(rest) = line.strip_prefix("SPAWN ") {
                        let mut parts = rest.split_whitespace();
                        let pid = parts.next()?.parse::<u32>().ok()?;
                        let argv = parts.map(str::to_owned).collect();
                        Some(SpawnEntry { pid, argv })
                    } else {
                        None
                    }
                })
                .collect()
        }

        pub(crate) async fn wait_for_spawns(
            &self,
            count: usize,
            timeout_dur: Duration,
        ) -> Vec<SpawnEntry> {
            let start = Instant::now();
            while start.elapsed() < timeout_dur {
                let spawns = self.read_spawns();
                if spawns.len() >= count {
                    return spawns;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            self.read_spawns()
        }

        pub(crate) fn prompts_received(&self) -> Vec<(u32, String)> {
            self.read_log_lines()
                .into_iter()
                .filter_map(|line| {
                    if let Some(rest) = line.strip_prefix("PROMPT ") {
                        let mut parts = rest.splitn(2, ' ');
                        let pid = parts.next()?.parse::<u32>().ok()?;
                        let prompt = parts.next()?.to_owned();
                        Some((pid, prompt))
                    } else {
                        None
                    }
                })
                .collect()
        }

        pub(crate) fn is_pid_alive(&self, pid: u32) -> bool {
            let proc_stat = format!("/proc/{pid}/stat");
            if let Ok(content) = std::fs::read_to_string(&proc_stat) {
                if let Some(idx) = content.rfind(')') {
                    let state = content[idx + 1..].trim().chars().next();
                    if let Some(s) = state {
                        return s != 'Z';
                    }
                }
                true
            } else {
                std::process::Command::new("kill")
                    .arg("-0")
                    .arg(pid.to_string())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false)
            }
        }

        pub(crate) async fn wait_for_pid_gone(&self, pid: u32, timeout_dur: Duration) -> bool {
            let start = Instant::now();
            while start.elapsed() < timeout_dur {
                if !self.is_pid_alive(pid) {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            !self.is_pid_alive(pid)
        }
    }

    #[cfg(unix)]
    impl Drop for FakeCursorFixture {
        fn drop(&mut self) {
            unsafe {
                std::env::remove_var("DMUXAI_CURSOR_BIN");
            }
        }
    }

    #[cfg(unix)]
    pub(crate) async fn wait_for_turn_completion(event_rx: &mut mpsc::Receiver<AgentOutput>) {
        let timeout_dur = Duration::from_secs(5);
        let start = Instant::now();
        while start.elapsed() < timeout_dur {
            match timeout(Duration::from_millis(500), event_rx.recv()).await {
                Ok(Some(output)) => {
                    if matches!(output.event, AgentEvent::Completed) {
                        return;
                    }
                }
                Ok(None) => panic!("event_rx closed unexpectedly"),
                Err(_) => {}
            }
        }
        panic!("turn did not complete within timeout");
    }

    #[tokio::test]
    #[cfg(unix)]
    // TC-805 / FR-805, FR-806: After a turn completes, a warm process exists with the next-turn argv;
    // the next prompt reuses it (one spawn total for turn 2); nothing is written to its stdin before the prompt.
    async fn prewarm_reused_when_args_match() {
        let fixture = FakeCursorFixture::new().await;
        let (event_tx, mut event_rx) = mpsc::channel(64);
        let config = CursorAdapterConfig {
            protocol: RunnerProtocol::Legacy,
            prewarm: true,
            prewarm_idle_secs: 300,
        };
        let workspace = tempfile::tempdir().unwrap();
        let (adapter, worker) =
            spawn_cursor_adapter(event_tx, workspace.path().to_path_buf(), config, Vec::new());

        // Turn 1
        let req1 = AgentRequest {
            conversation_id: super::super::ConversationId::new("conv-1"),
            turn_id: super::super::TurnId::new("turn-1"),
            prompt: "prompt 1".to_owned(),
            resume_session: None,
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        };
        adapter.send(req1).unwrap();
        wait_for_turn_completion(&mut event_rx).await;

        // After turn 1 completes, a warm process is spawned for turn 2 with --resume sess-1
        let spawns = fixture.wait_for_spawns(2, Duration::from_secs(3)).await;
        assert_eq!(spawns.len(), 2, "turn 1 spawn + warm process spawn");
        let warm_pid = spawns[1].pid;
        assert!(
            fixture.is_pid_alive(warm_pid),
            "warm process should be alive"
        );
        assert!(
            spawns[1]
                .argv
                .windows(2)
                .any(|w| w == ["--resume", "sess-1"]),
            "warm process should be configured to resume turn 1's session: {:?}",
            spawns[1].argv
        );

        // Verify nothing is written to warm process's stdin yet
        let prompts_before = fixture.prompts_received();
        assert_eq!(
            prompts_before.len(),
            1,
            "only turn 1 prompt was received so far"
        );
        assert_eq!(prompts_before[0].0, spawns[0].pid);
        let warm_logs_before = fixture
            .read_log_lines()
            .into_iter()
            .filter(|l| l.contains(&warm_pid.to_string()))
            .collect::<Vec<_>>();
        assert_eq!(
            warm_logs_before,
            vec![format!("SPAWN {} {}", warm_pid, spawns[1].argv.join(" "))],
            "no stdin bytes before prompt sent to warm process"
        );

        // Turn 2: matching arguments (--resume sess-1)
        let req2 = AgentRequest {
            conversation_id: super::super::ConversationId::new("conv-1"),
            turn_id: super::super::TurnId::new("turn-2"),
            prompt: "prompt 2".to_owned(),
            resume_session: Some("sess-1".to_owned()),
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        };
        adapter.send(req2).unwrap();
        wait_for_turn_completion(&mut event_rx).await;

        // Turn 2 reused the warm process: exactly one spawn handled turn 2 (the warm process itself)
        let prompts_after = fixture.prompts_received();
        assert_eq!(prompts_after.len(), 2);
        assert_eq!(
            prompts_after[1].0, warm_pid,
            "turn 2 prompt should be handled by warm process"
        );
        assert_eq!(prompts_after[1].1, "prompt 2");

        // Exactly 3 spawns total: turn 1, warm process for turn 2 (reused), and warm process for turn 3
        let spawns_after = fixture.wait_for_spawns(3, Duration::from_secs(3)).await;
        assert_eq!(
            spawns_after.len(),
            3,
            "one spawn total for turn 2 (reused warm process), plus next pre-start"
        );

        drop(adapter);
        let _ = worker.await;
    }

    // Dashboard double-message bug: a real cursor-agent keeps running for a
    // moment after its `result` line. A prompt sent in that window (the app
    // already saw `Completed`) must start after the old process exits, not be
    // refused with "already running a turn".
    #[tokio::test]
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    async fn send_right_after_result_is_queued_not_refused() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = TEST_CURSOR_MUTEX.lock().await;
        let _env_guard = dmuxai::state_path::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("linger.log");
        let script_path = dir.path().join("cursor-agent");
        let script = format!(
            "#!/bin/sh\n\
             if read -r line; then\n\
                 echo \"PROMPT $line\" >> \"{}\"\n\
                 echo '{{\"type\":\"assistant\",\"session_id\":\"sess-1\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"OK\"}}]}}}}'\n\
                 echo '{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"OK\",\"session_id\":\"sess-1\"}}'\n\
                 sleep 1\n\
             fi\n",
            log_path.display()
        );
        std::fs::write(&script_path, script).unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: TEST_CURSOR_MUTEX is held for the whole test.
        unsafe {
            std::env::set_var("DMUXAI_CURSOR_BIN", &script_path);
        }
        let (event_tx, mut event_rx) = mpsc::channel(64);
        let config = CursorAdapterConfig {
            protocol: RunnerProtocol::Legacy,
            prewarm: false,
            prewarm_idle_secs: 300,
        };
        let workspace = tempfile::tempdir().unwrap();
        let (adapter, worker) =
            spawn_cursor_adapter(event_tx, workspace.path().to_path_buf(), config, Vec::new());
        let request = |turn: &str, prompt: &str| AgentRequest {
            conversation_id: super::super::ConversationId::new("conv-1"),
            turn_id: super::super::TurnId::new(turn),
            prompt: prompt.to_owned(),
            resume_session: None,
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        };
        adapter.send(request("turn-1", "first")).unwrap();
        wait_for_turn_completion(&mut event_rx).await;
        // The first process is still alive (sleep 1): send the next prompt now.
        adapter.send(request("turn-2", "second")).unwrap();
        let mut errors = Vec::new();
        let mut completed_turn2 = false;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) && !completed_turn2 {
            if let Ok(Some(output)) = timeout(Duration::from_millis(500), event_rx.recv()).await {
                let turn = output.turn_id.as_ref().map(|t| t.as_str().to_owned());
                match output.event {
                    AgentEvent::Error(message) => errors.push(message),
                    AgentEvent::Completed if turn.as_deref() == Some("turn-2") => {
                        completed_turn2 = true;
                    }
                    _ => {}
                }
            }
        }
        assert!(errors.is_empty(), "no busy refusal expected: {errors:?}");
        assert!(completed_turn2, "the queued prompt must run");
        let prompts = std::fs::read_to_string(&log_path).unwrap();
        assert_eq!(
            prompts.lines().collect::<Vec<_>>(),
            vec!["PROMPT first", "PROMPT second"]
        );

        drop(adapter);
        let _ = worker.await;
        unsafe {
            std::env::remove_var("DMUXAI_CURSOR_BIN");
        }
    }

    /// A fake cursor-agent: the first run reports its result, then lingers
    /// (with a child that keeps stdout open) for `linger_secs`; later runs
    /// exit right after their result. Logs `PROMPT`, `LEADER` and `CHILD`.
    #[cfg(unix)]
    fn lingering_cursor_script(dir: &Path, linger_secs: u32) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let log_path = dir.join("linger.log");
        let marker = dir.join("first-run-done");
        let script_path = dir.join("cursor-agent");
        let script = format!(
            "#!/bin/sh\n\
             if read -r line; then\n\
                 echo \"PROMPT $line\" >> \"{log}\"\n\
                 echo '{{\"type\":\"assistant\",\"session_id\":\"sess-1\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"OK\"}}]}}}}'\n\
                 echo '{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"OK\",\"session_id\":\"sess-1\"}}'\n\
                 if [ ! -e \"{marker}\" ]; then\n\
                     : > \"{marker}\"\n\
                     echo \"LEADER $$\" >> \"{log}\"\n\
                     sleep {linger} &\n\
                     echo \"CHILD $!\" >> \"{log}\"\n\
                     sleep {linger}\n\
                 fi\n\
             fi\n",
            log = log_path.display(),
            marker = marker.display(),
            linger = linger_secs,
        );
        std::fs::write(&script_path, script).unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        (script_path, log_path)
    }

    #[cfg(unix)]
    fn logged_pids(log_path: &Path, tag: &str) -> Vec<u32> {
        std::fs::read_to_string(log_path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.strip_prefix(tag)?.trim().parse().ok())
            .collect()
    }

    /// Gone or a zombie waiting for its parent.
    #[cfg(unix)]
    fn pid_gone(pid: u32) -> bool {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat
                .rfind(')')
                .and_then(|idx| stat[idx + 1..].split_whitespace().next())
                .is_some_and(|state| state == "Z" || state == "X"),
            Err(_) => true,
        }
    }

    #[cfg(unix)]
    async fn wait_pids_gone(pids: &[u32], within: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < within {
            if pids.iter().all(|pid| pid_gone(*pid)) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        pids.iter().all(|pid| pid_gone(*pid))
    }

    #[cfg(unix)]
    fn linger_request(turn: &str, prompt: &str) -> AgentRequest {
        AgentRequest {
            conversation_id: super::super::ConversationId::new("conv-1"),
            turn_id: super::super::TurnId::new(turn),
            prompt: prompt.to_owned(),
            resume_session: None,
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        }
    }

    // Review fix (#82): a cursor-agent that never exits after its result (a
    // child even keeps its stdout open) is stopped after TERMINAL_EXIT_GRACE,
    // its whole group is gone, and the prompt queued behind it then runs.
    #[tokio::test]
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    async fn lingering_cli_is_stopped_after_grace_and_queued_prompt_runs() {
        let _guard = TEST_CURSOR_MUTEX.lock().await;
        let _env_guard = dmuxai::state_path::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let (script_path, log_path) = lingering_cursor_script(dir.path(), 60);
        // SAFETY: TEST_CURSOR_MUTEX is held for the whole test.
        unsafe {
            std::env::set_var("DMUXAI_CURSOR_BIN", &script_path);
        }
        let (event_tx, mut event_rx) = mpsc::channel(64);
        let config = CursorAdapterConfig {
            protocol: RunnerProtocol::Legacy,
            prewarm: false,
            prewarm_idle_secs: 300,
        };
        let workspace = tempfile::tempdir().unwrap();
        let (adapter, worker) =
            spawn_cursor_adapter(event_tx, workspace.path().to_path_buf(), config, Vec::new());
        adapter.send(linger_request("turn-1", "first")).unwrap();
        wait_for_turn_completion(&mut event_rx).await;
        let sent_at = Instant::now();
        adapter.send(linger_request("turn-2", "second")).unwrap();
        let mut errors = Vec::new();
        let mut completed_at = None;
        while sent_at.elapsed() < TERMINAL_EXIT_GRACE + Duration::from_secs(5)
            && completed_at.is_none()
        {
            if let Ok(Some(output)) = timeout(Duration::from_millis(200), event_rx.recv()).await {
                let turn = output.turn_id.as_ref().map(|t| t.as_str().to_owned());
                match output.event {
                    AgentEvent::Error(message) => errors.push(message),
                    AgentEvent::Completed if turn.as_deref() == Some("turn-2") => {
                        completed_at = Some(sent_at.elapsed());
                    }
                    _ => {}
                }
            }
        }
        assert!(errors.is_empty(), "no error expected: {errors:?}");
        let completed_at = completed_at.expect("the queued prompt must run");
        assert!(
            completed_at < TERMINAL_EXIT_GRACE + Duration::from_secs(2),
            "queued prompt waited {completed_at:?}"
        );
        let mut pids = logged_pids(&log_path, "LEADER ");
        pids.extend(logged_pids(&log_path, "CHILD "));
        assert_eq!(pids.len(), 2, "leader and child pids logged");
        assert!(
            wait_pids_gone(&pids, Duration::from_secs(2)).await,
            "the lingering group must be gone: {pids:?}"
        );
        let prompts = std::fs::read_to_string(&log_path).unwrap();
        let prompts: Vec<_> = prompts
            .lines()
            .filter(|l| l.starts_with("PROMPT"))
            .collect();
        assert_eq!(prompts, vec!["PROMPT first", "PROMPT second"]);

        drop(adapter);
        let _ = worker.await;
        unsafe {
            std::env::remove_var("DMUXAI_CURSOR_BIN");
        }
    }

    // Review fix (#82): Cancel while a prompt waits behind a lingering CLI
    // stops that CLI's whole group and drops the queued prompt (Cancelled).
    #[tokio::test]
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    async fn cancel_while_queued_kills_lingering_cli_and_drops_prompt() {
        let _guard = TEST_CURSOR_MUTEX.lock().await;
        let _env_guard = dmuxai::state_path::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let (script_path, log_path) = lingering_cursor_script(dir.path(), 60);
        // SAFETY: TEST_CURSOR_MUTEX is held for the whole test.
        unsafe {
            std::env::set_var("DMUXAI_CURSOR_BIN", &script_path);
        }
        let (event_tx, mut event_rx) = mpsc::channel(64);
        let config = CursorAdapterConfig {
            protocol: RunnerProtocol::Legacy,
            prewarm: false,
            prewarm_idle_secs: 300,
        };
        let workspace = tempfile::tempdir().unwrap();
        let (adapter, worker) =
            spawn_cursor_adapter(event_tx, workspace.path().to_path_buf(), config, Vec::new());
        adapter.send(linger_request("turn-1", "first")).unwrap();
        wait_for_turn_completion(&mut event_rx).await;
        adapter.send(linger_request("turn-2", "second")).unwrap();
        // Let the runtime queue it behind the lingering process.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let cancelled_at = Instant::now();
        adapter.cancel().unwrap();
        let mut cancelled_turn = None;
        while cancelled_at.elapsed() < Duration::from_secs(3) && cancelled_turn.is_none() {
            if let Ok(Some(output)) = timeout(Duration::from_millis(200), event_rx.recv()).await
                && output.event == AgentEvent::Cancelled
            {
                cancelled_turn = output.turn_id.map(|t| t.as_str().to_owned());
            }
        }
        assert_eq!(cancelled_turn.as_deref(), Some("turn-2"));
        assert!(
            cancelled_at.elapsed() < TERMINAL_EXIT_GRACE,
            "cancel must not wait for the grace period"
        );
        let mut pids = logged_pids(&log_path, "LEADER ");
        pids.extend(logged_pids(&log_path, "CHILD "));
        assert_eq!(pids.len(), 2, "leader and child pids logged");
        assert!(
            wait_pids_gone(&pids, Duration::from_secs(2)).await,
            "cancel must stop the lingering group: {pids:?}"
        );
        // The queued prompt never runs, even after the grace period.
        tokio::time::sleep(TERMINAL_EXIT_GRACE + Duration::from_millis(500)).await;
        let prompts = std::fs::read_to_string(&log_path).unwrap();
        let prompts: Vec<_> = prompts
            .lines()
            .filter(|l| l.starts_with("PROMPT"))
            .collect();
        assert_eq!(prompts, vec!["PROMPT first"]);

        drop(adapter);
        let _ = worker.await;
        unsafe {
            std::env::remove_var("DMUXAI_CURSOR_BIN");
        }
    }

    // Review fix (#82): the adapter closing while a prompt is queued reports
    // Cancelled for it instead of dropping it silently.
    #[tokio::test]
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    async fn adapter_close_while_queued_reports_cancelled() {
        let _guard = TEST_CURSOR_MUTEX.lock().await;
        let _env_guard = dmuxai::state_path::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let (script_path, log_path) = lingering_cursor_script(dir.path(), 60);
        // SAFETY: TEST_CURSOR_MUTEX is held for the whole test.
        unsafe {
            std::env::set_var("DMUXAI_CURSOR_BIN", &script_path);
        }
        let (event_tx, mut event_rx) = mpsc::channel(64);
        let config = CursorAdapterConfig {
            protocol: RunnerProtocol::Legacy,
            prewarm: false,
            prewarm_idle_secs: 300,
        };
        let workspace = tempfile::tempdir().unwrap();
        let (adapter, worker) =
            spawn_cursor_adapter(event_tx, workspace.path().to_path_buf(), config, Vec::new());
        adapter.send(linger_request("turn-1", "first")).unwrap();
        wait_for_turn_completion(&mut event_rx).await;
        adapter.send(linger_request("turn-2", "second")).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(adapter);
        let _ = timeout(Duration::from_secs(3), worker).await;
        let mut cancelled = Vec::new();
        while let Ok(Some(output)) = timeout(Duration::from_millis(200), event_rx.recv()).await {
            if output.event == AgentEvent::Cancelled {
                cancelled.push(output.turn_id.map(|t| t.as_str().to_owned()));
            }
        }
        assert_eq!(cancelled, vec![Some("turn-2".to_owned())]);
        let mut pids = logged_pids(&log_path, "LEADER ");
        pids.extend(logged_pids(&log_path, "CHILD "));
        assert!(
            wait_pids_gone(&pids, Duration::from_secs(2)).await,
            "closing the adapter must stop the lingering group: {pids:?}"
        );
        unsafe {
            std::env::remove_var("DMUXAI_CURSOR_BIN");
        }
    }

    #[tokio::test]
    #[cfg(unix)]
    // Review fix: When MCP servers apply to Cursor, prewarm is skipped to avoid writing guard prematurely.
    async fn prewarm_skipped_when_mcp_servers_apply() {
        use dmuxai::mcp::config::{ResolvedMcpServer, ServerKind, ToolNames, Transport};
        use std::collections::BTreeMap;

        let fixture = FakeCursorFixture::new().await;
        let (event_tx, _) = mpsc::channel(64);
        let config = CursorAdapterConfig {
            protocol: RunnerProtocol::Legacy,
            prewarm: true,
            prewarm_idle_secs: 300,
        };
        let workspace = tempfile::tempdir().unwrap();

        let server = ResolvedMcpServer {
            name: "test-server".to_owned(),
            kind: ServerKind::Generic,
            transport: Transport::Stdio {
                command: "echo".to_owned(),
                args: vec![],
                env: BTreeMap::new(),
            },
            runners: Some(vec!["cursor".to_owned()]),
            tools: ToolNames::default(),
        };

        let (adapter, worker) = spawn_cursor_adapter(
            event_tx,
            workspace.path().to_path_buf(),
            config,
            vec![server],
        );

        // Prewarm call should be a no-op because applicable MCP servers exist
        adapter.prewarm().unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            fixture.read_spawns().len(),
            0,
            "prewarm must be skipped when MCP servers apply to Cursor"
        );

        drop(adapter);
        let _ = worker.await;
    }

    #[tokio::test]
    #[cfg(unix)]
    // TC-806 / FR-806, FR-807: Changing the mode or model before the prompt discards the warm process,
    // and its pid no longer exists afterwards; a fresh one is spawned; /new discards it the same way.
    async fn prewarm_discarded_on_arg_change() {
        let fixture = FakeCursorFixture::new().await;
        let (event_tx, mut event_rx) = mpsc::channel(64);
        let config = CursorAdapterConfig {
            protocol: RunnerProtocol::Legacy,
            prewarm: true,
            prewarm_idle_secs: 300,
        };
        let workspace = tempfile::tempdir().unwrap();
        let (adapter, worker) =
            spawn_cursor_adapter(event_tx, workspace.path().to_path_buf(), config, Vec::new());

        // 1. Initial prewarm
        adapter.prewarm().unwrap();
        let spawns = fixture.wait_for_spawns(1, Duration::from_secs(3)).await;
        assert_eq!(spawns.len(), 1);
        let pid1 = spawns[0].pid;
        assert!(fixture.is_pid_alive(pid1));

        // 2. Change model
        adapter.select_model("custom-model".to_owned()).unwrap();
        let event = event_rx.recv().await.unwrap();
        assert_eq!(
            event.event,
            AgentEvent::ModelSelected("custom-model".to_owned())
        );

        assert!(
            fixture
                .wait_for_pid_gone(pid1, Duration::from_secs(3))
                .await,
            "pid1 should be dead after model change"
        );
        let spawns = fixture.wait_for_spawns(2, Duration::from_secs(3)).await;
        assert_eq!(spawns.len(), 2);
        let pid2 = spawns[1].pid;
        assert!(fixture.is_pid_alive(pid2));

        // 3. /new (reset)
        adapter.reset().unwrap();
        assert!(
            fixture
                .wait_for_pid_gone(pid2, Duration::from_secs(3))
                .await,
            "pid2 should be dead after /new reset"
        );
        // Reset never spawns; the next selection trigger does.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(fixture.read_spawns().len(), 2, "reset must not spawn");
        adapter.prewarm().unwrap();
        let spawns = fixture.wait_for_spawns(3, Duration::from_secs(3)).await;
        assert_eq!(spawns.len(), 3);
        let pid3 = spawns[2].pid;
        assert!(fixture.is_pid_alive(pid3));

        // 4. Next prompt runs using the warm process
        let req = AgentRequest {
            conversation_id: super::super::ConversationId::new("conv-1"),
            turn_id: super::super::TurnId::new("turn-1"),
            prompt: "hello".to_owned(),
            resume_session: None,
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        };
        adapter.send(req).unwrap();
        wait_for_turn_completion(&mut event_rx).await;

        let prompts = fixture.prompts_received();
        assert_eq!(prompts.len(), 1);
        assert_eq!(
            prompts[0].0, pid3,
            "prompt should run in the warm process spawned after reset"
        );

        drop(adapter);
        let _ = worker.await;
    }

    #[tokio::test]
    #[cfg(unix)]
    // TC-807 / FR-807: reaping a warm process kills its whole process group,
    // including a grandchild that ignores SIGTERM.
    async fn reaping_warm_process_kills_term_ignoring_grandchild() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = FakeCursorFixture::new().await;
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("grandchild.pid");
        let script = dir.path().join("cursor-agent");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\ntrap '' TERM\n(trap '' TERM; exec sleep 300) &\necho $! > \"{}\"\nread -r line\n",
                pid_file.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: the fixture holds TEST_CURSOR_MUTEX for the whole test.
        unsafe { std::env::set_var("DMUXAI_CURSOR_BIN", &script) };

        let warm = spawn_warm_process(None, None, TurnPolicy::ReadOnly, dir.path()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let grandchild = loop {
            if let Ok(text) = std::fs::read_to_string(&pid_file)
                && let Ok(pid) = text.trim().parse::<u32>()
            {
                break pid;
            }
            assert!(Instant::now() < deadline, "grandchild never started");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert!(fixture.is_pid_alive(grandchild));
        reap_warm_process(warm).await;
        assert!(
            fixture
                .wait_for_pid_gone(grandchild, Duration::from_secs(3))
                .await,
            "grandchild must die with the warm process group"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    // TC-807 / FR-807, NFR-805: No warm spawn when disabled, in headless run, or for graph nodes;
    // idle expiry and quit kill it (pid gone).
    async fn prewarm_disabled_and_expiry() {
        let fixture = FakeCursorFixture::new().await;
        let (event_tx, mut event_rx) = mpsc::channel(64);
        let workspace = tempfile::tempdir().unwrap();

        // 1. Headless run config check: default config has prewarm = false
        assert!(
            !CursorAdapterConfig::default().prewarm,
            "prewarm should default to false for headless/isolated use"
        );

        // 2. Disabled config: no warm spawn
        let disabled_config = CursorAdapterConfig {
            protocol: RunnerProtocol::Legacy,
            prewarm: false,
            prewarm_idle_secs: 300,
        };
        let (disabled_adapter, disabled_worker) = spawn_cursor_adapter(
            event_tx.clone(),
            workspace.path().to_path_buf(),
            disabled_config,
            Vec::new(),
        );
        disabled_adapter.prewarm().unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            fixture.read_spawns().len(),
            0,
            "disabled adapter must not prewarm"
        );

        let req = AgentRequest {
            conversation_id: super::super::ConversationId::new("c-dis"),
            turn_id: super::super::TurnId::new("t-dis"),
            prompt: "run disabled".to_owned(),
            resume_session: None,
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        };
        disabled_adapter.send(req).unwrap();
        wait_for_turn_completion(&mut event_rx).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            fixture.read_spawns().len(),
            1,
            "turn executed once, no warm process afterwards"
        );
        drop(disabled_adapter);
        let _ = disabled_worker.await;

        // 3. Graph nodes never prewarm
        let enabled_config = CursorAdapterConfig {
            protocol: RunnerProtocol::Legacy,
            prewarm: true,
            prewarm_idle_secs: 1, // short idle expiry for step 4
        };
        let (enabled_adapter, enabled_worker) = spawn_cursor_adapter(
            event_tx.clone(),
            workspace.path().to_path_buf(),
            enabled_config,
            Vec::new(),
        );
        enabled_adapter.prewarm().unwrap();
        let spawns = fixture.wait_for_spawns(2, Duration::from_secs(3)).await;
        let warm_pid = spawns[1].pid;
        assert!(fixture.is_pid_alive(warm_pid));

        let graph_req = AgentRequest {
            conversation_id: super::super::ConversationId::new("c-graph"),
            turn_id: super::super::TurnId::new("t-graph"),
            prompt: "graph node prompt".to_owned(),
            resume_session: None,
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        };
        enabled_adapter.send_graph_node(graph_req).unwrap();
        assert!(
            fixture
                .wait_for_pid_gone(warm_pid, Duration::from_secs(3))
                .await,
            "warm process must be reaped on graph node"
        );
        wait_for_turn_completion(&mut event_rx).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            fixture.read_spawns().len(),
            3,
            "graph turn completed without spawning a warm process"
        );

        // 4. Idle expiry: prewarm a process and verify it expires
        enabled_adapter.prewarm().unwrap();
        let spawns = fixture.wait_for_spawns(4, Duration::from_secs(3)).await;
        assert_eq!(spawns.len(), 4);
        let idle_pid = spawns[3].pid;
        assert!(fixture.is_pid_alive(idle_pid));

        tokio::time::sleep(Duration::from_millis(1200)).await;
        enabled_adapter.tick().unwrap();
        assert!(
            fixture
                .wait_for_pid_gone(idle_pid, Duration::from_secs(3))
                .await,
            "idle warm process should be reaped after expiry"
        );

        // 5. Quit / shutdown kills any warm process
        enabled_adapter.prewarm().unwrap();
        let spawns = fixture.wait_for_spawns(5, Duration::from_secs(3)).await;
        assert_eq!(spawns.len(), 5);
        let quit_pid = spawns[4].pid;
        assert!(fixture.is_pid_alive(quit_pid));

        drop(enabled_adapter);
        let _ = enabled_worker.await;
        assert!(
            fixture
                .wait_for_pid_gone(quit_pid, Duration::from_secs(3))
                .await,
            "quit/drop must reap warm process"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    // TC-808 / FR-806: A warm process that has exited is reaped and the prompt runs in a fresh process.
    async fn prewarm_exited_process_is_replaced() {
        const WAIT: Duration = Duration::from_secs(15);
        let fixture = FakeCursorFixture::new().await;
        let (event_tx, mut event_rx) = mpsc::channel(64);
        let config = CursorAdapterConfig {
            protocol: RunnerProtocol::Legacy,
            prewarm: true,
            prewarm_idle_secs: 300,
        };
        let workspace = tempfile::tempdir().unwrap();

        // Create the exit-once marker file so the warm process exits immediately on spawn
        std::fs::write(&fixture.exit_once_marker, b"exit").unwrap();

        let (adapter, worker) =
            spawn_cursor_adapter(event_tx, workspace.path().to_path_buf(), config, Vec::new());
        adapter.prewarm().unwrap();

        let spawns = fixture.wait_for_spawns(1, WAIT).await;
        assert_eq!(spawns.len(), 1);
        let exited_pid = spawns[0].pid;

        assert!(
            fixture.wait_for_pid_gone(exited_pid, WAIT).await,
            "warm process should have exited immediately"
        );

        // Now send prompt. Since the warm process already exited, it is reaped and a fresh process is spawned.
        let req = AgentRequest {
            conversation_id: super::super::ConversationId::new("conv-1"),
            turn_id: super::super::TurnId::new("turn-1"),
            prompt: "fresh prompt".to_owned(),
            resume_session: None,
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        };
        adapter.send(req).unwrap();
        wait_for_turn_completion(&mut event_rx).await;

        let spawns = fixture.read_spawns();
        assert!(
            spawns.len() >= 2,
            "exited warm process + at least one fresh process (got {spawns:?})"
        );
        let prompt_pid = fixture.prompts_received()[0].0;
        assert_ne!(
            prompt_pid, exited_pid,
            "prompt did not run on exited warm pid"
        );

        let prompts = fixture.prompts_received();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].0, prompt_pid, "fresh process handled the prompt");
        assert_eq!(prompts[0].1, "fresh prompt");

        drop(adapter);
        let _ = worker.await;
    }

    // TC-1300 / FR-1300: Cursor ACP protocol drives a fake ACP agent binary.
    #[cfg(unix)]
    mod fake_acp_agent_tests {
        use super::*;
        use crate::agent::{ConversationId, TurnId};

        /// Holds the shared cursor test lock and clears the fake env on drop.
        struct FakeEnvGuard(
            #[allow(dead_code)] tokio::sync::MutexGuard<'static, ()>,
            #[allow(dead_code)] std::sync::MutexGuard<'static, ()>,
        );

        impl Drop for FakeEnvGuard {
            fn drop(&mut self) {
                // SAFETY: still holding TEST_CURSOR_MUTEX and test_env_lock.
                unsafe {
                    for key in FAKE_VARS {
                        std::env::remove_var(key);
                    }
                    std::env::remove_var("DMUXAI_CURSOR_BIN");
                }
            }
        }

        async fn lock_env() -> FakeEnvGuard {
            FakeEnvGuard(
                TEST_CURSOR_MUTEX.lock().await,
                dmuxai::state_path::test_env_lock(),
            )
        }
        static BUILD: std::sync::Once = std::sync::Once::new();

        /// Path to `fake-acp-agent`, building it on first use (needs the
        /// `test-fixtures` feature only for the bin target, not for tests).
        fn fake_acp_agent_bin() -> PathBuf {
            let mut dir = std::env::current_exe().expect("test exe");
            dir.pop();
            if dir.ends_with("deps") {
                dir.pop();
            }
            let path = dir.join("fake-acp-agent");
            BUILD.call_once(|| {
                let target_dir = dir.parent().expect("target dir").to_path_buf();
                let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
                let status = std::process::Command::new(cargo)
                    .args([
                        "build",
                        "--quiet",
                        "--bin",
                        "fake-acp-agent",
                        "--features",
                        "test-fixtures",
                    ])
                    .arg("--target-dir")
                    .arg(&target_dir)
                    .current_dir(env!("CARGO_MANIFEST_DIR"))
                    .status()
                    .expect("spawn cargo build for fake-acp-agent");
                assert!(status.success(), "building fake-acp-agent failed");
            });
            assert!(
                path.is_file(),
                "fake-acp-agent missing at {}",
                path.display()
            );
            path
        }

        const FAKE_VARS: &[&str] = &[
            "FAKE_ACP_FAST_ONLY",
            "FAKE_ACP_PERMISSION_OUTSIDE",
            "FAKE_ACP_MODES",
            "FAKE_ACP_MODE_CAPTURE",
            "FAKE_ACP_PROMPT_CAPTURE",
            "FAKE_ACP_MCP_CAPTURE",
        ];

        fn set_fake_env(vars: &[(&str, &std::ffi::OsStr)]) {
            let fake = fake_acp_agent_bin();
            // SAFETY: callers hold TEST_CURSOR_MUTEX (lock_env) for the whole test.
            unsafe {
                for key in FAKE_VARS {
                    std::env::remove_var(key);
                }
                std::env::set_var("DMUXAI_CURSOR_BIN", &fake);
                for (key, value) in vars {
                    std::env::set_var(key, value);
                }
            }
        }

        fn request(policy: TurnPolicy) -> AgentRequest {
            AgentRequest {
                conversation_id: ConversationId::new("conv-acp"),
                turn_id: TurnId::new("turn-acp"),
                prompt: "hello acp".to_owned(),
                resume_session: None,
                image_paths: vec![],
                policy,
                require_file_edits: false,
            }
        }

        async fn drive(
            policy: TurnPolicy,
            model: Option<&str>,
            workspace: &Path,
        ) -> (Result<Option<String>, AgentError>, Vec<AgentEvent>) {
            let (event_tx, mut event_rx) = mpsc::channel(256);
            let notice = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let result = run_turn(
                &request(policy),
                model.map(str::to_owned),
                policy,
                workspace,
                &event_tx,
                RunnerProtocol::Acp,
                &[],
                &notice,
            )
            .await;
            drop(event_tx);
            let mut events = Vec::new();
            while let Some(output) = event_rx.recv().await {
                events.push(output.event);
            }
            (result, events)
        }

        fn live_fake_acp_agent_count() -> usize {
            let Ok(output) = std::process::Command::new("pgrep")
                .args(["-x", "fake-acp-agent"])
                .output()
            else {
                return 0;
            };
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|pid| {
                    let stat = std::process::Command::new("ps")
                        .args(["-o", "stat=", "-p", pid.trim()])
                        .output()
                        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                        .unwrap_or_default();
                    !stat.is_empty() && !stat.starts_with('Z')
                })
                .count()
        }

        #[tokio::test]
        async fn cursor_acp_turn_with_fake_agent() {
            let _guard = lock_env().await;
            set_fake_env(&[]);
            let workspace = tempfile::tempdir().unwrap();
            let (result, events) = drive(TurnPolicy::Agent, None, workspace.path()).await;
            result.expect("acp turn");
            let text: String = events
                .iter()
                .filter_map(|e| match e {
                    AgentEvent::TextDelta(d) => Some(d.as_str()),
                    _ => None,
                })
                .collect();
            assert!(events.iter().any(|e| matches!(e, AgentEvent::Completed)));
            assert!(text.contains("ACP_OK"), "{text}");
        }

        // TC-1412, TC-1329 / FR-1411, FR-1327: fast-only session refused before any
        // prompt; child reaped.
        #[tokio::test]
        async fn tc_1412_fast_only_refused_without_prompt() {
            let _guard = lock_env().await;
            let capture = tempfile::NamedTempFile::new().unwrap();
            std::fs::remove_file(capture.path()).ok();
            set_fake_env(&[
                ("FAKE_ACP_FAST_ONLY", "1".as_ref()),
                ("FAKE_ACP_PROMPT_CAPTURE", capture.path().as_os_str()),
            ]);
            let workspace = tempfile::tempdir().unwrap();
            let (result, _events) =
                drive(TurnPolicy::Agent, Some("composer-2.5"), workspace.path()).await;
            let err = result.unwrap_err();
            assert!(err.to_string().contains("fast"), "{err}");
            assert!(!capture.path().exists(), "no session/prompt may be sent");
            let mut alive = live_fake_acp_agent_count();
            for _ in 0..30 {
                if alive == 0 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                alive = live_fake_acp_agent_count();
            }
            assert_eq!(alive, 0, "fake-acp-agent process should not outlive turn");
        }

        // TC-1331, TC-1420, TC-1421 / FR-1416: denied permission ends Cancelled, not Error.
        #[tokio::test]
        async fn tc_1420_permission_denied_ends_cancelled() {
            let _guard = lock_env().await;
            set_fake_env(&[("FAKE_ACP_PERMISSION_OUTSIDE", "1".as_ref())]);
            let workspace = tempfile::tempdir().unwrap();
            let (result, events) = drive(TurnPolicy::Agent, None, workspace.path()).await;
            result.expect("denied turn is not an error");
            assert!(events.iter().any(|e| matches!(e, AgentEvent::Cancelled)));
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, AgentEvent::Error(_) | AgentEvent::Completed))
            );
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("denied")))
            );
        }

        async fn read_only_mode_call(shape: &str) -> String {
            let capture = tempfile::NamedTempFile::new().unwrap();
            set_fake_env(&[
                ("FAKE_ACP_MODES", shape.as_ref()),
                ("FAKE_ACP_MODE_CAPTURE", capture.path().as_os_str()),
            ]);
            let workspace = tempfile::tempdir().unwrap();
            let (result, _) = drive(TurnPolicy::ReadOnly, None, workspace.path()).await;
            result.expect("read-only turn");
            std::fs::read_to_string(capture.path()).unwrap_or_default()
        }

        // TC-1424, TC-1425 / FR-1413, NFR-1410: modes / configOptions / both / none.
        #[tokio::test]
        async fn tc_1424_read_only_mode_shapes() {
            let _guard = lock_env().await;
            assert_eq!(read_only_mode_call("modes").await, "session/set_mode ask");
            assert_eq!(
                read_only_mode_call("config").await,
                "session/set_config_option plan"
            );
            assert_eq!(read_only_mode_call("both").await, "session/set_mode ask");
            assert_eq!(read_only_mode_call("none").await, "");
        }
    }
}
