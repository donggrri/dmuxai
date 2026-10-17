//! Runtime for `[[agents.custom]]` headless agents (see docs/agents/custom-agents.md).

use std::{path::PathBuf, process::Stdio, time::Duration};

use dmuxai::custom_agent::{
    CustomAgentOutput, CustomAgentSpec, PromptVia, SubstitutionContext, build_argv, failure_reason,
    filter_models, parse_jsonl_line, spec_at, substitute,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::mpsc,
    task::JoinHandle,
};

use super::{
    AdapterError, Agent, AgentAdapter, AgentEvent, AgentOutput, AgentRequest, ExecutionMode,
    ModelInfo, OutputContext, SpawnedAdapter,
};

const COMMAND_CHANNEL_CAPACITY: usize = 8;
const STDERR_LIMIT: usize = 16 * 1024;

enum CustomCommand {
    Send(AgentRequest),
    Cancel,
    ListModels,
    SelectModel(String),
    SelectMode(ExecutionMode),
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
    fn select_mode(&self, mode: ExecutionMode) -> Result<(), AdapterError> {
        self.push(CustomCommand::SelectMode(mode))
    }
}

pub fn spawn_custom_adapter(
    index: u8,
    event_tx: mpsc::Sender<AgentOutput>,
    cwd: PathBuf,
) -> SpawnedAdapter {
    let (command_tx, command_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
    let worker = tokio::spawn(runtime_loop(index, command_rx, event_tx, cwd));
    (Box::new(CustomAdapter { index, command_tx }), worker)
}

struct ActiveTurn {
    handle: JoinHandle<()>,
    context: OutputContext,
    pgid: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

async fn runtime_loop(
    index: u8,
    mut command_rx: mpsc::Receiver<CustomCommand>,
    event_tx: mpsc::Sender<AgentOutput>,
    cwd: PathBuf,
) {
    let agent = Agent::Custom(index);
    let Some(spec) = spec_at(index) else {
        return;
    };
    let mut active: Option<ActiveTurn> = None;
    let mut selected_model = spec.default_model.clone();
    let mut selected_mode = ExecutionMode::default();

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
                active = Some(spawn_turn(
                    agent,
                    spec,
                    request,
                    selected_model.clone(),
                    selected_mode,
                    cwd.clone(),
                    event_tx.clone(),
                ));
            }
            CustomCommand::Cancel => stop_turn(agent, active.take(), &event_tx).await,
            CustomCommand::ListModels => {
                let event = match list_models(spec, &cwd).await {
                    Ok(models) => AgentEvent::ModelsListed {
                        models,
                        selected: selected_model.clone(),
                        selected_effort: None,
                    },
                    Err(error) => AgentEvent::Error(error),
                };
                send(&event_tx, agent, None, event).await;
            }
            CustomCommand::SelectModel(model) => {
                if let Err(error) = dmuxai::custom_agent::validate_custom_model(spec, &model) {
                    send(&event_tx, agent, None, AgentEvent::Error(error)).await;
                    continue;
                }
                selected_model = Some(model.clone());
                send(&event_tx, agent, None, AgentEvent::ModelSelected(model)).await;
            }
            CustomCommand::SelectMode(mode) => {
                selected_mode = mode;
                send(&event_tx, agent, None, AgentEvent::ModeSelected(mode)).await;
            }
        }
    }
}

