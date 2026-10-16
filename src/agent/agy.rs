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
    AdapterError, Agent, AgentAdapter, AgentEvent, AgentOutput, AgentRequest, ExecutionMode,
    ModelInfo, OutputContext, SpawnedAdapter, implement_outcome::ImplementTurnTracker,
};
use crate::process::{headless_command, resolve_program};

const COMMAND_CHANNEL_CAPACITY: usize = 8;
const STDERR_HISTORY_CAPACITY: usize = 8;
const AGY_BINARY_OVERRIDE: &str = "DMUXAI_AGY_BIN";

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
                AgyCommand::ListModels => match list_models(&cwd).await {
                    Ok(models) => {
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
    let mut input = serde_json::to_vec(&json!({
        "event": "user",
        "message": { "content": agy_prompt(mode, &request.prompt) }
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
        ExecutionMode::Implement => prompt.to_owned(),
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
            command.args(["--mode", "accept-edits", "--sandbox"]);
        }
    }
    if let Some(model) = selected_model {
        command.arg("--model").arg(model);
    }
    if let Some(conversation_id) = request.resume_session.as_deref() {
        command.arg("--conversation").arg(conversation_id);
    }
    for path in &request.image_paths {
        command.arg("--image").arg(path);
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

    while stdout_open || stderr_open || exit_status.is_none() {
        tokio::select! {
            line = stdout_lines.next_line(), if stdout_open => {
                match line? {
                    Some(line) => {
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
        Some("result") => {
            state.terminal = true;
            let status = message
                .pointer("/result/status")
                .and_then(Value::as_str)
                .unwrap_or("ERROR");
            match status {
                "SUCCESS" => {
                    if !state.emitted_text
                        && let Some(response) =
                            message.pointer("/result/response").and_then(Value::as_str)
                        && !response.is_empty()
                    {
                        state.response_text.push_str(response);
                        events.push(AgentEvent::TextDelta(response.to_owned()));
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
        assert_eq!(agy_prompt(ExecutionMode::Implement, "hello"), "hello");
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
}
