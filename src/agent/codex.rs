use std::{
    io,
    path::{Path, PathBuf},
    process::Stdio,
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStderr, ChildStdin, ChildStdout},
    sync::mpsc,
    task::JoinHandle,
    time::{Duration, Instant, timeout_at},
};

use super::{
    AdapterError, Agent, AgentAdapter, AgentEvent, AgentOutput, ModelInfo, SpawnedAdapter,
};
use crate::process::{headless_command, resolve_program};

const COMMAND_CHANNEL_CAPACITY: usize = 8;
const RAW_MESSAGE_CHANNEL_CAPACITY: usize = 64;
const CODEX_BINARY_OVERRIDE: &str = "DMUXAI_CODEX_BIN";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

enum CodexCommand {
    Send(String),
    Cancel,
    ListModels,
    SelectModel(String),
}

pub struct CodexAdapter {
    command_tx: mpsc::Sender<CodexCommand>,
}

impl AgentAdapter for CodexAdapter {
    fn agent(&self) -> Agent {
        Agent::Codex
    }

    fn send(&self, prompt: String) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(CodexCommand::Send(prompt))
            .map_err(map_send_error)
    }

    fn cancel(&self) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(CodexCommand::Cancel)
            .map_err(map_send_error)
    }

    fn request_models(&self) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(CodexCommand::ListModels)
            .map_err(map_send_error)
    }

    fn select_model(&self, model: String) -> Result<(), AdapterError> {
        self.command_tx
            .try_send(CodexCommand::SelectModel(model))
            .map_err(map_send_error)
    }
}

fn map_send_error(error: mpsc::error::TrySendError<CodexCommand>) -> AdapterError {
    match error {
        mpsc::error::TrySendError::Full(_) => AdapterError::CommandQueueFull,
        mpsc::error::TrySendError::Closed(_) => AdapterError::RuntimeStopped,
    }
}

