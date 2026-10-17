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
    AdapterError, Agent, AgentAdapter, AgentEvent, AgentOutput, AgentRequest, ExecutionMode,
    ModelInfo, OutputContext, SpawnedAdapter, implement_outcome::ImplementTurnTracker,
    prompt_with_image_tokens,
};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::process::{headless_command, resolve_program};

const COMMAND_CHANNEL_CAPACITY: usize = 8;
const STDERR_HISTORY_CAPACITY: usize = 8;
const AGY_BINARY_OVERRIDE: &str = "DMUXAI_AGY_BIN";
const AGY_MODEL_CACHE_TTL_SECS: u64 = 24 * 60 * 60;
const AGY_IDLE_TIMEOUT_ENV: &str = "DMUXAI_AGY_IDLE_TIMEOUT_SECS";
const AGY_IDLE_TIMEOUT_DEFAULT_SECS: u64 = 300;

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
    SelectMode(ExecutionMode),
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

    fn select_mode(&self, mode: ExecutionMode) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(AgyCommand::SelectMode(mode))
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
    let mut selected_mode = ExecutionMode::default();
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
                        selected_mode,
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
                AgyCommand::SelectMode(mode) => {
                    selected_mode = mode;
                    send_event(&event_tx, None, AgentEvent::ModeSelected(mode)).await;
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
                            AgentEvent::Notice(
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
                    Some(AgyCommand::SelectMode(mode)) => {
                        selected_mode = mode;
                        send_event(&event_tx, None, AgentEvent::ModeSelected(mode)).await;
                        send_event(
                            &event_tx,
                            None,
                            AgentEvent::Notice(
                                "AGY mode changed; it will apply to the next turn".to_owned(),
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
    mode: ExecutionMode,
    cwd: PathBuf,
    event_tx: mpsc::Sender<AgentOutput>,
) -> ActiveTurn {
    let context = OutputContext::from(&request);
    let error_context = context.clone();
    let handle = tokio::spawn(async move {
        if let Err(error) = run_turn(&request, selected_model, mode, &cwd, &event_tx).await {
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
    mode: ExecutionMode,
    cwd: &Path,
    event_tx: &mpsc::Sender<AgentOutput>,
) -> io::Result<Option<String>> {
    let program = resolve_program(AGY_BINARY_OVERRIDE, "agy.exe", "agy");
    let mut command = headless_command(program);
    configure_command(&mut command, request, selected_model.as_deref(), mode);
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

    let context = OutputContext::from(request);
    send_event(event_tx, Some(&context), AgentEvent::Started).await;
    let prompt = prompt_with_image_tokens(&request.prompt, &request.image_paths);
    let mut input = serde_json::to_vec(&json!({
        "event": "user",
        "message": { "content": agy_prompt(mode, &prompt) }
    }))
    .map_err(io::Error::other)?;
    input.push(b'\n');
    stdin.write_all(&input).await?;
    stdin.shutdown().await?;
    drop(stdin);

    stream_child(&mut child, stdout, stderr, event_tx, &context, mode).await
}

fn agy_prompt(mode: ExecutionMode, prompt: &str) -> String {
    match mode {
        ExecutionMode::Ask => format!(
            "Answer the question only. Do not modify files or execute implementation steps.\n\n{prompt}"
        ),
        ExecutionMode::Plan => format!(
            "Create a plan only. Do not modify files or execute implementation steps.\n\n{prompt}"
        ),
        ExecutionMode::Implement => format!(
            "Run every command in the foreground with a bounded timeout. Do not start background or detached processes.\n\n{prompt}"
        ),
    }
}

fn configure_command(
    command: &mut tokio::process::Command,
    request: &AgentRequest,
    selected_model: Option<&str>,
    mode: ExecutionMode,
) {
    command.args([
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
    ]);
    match mode {
        ExecutionMode::Ask | ExecutionMode::Plan => {
            command.args(["--mode", "plan", "--sandbox"]);
        }
        ExecutionMode::Implement => {
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
    if let Some(conversation_id) = request.resume_session.as_deref() {
        command.arg("--conversation").arg(conversation_id);
    }
}

async fn stream_child(
    child: &mut tokio::process::Child,
    stdout: ChildStdout,
    stderr: ChildStderr,
    event_tx: &mpsc::Sender<AgentOutput>,
    context: &OutputContext,
    mode: ExecutionMode,
) -> io::Result<Option<String>> {
    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut stderr_lines = BufReader::new(stderr).lines();
    let mut stderr_history = VecDeque::with_capacity(STDERR_HISTORY_CAPACITY);
    let mut stdout_open = true;
    let mut stderr_open = true;
    let mut exit_status = None;
    let mut state = AgyStreamState::default();
    let mut implement_tracker = ImplementTurnTracker::new(mode);

    let idle_secs = agy_idle_timeout_secs();
    let mut idle_deadline = idle_secs.map(|secs| Instant::now() + Duration::from_secs(secs));

    while stdout_open || stderr_open || exit_status.is_none() {
        let deadline = idle_deadline.unwrap_or_else(Instant::now);
        tokio::select! {
            line = stdout_lines.next_line(), if stdout_open => {
                match line? {
                    Some(line) => {
                        if let Some(secs) = idle_secs {
                            idle_deadline = Some(Instant::now() + Duration::from_secs(secs));
                        }
                        implement_tracker.observe_provider_line(&line);
                        let message: Value = serde_json::from_str(&line).map_err(|error| {
                            io::Error::other(format!("invalid AGY NDJSON: {error}"))
                        })?;
                        for event in normalize_event(&message, mode, &mut state) {
                            send_event(event_tx, Some(context), event).await;
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
            _ = tokio::time::sleep_until(deadline), if idle_deadline.is_some() => {
                child.kill().await.ok();
                return Err(io::Error::other(format!(
                    "AGY produced no events for {}s; it may be waiting on a background command. Run commands in the foreground with a bounded timeout.",
                    idle_secs.unwrap_or(AGY_IDLE_TIMEOUT_DEFAULT_SECS)
                )));
            }
        }
    }

    let status = exit_status.expect("child.wait runs before the loop can finish");
    let diagnostic = stderr_history.into_iter().collect::<Vec<_>>().join(" | ");
    if let Err(reason) = implement_tracker.finish(&status, &diagnostic, state.terminal) {
        return Err(io::Error::other(reason));
    }
    if !state.terminal {
        return Err(io::Error::other(if diagnostic.is_empty() {
            format!("AGY exited with {status} without a terminal result")
        } else {
            format!("AGY exited with {status}: {diagnostic}")
        }));
    }
    Ok(state.conversation_id)
}

#[derive(Default)]
struct AgyStreamState {
    conversation_id: Option<String>,
    emitted_text: bool,
    terminal: bool,
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
    mode: ExecutionMode,
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
            state.terminal = true;
            let status = message
                .pointer("/result/status")
                .and_then(Value::as_str)
                .unwrap_or("ERROR");
            match status {
                "SUCCESS" => {
                    if let Some(denied) = denied_actions_summary(message) {
                        events.push(AgentEvent::Error(format!(
                            "AGY denied permission for: {denied}. Headless AGY cannot prompt; allow it in AGY settings or use Implement mode."
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
                        events.push(AgentEvent::Error(
                            "AGY finished without producing any output.".to_owned(),
                        ));
                        return events;
                    }
                    if is_plan_review_response(&state.response_text) {
                        if mode == ExecutionMode::Implement {
                            events.push(AgentEvent::Error(
                                "AGY returned a plan for review instead of applying edits in Implement mode."
                                    .to_owned(),
                            ));
                            return events;
                        }
                        events.push(AgentEvent::Notice(
                            "AGY returned a plan for review; no file changes were executed (plan+sandbox mode)."
                                .to_owned(),
                        ));
                    }
                    events.push(AgentEvent::Completed);
                }
                "CANCELED" | "INTERRUPTED" => events.push(AgentEvent::Cancelled),
                _ => events.push(AgentEvent::Error(
                    message
                        .pointer("/result/error")
                        .and_then(Value::as_str)
                        .unwrap_or("AGY turn failed")
                        .to_owned(),
                )),
            }
        }
        _ => {}
    }
    events
}

fn is_plan_review_response(response: &str) -> bool {
    let response = response.to_ascii_lowercase();
    let mentions_plan = response.contains("plan.md")
        || response.contains("implementation plan")
        || response.contains("plan artifact");
    let requests_review =
        response.contains("approve") || response.contains("review") || response.contains("proceed");
    mentions_plan && requests_review
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
            normalize_event(&delta, ExecutionMode::Ask, &mut state),
            vec![
                AgentEvent::ProviderSessionBound("conversation".to_owned()),
                AgentEvent::TextDelta("AGY_OK".to_owned()),
            ]
        );
        assert_eq!(
            normalize_event(&result, ExecutionMode::Ask, &mut state),
            vec![AgentEvent::Completed]
        );
        assert_eq!(state.conversation_id.as_deref(), Some("conversation"));
    }

    #[test]
    fn agy_resume_session_is_passed_to_the_command() {
        let request = AgentRequest {
            conversation_id: super::super::ConversationId::new("conversation"),
            turn_id: super::super::TurnId::new("turn"),
            prompt: "hello".to_owned(),
            resume_session: Some("agy-conversation".to_owned()),
            image_paths: vec![],
        };
        let mut command = tokio::process::Command::new("agy");

        configure_command(
            &mut command,
            &request,
            Some("agy-model"),
            ExecutionMode::Ask,
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
        };
        let mut command = tokio::process::Command::new("agy");
        configure_command(&mut command, &request, None, ExecutionMode::Ask);
        let args = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(!args.iter().any(|arg| arg == "--image"));
        let prompt = prompt_with_image_tokens(&request.prompt, &request.image_paths);
        assert_eq!(prompt, "describe @/tmp/shot.png @/tmp/other.png");
        assert!(agy_prompt(ExecutionMode::Ask, &prompt).ends_with(&prompt));
    }

    #[test]
    fn agy_execution_modes_use_native_cli_flags_and_safe_prompts() {
        let request = AgentRequest {
            conversation_id: super::super::ConversationId::new("conversation"),
            turn_id: super::super::TurnId::new("turn"),
            prompt: "hello".to_owned(),
            resume_session: None,
            image_paths: vec![],
        };

        let args_for = |mode| {
            let mut command = tokio::process::Command::new("agy");
            configure_command(&mut command, &request, None, mode);
            command
                .as_std()
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };

        let ask = args_for(ExecutionMode::Ask);
        assert!(ask.windows(2).any(|args| args == ["--mode", "plan"]));
        assert!(agy_prompt(ExecutionMode::Ask, "hello").starts_with("Answer"));
        let plan = args_for(ExecutionMode::Plan);
        assert!(plan.windows(2).any(|args| args == ["--mode", "plan"]));
        assert!(agy_prompt(ExecutionMode::Plan, "hello").starts_with("Create a plan"));
        let implement = args_for(ExecutionMode::Implement);
        assert!(
            implement
                .windows(2)
                .any(|args| args == ["--mode", "accept-edits"])
        );
        assert!(implement.iter().any(|arg| arg == "--sandbox"));
        assert!(agy_prompt(ExecutionMode::Implement, "hello").ends_with("\n\nhello"));
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
            normalize_event(&result, ExecutionMode::Plan, &mut state),
            vec![
                AgentEvent::TextDelta(
                    "I created the implementation plan artifact at plan.md. Please review and approve the plan to proceed.".to_owned()
                ),
                AgentEvent::Notice(
                    "AGY returned a plan for review; no file changes were executed (plan+sandbox mode)."
                        .to_owned()
                ),
                AgentEvent::Completed,
            ]
        );
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
        };
        let turn = tokio::spawn(async move {
            run_turn(&request, None, ExecutionMode::Ask, &cwd, &event_tx).await
        });

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
                    | AgentEvent::ModeSelected(_)
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
        let events = normalize_event(&denied, ExecutionMode::Implement, &mut state);
        assert!(matches!(events.last(), Some(AgentEvent::Error(e)) if e.contains("RunCommand")));

        let mut state = AgyStreamState::default();
        let empty = json!({"event":"result","result":{"status":"SUCCESS","response":""}});
        let events = normalize_event(&empty, ExecutionMode::Ask, &mut state);
        assert!(
            matches!(events.last(), Some(AgentEvent::Error(e)) if e.contains("without producing"))
        );
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
        let events = normalize_event(&tool_update, ExecutionMode::Implement, &mut state);
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
        let events = normalize_event(&thinking_update, ExecutionMode::Implement, &mut state);
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
        // FR-443 / TC-443: Implement mode must instruct AGY to keep commands in
        // the foreground with a bounded timeout and to avoid background or
        // detached processes; Ask/Plan stay untouched.
        let wrapped = agy_prompt(ExecutionMode::Implement, "build the thing");
        assert!(
            wrapped.starts_with("Run every command in the foreground"),
            "unexpected Implement prompt: {wrapped}"
        );
        assert!(
            wrapped.contains("background"),
            "unexpected Implement prompt: {wrapped}"
        );
        assert!(wrapped.ends_with("\n\nbuild the thing"));
        assert!(!agy_prompt(ExecutionMode::Ask, "hi").contains("background"));
        assert!(!agy_prompt(ExecutionMode::Plan, "hi").contains("background"));
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
    async fn run_fake_agy_turn(mode: ExecutionMode) -> io::Result<Option<String>> {
        let request = AgentRequest {
            conversation_id: super::super::ConversationId::new("fake-conversation"),
            turn_id: super::super::TurnId::new("fake-turn"),
            prompt: "hello".to_owned(),
            resume_session: None,
            image_paths: vec![],
        };
        let (event_tx, _event_rx) = mpsc::channel(64);
        let cwd = std::env::temp_dir();
        run_turn(&request, None, mode, &cwd, &event_tx).await
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
                run_fake_agy_turn(ExecutionMode::Implement),
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
                "#!/bin/sh\nprintf '%s\\n' '{}'\nsleep 0.7\nprintf '%s\\n' '{}'\nsleep 0.7\nprintf '%s\\n' '{}'\nsleep 0.7\nprintf '%s\\n' '{}'\n",
                step("one"),
                step("two"),
                step("three"),
                result_event
            )
        });

        let outcome = with_agy_spawn_env(&fake, "2", async {
            timeout(
                Duration::from_secs(20),
                run_fake_agy_turn(ExecutionMode::Ask),
            )
            .await
            .expect("a continuously streaming child should finish promptly")
        })
        .await;

        outcome.expect("stdout events within the timeout should keep the turn alive");
    }
}
