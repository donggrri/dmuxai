//! Runtime for `[[agents.custom]]` headless agents (see docs/agents/custom-agents.md).

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use dmuxai::custom_agent::{
    BUILTIN_MODELS_TIMEOUT, CustomAgentOutput, CustomAgentSpec, SubstitutionContext, build_argv,
    failure_reason, grok_no_allowed_model_message, is_builtin_grok, is_builtin_opencode,
    list_model_ids, parse_jsonl_line, policy_message, select_grok_model, spec_at, substitute,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::mpsc,
    task::JoinHandle,
};

use super::{
    AdapterError, Agent, AgentAdapter, AgentEvent, AgentOutput, AgentRequest, ModelInfo,
    OutputContext, SpawnedAdapter, TurnPolicy,
};

const COMMAND_CHANNEL_CAPACITY: usize = 8;
const STDERR_LIMIT: usize = 16 * 1024;

enum CustomCommand {
    Send(AgentRequest),
    Cancel,
    ListModels,
    SelectModel(String),
}

pub struct CustomAdapter {
    index: u8,
    command_tx: mpsc::Sender<CustomCommand>,
}

impl CustomAdapter {
    fn push(&self, command: CustomCommand) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(command)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => AdapterError::CommandQueueFull,
                mpsc::error::TrySendError::Closed(_) => AdapterError::RuntimeStopped,
            })
    }
}

impl AgentAdapter for CustomAdapter {
    fn agent(&self) -> Agent {
        Agent::Custom(self.index)
    }
    fn send(&self, request: AgentRequest) -> Result<(), AdapterError> {
        self.push(CustomCommand::Send(request))
    }
    fn cancel(&self) -> Result<(), AdapterError> {
        self.push(CustomCommand::Cancel)
    }
    fn request_models(&self) -> Result<(), AdapterError> {
        self.push(CustomCommand::ListModels)
    }
    fn select_model(&self, model: String) -> Result<(), AdapterError> {
        self.push(CustomCommand::SelectModel(model))
    }
}