pub fn spawn_codex_adapter(event_tx: mpsc::Sender<AgentOutput>, cwd: PathBuf) -> SpawnedAdapter {
    let (command_tx, command_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
    let worker = tokio::spawn(runtime_loop(command_rx, event_tx, cwd));

    (Box::new(CodexAdapter { command_tx }), worker)
}

struct CodexClient {
    child: Child,
    stdin: ChildStdin,
    raw_rx: mpsc::Receiver<RawMessage>,
    readers: Vec<JoinHandle<()>>,
    thread_id: String,
    active_turn_id: Option<String>,
    pending_turn_request_id: Option<u64>,
    turn_ready_for_interrupt: bool,
    interrupt_sent: bool,
    cancel_requested: bool,
    next_request_id: u64,
}

enum RawMessage {
    Json(Value),
    Stderr(String),
    StdoutClosed,
}

impl CodexClient {
    async fn start(cwd: &Path, event_tx: &mpsc::Sender<AgentOutput>) -> io::Result<Self> {
        let program = resolve_program(CODEX_BINARY_OVERRIDE, "codex.exe", "codex");
        let mut command = headless_command(program);
        command
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("Codex stdin was not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("Codex stdout was not piped"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("Codex stderr was not piped"))?;

        let (raw_tx, raw_rx) = mpsc::channel(RAW_MESSAGE_CHANNEL_CAPACITY);
        let readers = vec![
            tokio::spawn(read_stdout(stdout, raw_tx.clone())),
            tokio::spawn(read_stderr(stderr, raw_tx)),
        ];
        let mut client = Self {
            child,
            stdin,
            raw_rx,
            readers,
            thread_id: String::new(),
            active_turn_id: None,
            pending_turn_request_id: None,
            turn_ready_for_interrupt: false,
            interrupt_sent: false,
            cancel_requested: false,
            next_request_id: 1,
        };

        let initialize_id = client.next_id();
        client
            .write_message(json!({
                "method": "initialize",
                "id": initialize_id,
                "params": {
                    "clientInfo": {
                        "name": "dmuxai",
                        "title": "DmuxAI",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }
            }))
            .await?;
        client
            .wait_for_success_response(initialize_id, event_tx)
            .await?;
        client
            .write_message(json!({ "method": "initialized", "params": {} }))
            .await?;

        let thread_id = client.next_id();
        client
            .write_message(json!({
                "method": "thread/start",
                "id": thread_id,
                "params": {
                    "cwd": cwd.to_string_lossy(),
                    "approvalPolicy": "never",
                    "sandbox": "read-only"
                }
            }))
            .await?;
        let response = client
            .wait_for_success_response(thread_id, event_tx)
            .await?;
        client.thread_id = extract_thread_id(&response)?;

        Ok(client)
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id += 1;
        id
    }

    async fn start_turn(&mut self, prompt: String, model: Option<&str>) -> io::Result<()> {
        let request_id = self.next_id();
        self.pending_turn_request_id = Some(request_id);
        self.turn_ready_for_interrupt = false;
        self.interrupt_sent = false;
        self.cancel_requested = false;
        let mut params = json!({
            "threadId": self.thread_id,
            "input": [{ "type": "text", "text": prompt }]
        });
        if let Some(model) = model {
            params["model"] = Value::String(model.to_owned());
        }
        self.write_message(json!({
            "method": "turn/start",
            "id": request_id,
            "params": params
        }))
        .await
    }

    async fn list_models(
        &mut self,
        event_tx: &mpsc::Sender<AgentOutput>,
    ) -> io::Result<(Vec<ModelInfo>, Option<String>)> {
        let request_id = self.next_id();
        self.write_message(json!({
            "method": "model/list",
            "id": request_id,
            "params": { "includeHidden": false }
        }))
        .await?;
        let response = self.wait_for_success_response(request_id, event_tx).await?;
        parse_model_list(&response)
    }

    async fn request_cancel(&mut self) -> io::Result<()> {
        if self.active_turn_id.is_some() || self.pending_turn_request_id.is_some() {
            self.cancel_requested = true;
            self.send_interrupt_if_ready().await?;
        }
        Ok(())
    }

    async fn send_interrupt_if_ready(&mut self) -> io::Result<()> {
        if self.turn_ready_for_interrupt
            && !self.interrupt_sent
            && let Some(turn_id) = self.active_turn_id.clone()
        {
            let request_id = self.next_id();
            self.write_message(json!({
                "method": "turn/interrupt",
                "id": request_id,
                "params": {
                    "threadId": self.thread_id,
                    "turnId": turn_id
                }
            }))
            .await?;
            self.interrupt_sent = true;
        }
        Ok(())
    }

    async fn write_message(&mut self, message: Value) -> io::Result<()> {
        let mut encoded = serde_json::to_vec(&message).map_err(io::Error::other)?;
        encoded.push(b'\n');
        self.stdin.write_all(&encoded).await?;
        self.stdin.flush().await
    }

    async fn wait_for_success_response(
        &mut self,
        request_id: u64,
        event_tx: &mpsc::Sender<AgentOutput>,
    ) -> io::Result<Value> {
        let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        let mut last_stderr = None;
        loop {
            let raw = match timeout_at(deadline, self.raw_rx.recv()).await {
                Ok(Some(raw)) => raw,
                Ok(None) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Codex app-server message channel closed during startup",
                    ));
                }
                Err(_) => {
                    let diagnostic = last_stderr
                        .map(|line: String| format!("; last stderr: {line}"))
                        .unwrap_or_default();
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("timed out waiting for Codex response id {request_id}{diagnostic}"),
                    ));
                }
            };

            match raw {
                RawMessage::Json(message) => {
                    if response_id(&message) == Some(request_id) {
                        if let Some(error) = response_error(&message) {
                            return Err(io::Error::other(error));
                        }
                        return Ok(message);
                    }
                    forward_notification(message, event_tx).await;
                }
                RawMessage::Stderr(line) => {
                    if !line.trim().is_empty() {
                        last_stderr = Some(line);
                    }
                }
                RawMessage::StdoutClosed => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Codex app-server closed stdout during startup",
                    ));
                }
            }
        }
    }

    async fn shutdown(mut self) {
        drop(self.stdin);
        if !matches!(
            tokio::time::timeout(Duration::from_secs(1), self.child.wait()).await,
            Ok(Ok(_))
        ) {
            let _ = self.child.kill().await;
        }
        for reader in self.readers.drain(..) {
            reader.abort();
            let _ = reader.await;
        }
    }
}

