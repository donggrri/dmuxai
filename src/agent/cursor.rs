use std::{
    collections::VecDeque,
    io,
    path::{Path, PathBuf},
    process::Stdio,
};

use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStderr, ChildStdout},
    sync::mpsc,
    task::JoinHandle,
};

use super::{
    AdapterError, Agent, AgentAdapter, AgentEvent, AgentOutput, AgentRequest, ExecutionMode,
    ModelInfo, OutputContext, SpawnedAdapter, implement_outcome::ImplementTurnTracker,
    prompt_with_image_tokens,
};
use crate::process::cursor_command;
use dmuxai::settings::DmuxSettings;

const COMMAND_CHANNEL_CAPACITY: usize = 8;
const STDERR_HISTORY_CAPACITY: usize = 8;

enum CursorCommand {
    Send(AgentRequest),
    Cancel,
    ListModels,
    SelectModel(String),
    SelectMode(ExecutionMode),
}

pub struct CursorAdapter {
    command_tx: mpsc::Sender<CursorCommand>,
    windows_unsandboxed: bool,
}

impl AgentAdapter for CursorAdapter {
    fn agent(&self) -> Agent {
        Agent::Cursor
    }

    fn send(&self, request: AgentRequest) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(CursorCommand::Send(request))
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

    fn validate_mode(&self, mode: ExecutionMode) -> Result<(), AdapterError> {
        validate_cursor_mode(mode, self.windows_unsandboxed)
    }

