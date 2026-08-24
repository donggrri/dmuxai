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
    ModelInfo, OutputContext, SpawnedAdapter,
};
use crate::process::cursor_command;

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

    fn select_mode(&self, mode: ExecutionMode) -> Result<(), AdapterError> {
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

pub fn spawn_cursor_adapter(event_tx: mpsc::Sender<AgentOutput>, cwd: PathBuf) -> SpawnedAdapter {
    let (command_tx, command_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
    let worker = tokio::spawn(runtime_loop(command_rx, event_tx, cwd));
    (Box::new(CursorAdapter { command_tx }), worker)
}

async fn runtime_loop(
    mut command_rx: mpsc::Receiver<CursorCommand>,
    event_tx: mpsc::Sender<AgentOutput>,
    cwd: PathBuf,
) {
    let mut active: Option<ActiveTurn> = None;
    let mut selected_model: Option<String> = None;
    let mut selected_mode = ExecutionMode::default();

    loop {
        if active.is_none() {
            let Some(command) = command_rx.recv().await else {
                break;
            };
            match command {
                CursorCommand::Send(request) => {
                    active = Some(spawn_turn(
                        request,
                        selected_model.clone(),
                        selected_mode,
                        cwd.clone(),
                        event_tx.clone(),
                    ));
                }
                CursorCommand::Cancel => {}
                CursorCommand::ListModels => match list_models(&cwd).await {
                    Ok((models, _advertised_default)) => {
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
                    }
                    Err(error) => {
                        send_event(&event_tx, None, AgentEvent::Error(error.to_string())).await;
                    }
                },
                CursorCommand::SelectModel(model) => {
                    selected_model = Some(model.clone());
                    send_event(&event_tx, None, AgentEvent::ModelSelected(model)).await;
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
                            AgentEvent::Notice(
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
    let mut command = cursor_command()?;
    configure_command(&mut command, request, selected_model.as_deref(), mode, cwd);
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
    send_event(event_tx, Some(&context), AgentEvent::Started).await;
    stdin.write_all(request.prompt.as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.shutdown().await?;
    drop(stdin);

    stream_child(&mut child, stdout, stderr, event_tx, &context).await
}

fn configure_command(
    command: &mut tokio::process::Command,
    request: &AgentRequest,
    selected_model: Option<&str>,
    mode: ExecutionMode,
    cwd: &Path,
) {
    command.arg("--print");
    match mode {
        ExecutionMode::Ask => {
            command.args(["--mode", "ask"]);
        }
        ExecutionMode::Plan => {
            command.args(["--mode", "plan"]);
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
    if let Some(model) = selected_model {
        command.arg("--model").arg(model);
    }
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
) -> io::Result<Option<String>> {
    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut stderr_lines = BufReader::new(stderr).lines();
    let mut stderr_history = VecDeque::with_capacity(STDERR_HISTORY_CAPACITY);
    let mut stdout_open = true;
    let mut stderr_open = true;
    let mut exit_status = None;
    let mut state = CursorStreamState::default();

    while stdout_open || stderr_open || exit_status.is_none() {
        tokio::select! {
            line = stdout_lines.next_line(), if stdout_open => {
                match line? {
                    Some(line) => {
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
    if !state.terminal {
        let diagnostic = stderr_history.into_iter().collect::<Vec<_>>().join(" | ");
        return Err(io::Error::other(if diagnostic.is_empty() {
            format!("Cursor exited with {status} without a terminal result")
        } else {
            format!("Cursor exited with {status}: {diagnostic}")
        }));
    }
    Ok(state.session_id)
}

#[derive(Default)]
struct CursorStreamState {
    session_id: Option<String>,
    emitted_text: bool,
    terminal: bool,
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
            let is_partial = message.get("timestamp_ms").is_some();
            if !text.is_empty() && (is_partial || !state.emitted_text) {
                state.emitted_text = true;
                events.push(AgentEvent::TextDelta(text));
            }
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

    #[test]
    fn cursor_resume_session_is_passed_to_the_command() {
        let cwd = Path::new("workspace");
        let request = AgentRequest {
            conversation_id: super::super::ConversationId::new("conversation"),
            turn_id: super::super::TurnId::new("turn"),
            prompt: "hello".to_owned(),
            resume_session: Some("cursor-session".to_owned()),
        };
        let mut command = tokio::process::Command::new("cursor-agent");

        configure_command(
            &mut command,
            &request,
            Some("cursor-model"),
            ExecutionMode::Ask,
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
                .any(|args| args == ["--model", "cursor-model"])
        );
    }

    #[test]
    fn cursor_execution_modes_use_native_cli_flags() {
        let request = AgentRequest {
            conversation_id: super::super::ConversationId::new("conversation"),
            turn_id: super::super::TurnId::new("turn"),
            prompt: "hello".to_owned(),
            resume_session: None,
        };
        let cwd = Path::new("workspace");

        let args_for = |mode| {
            let mut command = tokio::process::Command::new("cursor-agent");
            configure_command(&mut command, &request, None, mode, cwd);
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
        let (adapter, worker) = spawn_cursor_adapter(event_tx, PathBuf::from("workspace"));

        adapter.select_mode(ExecutionMode::Implement).unwrap();

        let output = timeout(Duration::from_secs(1), event_rx.recv())
            .await
            .expect("mode selection should be acknowledged")
            .expect("runtime should remain connected");
        assert_eq!(
            output.event,
            AgentEvent::ModeSelected(ExecutionMode::Implement)
        );
        drop(adapter);
        worker.await.unwrap();
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
        };
        let turn = tokio::spawn(async move {
            run_turn(
                &request,
                Some("cursor-grok-4.6-high".to_owned()),
                ExecutionMode::Ask,
                &cwd,
                &event_tx,
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
}