async fn runtime_loop(
    mut command_rx: mpsc::Receiver<CodexCommand>,
    event_tx: mpsc::Sender<AgentOutput>,
    cwd: PathBuf,
) {
    let mut client: Option<CodexClient> = None;
    let mut selected_model: Option<String> = None;

    loop {
        if client.is_none() {
            match command_rx.recv().await {
                Some(CodexCommand::Send(prompt)) => match CodexClient::start(&cwd, &event_tx).await
                {
                    Ok(mut started) => {
                        if let Err(error) =
                            started.start_turn(prompt, selected_model.as_deref()).await
                        {
                            send_event(&event_tx, AgentEvent::Error(error.to_string())).await;
                            started.shutdown().await;
                        } else {
                            send_event(&event_tx, AgentEvent::Started).await;
                            client = Some(started);
                        }
                    }
                    Err(error) => {
                        send_event(
                            &event_tx,
                            AgentEvent::Error(format!("could not start Codex: {error}")),
                        )
                        .await;
                    }
                },
                Some(CodexCommand::Cancel) => {}
                Some(CodexCommand::SelectModel(model)) => {
                    selected_model = Some(model.clone());
                    send_event(&event_tx, AgentEvent::ModelSelected(model)).await;
                }
                Some(CodexCommand::ListModels) => match CodexClient::start(&cwd, &event_tx).await {
                    Ok(mut started) => match started.list_models(&event_tx).await {
                        Ok((models, default_model)) => {
                            if selected_model.is_none() {
                                selected_model = default_model;
                            }
                            send_event(
                                &event_tx,
                                AgentEvent::ModelsListed {
                                    models,
                                    selected: selected_model.clone(),
                                },
                            )
                            .await;
                            client = Some(started);
                        }
                        Err(error) => {
                            send_event(&event_tx, AgentEvent::Error(error.to_string())).await;
                            started.shutdown().await;
                        }
                    },
                    Err(error) => {
                        send_event(
                            &event_tx,
                            AgentEvent::Error(format!("could not start Codex: {error}")),
                        )
                        .await;
                    }
                },
                None => break,
            }
            continue;
        }

        let active_client = client.as_mut().expect("checked above");
        tokio::select! {
            command = command_rx.recv() => {
                match command {
                    Some(CodexCommand::Send(prompt)) => {
                        if active_client.active_turn_id.is_some()
                            || active_client.pending_turn_request_id.is_some()
                        {
                            send_event(
                                &event_tx,
                                AgentEvent::Notice(
                                    "Codex is already running a turn; cancel it before sending another prompt"
                                        .to_owned(),
                                ),
                            ).await;
                        } else if let Err(error) = active_client
                            .start_turn(prompt, selected_model.as_deref())
                            .await
                        {
                            send_event(&event_tx, AgentEvent::Error(error.to_string())).await;
                        } else {
                            send_event(&event_tx, AgentEvent::Started).await;
                        }
                    }
                    Some(CodexCommand::Cancel) => {
                        if let Err(error) = active_client.request_cancel().await {
                            send_event(&event_tx, AgentEvent::Error(error.to_string())).await;
                        }
                    }
                    Some(CodexCommand::ListModels) => {
                        if active_client.active_turn_id.is_some()
                            || active_client.pending_turn_request_id.is_some()
                        {
                            send_event(
                                &event_tx,
                                AgentEvent::Notice(
                                    "finish or cancel the Codex turn before loading models".to_owned(),
                                ),
                            ).await;
                        } else {
                            match active_client.list_models(&event_tx).await {
                                Ok((models, default_model)) => {
                                    if selected_model.is_none() {
                                        selected_model = default_model;
                                    }
                                    send_event(
                                        &event_tx,
                                        AgentEvent::ModelsListed {
                                            models,
                                            selected: selected_model.clone(),
                                        },
                                    ).await;
                                }
                                Err(error) => {
                                    send_event(&event_tx, AgentEvent::Error(error.to_string())).await;
                                }
                            }
                        }
                    }
                    Some(CodexCommand::SelectModel(model)) => {
                        if active_client.active_turn_id.is_some()
                            || active_client.pending_turn_request_id.is_some()
                        {
                            send_event(
                                &event_tx,
                                AgentEvent::Notice(
                                    "finish or cancel the Codex turn before changing models".to_owned(),
                                ),
                            ).await;
                        } else {
                            selected_model = Some(model.clone());
                            send_event(&event_tx, AgentEvent::ModelSelected(model)).await;
                        }
                    }
                    None => break,
                }
            }
            raw = active_client.raw_rx.recv() => {
                match raw {
                    Some(RawMessage::Json(message)) => {
                        handle_message(active_client, message, &event_tx).await;
                    }
                    Some(RawMessage::Stderr(_line)) => {}
                    Some(RawMessage::StdoutClosed) | None => {
                        send_event(
                            &event_tx,
                            AgentEvent::Error("Codex app-server stopped unexpectedly".to_owned()),
                        ).await;
                        break;
                    }
                }
            }
        }
    }

    if let Some(client) = client {
        client.shutdown().await;
    }
}

