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
};

use super::{
    AdapterError, Agent, AgentAdapter, AgentEvent, AgentOutput, ModelInfo, SpawnedAdapter,
};
use crate::process::{headless_command, resolve_program};

const COMMAND_CHANNEL_CAPACITY: usize = 8;
const STDERR_HISTORY_CAPACITY: usize = 8;
const AGY_BINARY_OVERRIDE: &str = "DMUXAI_AGY_BIN";

enum AgyCommand {
    Send(String),
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

    fn send(&self, prompt: String) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(AgyCommand::Send(prompt))
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
    let mut active: Option<JoinHandle<Option<String>>> = None;
    let mut conversation_id: Option<String> = None;
    let mut selected_model: Option<String> = None;

    loop {
        if active.is_none() {
            let Some(command) = command_rx.recv().await else {
                break;
            };
            match command {
                AgyCommand::Send(prompt) => {
                    active = Some(spawn_turn(
                        prompt,
                        conversation_id.clone(),
                        selected_model.clone(),
                        cwd.clone(),
                        event_tx.clone(),
                    ));
                }
                AgyCommand::Cancel => {}
                AgyCommand::ListModels => match list_models(&cwd).await {
                    Ok(models) => {
                        send_event(
                            &event_tx,
                            AgentEvent::ModelsListed {
                                models,
                                selected: selected_model.clone(),
                            },
                        )
                        .await;
                    }
                    Err(error) => {
                        send_event(&event_tx, AgentEvent::Error(error.to_string())).await;
                    }
                },
                AgyCommand::SelectModel(model) => {
                    selected_model = Some(model.clone());
                    send_event(&event_tx, AgentEvent::ModelSelected(model)).await;
                }
            }
            continue;
        }

        let turn = active.as_mut().expect("checked above");
        tokio::select! {
            outcome = turn => {
                if let Ok(Some(updated_conversation)) = outcome {
                    conversation_id = Some(updated_conversation);
                }
                active = None;
            }
            command = command_rx.recv() => {
                match command {
                    Some(AgyCommand::Cancel) => {
                        stop_turn(active.take(), &event_tx).await;
                    }
                    Some(AgyCommand::Send(_)) => {
                        send_event(
                            &event_tx,
                            AgentEvent::Notice(
                                "AGY is already running a turn; cancel it before sending another prompt"
                                    .to_owned(),
                            ),
                        ).await;
                    }
                    Some(AgyCommand::ListModels) => {
                        send_event(
                            &event_tx,
                            AgentEvent::Notice(
                                "finish or cancel the AGY turn before loading models".to_owned(),
                            ),
                        ).await;
                    }
                    Some(AgyCommand::SelectModel(_)) => {
                        send_event(
                            &event_tx,
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
    prompt: String,
    conversation_id: Option<String>,
    selected_model: Option<String>,
    cwd: PathBuf,
    event_tx: mpsc::Sender<AgentOutput>,
) -> JoinHandle<Option<String>> {
    tokio::spawn(async move {
        match run_turn(prompt, conversation_id, selected_model, &cwd, &event_tx).await {
            Ok(conversation_id) => conversation_id,
            Err(error) => {
                send_event(&event_tx, AgentEvent::Error(error.to_string())).await;
                None
            }
        }
    })
}

async fn stop_turn(turn: Option<JoinHandle<Option<String>>>, event_tx: &mpsc::Sender<AgentOutput>) {
    if let Some(turn) = turn {
        turn.abort();
        let _ = turn.await;
        send_event(event_tx, AgentEvent::Cancelled).await;
    }
}

async fn run_turn(
    prompt: String,
    conversation_id: Option<String>,
    selected_model: Option<String>,
    cwd: &Path,
    event_tx: &mpsc::Sender<AgentOutput>,
) -> io::Result<Option<String>> {
    let program = resolve_program(AGY_BINARY_OVERRIDE, "agy.exe", "agy");
    let mut command = headless_command(program);
    command.args([
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--mode",
        "plan",
        "--sandbox",
    ]);
    if let Some(model) = selected_model {
        command.arg("--model").arg(model);
    }
    if let Some(conversation_id) = conversation_id {
        command.arg("--conversation").arg(conversation_id);
    }
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

    send_event(event_tx, AgentEvent::Started).await;
    let mut input = serde_json::to_vec(&json!({
        "event": "user",
        "message": { "content": prompt }
    }))
    .map_err(io::Error::other)?;
    input.push(b'\n');
    stdin.write_all(&input).await?;
    stdin.shutdown().await?;
    drop(stdin);

    stream_child(&mut child, stdout, stderr, event_tx).await
}

async fn stream_child(
    child: &mut tokio::process::Child,
    stdout: ChildStdout,
    stderr: ChildStderr,
    event_tx: &mpsc::Sender<AgentOutput>,
) -> io::Result<Option<String>> {
    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut stderr_lines = BufReader::new(stderr).lines();
    let mut stderr_history = VecDeque::with_capacity(STDERR_HISTORY_CAPACITY);
    let mut stdout_open = true;
    let mut stderr_open = true;
    let mut exit_status = None;
    let mut state = AgyStreamState::default();

    while stdout_open || stderr_open || exit_status.is_none() {
        tokio::select! {
            line = stdout_lines.next_line(), if stdout_open => {
                match line? {
                    Some(line) => {
                        let message: Value = serde_json::from_str(&line).map_err(|error| {
                            io::Error::other(format!("invalid AGY NDJSON: {error}"))
                        })?;
                        for event in normalize_event(&message, &mut state) {
                            send_event(event_tx, event).await;
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
}

fn normalize_event(message: &Value, state: &mut AgyStreamState) -> Vec<AgentEvent> {
    if let Some(conversation_id) = message
        .get("conversation_id")
        .or_else(|| message.pointer("/result/conversation_id"))
        .or_else(|| message.pointer("/step_update/conversation_id"))
        .and_then(Value::as_str)
    {
        state.conversation_id = Some(conversation_id.to_owned());
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
            if text.is_empty() {
                Vec::new()
            } else {
                state.emitted_text = true;
                vec![AgentEvent::TextDelta(text.to_owned())]
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
                    let mut events = Vec::new();
                    if !state.emitted_text
                        && let Some(response) =
                            message.pointer("/result/response").and_then(Value::as_str)
                        && !response.is_empty()
                    {
                        events.push(AgentEvent::TextDelta(response.to_owned()));
                    }
                    events.push(AgentEvent::Completed);
                    events
                }
                "CANCELED" | "INTERRUPTED" => vec![AgentEvent::Cancelled],
                _ => vec![AgentEvent::Error(
                    message
                        .pointer("/result/error")
                        .and_then(Value::as_str)
                        .unwrap_or("AGY turn failed")
                        .to_owned(),
                )],
            }
        }
        _ => Vec::new(),
    }
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

async fn send_event(event_tx: &mpsc::Sender<AgentOutput>, event: AgentEvent) {
    let _ = event_tx
        .send(AgentOutput {
            agent: Agent::Agy,
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
            normalize_event(&delta, &mut state),
            vec![AgentEvent::TextDelta("AGY_OK".to_owned())]
        );
        assert_eq!(
            normalize_event(&result, &mut state),
            vec![AgentEvent::Completed]
        );
        assert_eq!(state.conversation_id.as_deref(), Some("conversation"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a locally installed and authenticated AGY CLI"]
    async fn real_agy_turn_streams_to_completion() {
        let (event_tx, mut event_rx) = mpsc::channel(128);
        let cwd = std::env::current_dir().unwrap();
        let turn = tokio::spawn(async move {
            run_turn(
                "Reply with exactly AGY_OK. Do not use tools or modify files.".to_owned(),
                None,
                None,
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
                    AgentEvent::Error(error) => panic!("AGY turn failed: {error}"),
                    AgentEvent::Started
                    | AgentEvent::Notice(_)
                    | AgentEvent::ModelsListed { .. }
                    | AgentEvent::ModelSelected(_)
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
}