async fn list_models(spec: &CustomAgentSpec, cwd: &PathBuf) -> Result<Vec<ModelInfo>, String> {
    let mut ids = Vec::new();
    if let Some((program, args)) = spec.models_command.split_first() {
        let output = tokio::process::Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|error| format!("{} models: {error}", spec.label()))?;
        if !output.status.success() {
            return Err(format!(
                "{} models failed: {}",
                spec.label(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        ids = filter_models(spec, &String::from_utf8_lossy(&output.stdout));
    }
    if let Some(default) = &spec.default_model
        && !ids.contains(default)
    {
        ids.insert(0, default.clone());
    }
    Ok(ids
        .into_iter()
        .map(|id| ModelInfo::new(id.clone(), id))
        .collect())
}

fn mode_names(mode: ExecutionMode) -> (&'static str, &'static str) {
    match mode {
        ExecutionMode::Ask => ("ask", "deny"),
        ExecutionMode::Plan => ("plan", "deny"),
        ExecutionMode::Implement => ("implement", "allow"),
    }
}

fn spawn_turn(
    agent: Agent,
    spec: &'static CustomAgentSpec,
    request: AgentRequest,
    model: Option<String>,
    mode: ExecutionMode,
    cwd: PathBuf,
    event_tx: mpsc::Sender<AgentOutput>,
) -> ActiveTurn {
    let context = OutputContext::from(&request);
    let pgid = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let turn_context = context.clone();
    let turn_pgid = pgid.clone();
    let handle = tokio::spawn(async move {
        send(&event_tx, agent, Some(&turn_context), AgentEvent::Started).await;
        let event = match run_turn(
            agent,
            spec,
            &request,
            model,
            mode,
            &cwd,
            &event_tx,
            &turn_context,
            &turn_pgid,
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

#[allow(clippy::too_many_arguments)]
async fn run_turn(
    agent: Agent,
    spec: &CustomAgentSpec,
    request: &AgentRequest,
    model: Option<String>,
    mode: ExecutionMode,
    cwd: &PathBuf,
    event_tx: &mpsc::Sender<AgentOutput>,
    context: &OutputContext,
    pgid: &std::sync::atomic::AtomicU32,
) -> Result<(), String> {
    let (mode_name, edit) = mode_names(mode);
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
        let body = substitute(template, &ctx).unwrap_or_else(|| template.clone());
        std::fs::write(&path, body).map_err(|error| format!("temp config: {error}"))?;
        ctx.temp_config = Some(path.display().to_string());
        temp.0 = Some(path);
    }

    let argv = build_argv(spec, &ctx);
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| format!("{}: empty command", spec.label()))?;
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(if spec.prompt_via == PromptVia::Stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (key, value) in &spec.env {
        if let Some(value) = substitute(value, &ctx) {
            command.env(key, value);
        }
    }
    #[cfg(unix)]
    command.process_group(0);

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
    let mut lines = BufReader::new(stdout).lines();
    let read = async {
        while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
            match spec.output {
                CustomAgentOutput::Text => {
                    let chunk = format!("{line}\n");
                    reply.push_str(&chunk);
                    send(event_tx, agent, Some(context), AgentEvent::TextDelta(chunk)).await;
                }
                CustomAgentOutput::Jsonl => {
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
                        reply.push_str(&text);
                        send(event_tx, agent, Some(context), AgentEvent::TextDelta(text)).await;
                    }
                    if let Some(error) = parsed.error {
                        jsonl_error = Some(error);
                    }
                }
            }
        }
        child.wait().await.map_err(|e| e.to_string())
    };
    let status = tokio::time::timeout(Duration::from_secs(spec.timeout_secs()), read)
        .await
        .map_err(|_| {
            kill_group(pgid.load(std::sync::atomic::Ordering::SeqCst));
            format!("{} timed out after {}s", spec.label(), spec.timeout_secs())
        })??;
    let stderr_text = stderr_task.await.unwrap_or_default();
    match failure_reason(
        spec,
        status.success(),
        &reply,
        &stderr_text,
        jsonl_error.as_deref(),
    ) {
        Some(reason) => Err(format!("{} failed: {reason}", spec.label())),
        None => Ok(()),
    }
}

fn kill_group(pgid: u32) {
    #[cfg(unix)]
    if pgid > 0 {
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &format!("-{pgid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(unix))]
    let _ = pgid;
}

async fn stop_turn(agent: Agent, turn: Option<ActiveTurn>, event_tx: &mpsc::Sender<AgentOutput>) {
    if let Some(turn) = turn {
        kill_group(turn.pgid.load(std::sync::atomic::Ordering::SeqCst));
        turn.handle.abort();
        let _ = turn.handle.await;
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
        let (tx, mut rx) = mpsc::channel(64);
        let request = AgentRequest {
            conversation_id: ConversationId::new("c"),
            turn_id: TurnId::new("t"),
            prompt: "hi --model x".to_owned(),
            resume_session: None,
            image_paths: Vec::new(),
        };
        let context = OutputContext::from(&request);
        let pgid = std::sync::atomic::AtomicU32::new(0);
        let cwd = std::env::temp_dir();
        let result = run_turn(
            Agent::Custom(0),
            spec,
            &request,
            model.map(str::to_owned),
            ExecutionMode::Implement,
            &cwd,
            &tx,
            &context,
            &pgid,
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
}