async fn handle_message(
    client: &mut CodexClient,
    message: Value,
    event_tx: &mpsc::Sender<AgentOutput>,
) {
    if is_pending_turn_response(client.pending_turn_request_id, &message) {
        client.pending_turn_request_id = None;
        if let Some(error) = response_error(&message) {
            send_event(event_tx, AgentEvent::Error(error)).await;
            return;
        }
        client.active_turn_id = extract_turn_id(&message);
        return;
    }

    if message.get("method").and_then(Value::as_str) == Some("turn/started") {
        if let Some(turn_id) = message.pointer("/params/turn/id").and_then(Value::as_str) {
            client.active_turn_id = Some(turn_id.to_owned());
        }
        client.turn_ready_for_interrupt = true;
        if client.cancel_requested
            && let Err(error) = client.send_interrupt_if_ready().await
        {
            send_event(event_tx, AgentEvent::Error(error.to_string())).await;
        }
        return;
    }

    let event = normalized_event(&message);
    match event {
        Some(event @ (AgentEvent::Completed | AgentEvent::Cancelled)) => {
            client.active_turn_id = None;
            client.turn_ready_for_interrupt = false;
            client.interrupt_sent = false;
            client.cancel_requested = false;
            send_event(event_tx, event).await;
        }
        Some(event) => send_event(event_tx, event).await,
        None => {}
    }
}

fn is_pending_turn_response(pending_request_id: Option<u64>, message: &Value) -> bool {
    pending_request_id.is_some() && pending_request_id == response_id(message)
}

async fn forward_notification(message: Value, event_tx: &mpsc::Sender<AgentOutput>) {
    if let Some(event) = normalized_event(&message) {
        send_event(event_tx, event).await;
    }
}