pub fn spawn_custom_adapter(
    index: u8,
    event_tx: mpsc::Sender<AgentOutput>,
    cwd: PathBuf,
    mcp_servers: Vec<dmuxai::mcp::ResolvedMcpServer>,
) -> SpawnedAdapter {
    let (command_tx, command_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
    let worker = tokio::spawn(runtime_loop(index, command_rx, event_tx, cwd, mcp_servers));
    (Box::new(CustomAdapter { index, command_tx }), worker)
}

struct ActiveTurn {
    handle: JoinHandle<()>,
    context: OutputContext,
    pgid: std::sync::Arc<std::sync::atomic::AtomicU32>,
    acp_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

async fn runtime_loop(
    index: u8,
    command_rx: mpsc::Receiver<CustomCommand>,
    event_tx: mpsc::Sender<AgentOutput>,
    cwd: PathBuf,
    mcp_servers: Vec<dmuxai::mcp::ResolvedMcpServer>,
) {
    let Some(spec) = spec_at(index) else {
        return;
    };
    runtime_loop_with_spec(
        Agent::Custom(index),
        spec,
        command_rx,
        event_tx,
        cwd,
        mcp_servers,
    )
    .await;
}

async fn runtime_loop_with_spec(
    agent: Agent,
    spec: &'static CustomAgentSpec,
    mut command_rx: mpsc::Receiver<CustomCommand>,
    event_tx: mpsc::Sender<AgentOutput>,
    cwd: PathBuf,
    mcp_servers: Vec<dmuxai::mcp::ResolvedMcpServer>,
) {
    let applicable: Vec<dmuxai::mcp::ResolvedMcpServer> = mcp_servers
        .into_iter()
        .filter(|s| s.applies_to(&spec.name))
        .collect();
    let mut active: Option<ActiveTurn> = None;
    let mut selected_model = spec.default_model.clone();
    let mcp_notice_shown = Arc::new(AtomicBool::new(false));

    loop {
        let command = if let Some(turn) = active.as_mut() {
            tokio::select! {
                _ = &mut turn.handle => { active = None; continue; }
                command = command_rx.recv() => command,
            }
        } else {
            command_rx.recv().await
        };
        let Some(command) = command else {
            stop_turn(agent, active.take(), &event_tx).await;
            break;
        };
        match command {
            CustomCommand::Send(request) if active.is_some() => {
                let context = OutputContext::from(&request);
                send(
                    &event_tx,
                    agent,
                    Some(&context),
                    AgentEvent::Error(format!(
                        "{} is already running a turn; cancel it before sending another prompt",
                        spec.label()
                    )),
                )
                .await;
            }
            CustomCommand::Send(request) => {
                let ensured = if is_builtin_grok(spec) {
                    ensure_grok_model(spec, &cwd, &mut selected_model).await
                } else {
                    ensure_builtin_model(spec, &cwd, &mut selected_model).await
                };
                if let Err(error) = ensured {
                    let context = OutputContext::from(&request);
                    send(&event_tx, agent, Some(&context), AgentEvent::Error(error)).await;
                    continue;
                }
                active = Some(spawn_turn(
                    agent,
                    spec,
                    request,
                    selected_model.clone(),
                    cwd.clone(),
                    event_tx.clone(),
                    applicable.clone(),
                    mcp_notice_shown.clone(),
                ));
            }
            CustomCommand::Cancel => stop_turn(agent, active.take(), &event_tx).await,
            CustomCommand::ListModels => {
                let event = match list_models(spec, &cwd).await {
                    Ok(models) => {
                        if is_builtin_grok(spec) {
                            let ids: Vec<String> =
                                models.iter().map(|model| model.id.clone()).collect();
                            selected_model = select_grok_model(&ids, selected_model.as_deref());
                        } else if spec.builtin && selected_model.is_none() {
                            selected_model = models.first().map(|model| model.id.clone());
                        }
                        AgentEvent::ModelsListed {
                            models,
                            selected: selected_model.clone(),
                            selected_effort: None,
                        }
                    }
                    Err(error) => AgentEvent::Error(error),
                };
                send(&event_tx, agent, None, event).await;
            }
            CustomCommand::SelectModel(model) => {
                let validated = if is_builtin_grok(spec) {
                    dmuxai::model_policy::validate_model_for_runner("grok", &model)
                } else {
                    dmuxai::custom_agent::validate_custom_model(spec, &model)
                };
                if let Err(error) = validated {
                    send(&event_tx, agent, None, AgentEvent::Error(error)).await;
                    continue;
                }
                selected_model = Some(model.clone());
                send(&event_tx, agent, None, AgentEvent::ModelSelected(model)).await;
            }
        }
    }
}

/// Lists the models for the catalog. The built-in preset is bounded by a
/// timeout and its policy errors name the way out (FR-812).
async fn list_models(spec: &CustomAgentSpec, cwd: &Path) -> Result<Vec<ModelInfo>, String> {
    let timeout = spec.builtin.then_some(BUILTIN_MODELS_TIMEOUT);
    let mut ids = list_model_ids(spec, cwd, timeout).await?;
    // Grok never inserts an id that `grok models` did not list (FR-895).
    if !is_builtin_grok(spec)
        && let Some(default) = &spec.default_model
        && !ids.contains(default)
    {
        ids.insert(0, default.clone());
    }
    Ok(ids
        .into_iter()
        .map(|id| ModelInfo::new(id.clone(), id))
        .collect())
}

/// The built-in preset without a picked model uses the first allowed listed
/// model, so a turn never falls back to opencode's own default model (FR-812).
async fn ensure_builtin_model(
    spec: &CustomAgentSpec,
    cwd: &Path,
    selected: &mut Option<String>,
) -> Result<(), String> {
    if !spec.builtin || selected.is_some() {
        return Ok(());
    }
    let models = list_models(spec, cwd).await?;
    let first = models
        .first()
        .map(|model| model.id.clone())
        .ok_or_else(|| policy_message(spec, None))?;
    *selected = Some(first);
    Ok(())
}

/// Grok probes `grok models` on every send, even when a model string is already
/// filled in (FR-896). Auth and the allowed-id filter run before `grok -p`.
async fn ensure_grok_model(
    spec: &CustomAgentSpec,
    cwd: &Path,
    selected: &mut Option<String>,
) -> Result<(), String> {
    let models = list_models(spec, cwd).await?;
    let ids: Vec<String> = models.into_iter().map(|model| model.id).collect();
    match select_grok_model(&ids, selected.as_deref()) {
        Some(id) => {
            *selected = Some(id);
            Ok(())
        }
        None => Err(grok_no_allowed_model_message()),
    }
}

fn mode_names(policy: TurnPolicy) -> (&'static str, &'static str) {
    match policy {
        TurnPolicy::ReadOnly => ("ask", "deny"),
        TurnPolicy::Agent => ("implement", "allow"),
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_turn(
    agent: Agent,
    spec: &'static CustomAgentSpec,
    request: AgentRequest,
    model: Option<String>,
    cwd: PathBuf,
    event_tx: mpsc::Sender<AgentOutput>,
    mcp_servers: Vec<dmuxai::mcp::ResolvedMcpServer>,
    mcp_notice_shown: Arc<AtomicBool>,
) -> ActiveTurn {
    let context = OutputContext::from(&request);
    let pgid = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let settings = dmuxai::settings::DmuxSettings::load(&cwd);
    let uses_acp = (is_builtin_grok(spec)
        && settings.grok_protocol == dmuxai::runner_protocol::RunnerProtocol::Acp)
        || (is_builtin_opencode(spec)
            && settings.opencode_protocol == dmuxai::runner_protocol::RunnerProtocol::Acp);
    let acp_cancel = uses_acp.then(super::acp::new_cancel_flag);
    let turn_context = context.clone();
    let turn_pgid = pgid.clone();
    let cancel_for_turn = acp_cancel.clone();
    let handle = tokio::spawn(async move {
        send(&event_tx, agent, Some(&turn_context), AgentEvent::Started).await;
        let event = match run_turn(
            agent,
            spec,
            &request,
            model,
            &cwd,
            &event_tx,
            &turn_context,
            &turn_pgid,
            &mcp_servers,
            &mcp_notice_shown,
            cancel_for_turn,
        )
        .await
        {
            Ok(()) => AgentEvent::Completed,
            Err(error) => AgentEvent::Error(error),
        };
        send(&event_tx, agent, Some(&turn_context), event).await;
    });
    ActiveTurn {
        handle,
        context,
        pgid,
        acp_cancel,
    }
}

/// Removes the temp config file when the turn ends (including cancellation).
struct TempFile(Option<PathBuf>);
impl Drop for TempFile {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Writes content to a private temp file with 0600 permissions on unix (create_new).
fn write_private_temp_file(path: &Path, content: &str, label: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| format!("{label}: {e}"))?;
        file.write_all(content.as_bytes())
            .map_err(|e| format!("{label}: {e}"))?;
    }
    #[cfg(not(unix))]
    {
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| format!("{label}: {e}"))?;
        file.write_all(content.as_bytes())
            .map_err(|e| format!("{label}: {e}"))?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_turn(
    agent: Agent,
    spec: &CustomAgentSpec,
    request: &AgentRequest,
    model: Option<String>,
    cwd: &Path,
    event_tx: &mpsc::Sender<AgentOutput>,
    context: &OutputContext,
    pgid: &std::sync::atomic::AtomicU32,
    mcp_servers: &[dmuxai::mcp::ResolvedMcpServer],
    mcp_notice_shown: &AtomicBool,
    acp_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<(), String> {
    let settings = dmuxai::settings::DmuxSettings::load(cwd);
    if is_builtin_grok(spec)
        && settings.grok_protocol == dmuxai::runner_protocol::RunnerProtocol::Acp
    {
        let model_id = model.as_deref().ok_or_else(grok_no_allowed_model_message)?;
        let program = spec
            .command
            .first()
            .ok_or_else(|| format!("{}: empty command", spec.label()))?;
        let effort = spec
            .env
            .get("GROK_REASONING_EFFORT")
            .cloned()
            .or_else(|| std::env::var("GROK_REASONING_EFFORT").ok());
        let args = super::acp_runner::grok_acp_argv(model_id, effort.as_deref());
        let permission_ctx = super::acp::PermissionContext {
            workspace: cwd.to_string_lossy().into_owned(),
            sensitive_gate_active: false,
            read_only_fail_closed: false,
        };
        let cancel = acp_cancel.unwrap_or_else(super::acp::new_cancel_flag);
        super::acp_runner::run_agent_acp_turn(
            Path::new(program),
            &args,
            cwd,
            request,
            agent,
            event_tx,
            permission_ctx,
            super::acp::AcpRunnerKind::Grok,
            super::acp::AcpLifecycle::Delegate,
            cancel,
            Some(model_id.to_owned()),
            None,
            None,
        )
        .await?;
        return Ok(());
    }
    if is_builtin_opencode(spec)
        && settings.opencode_protocol == dmuxai::runner_protocol::RunnerProtocol::Acp
    {
        let model_id = model
            .as_deref()
            .ok_or_else(|| dmuxai::custom_agent::policy_message(spec, None))?;
        dmuxai::custom_agent::validate_custom_model(spec, model_id)?;
        let program = spec
            .command
            .first()
            .ok_or_else(|| format!("{}: empty command", spec.label()))?;
        let program_path = Path::new(program);
        if !program_path.is_file() {
            if let Some(message) = dmuxai::settings::opencode_bin_explicit_missing_message() {
                return Err(message);
            }
            return Err(format!(
                "opencode binary not found at {}",
                program_path.display()
            ));
        }
        let args = super::acp_runner::opencode_acp_argv(cwd);
        let permission_ctx = super::acp::PermissionContext {
            workspace: cwd.to_string_lossy().into_owned(),
            sensitive_gate_active: false,
            read_only_fail_closed: false,
        };
        let cancel = acp_cancel.unwrap_or_else(super::acp::new_cancel_flag);
        super::acp_runner::run_agent_acp_turn(
            Path::new(program),
            &args,
            cwd,
            request,
            agent,
            event_tx,
            permission_ctx,
            super::acp::AcpRunnerKind::Opencode,
            super::acp::AcpLifecycle::Delegate,
            cancel,
            Some(model_id.to_owned()),
            Some(model_id),
            None,
        )
        .await?;
        return Ok(());
    }

    let (mode_name, edit) = mode_names(request.policy);
    let mut ctx = SubstitutionContext::new(
        request.prompt.clone(),
        cwd.display().to_string(),
        mode_name,
        edit,
    );
    ctx.model = model;
    ctx.images = request
        .image_paths
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    ctx.session_id = request.resume_session.clone();

    let mut temp = TempFile(None);
    if let Some(template) = &spec.temp_config_template {
        let path = std::env::temp_dir().join(format!(
            "dmuxai-{}-{}.json",
            spec.name,
            uuid::Uuid::new_v4()
        ));
        let mut body = substitute(template, &ctx).unwrap_or_else(|| template.clone());
        if !mcp_servers.is_empty() {
            let (merged, applied) = dmuxai::mcp::propagate::merge_opencode_mcp(&body, mcp_servers);
            body = merged;
            if !applied && !mcp_notice_shown.swap(true, std::sync::atomic::Ordering::SeqCst) {
                send(
                    event_tx,
                    agent,
                    Some(context),
                    AgentEvent::Notice(format!(
                        "{}: could not merge MCP servers into config; temp_config_template is not a JSON object or its 'mcp' key is not an object",
                        spec.label()
                    )),
                )
                .await;
            }
        }
        write_private_temp_file(&path, &body, "temp config")?;
        ctx.temp_config = Some(path.display().to_string());
        temp.0 = Some(path);
    }

    let mut mcp_temp = TempFile(None);
    if !mcp_servers.is_empty() {
        let path = std::env::temp_dir().join(format!(
            "dmuxai-mcp-{}-{}.json",
            spec.name,
            uuid::Uuid::new_v4()
        ));
        let body = dmuxai::mcp::propagate::custom_mcp_json(mcp_servers);
        write_private_temp_file(&path, &body, "mcp config")?;
        ctx.mcp_config = Some(path.display().to_string());
        mcp_temp.0 = Some(path);
    }

    let argv = build_argv(spec, &ctx);
    let program = argv
        .first()
        .ok_or_else(|| format!("{}: empty command", spec.label()))?;
    let mut command = dmuxai::custom_agent::build_custom_command(spec, &ctx, cwd)?;

    let mut child = command
        .spawn()
        .map_err(|error| format!("{}: failed to start {program}: {error}", spec.label()))?;
    if let Some(id) = child.id() {
        pgid.store(id, std::sync::atomic::Ordering::SeqCst);
    }
    if let Some(mut stdin) = child.stdin.take() {
        let mut prompt = ctx.prompt.clone();
        for image in &ctx.images {
            prompt.push_str(&format!(" @{image}"));
        }
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(|error| format!("stdin: {error}"))?;
        drop(stdin);
    }
    let stdout = child.stdout.take().ok_or("stdout not piped")?;
    let mut stderr = child.stderr.take().ok_or("stderr not piped")?;
    let stderr_task = tokio::spawn(async move {
        let mut buffer = Vec::new();
        let _ = (&mut stderr)
            .take(STDERR_LIMIT as u64)
            .read_to_end(&mut buffer)
            .await;
        let mut sink = Vec::new();
        let _ = stderr.read_to_end(&mut sink).await;
        String::from_utf8_lossy(&buffer).into_owned()
    });

    let mut reply = String::new();
    let mut jsonl_error: Option<String> = None;
    let mut bound_session: Option<String> = None;
    let mut break_pending = false;
    let mut grok_tools = dmuxai::custom_agent::is_builtin_grok(spec)
        .then(|| dmuxai::grok_activity::GrokToolTracker::new(cwd));
    let mut done = false;
    let mut lines = BufReader::new(stdout).lines();
    let pgid_value = || pgid.load(std::sync::atomic::Ordering::SeqCst);
    let read = async {
        while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
            match spec.output {
                CustomAgentOutput::Text => {
                    let chunk = format!("{line}\n");
                    reply.push_str(&chunk);
                    send(event_tx, agent, Some(context), AgentEvent::TextDelta(chunk)).await;
                }
                CustomAgentOutput::Jsonl => {
                    if let Some(tracker) = grok_tools.as_mut()
                        && let Some(item) = tracker.observe(&line)
                    {
                        send(event_tx, agent, Some(context), AgentEvent::Activity(item)).await;
                    }
                    let parsed = parse_jsonl_line(spec, &line);
                    if let Some(session) = parsed.session_id
                        && bound_session.as_deref() != Some(session.as_str())
                    {
                        bound_session = Some(session.clone());
                        send(
                            event_tx,
                            agent,
                            Some(context),
                            AgentEvent::ProviderSessionBound(session),
                        )
                        .await;
                    }
                    if let Some(text) = parsed.text {
                        let chunk = dmuxai::custom_agent::append_reply_text(
                            &mut reply,
                            &text,
                            &mut break_pending,
                        );
                        send(event_tx, agent, Some(context), AgentEvent::TextDelta(chunk)).await;
                    }
                    if parsed.text_break {
                        break_pending = true;
                    }
                    if let Some(error) = parsed.error {
                        jsonl_error = Some(error);
                    }
                    if parsed.done {
                        // FR-914: the provider said the turn is over. Do not wait
                        // for EOF: a leftover child can hold stdout open.
                        done = true;
                        break;
                    }
                }
            }
        }
        if done {
            return Ok(finish_after_done(&mut child, pgid_value()).await);
        }
        child.wait().await.map_err(|e| e.to_string())
    };
    let status = tokio::time::timeout(Duration::from_secs(spec.timeout_secs()), read)
        .await
        .map_err(|_| {
            kill_group(pgid.load(std::sync::atomic::Ordering::SeqCst));
            format!("{} timed out after {}s", spec.label(), spec.timeout_secs())
        })??;
    // A leftover child that inherited stderr must not block the turn either.
    let stderr_text = match tokio::time::timeout(STDERR_DRAIN_GRACE, stderr_task).await {
        Ok(joined) => joined.unwrap_or_default(),
        Err(_) => String::new(),
    };
    match failure_reason(
        spec,
        done || status.success(),
        &reply,
        &stderr_text,
        jsonl_error.as_deref(),
    ) {
        Some(reason) => Err(format!("{} failed: {reason}", spec.label())),
        None => Ok(()),
    }
}

/// How long the process may take to exit on its own after its `done_when` line.
const DONE_EXIT_GRACE: Duration = Duration::from_secs(3);
/// How long SIGTERM gets before SIGKILL when stopping a finished turn's group.
const TERM_GRACE: Duration = Duration::from_secs(2);
/// How long stderr may stay open after the process exited.
const STDERR_DRAIN_GRACE: Duration = Duration::from_secs(2);

/// After the `done_when` line (FR-914): wait briefly for a normal exit, then
/// stop the whole process group (TERM, then KILL) so neither the runner nor a
/// child it left behind keeps the turn open. The turn's success does not
/// depend on how the process ended, because the provider already finished it.
async fn finish_after_done(
    child: &mut tokio::process::Child,
    pgid: u32,
) -> std::process::ExitStatus {
    let status = match tokio::time::timeout(DONE_EXIT_GRACE, child.wait()).await {
        Ok(Ok(status)) => Some(status),
        _ => None,
    };
    // Reap leftovers in the group even after a clean exit of the leader.
    kill_group(pgid);
    if let Some(status) = status {
        return status;
    }
    match tokio::time::timeout(TERM_GRACE, child.wait()).await {
        Ok(Ok(status)) => status,
        _ => {
            kill_group_with(pgid, "-KILL");
            // The group signal is Unix-only; this also stops the leader elsewhere.
            let _ = child.start_kill();
            child.wait().await.unwrap_or_else(|_| exit_status_ok())
        }
    }
}

#[cfg(unix)]
fn exit_status_ok() -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(0)
}

#[cfg(not(unix))]
fn exit_status_ok() -> std::process::ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(0)
}

fn kill_group(pgid: u32) {
    kill_group_with(pgid, "-TERM");
}

fn kill_group_with(pgid: u32, signal: &str) {
    #[cfg(unix)]
    if pgid > 0 {
        let _ = std::process::Command::new("kill")
            .args([signal, "--", &format!("-{pgid}")])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(not(unix))]
    let _ = (pgid, signal);
}

async fn stop_turn(agent: Agent, turn: Option<ActiveTurn>, event_tx: &mpsc::Sender<AgentOutput>) {
    if let Some(turn) = turn {
        if let Some(cancel) = &turn.acp_cancel {
            cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = tokio::time::timeout(std::time::Duration::from_millis(1600), turn.handle).await;
        } else {
            kill_group(turn.pgid.load(std::sync::atomic::Ordering::SeqCst));
            turn.handle.abort();
            let _ = turn.handle.await;
        }
        send(event_tx, agent, Some(&turn.context), AgentEvent::Cancelled).await;
    }
}

async fn send(
    event_tx: &mpsc::Sender<AgentOutput>,
    agent: Agent,
    context: Option<&OutputContext>,
    event: AgentEvent,
) {
    let _ = event_tx
        .send(AgentOutput {
            agent,
            conversation_id: context.map(|c| c.conversation_id.clone()),
            turn_id: context.map(|c| c.turn_id.clone()),
            event,
        })
        .await;
}

// The fake agent is a POSIX shell script; Windows CI covers the pure parts in
// `dmuxai::custom_agent` tests.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::agent::{ConversationId, TurnId};

    fn fixture() -> String {
        format!(
            "{}/tests/fixtures/fake-agent.sh",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    fn spec(behavior: &str, extra: &str) -> &'static CustomAgentSpec {
        let toml = format!(
            "name = \"fake\"\ncommand = [\"{}\", \"{behavior}\", \"-m\", \"{{model}}\", \"{{prompt}}\"]\nfailure_stderr = \"(?i)permission denied\"\n{extra}",
            fixture()
        );
        Box::leak(Box::new(toml::from_str(&toml).unwrap()))
    }

    async fn run(spec: &CustomAgentSpec, model: Option<&str>) -> (Result<(), String>, String) {
        run_with_mcp(spec, model, &[]).await
    }

    async fn run_with_mcp(
        spec: &CustomAgentSpec,
        model: Option<&str>,
        mcp_servers: &[dmuxai::mcp::ResolvedMcpServer],
    ) -> (Result<(), String>, String) {
        let (tx, mut rx) = mpsc::channel(64);
        let request = AgentRequest {
            conversation_id: ConversationId::new("c"),
            turn_id: TurnId::new("t"),
            prompt: "hi --model x".to_owned(),
            resume_session: None,
            image_paths: Vec::new(),
            policy: TurnPolicy::Agent,
        };
        let context = OutputContext::from(&request);
        let pgid = std::sync::atomic::AtomicU32::new(0);
        let cwd = std::env::temp_dir();
        let notice_shown = AtomicBool::new(false);
        let result = run_turn(
            Agent::Custom(0),
            spec,
            &request,
            model.map(str::to_owned),
            &cwd,
            &tx,
            &context,
            &pgid,
            mcp_servers,
            &notice_shown,
            None,
        )
        .await;
        drop(tx);
        let mut text = String::new();
        while let Some(output) = rx.recv().await {
            if let AgentEvent::TextDelta(delta) = output.event {
                text.push_str(&delta);
            }
        }
        (result, text)
    }

    #[tokio::test]
    async fn jsonl_agent_streams_text_and_keeps_prompt_as_one_arg() {
        let spec = spec(
            "jsonl",
            "output = \"jsonl\"\ntext_path = \"/part/text\"\nsession_id_path = \"/sessionID\"\nerror_path = \"/error/message\"",
        );
        let (result, text) = run(spec, Some("m1")).await;
        assert_eq!(result, Ok(()));
        assert_eq!(text, "echo:hi --model x");
    }

    #[tokio::test]
    async fn text_agent_and_unset_model_flag_is_dropped() {
        let (result, text) = run(spec("text", ""), None).await;
        assert_eq!(result, Ok(()));
        assert_eq!(text, "hello hi --model x\n");
    }

    #[tokio::test]
    async fn stdin_prompt_delivery() {
        let (result, text) = run(spec("stdin", "prompt_via = \"stdin\""), None).await;
        assert_eq!(result, Ok(()));
        assert_eq!(text.trim(), "stdin:hi --model x");
    }

    #[tokio::test]
    async fn failures_are_detected() {
        let (empty, _) = run(spec("empty", ""), None).await;
        assert!(empty.unwrap_err().contains("empty reply"));
        let (denied, _) = run(spec("denied", ""), None).await;
        assert!(denied.unwrap_err().contains("permission denied"));
        let (fail, _) = run(spec("fail", ""), None).await;
        assert!(fail.unwrap_err().contains("boom"));
        let (error, _) = run(
            spec(
                "error",
                "output = \"jsonl\"\ntext_path = \"/part/text\"\nerror_path = \"/error/message\"",
            ),
            None,
        )
        .await;
        assert!(error.unwrap_err().contains("model not found"));
    }

    #[tokio::test]
    async fn temp_config_is_written_passed_and_removed() {
        let spec = spec(
            "env",
            "temp_config_template = '{\"edit\":\"{edit}\"}'\nenv = { FAKE_CFG = \"{temp_config}\" }",
        );
        let (result, text) = run(spec, None).await;
        assert_eq!(result, Ok(()));
        assert!(text.contains("content={\"edit\":\"allow\"}"), "{text}");
        let path = text
            .split("cfg=")
            .nth(1)
            .unwrap()
            .split(' ')
            .next()
            .unwrap();
        assert!(!std::path::Path::new(path).exists());
    }

    #[tokio::test]
    async fn timeout_kills_the_process_group() {
        let spec = spec("sleep", "timeout_secs = 1");
        let started = std::time::Instant::now();
        let (result, _) = run(spec, None).await;
        assert!(result.unwrap_err().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn custom_turn_gets_mcp_config_file_and_env() {
        // TC-565, INT-561: custom agent turn merges mcp into rendered temp_config,
        // writes custom_mcp_json to a per-turn temp file exposed as {mcp_config}
        // and DMUXAI_MCP_CONFIG, and removes it after the turn.
        use dmuxai::mcp::config::{ResolvedMcpServer, ServerKind, ToolNames, Transport};
        let server = ResolvedMcpServer {
            name: "demo".to_owned(),
            kind: ServerKind::Generic,
            transport: Transport::Stdio {
                command: "echo".to_owned(),
                args: vec!["test".to_owned()],
                env: [("API_KEY".to_owned(), "secret".to_owned())]
                    .into_iter()
                    .collect(),
            },
            runners: None,
            tools: ToolNames::default(),
        };

        let toml = r#"
name = "mcp-agent"
command = ["sh", "-c", "echo env=$DMUXAI_MCP_CONFIG arg=$1; echo \"---mcp---\"; cat \"$DMUXAI_MCP_CONFIG\"; echo \"---cfg---\"; cat \"$FAKE_CFG\"", "--", "{mcp_config}"]
temp_config_template = '{"permission":{"edit":"allow"}}'
env = { FAKE_CFG = "{temp_config}" }
"#;
        let test_spec: &'static CustomAgentSpec =
            Box::leak(Box::new(toml::from_str(toml).unwrap()));
        let (result, text) = run_with_mcp(test_spec, None, &[server]).await;
        assert_eq!(result, Ok(()));
        assert!(
            text.contains("---mcp---"),
            "output must contain separator: {text}"
        );
        assert!(
            text.contains("mcpServers"),
            "mcp config must contain mcpServers: {text}"
        );
        assert!(
            text.contains("demo"),
            "mcp config must contain server: {text}"
        );
        assert!(
            text.contains("\"permission\""),
            "temp config must retain permission: {text}"
        );
        assert!(
            text.contains("\"mcp\""),
            "temp config must contain merged mcp: {text}"
        );

        // Extract mcp file path
        let env_prefix = "env=";
        let env_pos = text.find(env_prefix).expect("env= in output");
        let after_env = &text[env_pos + env_prefix.len()..];
        let path = after_env.split(' ').next().unwrap().trim();
        assert!(!path.is_empty());
        assert!(text.contains(&format!("arg={path}")));
        assert!(
            !std::path::Path::new(path).exists(),
            "temp mcp config must be removed after turn"
        );

        // When no MCP servers are configured, no mcp config is written and placeholder flag is dropped
        let toml_clean = r#"
name = "clean-agent"
command = ["sh", "-c", "echo count=$# env=$DMUXAI_MCP_CONFIG", "--", "{mcp_config}"]
temp_config_template = '{"permission":{"edit":"allow"}}'
env = { FAKE_CFG = "{temp_config}" }
"#;
        let clean_spec: &'static CustomAgentSpec =
            Box::leak(Box::new(toml::from_str(toml_clean).unwrap()));
        let (res_clean, text_clean) = run_with_mcp(clean_spec, None, &[]).await;
        assert_eq!(res_clean, Ok(()));
        assert!(text_clean.contains("env="));
        assert!(!text_clean.contains("dmuxai-mcp-"));
    }

    async fn run_turn_capture_events(
        spec: &CustomAgentSpec,
        mcp_servers: &[dmuxai::mcp::ResolvedMcpServer],
        notice_shown: &AtomicBool,
    ) -> (Result<(), String>, Vec<AgentEvent>) {
        let (tx, mut rx) = mpsc::channel(64);
        let request = AgentRequest {
            conversation_id: ConversationId::new("c"),
            turn_id: TurnId::new("t"),
            prompt: "hi --model x".to_owned(),
            resume_session: None,
            image_paths: Vec::new(),
            policy: TurnPolicy::Agent,
        };
        let context = OutputContext::from(&request);
        let pgid = std::sync::atomic::AtomicU32::new(0);
        let cwd = std::env::temp_dir();
        let result = run_turn(
            Agent::Custom(0),
            spec,
            &request,
            None,
            &cwd,
            &tx,
            &context,
            &pgid,
            mcp_servers,
            notice_shown,
            None,
        )
        .await;
        drop(tx);
        let mut events = Vec::new();
        while let Some(output) = rx.recv().await {
            events.push(output.event);
        }
        (result, events)
    }

    #[tokio::test]
    async fn mcp_merge_unsupported_json_emits_notice_once_per_session() {
        use dmuxai::mcp::config::{ResolvedMcpServer, ServerKind, ToolNames, Transport};
        let server = ResolvedMcpServer {
            name: "demo".to_owned(),
            kind: ServerKind::Generic,
            transport: Transport::Stdio {
                command: "echo".to_owned(),
                args: vec![],
                env: Default::default(),
            },
            runners: None,
            tools: ToolNames::default(),
        };
        let toml = r#"
name = "bad-json"
label = "Bad Json Agent"
command = ["echo", "hello"]
temp_config_template = '{"mcp": "not-an-object"}'
"#;
        let test_spec: &'static CustomAgentSpec =
            Box::leak(Box::new(toml::from_str(toml).unwrap()));
        let notice_shown = AtomicBool::new(false);

        // Turn 1: should emit Notice naming the runner
        let (res1, events1) =
            run_turn_capture_events(test_spec, std::slice::from_ref(&server), &notice_shown).await;
        assert_eq!(res1, Ok(()));
        let notices1: Vec<_> = events1
            .iter()
            .filter_map(|e| match e {
                AgentEvent::Notice(msg) => Some(msg.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(notices1.len(), 1, "expected 1 notice, got: {notices1:?}");
        assert!(
            notices1[0].contains("Bad Json Agent"),
            "notice must name runner: {}",
            notices1[0]
        );

        // Turn 2: same session (same notice_shown) -> no notice emitted
        let (res2, events2) =
            run_turn_capture_events(test_spec, std::slice::from_ref(&server), &notice_shown).await;
        assert_eq!(res2, Ok(()));
        let notices2: Vec<_> = events2
            .iter()
            .filter_map(|e| match e {
                AgentEvent::Notice(msg) => Some(msg.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            notices2.len(),
            0,
            "notice should only be emitted once per session"
        );
    }

    #[test]
    fn private_temp_file_has_0600_permissions() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test-private.json");
        write_private_temp_file(&path, "secret-data", "test").unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    }

    fn fake_grok() -> String {
        // The fixture is committed with mode 100755; tests never rewrite it.
        format!("{}/tests/fixtures/fake-grok.sh", env!("CARGO_MANIFEST_DIR"))
    }

    fn grok_spec(env: &[(&str, &str)]) -> &'static CustomAgentSpec {
        let mut spec = dmuxai::custom_agent::grok_preset(&fake_grok());
        for (key, value) in env {
            spec.env.insert((*key).to_owned(), (*value).to_owned());
        }
        Box::leak(Box::new(spec))
    }

    fn request(prompt: &str, resume: Option<&str>) -> AgentRequest {
        AgentRequest {
            conversation_id: ConversationId::new("c"),
            turn_id: TurnId::new("t"),
            prompt: prompt.to_owned(),
            resume_session: resume.map(str::to_owned),
            image_paths: Vec::new(),
            policy: TurnPolicy::Agent,
        }
    }

    async fn drive(
        spec: &'static CustomAgentSpec,
    ) -> (
        mpsc::Sender<CustomCommand>,
        mpsc::Receiver<AgentOutput>,
        JoinHandle<()>,
    ) {
        let workspace = tempfile::tempdir().expect("grok test workspace");
        std::fs::write(
            workspace.path().join("dmuxai.toml"),
            "[agents]\ngrok_protocol = \"legacy\"\n",
        )
        .expect("grok legacy settings");
        let (event_tx, event_rx) = mpsc::channel(64);
        let (command_tx, command_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
        let worker = tokio::spawn(runtime_loop_with_spec(
            Agent::Custom(0),
            spec,
            command_rx,
            event_tx,
            workspace.path().to_path_buf(),
            Vec::new(),
        ));
        std::mem::forget(workspace);
        (command_tx, event_rx, worker)
    }

    async fn until_terminal(rx: &mut mpsc::Receiver<AgentOutput>) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let output = tokio::time::timeout(remaining, rx.recv())
                .await
                .expect("grok runtime timed out")
                .expect("grok runtime closed");
            let done = matches!(
                output.event,
                AgentEvent::Completed | AgentEvent::Error(_) | AgentEvent::Cancelled
            );
            events.push(output.event);
            if done {
                break;
            }
        }
        events
    }

    #[cfg(target_os = "linux")]
    fn pgrp(pid: u32) -> Option<u32> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let rest = stat.rsplit_once(')')?.1;
        let mut fields = rest.split_whitespace();
        let _state = fields.next()?;
        let _ppid = fields.next()?;
        fields.next()?.parse().ok()
    }

    #[cfg(target_os = "linux")]
    fn still_running(pid: u32) -> bool {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        let Some((_, rest)) = stat.rsplit_once(')') else {
            return false;
        };
        matches!(rest.split_whitespace().next(), Some("S" | "R" | "D"))
    }

    // TC-894 / FR-893, FR-894, FR-895
    #[tokio::test]
    async fn grok_mock_turn_returns_text_and_reports_errors() {
        let dir = tempfile::tempdir().unwrap();
        let argv_path = dir.path().join("argv.txt");
        let spec = grok_spec(&[(
            "GROK_FAKE_ARGV",
            argv_path.to_str().expect("utf8 temp path"),
        )]);
        assert!(spec.builtin && spec.name == "grok");
        let prompt = "hello; $(rm)";
        let (tx, mut rx, worker) = drive(spec).await;
        tx.send(CustomCommand::Send(request(prompt, None)))
            .await
            .unwrap();
        let events = until_terminal(&mut rx).await;
        let text: String = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::TextDelta(delta) => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "OK");
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::Completed))
        );
        let session = "11111111-1111-1111-1111-111111111111";
        assert!(
            events.iter().any(|event| matches!(
                event,
                AgentEvent::ProviderSessionBound(id) if id == session
            )),
            "{events:?}"
        );
        let argv = std::fs::read_to_string(&argv_path).unwrap();
        assert!(argv.contains(prompt), "{argv}");
        assert!(argv.contains("\n-m\ngrok-4.7\n"), "{argv}");
        assert!(!argv.contains("grok-4.7-build-fast"), "{argv}");
        assert!(argv.lines().all(|line| line != "sh" && line != "cmd"));

        tx.send(CustomCommand::Send(request(prompt, Some(session))))
            .await
            .unwrap();
        let second = until_terminal(&mut rx).await;
        assert!(
            second
                .iter()
                .any(|event| matches!(event, AgentEvent::Completed)),
            "{second:?}"
        );
        let argv = std::fs::read_to_string(&argv_path).unwrap();
        assert!(argv.contains(&format!("\n-r\n{session}\n")), "{argv}");
        drop(tx);
        worker.await.unwrap();

        for (mode, needle) in [("fail", "boom"), ("empty", "empty reply")] {
            let spec = grok_spec(&[("GROK_FAKE_MODE", mode)]);
            let (tx, mut rx, worker) = drive(spec).await;
            tx.send(CustomCommand::Send(request(prompt, None)))
                .await
                .unwrap();
            let events = until_terminal(&mut rx).await;
            let error = events.iter().find_map(|event| match event {
                AgentEvent::Error(error) => Some(error.as_str()),
                _ => None,
            });
            assert!(
                error.is_some_and(|error| error.contains(needle)),
                "{mode}: {events:?}"
            );
            drop(tx);
            worker.await.unwrap();
        }
    }

    // TC-897 / FR-896
    #[tokio::test]
    async fn grok_unauthenticated_send_explains_login() {
        let dir = tempfile::tempdir().unwrap();
        for auth in ["0", "fail"] {
            let argv_path = dir.path().join(format!("argv-{auth}.txt"));
            let spec = grok_spec(&[
                ("GROK_FAKE_AUTH", auth),
                (
                    "GROK_FAKE_ARGV",
                    argv_path.to_str().expect("utf8 temp path"),
                ),
            ]);
            assert!(spec.builtin);
            assert_eq!(spec.name, "grok");
            assert!(spec.command.iter().any(|arg| arg == "-p"));
            let (tx, mut rx, worker) = drive(spec).await;
            tx.send(CustomCommand::ListModels).await.unwrap();
            let listed = until_terminal(&mut rx).await;
            let list_error = listed.iter().find_map(|event| match event {
                AgentEvent::Error(error) => Some(error.clone()),
                _ => None,
            });
            let list_error = list_error.expect("models error");
            assert!(list_error.contains("grok login"), "{auth}: {list_error}");

            tx.send(CustomCommand::Send(request("hello; $(rm)", None)))
                .await
                .unwrap();
            let sent = until_terminal(&mut rx).await;
            let send_error = sent.iter().find_map(|event| match event {
                AgentEvent::Error(error) => Some(error.clone()),
                _ => None,
            });
            let send_error = send_error.expect("send error");
            assert!(send_error.contains("grok login"), "{auth}: {send_error}");
            let argv = std::fs::read_to_string(&argv_path).unwrap_or_default();
            assert!(!argv.lines().any(|line| line == "-p"), "{auth}: {argv}");
            drop(tx);
            worker.await.unwrap();
        }

        let fast = grok_spec(&[("GROK_FAKE_AUTH", "fast")]);
        let (tx, mut rx, worker) = drive(fast).await;
        tx.send(CustomCommand::Send(request("hello", None)))
            .await
            .unwrap();
        let events = until_terminal(&mut rx).await;
        let error = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::Error(error) => Some(error.as_str()),
                _ => None,
            })
            .expect("fast-only error");
        assert!(!error.contains("grok login"), "{error}");
        assert!(error.contains("fast"), "{error}");
        assert_ne!(error, dmuxai::custom_agent::grok_login_message());
        drop(tx);
        worker.await.unwrap();
    }

    // TC-898 / FR-897
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn grok_cancel_kills_the_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("grandchild.pid");
        let spec = grok_spec(&[
            ("GROK_FAKE_MODE", "sleep"),
            (
                "GROK_FAKE_PIDFILE",
                pidfile.to_str().expect("utf8 temp path"),
            ),
        ]);
        let (tx, mut rx, worker) = drive(spec).await;
        tx.send(CustomCommand::Send(request("hello", None)))
            .await
            .unwrap();

        let started = std::time::Instant::now();
        let grandchild = loop {
            if let Ok(text) = std::fs::read_to_string(&pidfile)
                && let Ok(pid) = text.trim().parse::<u32>()
                && pid > 0
            {
                break pid;
            }
            if started.elapsed() > Duration::from_secs(2) {
                panic!("grandchild pid was not written");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        let leader = pgrp(grandchild).expect("process group");
        assert!(leader > 0);
        assert!(still_running(leader));
        assert!(still_running(grandchild));

        tx.send(CustomCommand::Cancel).await.unwrap();
        let events = until_terminal(&mut rx).await;
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::Cancelled)),
            "{events:?}"
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while still_running(leader) || still_running(grandchild) {
            if std::time::Instant::now() > deadline {
                panic!("leader {leader} or grandchild {grandchild} still running");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        drop(tx);
        worker.await.unwrap();
    }

    fn texts(events: &[AgentEvent]) -> String {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::TextDelta(delta) => Some(delta.as_str()),
                _ => None,
            })
            .collect()
    }

    fn completed(events: &[AgentEvent]) -> bool {
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::Completed))
    }

    // TC-981 / FR-980
    #[cfg(unix)]
    #[tokio::test]
    async fn grok_tool_calls_emit_activity_events() {
        let spec = grok_spec(&[("GROK_FAKE_MODE", "tools")]);
        let (tx, mut rx, worker) = drive(spec).await;
        tx.send(CustomCommand::Send(request("read src/app.rs", None)))
            .await
            .unwrap();
        let events = until_terminal(&mut rx).await;
        let tools: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Activity(dmuxai::activity::ActivityItem::Tool {
                    name,
                    target,
                    status,
                    ..
                }) => Some((name.as_str(), target.as_str(), *status)),
                _ => None,
            })
            .collect();
        assert_eq!(
            tools,
            vec![
                ("read", "src/app.rs", dmuxai::activity::ToolStatus::Running),
                ("read", "src/app.rs", dmuxai::activity::ToolStatus::Ok),
            ],
            "{events:?}"
        );
        let completed_at = events
            .iter()
            .position(|event| matches!(event, AgentEvent::Completed))
            .expect("completed");
        assert!(
            events.iter().rposition(|event| {
                matches!(
                    event,
                    AgentEvent::Activity(dmuxai::activity::ActivityItem::Tool { .. })
                )
            }) < Some(completed_at),
            "{events:?}"
        );
        // FR-916 adds "\n\n" only when a break is pending and the reply is
        // already non-empty. The tool_call precedes the first text, so the
        // streamed reply is "OK" with no leading break.
        assert_eq!(texts(&events), "OK");
        assert!(completed(&events));
        drop(tx);
        worker.await.unwrap();
    }

    #[cfg(target_os = "linux")]
    async fn read_pid(pidfile: &std::path::Path) -> u32 {
        let started = std::time::Instant::now();
        loop {
            if let Ok(text) = std::fs::read_to_string(pidfile)
                && let Ok(pid) = text.trim().parse::<u32>()
                && pid > 0
            {
                return pid;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "child pid was not written"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[cfg(target_os = "linux")]
    async fn assert_gone(pid: u32) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while still_running(pid) {
            assert!(
                std::time::Instant::now() < deadline,
                "process {pid} still running after the turn"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Runs one mock turn in `mode` and returns its events, the elapsed time,
    /// and the pid of the child the mock left holding stdout.
    #[cfg(target_os = "linux")]
    async fn lingering_turn(mode: &str) -> (Vec<AgentEvent>, Duration, u32) {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("child.pid");
        let spec = grok_spec(&[
            ("GROK_FAKE_MODE", mode),
            (
                "GROK_FAKE_PIDFILE",
                pidfile.to_str().expect("utf8 temp path"),
            ),
        ]);
        let (tx, mut rx, worker) = drive(spec).await;
        let started = std::time::Instant::now();
        tx.send(CustomCommand::Send(request("hello", None)))
            .await
            .unwrap();
        let events = until_terminal(&mut rx).await;
        let elapsed = started.elapsed();
        let child = read_pid(&pidfile).await;
        drop(tx);
        worker.await.unwrap();
        (events, elapsed, child)
    }

    // TC-914 / FR-914: the end line finishes the turn although the runner
    // idles afterwards and a child still holds stdout. Both are stopped.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn grok_end_line_finishes_turn_and_reaps_group() {
        let (events, elapsed, child) = lingering_turn("linger").await;
        assert!(completed(&events), "{events:?}");
        // 3 s grace, then TERM; generous margin for loaded CI machines.
        assert!(elapsed < Duration::from_secs(8), "took {elapsed:?}");
        assert_gone(child).await;
    }

    // TC-915 / FR-914: a runner that exits right after the end line while a
    // child keeps stdout open completes without the grace period, and the
    // child is stopped.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn grok_orphan_holding_stdout_does_not_block_turn() {
        let (events, elapsed, child) = lingering_turn("orphan").await;
        assert_eq!(texts(&events), "OK");
        assert!(completed(&events), "{events:?}");
        // Below the 3 s grace: the leader's own exit ends the turn.
        assert!(elapsed < Duration::from_millis(2500), "took {elapsed:?}");
        assert_gone(child).await;
    }

    // TC-916 / FR-915: without an end line the turn still ends at EOF and exit.
    #[tokio::test]
    async fn grok_turn_without_end_line_completes_at_exit() {
        let spec = grok_spec(&[("GROK_FAKE_MODE", "noend")]);
        let (tx, mut rx, worker) = drive(spec).await;
        tx.send(CustomCommand::Send(request("hello", None)))
            .await
            .unwrap();
        let events = until_terminal(&mut rx).await;
        assert_eq!(texts(&events), "OK");
        assert!(completed(&events), "{events:?}");
        drop(tx);
        worker.await.unwrap();
    }

    // TC-918 / FR-916: text split around a tool call streams with a paragraph
    // break, so "Checking" and "Done." do not run together.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn grok_text_after_tool_call_starts_new_paragraph() {
        let (events, _, _) = lingering_turn("linger").await;
        assert_eq!(texts(&events), "Checking\n\nDone.");
    }

    // TC-907 / FR-892, FR-893. Ignored: needs a real logged-in grok. Default
    // `cargo test` does not run it and must not count it as passed.
    #[tokio::test]
    #[ignore = "live grok smoke; requires a logged-in grok binary"]
    async fn grok_live_smoke_headless_turn() {
        let Some(bin) = dmuxai::settings::find_grok_bin() else {
            panic!("grok binary not found");
        };
        let spec: &'static CustomAgentSpec = Box::leak(Box::new(
            dmuxai::custom_agent::grok_preset(&bin.to_string_lossy()),
        ));
        let cwd = std::env::temp_dir();
        let ids = list_model_ids(spec, &cwd, Some(BUILTIN_MODELS_TIMEOUT))
            .await
            .unwrap_or_else(|error| panic!("grok models failed: {error}"));
        assert!(!ids.is_empty(), "logged-in grok listed no allowed model");
        let (tx, mut rx, worker) = drive(spec).await;
        tx.send(CustomCommand::Send(request(
            "Reply with the single word pong.",
            None,
        )))
        .await
        .unwrap();
        let events = until_terminal(&mut rx).await;
        let text: String = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::TextDelta(delta) => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert!(!text.trim().is_empty(), "{events:?}");
        assert!(
            events.iter().any(|event| matches!(
                event,
                AgentEvent::ProviderSessionBound(id) if !id.is_empty()
            )),
            "{events:?}"
        );
        drop(tx);
        worker.await.unwrap();
    }

    #[cfg(feature = "test-fixtures")]
    mod opencode_acp_fake_tests {
        use super::*;

        fn fake_acp_agent_bin() -> std::path::PathBuf {
            let mut path = std::env::current_exe().expect("test exe");
            path.pop();
            if path.ends_with("deps") {
                path.pop();
            }
            path.push("fake-acp-agent");
            path
        }

        fn opencode_acp_spec(program: &str) -> &'static CustomAgentSpec {
            let mut spec = dmuxai::custom_agent::opencode_preset(program);
            spec.default_model = Some("opencode/glm-5.3-flash-free".to_owned());
            Box::leak(Box::new(spec))
        }

        async fn drive_opencode_acp(
            spec: &'static CustomAgentSpec,
        ) -> (
            mpsc::Sender<CustomCommand>,
            mpsc::Receiver<AgentOutput>,
            JoinHandle<()>,
        ) {
            let workspace = tempfile::tempdir().expect("opencode test workspace");
            std::fs::write(
                workspace.path().join("dmuxai.toml"),
                "[agents]\nopencode_protocol = \"acp\"\n",
            )
            .expect("opencode acp settings");
            let (event_tx, event_rx) = mpsc::channel(64);
            let (command_tx, command_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
            let worker = tokio::spawn(runtime_loop_with_spec(
                Agent::Custom(0),
                spec,
                command_rx,
                event_tx,
                workspace.path().to_path_buf(),
                Vec::new(),
            ));
            std::mem::forget(workspace);
            (command_tx, event_rx, worker)
        }

        // TC-1327 / FR-1326: opencode ACP turn sets model via session config.
        #[tokio::test]
        async fn opencode_acp_turn_with_fake_agent() {
            let fake = fake_acp_agent_bin();
            assert!(
                fake.is_file(),
                "build fake-acp-agent first (expected at {})",
                fake.display()
            );
            let dir = tempfile::tempdir().unwrap();
            let model_capture = dir.path().join("model.txt");
            unsafe {
                std::env::set_var(
                    "FAKE_ACP_MODEL_CAPTURE",
                    model_capture.to_str().expect("utf8"),
                );
            }
            let spec = opencode_acp_spec(&fake.to_string_lossy());
            assert!(spec.builtin && spec.name == "opencode");
            let (tx, mut rx, worker) = drive_opencode_acp(spec).await;
            tx.send(CustomCommand::Send(request("hello acp", None)))
                .await
                .unwrap();
            let events = until_terminal(&mut rx).await;
            let text: String = events
                .iter()
                .filter_map(|event| match event {
                    AgentEvent::TextDelta(delta) => Some(delta.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(text, "ACP_OK");
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, AgentEvent::Completed))
            );
            let captured = std::fs::read_to_string(&model_capture).unwrap_or_default();
            assert_eq!(captured, "opencode/glm-5.3-flash-free");
            drop(tx);
            worker.await.unwrap();
        }
    }
}
