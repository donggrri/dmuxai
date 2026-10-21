use std::{
    collections::VecDeque,
    io,
    path::{Path, PathBuf},
    process::Stdio,
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStderr, ChildStdout},
    sync::mpsc,
    task::JoinHandle,
    time::{Duration, Instant},
};

use super::{
    AdapterError, Agent, AgentAdapter, AgentEvent, AgentOutput, AgentRequest, ModelInfo,
    OutputContext, SpawnedAdapter, TurnPolicy,
    implement_outcome::{
        ImplementTurnTracker, is_plan_only_implement_outcome, is_plan_review_response,
    },
    prompt_with_image_tokens,
};
use std::time::{SystemTime, UNIX_EPOCH};

use dmuxai::process::{headless_command, resolve_program};

const COMMAND_CHANNEL_CAPACITY: usize = 8;
const STDERR_HISTORY_CAPACITY: usize = 8;
const AGY_BINARY_OVERRIDE: &str = "DMUXAI_AGY_BIN";
const AGY_MODEL_CACHE_TTL_SECS: u64 = 24 * 60 * 60;
const AGY_IDLE_TIMEOUT_ENV: &str = "DMUXAI_AGY_IDLE_TIMEOUT_SECS";
const AGY_IDLE_TIMEOUT_DEFAULT_SECS: u64 = 300;
const AGY_RESULT_GRACE_SECS: u64 = 3;
const AGY_PLAN_ONLY_MAX_ATTEMPTS: usize = 2;

/// Resolves the idle timeout from `DMUXAI_AGY_IDLE_TIMEOUT_SECS`.
///
/// `None` disables the timeout; `0` also disables it; any other value is used
/// verbatim when positive and valid, and falls back to the default otherwise.
fn parse_idle_timeout_secs(raw: Option<&str>) -> Option<u64> {
    let Some(raw) = raw else {
        return Some(AGY_IDLE_TIMEOUT_DEFAULT_SECS);
    };
    if raw.trim() == "0" {
        return None;
    }
    Some(
        raw.trim()
            .parse::<u64>()
            .ok()
            .filter(|secs| *secs > 0)
            .unwrap_or(AGY_IDLE_TIMEOUT_DEFAULT_SECS),
    )
}

fn agy_idle_timeout_secs() -> Option<u64> {
    parse_idle_timeout_secs(std::env::var(AGY_IDLE_TIMEOUT_ENV).ok().as_deref())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct CachedAgyModels {
    pub timestamp_secs: u64,
    pub models: Vec<ModelInfo>,
}

pub(crate) fn agy_models_cache_path() -> io::Result<PathBuf> {
    if let Some(base) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(base).join("dmuxai").join("agy-models.json"));
    }
    #[cfg(windows)]
    {
        if let Some(base) = std::env::var_os("LOCALAPPDATA") {
            return Ok(PathBuf::from(base).join("DmuxAI").join("agy-models.json"));
        }
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
        .join("agy-models.json"))
}

pub(crate) fn read_disk_cache(
    path: &Path,
    now_secs: u64,
    ttl_secs: u64,
) -> Option<(Vec<ModelInfo>, bool)> {
    let content = std::fs::read_to_string(path).ok()?;
    let cached: CachedAgyModels = serde_json::from_str(&content).ok()?;
    let is_fresh = now_secs.saturating_sub(cached.timestamp_secs) < ttl_secs;
    Some((cached.models, is_fresh))
}

pub(crate) fn write_disk_cache(
    path: &Path,
    models: &[ModelInfo],
    timestamp_secs: u64,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let cached = CachedAgyModels {
        timestamp_secs,
        models: models.to_vec(),
    };
    let json = serde_json::to_string_pretty(&cached)?;
    std::fs::write(path, json)?;
    Ok(())
}

fn current_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

enum AgyCommand {
    Send(AgentRequest),
    Cancel,
    ListModels,
    SelectModel(String),
}

pub struct AgyAdapter {
    command_tx: mpsc::Sender<AgyCommand>,
}

impl AgentAdapter for AgyAdapter {
    fn agent(&self) -> Agent {
        Agent::Agy
    }

    fn send(&self, request: AgentRequest) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(AgyCommand::Send(request))
            .map_err(map_send_error)
    }

    fn cancel(&self) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(AgyCommand::Cancel)
            .map_err(map_send_error)
    }

    fn request_models(&self) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(AgyCommand::ListModels)
            .map_err(map_send_error)
    }

    fn select_model(&self, model: String) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(AgyCommand::SelectModel(model))
            .map_err(map_send_error)
    }
}

fn map_send_error(error: mpsc::error::TrySendError<AgyCommand>) -> AdapterError {
    match error {
        mpsc::error::TrySendError::Full(_) => AdapterError::CommandQueueFull,
        mpsc::error::TrySendError::Closed(_) => AdapterError::RuntimeStopped,
    }
}