fn normalized_event(message: &Value) -> Option<AgentEvent> {
    match message.get("method")?.as_str()? {
        "item/agentMessage/delta" => message
            .pointer("/params/delta")
            .and_then(Value::as_str)
            .map(|delta| AgentEvent::TextDelta(delta.to_owned())),
        "turn/completed" => match message
            .pointer("/params/turn/status")
            .and_then(Value::as_str)
        {
            Some("completed") => Some(AgentEvent::Completed),
            Some("interrupted") => Some(AgentEvent::Cancelled),
            Some("failed") => Some(AgentEvent::Error(
                message
                    .pointer("/params/turn/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("Codex turn failed")
                    .to_owned(),
            )),
            _ => None,
        },
        "error"
            if !message
                .pointer("/params/willRetry")
                .and_then(Value::as_bool)
                .unwrap_or(false) =>
        {
            Some(AgentEvent::Error(
                message
                    .pointer("/params/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("Codex reported an error")
                    .to_owned(),
            ))
        }
        _ => None,
    }
}

fn response_id(message: &Value) -> Option<u64> {
    message.get("id").and_then(Value::as_u64)
}

fn response_error(message: &Value) -> Option<String> {
    message
        .pointer("/error/message")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn parse_model_list(message: &Value) -> io::Result<(Vec<ModelInfo>, Option<String>)> {
    let data = message
        .pointer("/result/data")
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("model/list response did not include a data array"))?;
    let mut selected = None;
    let models = data
        .iter()
        .filter_map(|model| {
            let id = model.get("model")?.as_str()?.to_owned();
            let label = model
                .get("displayName")
                .and_then(Value::as_str)
                .unwrap_or(&id)
                .to_owned();
            if model
                .get("isDefault")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                selected = Some(id.clone());
            }
            Some(ModelInfo::new(id, label))
        })
        .collect::<Vec<_>>();
    Ok((models, selected))
}

fn extract_thread_id(message: &Value) -> io::Result<String> {
    message
        .pointer("/result/thread/id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other("thread/start response did not include a thread id"))
}

fn extract_turn_id(message: &Value) -> Option<String> {
    message
        .pointer("/result/turn/id")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

async fn send_event(event_tx: &mpsc::Sender<AgentOutput>, event: AgentEvent) {
    let _ = event_tx
        .send(AgentOutput {
            agent: Agent::Codex,
            event,
        })
        .await;
}

async fn read_stdout(stdout: ChildStdout, raw_tx: mpsc::Sender<RawMessage>) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        match serde_json::from_str(&line) {
            Ok(message) => {
                if raw_tx.send(RawMessage::Json(message)).await.is_err() {
                    return;
                }
            }
            Err(error) => {
                let message = json!({
                    "method": "error",
                    "params": {
                        "willRetry": false,
                        "error": { "message": format!("invalid Codex JSONL: {error}") }
                    }
                });
                if raw_tx.send(RawMessage::Json(message)).await.is_err() {
                    return;
                }
            }
        }
    }
    let _ = raw_tx.send(RawMessage::StdoutClosed).await;
}

async fn read_stderr(stderr: ChildStderr, raw_tx: mpsc::Sender<RawMessage>) {
    let mut lines: Lines<BufReader<ChildStderr>> = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if raw_tx.send(RawMessage::Stderr(line)).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_thread_and_turn_ids_from_responses() {
        let thread = json!({ "id": 2, "result": { "thread": { "id": "thr_123" } } });
        let turn = json!({ "id": 3, "result": { "turn": { "id": "turn_456" } } });

        assert_eq!(extract_thread_id(&thread).unwrap(), "thr_123");
        assert_eq!(extract_turn_id(&turn).as_deref(), Some("turn_456"));
    }

    #[test]
    fn normalizes_text_delta() {
        let message = json!({
            "method": "item/agentMessage/delta",
            "params": { "delta": "hello" }
        });

        assert_eq!(
            normalized_event(&message),
            Some(AgentEvent::TextDelta("hello".to_owned()))
        );
    }

    #[test]
    fn notification_without_an_id_is_not_a_pending_turn_response() {
        let notification = json!({
            "method": "item/agentMessage/delta",
            "params": { "delta": "hello" }
        });

        assert!(!is_pending_turn_response(None, &notification));
        assert!(!is_pending_turn_response(Some(3), &notification));
        assert!(is_pending_turn_response(
            Some(3),
            &json!({ "id": 3, "result": {} })
        ));
    }

    #[test]
    fn parses_codex_model_catalog_and_default() {
        let response = json!({
            "result": {
                "data": [
                    { "model": "gpt-a", "displayName": "GPT A", "isDefault": false },
                    { "model": "gpt-b", "displayName": "GPT B", "isDefault": true }
                ]
            }
        });

        let (models, selected) = parse_model_list(&response).unwrap();
        assert_eq!(
            models,
            vec![
                ModelInfo::new("gpt-a", "GPT A"),
                ModelInfo::new("gpt-b", "GPT B")
            ]
        );
        assert_eq!(selected.as_deref(), Some("gpt-b"));
    }

    #[test]
    fn normalizes_all_terminal_turn_statuses() {
        let completed = json!({
            "method": "turn/completed",
            "params": { "turn": { "status": "completed" } }
        });
        let interrupted = json!({
            "method": "turn/completed",
            "params": { "turn": { "status": "interrupted" } }
        });
        let failed = json!({
            "method": "turn/completed",
            "params": {
                "turn": { "status": "failed", "error": { "message": "boom" } }
            }
        });

        assert_eq!(normalized_event(&completed), Some(AgentEvent::Completed));
        assert_eq!(normalized_event(&interrupted), Some(AgentEvent::Cancelled));
        assert_eq!(
            normalized_event(&failed),
            Some(AgentEvent::Error("boom".to_owned()))
        );
    }

    #[test]
    fn ignores_retrying_errors_but_surfaces_final_errors() {
        let retrying = json!({
            "method": "error",
            "params": { "willRetry": true, "error": { "message": "retry" } }
        });
        let final_error = json!({
            "method": "error",
            "params": { "willRetry": false, "error": { "message": "failed" } }
        });

        assert_eq!(normalized_event(&retrying), None);
        assert_eq!(
            normalized_event(&final_error),
            Some(AgentEvent::Error("failed".to_owned()))
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a locally installed and authenticated Codex CLI"]
    async fn real_codex_app_server_handshake() {
        let (event_tx, _event_rx) = mpsc::channel(8);
        let cwd = std::env::current_dir().unwrap();
        let client = CodexClient::start(&cwd, &event_tx)
            .await
            .expect("the installed Codex app-server should complete its handshake");

        assert!(!client.thread_id.is_empty());
        client.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a locally installed and authenticated Codex CLI"]
    async fn real_codex_turn_streams_to_completion() {
        let (event_tx, mut event_rx) = mpsc::channel(128);
        let cwd = std::env::current_dir().unwrap();
        let mut client = CodexClient::start(&cwd, &event_tx)
            .await
            .expect("the installed Codex app-server should complete its handshake");
        client
            .start_turn(
                "Reply with exactly CODEX_OK. Do not use tools or modify files.".to_owned(),
                None,
            )
            .await
            .unwrap();

        let answer = tokio::time::timeout(Duration::from_secs(90), async {
            let mut answer = String::new();
            loop {
                let raw = client
                    .raw_rx
                    .recv()
                    .await
                    .expect("Codex output should remain connected");
                match raw {
                    RawMessage::Json(message) => {
                        handle_message(&mut client, message, &event_tx).await;
                    }
                    RawMessage::Stderr(line) => eprintln!("codex stderr: {line}"),
                    RawMessage::StdoutClosed => panic!("Codex stdout closed during the turn"),
                }

                while let Ok(output) = event_rx.try_recv() {
                    match output.event {
                        AgentEvent::TextDelta(delta) => answer.push_str(&delta),
                        AgentEvent::Completed => return answer,
                        AgentEvent::Error(error) => panic!("Codex turn failed: {error}"),
                        AgentEvent::Started
                        | AgentEvent::Notice(_)
                        | AgentEvent::ModelsListed { .. }
                        | AgentEvent::ModelSelected(_)
                        | AgentEvent::Cancelled => {}
                    }
                }
            }
        })
        .await
        .expect("Codex should complete the smoke-test turn within 90 seconds");

        assert!(answer.contains("CODEX_OK"), "unexpected answer: {answer}");
        client.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a locally installed and authenticated Codex CLI"]
    async fn real_codex_turn_can_be_interrupted() {
        let (event_tx, mut event_rx) = mpsc::channel(128);
        let cwd = std::env::current_dir().unwrap();
        let mut client = CodexClient::start(&cwd, &event_tx)
            .await
            .expect("the installed Codex app-server should complete its handshake");
        client
            .start_turn(
                "Write a very detailed 5,000-word explanation of Rust ownership. Do not use tools."
                    .to_owned(),
                None,
            )
            .await
            .unwrap();
        client.request_cancel().await.unwrap();

        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let raw = client
                    .raw_rx
                    .recv()
                    .await
                    .expect("Codex output should remain connected");
                match raw {
                    RawMessage::Json(message) => {
                        handle_message(&mut client, message, &event_tx).await;
                    }
                    RawMessage::Stderr(line) => eprintln!("codex stderr: {line}"),
                    RawMessage::StdoutClosed => panic!("Codex stdout closed during cancellation"),
                }

                while let Ok(output) = event_rx.try_recv() {
                    match output.event {
                        AgentEvent::Cancelled => return,
                        AgentEvent::Completed => {
                            panic!("Codex completed before acknowledging the interrupt")
                        }
                        AgentEvent::Error(error) => panic!("Codex turn failed: {error}"),
                        AgentEvent::Started
                        | AgentEvent::TextDelta(_)
                        | AgentEvent::Notice(_)
                        | AgentEvent::ModelsListed { .. }
                        | AgentEvent::ModelSelected(_) => {}
                    }
                }
            }
        })
        .await
        .expect("Codex should acknowledge turn/interrupt within 30 seconds");

        client.shutdown().await;
    }
}