    fn select_mode(&self, mode: ExecutionMode) -> Result<(), AdapterError> {
        validate_cursor_mode(mode, self.windows_unsandboxed)?;
        self.command_tx
            .try_send(CursorCommand::SelectMode(mode))
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
    settings: DmuxSettings,
    mcp_servers: Vec<dmuxai::mcp::ResolvedMcpServer>,
) -> SpawnedAdapter {
    let windows_unsandboxed = settings.cursor_windows_unsandboxed;
    let (command_tx, command_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
    let worker = tokio::spawn(runtime_loop(
        command_rx,
        event_tx,
        cwd,
        windows_unsandboxed,
        mcp_servers,
    ));
    (
        Box::new(CursorAdapter {
            command_tx,
            windows_unsandboxed,
        }),
        worker,
    )
}

async fn runtime_loop(
    mut command_rx: mpsc::Receiver<CursorCommand>,
    event_tx: mpsc::Sender<AgentOutput>,
    cwd: PathBuf,
    windows_unsandboxed: bool,
    mcp_servers: Vec<dmuxai::mcp::ResolvedMcpServer>,
) {
    let applicable: Vec<dmuxai::mcp::ResolvedMcpServer> = mcp_servers
        .into_iter()
        .filter(|s| s.applies_to("cursor"))
        .collect();
    let cursor_notice_emitted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut active: Option<ActiveTurn> = None;
    let mut selected_model: Option<String> = Some(dmuxai::model_policy::ALLOWED_MODEL.to_owned());
    let mut selected_mode = ExecutionMode::default();

    loop {
        if active.is_none() {
            let Some(command) = command_rx.recv().await else {
                break;
            };
            match command {
                CursorCommand::Send(request) => {
                    if let Err(error) = validate_cursor_mode(selected_mode, windows_unsandboxed) {
                        let context = OutputContext::from(&request);
                        send_event(
                            &event_tx,
                            Some(&context),
                            AgentEvent::Error(error.to_string()),
                        )
                        .await;
                    } else {
                        active = Some(spawn_turn(
                            request,
                            selected_model.clone(),
                            selected_mode,
                            cwd.clone(),
                            event_tx.clone(),
                            windows_unsandboxed,
                            applicable.clone(),
                            cursor_notice_emitted.clone(),
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
                        send_event(&event_tx, None, AgentEvent::Error(error.to_string())).await;
                    }
                },
                CursorCommand::SelectModel(model) => {
                    let effective =
                        match dmuxai::model_policy::validate_model_for_runner("cursor", &model) {
                            Ok(()) => model,
                            Err(_) => dmuxai::model_policy::ALLOWED_MODEL.to_owned(),
                        };
                    selected_model = Some(effective.clone());
                    send_event(&event_tx, None, AgentEvent::ModelSelected(effective)).await;
                }
                CursorCommand::SelectMode(mode) => {
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
                    Some(CursorCommand::Cancel) => {
                        stop_turn(active.take(), &event_tx).await;
                    }
                    Some(CursorCommand::Send(request)) => {
                        let context = OutputContext::from(&request);
                        send_event(
                            &event_tx,
                            Some(&context),
                            AgentEvent::Error(
                                "Cursor is already running a turn; cancel it before sending another prompt"
                                    .to_owned(),
                            ),
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
                    Some(CursorCommand::SelectMode(mode)) => {
                        selected_mode = mode;
                        send_event(&event_tx, None, AgentEvent::ModeSelected(mode)).await;
                        send_event(
                            &event_tx,
                            None,
                            AgentEvent::Notice(
                                "Cursor mode changed; it will apply to the next turn".to_owned(),
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

#[allow(clippy::too_many_arguments)]
fn spawn_turn(
    request: AgentRequest,
    selected_model: Option<String>,
    mode: ExecutionMode,
    cwd: PathBuf,
    event_tx: mpsc::Sender<AgentOutput>,
    windows_unsandboxed: bool,
    mcp_servers: Vec<dmuxai::mcp::ResolvedMcpServer>,
    cursor_notice_emitted: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> ActiveTurn {
    let context = OutputContext::from(&request);
    let error_context = context.clone();
    let handle = tokio::spawn(async move {
        if let Err(error) = run_turn(
            &request,
            selected_model,
            mode,
            &cwd,
            &event_tx,
            windows_unsandboxed,
            &mcp_servers,
            &cursor_notice_emitted,
        )
        .await
        {
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

#[allow(clippy::too_many_arguments)]
async fn run_turn(
    request: &AgentRequest,
    selected_model: Option<String>,
    mode: ExecutionMode,
    cwd: &Path,
    event_tx: &mpsc::Sender<AgentOutput>,
    windows_unsandboxed: bool,
    mcp_servers: &[dmuxai::mcp::ResolvedMcpServer],
    cursor_notice_emitted: &std::sync::atomic::AtomicBool,
) -> io::Result<Option<String>> {
    validate_cursor_mode(mode, windows_unsandboxed)
        .map_err(|error| io::Error::other(error.to_string()))?;

    let context = OutputContext::from(request);
    let _guard = if !mcp_servers.is_empty() {
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

    let mut command = cursor_command()?;
    configure_command(
        &mut command,
        request,
        selected_model.as_deref(),
        mode,
        cwd,
        windows_unsandboxed,
    );
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
        .ok_or_else(|| io::Error::other("Cursor stdin was not piped"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("Cursor stdout was not piped"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("Cursor stderr was not piped"))?;

    let context = OutputContext::from(request);
    if mode == ExecutionMode::Implement && cfg!(windows) && windows_unsandboxed {
        send_event(
            event_tx,
            Some(&context),
            AgentEvent::Notice(
                "Warning: Cursor Implement is running without sandbox on Windows (opt-in). The agent can modify files outside the workspace sandbox.".to_owned(),
            ),
        )
        .await;
    }
    send_event(event_tx, Some(&context), AgentEvent::Started).await;
    let prompt = prompt_with_image_tokens(&request.prompt, &request.image_paths);
    stdin.write_all(prompt.as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.shutdown().await?;
    drop(stdin);

    stream_child(&mut child, stdout, stderr, event_tx, &context, mode).await
}

fn configure_command(
    command: &mut tokio::process::Command,
    request: &AgentRequest,
    selected_model: Option<&str>,
    mode: ExecutionMode,
    cwd: &Path,
    windows_unsandboxed: bool,
) {
    command.arg("--print");
    match mode {
        ExecutionMode::Ask => {
            command.args(["--mode", "ask"]);
        }
        ExecutionMode::Plan => {
            command.args(["--mode", "plan"]);
        }
        ExecutionMode::Implement if cfg!(windows) && windows_unsandboxed => {
            command.arg("--force");
        }
        ExecutionMode::Implement => {
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
    if let Some(session_id) = request.resume_session.as_deref() {
        command.arg("--resume").arg(session_id);
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
    let mut state = CursorStreamState::default();
    let mut implement_tracker = ImplementTurnTracker::new(mode);

    while stdout_open || stderr_open || exit_status.is_none() {
        tokio::select! {
            line = stdout_lines.next_line(), if stdout_open => {
                match line? {
                    Some(line) => {
                        implement_tracker.observe_provider_line(&line);
                        let message: Value = serde_json::from_str(&line).map_err(|error| {
                            io::Error::other(format!("invalid Cursor NDJSON: {error}"))
                        })?;
                        for event in normalize_event(&message, &mut state) {
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
        }
    }

    let status = exit_status.expect("child.wait runs before the loop can finish");
    let diagnostic = stderr_history.into_iter().collect::<Vec<_>>().join(" | ");
    if let Err(reason) = implement_tracker.finish(
        &status,
        &diagnostic,
        state.terminal,
        mode == ExecutionMode::Implement,
        false,
    ) {
        return Err(io::Error::other(reason));
    }
    if !state.terminal {
        return Err(io::Error::other(if diagnostic.is_empty() {
            format!("Cursor exited with {status} without a terminal result")
        } else {
            format!("Cursor exited with {status}: {diagnostic}")
        }));
    }
    Ok(state.session_id)
}

fn validate_cursor_mode(
    mode: ExecutionMode,
    windows_unsandboxed: bool,
) -> Result<(), AdapterError> {
    if mode == ExecutionMode::Implement && cfg!(windows) && !windows_unsandboxed {
        return Err(AdapterError::UnsupportedMode {
            agent: Agent::Cursor,
            mode,
        });
    }
    Ok(())
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
                events.push(AgentEvent::Error(
                    message
                        .get("result")
                        .and_then(Value::as_str)
                        .unwrap_or("Cursor turn failed")
                        .to_owned(),
                ));
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
mod tests {
    use serde_json::json;
    use tokio::time::{Duration, timeout};

    use super::*;

    #[test]
    // TC-480, TC-483 / INT-480, INT-481: only Windows Cursor Implement is blocked.
    fn cursor_mode_validation_matches_platform() {
        assert_eq!(validate_cursor_mode(ExecutionMode::Ask, false), Ok(()));
        assert_eq!(validate_cursor_mode(ExecutionMode::Plan, false), Ok(()));
        assert_eq!(
            validate_cursor_mode(ExecutionMode::Implement, false).is_ok(),
            !cfg!(windows)
        );
        assert_eq!(validate_cursor_mode(ExecutionMode::Implement, true), Ok(()));
    }

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
            matches!(events.as_slice(), [AgentEvent::TextDelta(text), AgentEvent::Error(error)] if text == "Partial" && error == "boom")
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
        };
        let mut command = tokio::process::Command::new("cursor-agent");

        configure_command(
            &mut command,
            &request,
            Some("composer-2.5"),
            ExecutionMode::Ask,
            cwd,
            false,
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
        };
        let mut command = tokio::process::Command::new("cursor-agent");
        configure_command(&mut command, &request, None, ExecutionMode::Ask, cwd, false);

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
        };

        for disallowed in ["composer-2.5-fast", "claude-3-5-sonnet-fast", "gpt-5", ""] {
            let mut command = tokio::process::Command::new("cursor-agent");
            configure_command(
                &mut command,
                &request,
                Some(disallowed),
                ExecutionMode::Ask,
                cwd,
                false,
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
        };
        let mut command = tokio::process::Command::new("cursor-agent");
        configure_command(&mut command, &request, None, ExecutionMode::Ask, cwd, false);
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
        };
        let cwd = Path::new("workspace");

        let args_for = |mode| {
            let mut command = tokio::process::Command::new("cursor-agent");
            configure_command(&mut command, &request, None, mode, cwd, false);
            command
                .as_std()
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };

        let ask = args_for(ExecutionMode::Ask);
        assert!(ask.windows(2).any(|args| args == ["--mode", "ask"]));
        let plan = args_for(ExecutionMode::Plan);
        assert!(plan.windows(2).any(|args| args == ["--mode", "plan"]));
        let implement = args_for(ExecutionMode::Implement);
        assert!(implement.iter().any(|arg| arg == "--force"));
        assert!(
            implement
                .windows(2)
                .any(|args| args == ["--sandbox", "enabled"])
        );
        assert!(!implement.iter().any(|arg| arg == "--mode"));
    }

    #[tokio::test]
    async fn selecting_mode_is_acknowledged_without_starting_a_process() {
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let (adapter, worker) = spawn_cursor_adapter(
            event_tx,
            PathBuf::from("workspace"),
            dmuxai::settings::DmuxSettings::default(),
            Vec::new(),
        );

        adapter.select_mode(ExecutionMode::Plan).unwrap();

        let output = timeout(Duration::from_secs(1), event_rx.recv())
            .await
            .expect("mode selection should be acknowledged")
            .expect("runtime should remain connected");
        assert_eq!(output.event, AgentEvent::ModeSelected(ExecutionMode::Plan));
        drop(adapter);
        worker.await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    // TC-480 / INT-480: Windows rejects Cursor Implement before queueing a mode change.
    async fn windows_cursor_implement_mode_is_rejected_before_queueing() {
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let (adapter, worker) = spawn_cursor_adapter(
            event_tx,
            PathBuf::from("workspace"),
            dmuxai::settings::DmuxSettings::default(),
            Vec::new(),
        );
        let expected = AdapterError::UnsupportedMode {
            agent: Agent::Cursor,
            mode: ExecutionMode::Implement,
        };

        assert_eq!(
            adapter.validate_mode(ExecutionMode::Implement),
            Err(expected)
        );
        assert_eq!(adapter.select_mode(ExecutionMode::Implement), Err(expected));
        assert!(
            timeout(Duration::from_millis(100), event_rx.recv())
                .await
                .is_err()
        );

        drop(adapter);
        worker.await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    // TC-481 / NFR-480: the runtime guard runs before Cursor command lookup or spawning.
    async fn windows_cursor_implement_turn_is_rejected_before_process_spawn() {
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let request = AgentRequest {
            conversation_id: super::super::ConversationId::new("conversation"),
            turn_id: super::super::TurnId::new("turn"),
            prompt: "hello".to_owned(),
            resume_session: None,
            image_paths: vec![],
        };

        let error = run_turn(
            &request,
            None,
            ExecutionMode::Implement,
            Path::new("missing-workspace"),
            &event_tx,
            false,
            &[],
            &std::sync::atomic::AtomicBool::new(false),
        )
        .await
        .expect_err("Windows Cursor Implement must be blocked");

        assert_eq!(
            error.to_string(),
            AdapterError::UnsupportedMode {
                agent: Agent::Cursor,
                mode: ExecutionMode::Implement,
            }
            .to_string()
        );
        assert!(event_rx.try_recv().is_err());
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
        };
        let turn = tokio::spawn(async move {
            run_turn(
                &request,
                Some("composer-2.5".to_owned()),
                ExecutionMode::Ask,
                &cwd,
                &event_tx,
                false,
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
        .expect("Cursor should complete within 90 seconds");

        let session = turn
            .await
            .unwrap()
            .expect("real Cursor turn should not return an I/O error")
            .expect("real Cursor turn should return a session ID");
        assert!(!session.is_empty());
        assert!(answer.contains("CURSOR_OK"), "unexpected answer: {answer}");
    }

    #[tokio::test]
    async fn cursor_mcp_notice_once_per_session() {
        // TC-578, INT-561: Cursor runner with an existing user .cursor/mcp.json; two turns.
        // The "not propagated" notice is emitted once per session; the user file is untouched.
        use dmuxai::mcp::config::{ResolvedMcpServer, ServerKind, ToolNames, Transport};
        use std::collections::BTreeMap;
        use std::sync::atomic::AtomicBool;

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
        };

        // Override cursor binary so the test does not spawn a live external cursor-agent CLI
        unsafe {
            std::env::set_var("DMUXAI_CURSOR_BIN", "nonexistent-cursor-bin");
        }

        // Turn 1
        let _ = run_turn(
            &req1,
            None,
            ExecutionMode::Plan,
            temp.path(),
            &event_tx,
            false,
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
        };

        // Turn 2
        let _ = run_turn(
            &req2,
            None,
            ExecutionMode::Plan,
            temp.path(),
            &event_tx,
            false,
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
}