pub fn spawn_agy_adapter(event_tx: mpsc::Sender<AgentOutput>, cwd: PathBuf) -> SpawnedAdapter {
    let (command_tx, command_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
    let worker = tokio::spawn(runtime_loop(command_rx, event_tx, cwd));
    (Box::new(AgyAdapter { command_tx }), worker)
}

async fn runtime_loop(
    mut command_rx: mpsc::Receiver<AgyCommand>,
    event_tx: mpsc::Sender<AgentOutput>,
    cwd: PathBuf,
) {
    let mut active: Option<ActiveTurn> = None;
    let mut selected_model: Option<String> = None;
    let memory_models_cache = std::sync::Arc::new(std::sync::Mutex::new(None));

    loop {
        if active.is_none() {
            let Some(command) = command_rx.recv().await else {
                break;
            };
            match command {
                AgyCommand::Send(request) => {
                    active = Some(spawn_turn(
                        request,
                        selected_model.clone(),
                        cwd.clone(),
                        event_tx.clone(),
                    ));
                }
                AgyCommand::Cancel => {}
                AgyCommand::ListModels => {
                    let now = current_timestamp_secs();
                    let cache_path = agy_models_cache_path().ok();

                    let (cached_models, is_fresh) = {
                        let mem = memory_models_cache.lock().unwrap().clone();
                        if let Some(models) = mem {
                            (Some(models), true)
                        } else if let Some(ref path) = cache_path {
                            if let Some((disk_models, fresh)) =
                                read_disk_cache(path, now, AGY_MODEL_CACHE_TTL_SECS)
                            {
                                *memory_models_cache.lock().unwrap() = Some(disk_models.clone());
                                (Some(disk_models), fresh)
                            } else {
                                (None, false)
                            }
                        } else {
                            (None, false)
                        }
                    };

                    if let Some(models) = cached_models {
                        send_event(
                            &event_tx,
                            None,
                            AgentEvent::ModelsListed {
                                models,
                                selected: selected_model.clone(),
                                selected_effort: None,
                            },
                        )
                        .await;

                        if !is_fresh {
                            let event_tx_clone = event_tx.clone();
                            let cwd_clone = cwd.clone();
                            let selected_model_clone = selected_model.clone();
                            let cache_path_clone = cache_path.clone();
                            let memory_cache_clone = memory_models_cache.clone();
                            tokio::spawn(async move {
                                if let Ok(fresh_models) = list_models(&cwd_clone).await {
                                    *memory_cache_clone.lock().unwrap() =
                                        Some(fresh_models.clone());
                                    if let Some(ref path) = cache_path_clone {
                                        let _ = write_disk_cache(
                                            path,
                                            &fresh_models,
                                            current_timestamp_secs(),
                                        );
                                    }
                                    send_event(
                                        &event_tx_clone,
                                        None,
                                        AgentEvent::ModelsListed {
                                            models: fresh_models,
                                            selected: selected_model_clone,
                                            selected_effort: None,
                                        },
                                    )
                                    .await;
                                }
                            });
                        }
                    } else {
                        let event_tx_clone = event_tx.clone();
                        let cwd_clone = cwd.clone();
                        let selected_model_clone = selected_model.clone();
                        let cache_path_clone = cache_path.clone();
                        let memory_cache_clone = memory_models_cache.clone();
                        tokio::spawn(async move {
                            match list_models(&cwd_clone).await {
                                Ok(fresh_models) => {
                                    *memory_cache_clone.lock().unwrap() =
                                        Some(fresh_models.clone());
                                    if let Some(ref path) = cache_path_clone {
                                        let _ = write_disk_cache(
                                            path,
                                            &fresh_models,
                                            current_timestamp_secs(),
                                        );
                                    }
                                    send_event(
                                        &event_tx_clone,
                                        None,
                                        AgentEvent::ModelsListed {
                                            models: fresh_models,
                                            selected: selected_model_clone,
                                            selected_effort: None,
                                        },
                                    )
                                    .await;
                                }
                                Err(error) => {
                                    send_event(
                                        &event_tx_clone,
                                        None,
                                        AgentEvent::Error(error.to_string()),
                                    )
                                    .await;
                                }
                            }
                        });
                    }
                }
                AgyCommand::SelectModel(model) => {
                    selected_model = Some(model.clone());
                    send_event(&event_tx, None, AgentEvent::ModelSelected(model)).await;
                }
            }
            continue;
        }

        let turn = &mut active.as_mut().expect("checked above").handle;
        tokio::select! {
            _ = turn => {
                active = None;
            }
            command = command_rx.recv() => {
                match command {
                    Some(AgyCommand::Cancel) => {
                        stop_turn(active.take(), &event_tx).await;
                    }
                    Some(AgyCommand::Send(request)) => {
                        let context = OutputContext::from(&request);
                        send_event(
                            &event_tx,
                            Some(&context),
                            AgentEvent::Error(
                                "AGY is already running a turn; cancel it before sending another prompt"
                                    .to_owned(),
                            ),
                        ).await;
                    }
                    Some(AgyCommand::ListModels) => {
                        send_event(
                            &event_tx,
                            None,
                            AgentEvent::Notice(
                                "finish or cancel the AGY turn before loading models".to_owned(),
                            ),
                        ).await;
                    }
                    Some(AgyCommand::SelectModel(_)) => {
                        send_event(
                            &event_tx,
                            None,
                            AgentEvent::Notice(
                                "finish or cancel the AGY turn before changing models".to_owned(),
                            ),
                        ).await;
                    }
                    None => {
                        stop_turn(active.take(), &event_tx).await;
                        break;
                    }
                }
            }
        }
    }
}

fn spawn_turn(
    request: AgentRequest,
    selected_model: Option<String>,
    cwd: PathBuf,
    event_tx: mpsc::Sender<AgentOutput>,
) -> ActiveTurn {
    let context = OutputContext::from(&request);
    let error_context = context.clone();
    let handle = tokio::spawn(async move {
        if let Err(error) = run_turn(&request, selected_model, &cwd, &event_tx).await {
            send_event(
                &event_tx,
                Some(&error_context),
                AgentEvent::Error(error.to_string()),
            )
            .await;
        }
    });
    ActiveTurn { handle, context }
}

struct ActiveTurn {
    handle: JoinHandle<()>,
    context: OutputContext,
}

async fn stop_turn(turn: Option<ActiveTurn>, event_tx: &mpsc::Sender<AgentOutput>) {
    if let Some(turn) = turn {
        turn.handle.abort();
        let _ = turn.handle.await;
        send_event(event_tx, Some(&turn.context), AgentEvent::Cancelled).await;
    }
}

async fn run_turn(
    request: &AgentRequest,
    selected_model: Option<String>,
    cwd: &Path,
    event_tx: &mpsc::Sender<AgentOutput>,
) -> io::Result<Option<String>> {
    let context = OutputContext::from(request);
    send_event(event_tx, Some(&context), AgentEvent::Started).await;

    let policy = request.policy;
    let base_prompt = prompt_with_image_tokens(&request.prompt, &request.image_paths);
    let mut resume_session = request.resume_session.clone();
    let mut attempt = 0usize;

    while attempt < AGY_PLAN_ONLY_MAX_ATTEMPTS {
        let strict = attempt > 0;
        let user_prompt = if strict {
            agy_implement_retry_prompt(&base_prompt)
        } else {
            agy_prompt(policy, &base_prompt)
        };

        let outcome = run_single_agy_process(
            request,
            selected_model.as_deref(),
            policy,
            cwd,
            event_tx,
            &context,
            resume_session.as_deref(),
            &user_prompt,
        )
        .await?;

        resume_session = outcome.conversation_id.clone();

        // A failed `finish` (permission denial, protocol, or status check)
        // surfaces as an `Error` terminal; only a `Completed` turn with no
        // writes and a plan-like reply counts as a plan-only retry.
        if !request.require_file_edits
            || policy != TurnPolicy::Agent
            || !matches!(outcome.terminal, AgentEvent::Completed)
            || !is_plan_only_implement_outcome(outcome.writes_observed, &outcome.response_text)
        {
            emit_terminal_event(event_tx, &context, outcome.terminal).await;
            return Ok(outcome.conversation_id);
        }

        attempt += 1;
        if attempt >= AGY_PLAN_ONLY_MAX_ATTEMPTS {
            return Err(io::Error::other(
                "AGY returned plan-only responses twice without applying file edits.",
            ));
        }

        send_event(
            event_tx,
            Some(&context),
            AgentEvent::Notice(
                "AGY returned a plan without file edits; retrying once with a stricter edit instruction."
                    .to_owned(),
            ),
        )
        .await;
    }

    Ok(resume_session)
}

struct SingleAgyOutcome {
    conversation_id: Option<String>,
    response_text: String,
    writes_observed: bool,
    terminal: AgentEvent,
}

#[allow(clippy::too_many_arguments)]
async fn run_single_agy_process(
    request: &AgentRequest,
    selected_model: Option<&str>,
    mode: TurnPolicy,
    cwd: &Path,
    event_tx: &mpsc::Sender<AgentOutput>,
    context: &OutputContext,
    resume_session: Option<&str>,
    user_prompt: &str,
) -> io::Result<SingleAgyOutcome> {
    let program = resolve_program(AGY_BINARY_OVERRIDE, "agy.exe", "agy");
    let mut command = headless_command(program);
    configure_command_for_resume(&mut command, request, selected_model, mode, resume_session);
    command
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = command.spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("AGY stdin was not piped"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("AGY stdout was not piped"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("AGY stderr was not piped"))?;

    let mut input = serde_json::to_vec(&json!({
        "event": "user",
        "message": { "content": user_prompt }
    }))
    .map_err(io::Error::other)?;
    input.push(b'\n');
    stdin.write_all(&input).await?;
    stdin.shutdown().await?;
    drop(stdin);

    stream_child(&mut child, stdout, stderr, event_tx, context, mode).await
}

fn configure_command_for_resume(
    command: &mut tokio::process::Command,
    request: &AgentRequest,
    selected_model: Option<&str>,
    mode: TurnPolicy,
    resume_session: Option<&str>,
) {
    configure_command(command, selected_model, mode);
    let conversation_id = resume_session.or(request.resume_session.as_deref());
    if let Some(conversation_id) = conversation_id {
        command.arg("--conversation").arg(conversation_id);
    }
}

async fn emit_terminal_event(
    event_tx: &mpsc::Sender<AgentOutput>,
    context: &OutputContext,
    event: AgentEvent,
) {
    send_event(event_tx, Some(context), event).await;
}

fn agy_prompt(policy: TurnPolicy, prompt: &str) -> String {
    match policy {
        TurnPolicy::ReadOnly => prompt.to_owned(),
        TurnPolicy::Agent => format!(
            "Apply edits directly to files in the workspace. Do not produce plans, plan artifacts, or ask for review before editing. Run every command in the foreground with a bounded timeout. Do not start background or detached processes.\n\n{prompt}"
        ),
    }
}

fn agy_implement_retry_prompt(prompt: &str) -> String {
    format!(
        "CRITICAL: Apply file edits now. Do not write plans, do not create plan.md, and do not ask for approval. Implement every change in the workspace immediately.\n\n{prompt}"
    )
}

fn configure_command(
    command: &mut tokio::process::Command,
    selected_model: Option<&str>,
    mode: TurnPolicy,
) {
    command.args([
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
    ]);
    match mode {
        TurnPolicy::ReadOnly => {
            command.args(["--mode", "plan", "--sandbox"]);
        }
        TurnPolicy::Agent => {
            // Headless AGY auto-denies tools it cannot prompt for (e.g. run_command),
            // which silently produced empty Implement turns. The sandbox still applies.
            command.args([
                "--mode",
                "accept-edits",
                "--sandbox",
                "--dangerously-skip-permissions",
            ]);
        }
    }
    if let Some(model) = selected_model {
        command.arg("--model").arg(model);
    }
}

async fn stream_child(
    child: &mut tokio::process::Child,
    stdout: ChildStdout,
    stderr: ChildStderr,
    event_tx: &mpsc::Sender<AgentOutput>,
    context: &OutputContext,
    mode: TurnPolicy,
) -> io::Result<SingleAgyOutcome> {
    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut stderr_lines = BufReader::new(stderr).lines();
    let mut stderr_history = VecDeque::with_capacity(STDERR_HISTORY_CAPACITY);
    let mut stdout_open = true;
    let mut stderr_open = true;
    let mut exit_status = None;
    let mut killed_after_result = false;
    let mut state = AgyStreamState::default();
    let mut implement_tracker = ImplementTurnTracker::new(mode);

    let idle_secs = agy_idle_timeout_secs();
    let mut idle_deadline = idle_secs.map(|secs| Instant::now() + Duration::from_secs(secs));
    let mut result_grace_deadline: Option<Instant> = None;

    while stdout_open || stderr_open || exit_status.is_none() {
        let idle_deadline_at = idle_deadline.unwrap_or_else(Instant::now);
        let grace_deadline_at = result_grace_deadline.unwrap_or_else(Instant::now);
        tokio::select! {
            line = stdout_lines.next_line(), if stdout_open => {
                match line? {
                    Some(line) => {
                        if let Some(secs) = idle_secs {
                            idle_deadline = Some(Instant::now() + Duration::from_secs(secs));
                        }
                        let message: Value = serde_json::from_str(&line).map_err(|error| {
                            io::Error::other(format!("invalid AGY NDJSON: {error}"))
                        })?;
                        implement_tracker.observe_agy_message(&message);
                        for event in normalize_event(&message, mode, &mut state) {
                            send_event(event_tx, Some(context), event).await;
                        }
                        if state.result_received {
                            stdout_open = false;
                            if result_grace_deadline.is_none() {
                                result_grace_deadline = Some(
                                    Instant::now() + Duration::from_secs(AGY_RESULT_GRACE_SECS),
                                );
                            }
                        }
                    }
                    None => stdout_open = false,
                }
            }
            line = stderr_lines.next_line(), if stderr_open => {
                match line? {
                    Some(line) => push_stderr(&mut stderr_history, line),
                    None => stderr_open = false,
                }
            }
            status = child.wait(), if exit_status.is_none() => {
                exit_status = Some(status?);
            }
            _ = tokio::time::sleep_until(idle_deadline_at), if idle_deadline.is_some() => {
                child.kill().await.ok();
                return Err(io::Error::other(format!(
                    "AGY produced no events for {}s; it may be waiting on a background command. Run commands in the foreground with a bounded timeout.",
                    idle_secs.unwrap_or(AGY_IDLE_TIMEOUT_DEFAULT_SECS)
                )));
            }
            _ = tokio::time::sleep_until(grace_deadline_at), if result_grace_deadline.is_some() => {
                // The result line arrived but the loop is still open. Either the
                // child is lingering (kill and reap it) or it already exited
                // while a grandchild keeps the stderr pipe open (just stop
                // draining). Closing both pipes guarantees termination even
                // when no idle timeout is configured.
                if exit_status.is_none() {
                    killed_after_result = true;
                    child.kill().await.ok();
                    exit_status = Some(child.wait().await?);
                }
                stdout_open = false;
                stderr_open = false;
            }
        }
    }

    let status = exit_status.expect("child.wait runs before the loop can finish");
    let diagnostic = stderr_history.into_iter().collect::<Vec<_>>().join(" | ");
    let protocol_completed = state.pending_terminal.is_some();
    let require_writes = false;
    let writes_observed = implement_tracker.writes_observed();
    if let Err(reason) = implement_tracker.finish(
        &status,
        &diagnostic,
        protocol_completed,
        require_writes,
        killed_after_result,
    ) {
        return Ok(SingleAgyOutcome {
            conversation_id: state.conversation_id,
            response_text: state.response_text,
            writes_observed,
            terminal: AgentEvent::Error(reason),
        });
    }
    if !protocol_completed {
        let message = if diagnostic.is_empty() {
            format!("AGY exited with {status} without a terminal result")
        } else {
            format!("AGY exited with {status}: {diagnostic}")
        };
        return Ok(SingleAgyOutcome {
            conversation_id: state.conversation_id,
            response_text: state.response_text,
            writes_observed,
            terminal: AgentEvent::Error(message),
        });
    }

    Ok(SingleAgyOutcome {
        conversation_id: state.conversation_id,
        response_text: state.response_text,
        writes_observed,
        terminal: state
            .pending_terminal
            .expect("protocol_completed implies a pending terminal event"),
    })
}

#[derive(Default)]
struct AgyStreamState {
    conversation_id: Option<String>,
    emitted_text: bool,
    result_received: bool,
    pending_terminal: Option<AgentEvent>,
    response_text: String,
}

fn denied_actions_summary(message: &Value) -> Option<String> {
    let denied = message.pointer("/result/denied_actions")?.as_array()?;
    let names = denied
        .iter()
        .map(|action| {
            action
                .get("display_name")
                .or_else(|| action.get("action"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned()
        })
        .collect::<Vec<_>>();
    (!names.is_empty()).then(|| names.join(", "))
}

fn normalize_event(
    message: &Value,
    mode: TurnPolicy,
    state: &mut AgyStreamState,
) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    if let Some(conversation_id) = message
        .get("conversation_id")
        .or_else(|| message.pointer("/result/conversation_id"))
        .or_else(|| message.pointer("/step_update/conversation_id"))
        .and_then(Value::as_str)
        && state.conversation_id.as_deref() != Some(conversation_id)
    {
        state.conversation_id = Some(conversation_id.to_owned());
        events.push(AgentEvent::ProviderSessionBound(conversation_id.to_owned()));
    }

    match message.get("event").and_then(Value::as_str) {
        Some("step_update")
            if message
                .pointer("/step_update/step_type")
                .and_then(Value::as_str)
                == Some("agent_response") =>
        {
            let text = message
                .pointer("/step_update/text_delta")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !text.is_empty() {
                state.emitted_text = true;
                state.response_text.push_str(text);
                events.push(AgentEvent::TextDelta(text.to_owned()));
            }
        }
        Some("step_update") => {
            if let Some(activity) = dmuxai::activity::from_agy_step(message) {
                events.push(AgentEvent::Activity(activity));
            }
        }
        Some("result") => {
            state.result_received = true;
            let status = message
                .pointer("/result/status")
                .and_then(Value::as_str)
                .unwrap_or("ERROR");
            match status {
                "SUCCESS" => {
                    if let Some(denied) = denied_actions_summary(message) {
                        state.pending_terminal = Some(AgentEvent::Error(format!(
                            "AGY denied permission for: {denied}. Headless AGY cannot prompt; allow it in AGY settings or run with write access."
                        )));
                        return events;
                    }
                    if !state.emitted_text
                        && let Some(response) =
                            message.pointer("/result/response").and_then(Value::as_str)
                        && !response.is_empty()
                    {
                        state.response_text.push_str(response);
                        events.push(AgentEvent::TextDelta(response.to_owned()));
                    }
                    if state.response_text.trim().is_empty() {
                        state.pending_terminal = Some(AgentEvent::Error(
                            "AGY finished without producing any output.".to_owned(),
                        ));
                        return events;
                    }
                    if is_plan_review_response(&state.response_text) && mode != TurnPolicy::Agent {
                        events.push(AgentEvent::Notice(
                            "AGY returned a plan for review; no file changes were executed (plan+sandbox mode)."
                                .to_owned(),
                        ));
                    }
                    state.pending_terminal = Some(AgentEvent::Completed);
                }
                "CANCELED" | "INTERRUPTED" => {
                    state.pending_terminal = Some(AgentEvent::Cancelled);
                }
                _ => {
                    state.pending_terminal = Some(AgentEvent::Error(
                        message
                            .pointer("/result/error")
                            .and_then(Value::as_str)
                            .unwrap_or("AGY turn failed")
                            .to_owned(),
                    ));
                }
            }
        }
        _ => {}
    }
    events
}

async fn list_models(cwd: &Path) -> io::Result<Vec<ModelInfo>> {
    let program = resolve_program(AGY_BINARY_OVERRIDE, "agy.exe", "agy");
    let mut command = headless_command(program);
    command
        .arg("models")
        .current_dir(cwd)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = command.output().await?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "AGY model discovery failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(parse_model_list(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_model_list(output: &str) -> Vec<ModelInfo> {
    output
        .lines()
        .filter_map(|line| {
            let (id, label) = line.split_once('\t')?;
            let id = id.trim();
            let label = label.trim();
            (!id.is_empty() && !label.is_empty()).then(|| ModelInfo::new(id, label))
        })
        .collect()
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
            agent: Agent::Agy,
            conversation_id: context.map(|context| context.conversation_id.clone()),
            turn_id: context.map(|context| context.turn_id.clone()),
            event,
        })
        .await;
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::time::{Duration, timeout};

    use super::*;

    #[test]
    fn parses_agy_models() {
        let models = parse_model_list(
            "Fetching available models...\ngemini-high\tGemini High\nclaude\tClaude",
        );
        assert_eq!(
            models,
            vec![
                ModelInfo::new("gemini-high", "Gemini High"),
                ModelInfo::new("claude", "Claude")
            ]
        );
    }

    #[test]
    fn normalizes_agy_delta_and_success() {
        let mut state = AgyStreamState::default();
        let delta = json!({
            "event": "step_update",
            "step_update": {
                "conversation_id": "conversation",
                "step_type": "agent_response",
                "text_delta": "AGY_OK"
            }
        });
        let result = json!({
            "event": "result",
            "result": {
                "conversation_id": "conversation",
                "status": "SUCCESS",
                "response": "AGY_OK"
            }
        });

        assert_eq!(
            normalize_event(&delta, TurnPolicy::ReadOnly, &mut state),
            vec![
                AgentEvent::ProviderSessionBound("conversation".to_owned()),
                AgentEvent::TextDelta("AGY_OK".to_owned()),
            ]
        );
        assert_eq!(
            normalize_event(&result, TurnPolicy::ReadOnly, &mut state),
            vec![]
        );
        assert!(matches!(
            state.pending_terminal,
            Some(AgentEvent::Completed)
        ));
        assert_eq!(state.conversation_id.as_deref(), Some("conversation"));
    }

    // TC-1010 / FR-1010: the shared headless builder (used by every runner,
    // judge and planner turn) removes the dashboard token without any extra call.
    #[test]
    fn agent_child_env_drops_web_token() {
        fn token_removed(command: &tokio::process::Command) -> bool {
            command
                .as_std()
                .get_envs()
                .any(|(key, value)| key == "DMUXAI_WEB_TOKEN" && value.is_none())
        }
        let command = dmuxai::process::headless_command(std::path::PathBuf::from("agy"));
        assert!(token_removed(&command), "headless_command");
        match dmuxai::process::cursor_command() {
            Ok(command) => assert!(token_removed(&command), "cursor_command"),
            // Windows without LOCALAPPDATA cannot build the command at all.
            Err(error) => {
                if !cfg!(windows) {
                    panic!("cursor_command failed: {error}");
                }
            }
        }
    }

    #[test]
    fn agy_resume_session_is_passed_to_the_command() {
        let request = AgentRequest {
            conversation_id: super::super::ConversationId::new("conversation"),
            turn_id: super::super::TurnId::new("turn"),
            prompt: "hello".to_owned(),
            resume_session: Some("agy-conversation".to_owned()),
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        };
        let mut command = tokio::process::Command::new("agy");

        configure_command_for_resume(
            &mut command,
            &request,
            Some("agy-model"),
            TurnPolicy::Agent,
            Some("agy-conversation"),
        );

        let args = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            args.windows(2)
                .any(|args| args == ["--conversation", "agy-conversation"])
        );
        assert!(args.windows(2).any(|args| args == ["--model", "agy-model"]));
    }

    #[test]
    // FR-IMG-5 / INT-IMG-2: AGY has no `--image` flag; image paths become
    // `@path` tokens appended to the text prompt.
    fn agy_passes_image_paths_to_cli() {
        let request = AgentRequest {
            conversation_id: super::super::ConversationId::new("conversation"),
            turn_id: super::super::TurnId::new("turn"),
            prompt: "describe".to_owned(),
            resume_session: None,
            image_paths: vec![
                PathBuf::from("/tmp/shot.png"),
                PathBuf::from("/tmp/other.png"),
            ],
            policy: TurnPolicy::ReadOnly,
            require_file_edits: false,
        };
        let mut command = tokio::process::Command::new("agy");
        configure_command(&mut command, None, TurnPolicy::ReadOnly);
        let args = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(!args.iter().any(|arg| arg == "--image"));
        let prompt = prompt_with_image_tokens(&request.prompt, &request.image_paths);
        assert_eq!(prompt, "describe @/tmp/shot.png @/tmp/other.png");
        assert!(agy_prompt(TurnPolicy::ReadOnly, &prompt).ends_with(&prompt));
    }

    #[test]
    fn agy_execution_modes_use_native_cli_flags_and_safe_prompts() {
        let args_for = |mode| {
            let mut command = tokio::process::Command::new("agy");
            configure_command(&mut command, None, mode);
            command
                .as_std()
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };

        let read_only = args_for(TurnPolicy::ReadOnly);
        assert!(read_only.windows(2).any(|args| args == ["--mode", "plan"]));
        assert_eq!(agy_prompt(TurnPolicy::ReadOnly, "hello"), "hello");
        let implement = args_for(TurnPolicy::Agent);
        assert!(
            implement
                .windows(2)
                .any(|args| args == ["--mode", "accept-edits"])
        );
        assert!(implement.iter().any(|arg| arg == "--sandbox"));
        assert!(agy_prompt(TurnPolicy::Agent, "hello").ends_with("\n\nhello"));
    }

    #[test]
    fn marks_plan_review_responses_as_a_notice() {
        let mut state = AgyStreamState::default();
        let result = json!({
            "event": "result",
            "result": {
                "status": "SUCCESS",
                "response": "I created the implementation plan artifact at plan.md. Please review and approve the plan to proceed."
            }
        });

        assert_eq!(
            normalize_event(&result, TurnPolicy::ReadOnly, &mut state),
            vec![
                AgentEvent::TextDelta(
                    "I created the implementation plan artifact at plan.md. Please review and approve the plan to proceed.".to_owned()
                ),
                AgentEvent::Notice(
                    "AGY returned a plan for review; no file changes were executed (plan+sandbox mode)."
                        .to_owned()
                ),
            ]
        );
        assert!(matches!(
            state.pending_terminal,
            Some(AgentEvent::Completed)
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a locally installed and authenticated AGY CLI"]
    async fn real_agy_turn_streams_to_completion() {
        let (event_tx, mut event_rx) = mpsc::channel(128);
        let cwd = std::env::current_dir().unwrap();
        let request = AgentRequest {
            conversation_id: super::super::ConversationId::new("real-conversation"),
            turn_id: super::super::TurnId::new("real-turn"),
            prompt: "Reply with exactly AGY_OK. Do not use tools or modify files.".to_owned(),
            resume_session: None,
            image_paths: vec![],
            policy: TurnPolicy::ReadOnly,
            require_file_edits: false,
        };
        let turn = tokio::spawn(async move { run_turn(&request, None, &cwd, &event_tx).await });

        let answer = timeout(Duration::from_secs(90), async {
            let mut answer = String::new();
            while let Some(output) = event_rx.recv().await {
                match output.event {
                    AgentEvent::TextDelta(delta) => answer.push_str(&delta),
                    AgentEvent::Completed => return answer,
                    AgentEvent::Error(error) => panic!("AGY turn failed: {error}"),
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
        .expect("AGY should complete within 90 seconds");

        let conversation = turn
            .await
            .unwrap()
            .expect("real AGY turn should not return an I/O error")
            .expect("real AGY turn should return a conversation ID");
        assert!(!conversation.is_empty());
        assert!(answer.contains("AGY_OK"), "unexpected answer: {answer}");
    }

    #[test]
    fn denied_actions_and_empty_output_are_errors() {
        let mut state = AgyStreamState::default();
        let denied = json!({"event":"result","result":{"status":"SUCCESS","response":"","denied_actions":[{"action":"command","display_name":"RunCommand"}]}});
        let events = normalize_event(&denied, TurnPolicy::Agent, &mut state);
        assert!(events.is_empty());
        assert!(matches!(
            state.pending_terminal,
            Some(AgentEvent::Error(e)) if e.contains("RunCommand")
        ));

        let mut state = AgyStreamState::default();
        let empty = json!({"event":"result","result":{"status":"SUCCESS","response":""}});
        let _events = normalize_event(&empty, TurnPolicy::ReadOnly, &mut state);
        assert!(matches!(
            state.pending_terminal,
            Some(AgentEvent::Error(e)) if e.contains("without producing")
        ));
    }

    #[test]
    fn test_disk_cache_roundtrip_and_freshness() {
        let dir = tempfile::tempdir().unwrap();
        let cache_file = dir.path().join("agy-models.json");

        let sample_models = vec![
            ModelInfo::new("gemini-3.8-flash", "Gemini 3.8 Flash"),
            ModelInfo::new("gemini-pro", "Gemini Pro"),
        ];

        let base_time = 1_000_000;
        write_disk_cache(&cache_file, &sample_models, base_time).unwrap();

        // Fresh check: 10 hours later (< 24h)
        let now_fresh = base_time + 10 * 3600;
        let result = read_disk_cache(&cache_file, now_fresh, AGY_MODEL_CACHE_TTL_SECS);
        assert!(result.is_some());
        let (models, is_fresh) = result.unwrap();
        assert!(is_fresh);
        assert_eq!(models, sample_models);

        // Expired check: 25 hours later (> 24h)
        let now_expired = base_time + 25 * 3600;
        let result_expired = read_disk_cache(&cache_file, now_expired, AGY_MODEL_CACHE_TTL_SECS);
        assert!(result_expired.is_some());
        let (models_exp, is_fresh_exp) = result_expired.unwrap();
        assert!(!is_fresh_exp);
        assert_eq!(models_exp, sample_models);
    }

    #[test]
    fn test_disk_cache_missing_or_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let cache_file = dir.path().join("missing.json");
        assert!(read_disk_cache(&cache_file, 1000, 3600).is_none());

        let corrupt_file = dir.path().join("corrupt.json");
        std::fs::write(&corrupt_file, "not valid json").unwrap();
        assert!(read_disk_cache(&corrupt_file, 1000, 3600).is_none());
    }

    #[test]
    fn normalizes_agy_activity_step_updates() {
        let mut state = AgyStreamState::default();
        let tool_update = json!({
            "event": "step_update",
            "step_update": {
                "step_type": "tool",
                "tool_name": "run",
                "tool_info": {
                    "parameters": {
                        "command": "cargo test"
                    }
                },
                "state": "ACTIVE"
            }
        });
        let events = normalize_event(&tool_update, TurnPolicy::Agent, &mut state);
        assert_eq!(
            events,
            vec![AgentEvent::Activity(dmuxai::activity::ActivityItem::Tool {
                name: "run".to_owned(),
                target: "cargo test".to_owned(),
                status: dmuxai::activity::ToolStatus::Running,
                duration_ms: None,
                detail: serde_json::to_string_pretty(
                    &tool_update["step_update"]["tool_info"]["parameters"]
                )
                .unwrap(),
            })]
        );

        let thinking_update = json!({
            "event": "step_update",
            "step_update": {
                "step_type": "thinking",
                "thinking_delta": "thinking about code"
            }
        });
        let events = normalize_event(&thinking_update, TurnPolicy::Agent, &mut state);
        assert_eq!(
            events,
            vec![AgentEvent::Activity(
                dmuxai::activity::ActivityItem::Thinking {
                    text: "thinking about code".to_owned(),
                    started_ms: 0,
                    finished_ms: None,
                }
            )]
        );
    }

    #[test]
    fn agy_idle_timeout_env_parsing() {
        // FR-442 / TC-442: unset uses the default, "0" disables, a valid value
        // overrides, and invalid or non-positive values use the default.
        assert_eq!(parse_idle_timeout_secs(None), Some(300));
        assert_eq!(parse_idle_timeout_secs(Some("0")), None);
        assert_eq!(parse_idle_timeout_secs(Some("45")), Some(45));
        assert_eq!(parse_idle_timeout_secs(Some(" 7 ")), Some(7));
        assert_eq!(parse_idle_timeout_secs(Some(" 90 ")), Some(90));
        assert_eq!(parse_idle_timeout_secs(Some("ABC")), Some(300));
        assert_eq!(parse_idle_timeout_secs(Some("-5")), Some(300));
        assert_eq!(parse_idle_timeout_secs(Some("1.5")), Some(300));
        assert_eq!(parse_idle_timeout_secs(Some("")), Some(300));
    }

    #[test]
    fn implement_prompt_forbids_background_commands() {
        // FR-443 / TC-443, FR-502 / TC-502: Implement mode must apply edits,
        // keep commands in the foreground with a bounded timeout, and avoid
        // background or detached processes; Ask/Plan stay untouched.
        let wrapped = agy_prompt(TurnPolicy::Agent, "build the thing");
        assert!(
            wrapped.starts_with("Apply edits directly"),
            "unexpected Implement prompt: {wrapped}"
        );
        assert!(
            wrapped.contains("Do not produce plans"),
            "unexpected Implement prompt: {wrapped}"
        );
        assert!(
            wrapped.contains("background"),
            "unexpected Implement prompt: {wrapped}"
        );
        assert!(wrapped.ends_with("\n\nbuild the thing"));
        assert!(!agy_prompt(TurnPolicy::ReadOnly, "hi").contains("background"));
        assert!(!agy_prompt(TurnPolicy::ReadOnly, "hi").contains("background"));
    }

    #[test]
    fn result_event_defers_terminal_until_process_exit() {
        let mut state = AgyStreamState::default();
        let result = json!({
            "event": "result",
            "result": { "status": "SUCCESS", "response": "AGY_OK" }
        });
        let events = normalize_event(&result, TurnPolicy::Agent, &mut state);
        assert!(!events.iter().any(|event| {
            matches!(
                event,
                AgentEvent::Completed | AgentEvent::Cancelled | AgentEvent::Error(_)
            )
        }));
        assert!(state.result_received);
        assert!(matches!(
            state.pending_terminal,
            Some(AgentEvent::Completed)
        ));
    }

    #[test]
    fn implement_plan_only_does_not_error_on_first_result_line() {
        let mut state = AgyStreamState::default();
        let result = json!({
            "event": "result",
            "result": {
                "status": "SUCCESS",
                "response": "I wrote the plan to plan.md. Please review and approve before I edit files."
            }
        });
        let events = normalize_event(&result, TurnPolicy::Agent, &mut state);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], AgentEvent::TextDelta(_)));
        assert!(matches!(
            state.pending_terminal,
            Some(AgentEvent::Completed)
        ));
    }

    #[cfg(unix)]
    static AGY_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[cfg(unix)]
    async fn with_agy_spawn_env<F>(program: &Path, idle_secs: &str, work: F) -> F::Output
    where
        F: std::future::Future,
    {
        let _guard = AGY_ENV_LOCK.lock().await;
        let saved_bin = std::env::var(AGY_BINARY_OVERRIDE).ok();
        let saved_idle = std::env::var(AGY_IDLE_TIMEOUT_ENV).ok();
        unsafe {
            std::env::set_var(AGY_BINARY_OVERRIDE, program);
            std::env::set_var(AGY_IDLE_TIMEOUT_ENV, idle_secs);
        }
        let result = work.await;
        unsafe {
            match saved_bin {
                Some(value) => std::env::set_var(AGY_BINARY_OVERRIDE, value),
                None => std::env::remove_var(AGY_BINARY_OVERRIDE),
            }
            match saved_idle {
                Some(value) => std::env::set_var(AGY_IDLE_TIMEOUT_ENV, value),
                None => std::env::remove_var(AGY_IDLE_TIMEOUT_ENV),
            }
        }
        result
    }

    #[cfg(unix)]
    fn write_fake_agy(build_script: impl FnOnce(&Path) -> String) -> (tempfile::TempDir, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("fake AGY tempdir");
        let script = build_script(dir.path());
        let path = dir.path().join("agy");
        std::fs::write(&path, script).expect("fake AGY script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("fake AGY chmod");
        (dir, path)
    }

    #[cfg(unix)]
    async fn run_fake_agy_turn(
        mode: TurnPolicy,
        require_file_edits: bool,
    ) -> io::Result<Option<String>> {
        let request = AgentRequest {
            conversation_id: super::super::ConversationId::new("fake-conversation"),
            turn_id: super::super::TurnId::new("fake-turn"),
            prompt: "hello".to_owned(),
            resume_session: None,
            image_paths: vec![],
            policy: mode,
            require_file_edits,
        };
        let (event_tx, _event_rx) = mpsc::channel(64);
        let cwd = std::env::temp_dir();
        run_turn(&request, None, &cwd, &event_tx).await
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn agy_idle_timeout_kills_silent_child() {
        // FR-440 / TC-440: after one activity line the fake AGY goes silent, so
        // the idle deadline must kill the child and fail the turn with a
        // `no events for <N>s` error that calls out background commands.
        let (dir, fake) = write_fake_agy(|tmp| {
            let init_event = json!({
                "event": "step_update",
                "step_update": {
                    "step_type": "tool",
                    "tool_name": "run",
                    "tool_info": {"parameters": {"command": "cargo build &"}},
                    "state": "ACTIVE"
                }
            });
            format!(
                "#!/bin/sh\necho $$ > {}\nprintf '%s\\n' '{}'\nsleep 600\n",
                tmp.join("agy.pid").display(),
                init_event
            )
        });
        let pid_path = dir.path().join("agy.pid");

        let outcome = with_agy_spawn_env(&fake, "1", async {
            timeout(
                Duration::from_secs(20),
                run_fake_agy_turn(TurnPolicy::Agent, false),
            )
            .await
            .expect("the idle timeout should end the turn far before the child settles")
        })
        .await;

        let error = outcome.expect_err("a silent child should fail the turn");
        let message = error.to_string();
        assert!(
            message.contains("no events for 1s"),
            "unexpected timeout error: {message}"
        );
        assert!(
            message.contains("background"),
            "unexpected timeout error: {message}"
        );

        // The fake AGY recorded its PID only once it started, and the idle
        // path must have killed it by now.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let pid = std::fs::read_to_string(&pid_path)
            .expect("fake AGY should have written its PID")
            .trim()
            .to_owned();
        assert!(!pid.is_empty());
        let dead = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if std::process::Command::new("kill")
                    .arg("-0")
                    .arg(&pid)
                    .status()
                    .map(|status| !status.success())
                    .unwrap_or(true)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .is_ok();
        assert!(
            dead,
            "fake AGY process {pid} should be killed on idle timeout"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn agy_idle_timer_resets_on_each_event() {
        // FR-441 / TC-441: every stdout line re-arms the idle deadline, so the
        // turn survives when the cumulative runtime exceeds the timeout while
        // no single gap between stdout lines does.
        let step = |delta: &str| {
            json!({
                "event": "step_update",
                "step_update": {
                    "step_type": "thinking",
                    "thinking_delta": delta
                }
            })
            .to_string()
        };
        let result_event = json!({
            "event": "result",
            "result": {"status": "SUCCESS", "response": "AGY_OK"}
        });
        let (_dir, fake) = write_fake_agy(|_| {
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{}'\nsleep 0.4\nprintf '%s\\n' '{}'\nsleep 0.4\nprintf '%s\\n' '{}'\nsleep 0.4\nprintf '%s\\n' '{}'\n",
                step("one"),
                step("two"),
                step("three"),
                result_event
            )
        });

        let outcome = with_agy_spawn_env(&fake, "5", async {
            timeout(
                Duration::from_secs(45),
                run_fake_agy_turn(TurnPolicy::ReadOnly, false),
            )
            .await
            .expect("a continuously streaming child should finish promptly")
        })
        .await;

        outcome.expect("stdout events within the timeout should keep the turn alive");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn agy_result_grace_kills_lingering_child_after_result() {
        // FR-503 / TC-505: after the result line the adapter waits briefly for
        // process exit, then kills the child and emits a single terminal event.
        let result_event = json!({
            "event": "result",
            "result": { "status": "SUCCESS", "response": "AGY_OK" }
        });
        let line = result_event.to_string();
        let (_dir, fake) = write_fake_agy(|_| {
            format!("#!/bin/sh\nread -r _dmuxai_user\nprintf '%s\\n' '{line}'\nsleep 60\n")
        });

        let (outcome, elapsed) = with_agy_spawn_env(&fake, "0", async {
            // Measure inside the env lock; tests serialize on it, so an outer
            // timer would include other tests' cleanup time.
            let started = Instant::now();
            let outcome = timeout(
                Duration::from_secs(15),
                run_fake_agy_turn(TurnPolicy::ReadOnly, false),
            )
            .await
            .expect("grace kill should finish quickly");
            (outcome, started.elapsed())
        })
        .await;

        outcome.expect("result should succeed after grace kill");
        assert!(
            elapsed < Duration::from_secs(12),
            "turn took too long: {}",
            elapsed.as_secs()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn agy_implement_grace_kill_after_success_result_completes() {
        // FR-503 / TC-505: in Implement mode a child that lingers after a
        // successful result is grace-killed; the forced signal exit must not be
        // turned into an Error by `finish`. Exactly one terminal event,
        // `Completed`, and the turn ends promptly.
        let write_step = json!({
            "event": "step_update",
            "step_update": {
                "step_type": "tool",
                "tool_name": "write_to_file"
            }
        });
        let result_event = json!({
            "event": "result",
            "result": { "status": "SUCCESS", "response": "AGY_OK" }
        });
        let script = format!(
            "#!/bin/sh\nread -r _dmuxai_user\nprintf '%s\\n' '{write_step}'\nprintf '%s\\n' '{result_event}'\nsleep 60\n"
        );
        let (_dir, fake) = write_fake_agy(|_| script);

        let request = AgentRequest {
            conversation_id: super::super::ConversationId::new("grace-conversation"),
            turn_id: super::super::TurnId::new("grace-turn"),
            prompt: "hello".to_owned(),
            resume_session: None,
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        };
        let (event_tx, mut event_rx) = mpsc::channel(64);
        let cwd = std::env::temp_dir();

        let (outcome, terminals, elapsed) = with_agy_spawn_env(&fake, "0", async {
            // Measure inside the env lock; AGY spawn tests serialize on it.
            let started = Instant::now();
            let turn = tokio::spawn(async move { run_turn(&request, None, &cwd, &event_tx).await });

            let terminals = timeout(Duration::from_secs(10), async {
                let mut terminals = Vec::new();
                while let Some(output) = event_rx.recv().await {
                    match output.event {
                        AgentEvent::Completed | AgentEvent::Cancelled | AgentEvent::Error(_) => {
                            terminals.push(output.event);
                        }
                        _ => {}
                    }
                }
                terminals
            })
            .await
            .expect("grace kill after a successful result should finish within 10s");

            let outcome = turn.await.expect("turn task should not panic");
            (outcome, terminals, started.elapsed())
        })
        .await;

        outcome.expect("implement turn should succeed after the grace kill");
        assert_eq!(
            terminals.len(),
            1,
            "expected exactly one terminal event, got {terminals:?}"
        );
        assert!(
            matches!(terminals[0], AgentEvent::Completed),
            "terminal event should be Completed, got {:?}",
            terminals[0]
        );
        assert!(
            elapsed < Duration::from_secs(10),
            "turn took too long: {}",
            elapsed.as_secs()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn agy_grace_closes_when_grandchild_holds_stderr_after_exit() {
        // FR-503 / TC-505: after the result line the adapter stops reading
        // stdout; if the AGY child exits but a grandchild inherits and holds the
        // stderr pipe open, the grace deadline must still close the loop instead
        // of hanging forever when no idle timeout is configured.
        let write_step = json!({
            "event": "step_update",
            "step_update": {
                "step_type": "tool",
                "tool_name": "write_to_file"
            }
        });
        let result_event = json!({
            "event": "result",
            "result": { "status": "SUCCESS", "response": "AGY_OK" }
        });
        let script = format!(
            "#!/bin/sh\nread -r _dmuxai_user\nprintf '%s\\n' '{write_step}'\nprintf '%s\\n' '{result_event}'\n( sleep 60 >&2 ) &\nexit 0\n"
        );
        let (_dir, fake) = write_fake_agy(|_| script);

        let request = AgentRequest {
            conversation_id: super::super::ConversationId::new("grandchild-conversation"),
            turn_id: super::super::TurnId::new("grandchild-turn"),
            prompt: "hello".to_owned(),
            resume_session: None,
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        };
        let (event_tx, mut event_rx) = mpsc::channel(64);
        let cwd = std::env::temp_dir();

        let (outcome, terminals, elapsed) = with_agy_spawn_env(&fake, "0", async {
            // Measure inside the env lock; AGY spawn tests serialize on it.
            let started = Instant::now();
            let turn = tokio::spawn(async move { run_turn(&request, None, &cwd, &event_tx).await });

            let terminals = timeout(Duration::from_secs(10), async {
                let mut terminals = Vec::new();
                while let Some(output) = event_rx.recv().await {
                    match output.event {
                        AgentEvent::Completed | AgentEvent::Cancelled | AgentEvent::Error(_) => {
                            terminals.push(output.event);
                        }
                        _ => {}
                    }
                }
                terminals
            })
            .await
            .expect("grace close should finish within 10s even with a stderr-holding grandchild");

            let outcome = turn.await.expect("turn task should not panic");
            (outcome, terminals, started.elapsed())
        })
        .await;

        outcome.expect("implement turn should succeed");
        assert_eq!(
            terminals.len(),
            1,
            "expected exactly one terminal event, got {terminals:?}"
        );
        assert!(
            matches!(terminals[0], AgentEvent::Completed),
            "terminal event should be Completed, got {:?}",
            terminals[0]
        );
        assert!(
            elapsed < Duration::from_secs(10),
            "turn took too long: {}",
            elapsed.as_secs()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn implement_plan_only_retries_once_then_fails() {
        // FR-506 / TC-506: plan-only Implement outcomes retry once, then fail.
        let plan_response = json!({
            "event": "result",
            "result": {
                "status": "SUCCESS",
                "conversation_id": "plan-only",
                "response": "I saved the implementation plan to plan.md. Please review and approve before editing."
            }
        });
        let plan_line = plan_response.to_string();
        let (_dir, fake) = write_fake_agy(|_| {
            format!("#!/bin/sh\nread -r _dmuxai_user\nprintf '%s\\n' '{plan_line}'\n")
        });

        let outcome = with_agy_spawn_env(&fake, "0", async {
            timeout(
                Duration::from_secs(20),
                run_fake_agy_turn(TurnPolicy::Agent, true),
            )
            .await
            .expect("plan-only retry should finish promptly")
        })
        .await;

        let error = outcome.expect_err("second plan-only attempt should fail the turn");
        assert!(
            error.to_string().contains("plan-only"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn agy_question_plan_review_text_does_not_force_edit_retry() {
        // FR-506: ordinary questions must not trigger the plan-only edit retry.
        let plan_response = json!({
            "event": "result",
            "result": {
                "status": "SUCCESS",
                "response": "Please review plan.md and confirm before I edit anything."
            }
        });
        let plan_line = plan_response.to_string();
        let (_dir, fake) = write_fake_agy(|_| {
            format!("#!/bin/sh\nread -r _dmuxai_user\nprintf '%s\\n' '{plan_line}'\n")
        });

        let request = AgentRequest {
            conversation_id: super::super::ConversationId::new("question-conversation"),
            turn_id: super::super::TurnId::new("question-turn"),
            prompt: "What does PLAN.md propose? Don't modify anything.".to_owned(),
            resume_session: None,
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        };
        let (event_tx, mut event_rx) = mpsc::channel(64);
        let cwd = std::env::temp_dir();

        let outcome = with_agy_spawn_env(&fake, "0", async {
            let turn = tokio::spawn(async move { run_turn(&request, None, &cwd, &event_tx).await });
            let mut terminals = Vec::new();
            while let Some(output) = event_rx.recv().await {
                if matches!(
                    output.event,
                    AgentEvent::Completed | AgentEvent::Cancelled | AgentEvent::Error(_)
                ) {
                    terminals.push(output.event);
                }
            }
            turn.await.expect("turn task")
        })
        .await;

        outcome.expect("question turn should complete without edit retry");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(unix)]
    async fn implement_permission_denial_with_plan_like_response_is_not_a_plan_only_retry() {
        // FR-508 / TC-509: a permission-denial `finish` failure must fail the
        // Implement turn immediately, even when the (no-write) response text
        // looks plan-only; the denial terminal must not be swallowed by the
        // plan-only retry path.
        let plan_response = json!({
            "event": "result",
            "result": {
                "status": "SUCCESS",
                "response": "I saved the implementation plan to plan.md. Please review and approve before editing."
            }
        });
        let plan_line = plan_response.to_string();
        let (_dir, fake) = write_fake_agy(|_| {
            format!(
                "#!/bin/sh\nread -r _dmuxai_user\nprintf 'error: permission denied: sandbox\\n' >&2\nprintf '%s\\n' '{plan_line}'\n"
            )
        });

        let request = AgentRequest {
            conversation_id: super::super::ConversationId::new("denial-conversation"),
            turn_id: super::super::TurnId::new("denial-turn"),
            prompt: "hello".to_owned(),
            resume_session: None,
            image_paths: vec![],
            policy: TurnPolicy::Agent,
            require_file_edits: false,
        };
        let (event_tx, mut event_rx) = mpsc::channel(64);
        let cwd = std::env::temp_dir();

        let (outcome, terminals) = with_agy_spawn_env(&fake, "0", async {
            let turn = tokio::spawn(async move { run_turn(&request, None, &cwd, &event_tx).await });

            let terminals = timeout(Duration::from_secs(10), async {
                let mut terminals = Vec::new();
                while let Some(output) = event_rx.recv().await {
                    match output.event {
                        AgentEvent::Completed | AgentEvent::Cancelled | AgentEvent::Error(_) => {
                            terminals.push(output.event);
                        }
                        _ => {}
                    }
                }
                terminals
            })
            .await
            .expect("a denied Implement turn should finish within 10s");

            let outcome = turn.await.expect("turn task should not panic");
            (outcome, terminals)
        })
        .await;

        assert_eq!(
            terminals.len(),
            1,
            "expected exactly one terminal event, got {terminals:?}"
        );
        assert!(
            matches!(&terminals[0], AgentEvent::Error(message) if message.contains("permission denied")),
            "terminal event should carry the denial, got {:?}",
            terminals[0]
        );
        assert!(
            outcome.is_ok(),
            "the turn returns conversation id; the denial is the terminal event"
        );
    }
}
