//! Runtime for `[[agents.custom]]` headless agents (see docs/agents/custom-agents.md).

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use dmuxai::custom_agent::{
    BUILTIN_MODELS_TIMEOUT, CustomAgentOutput, CustomAgentSpec, SubstitutionContext, build_argv,
    failure_reason, grok_no_allowed_model_message, is_builtin_grok, list_model_ids,
    parse_jsonl_line, policy_message, select_grok_model, spec_at, substitute, timeout_parts,
};
use dmuxai::mcp::RunnerMcp;
use tokio::{io::AsyncWriteExt, sync::mpsc, task::JoinHandle};

use super::{
    AdapterError, Agent, AgentAdapter, AgentError, AgentErrorKind, AgentEvent, AgentOutput,
    AgentRequest, ModelInfo, OutputContext, SpawnedAdapter, TurnPolicy,
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
            .map_err(super::map_send_error)
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
    mcp: RunnerMcp,
) -> SpawnedAdapter {
    let (command_tx, command_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
    let worker = tokio::spawn(runtime_loop(index, command_rx, event_tx, cwd, mcp));
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
    mcp: RunnerMcp,
) {
    let Some(spec) = spec_at(index) else {
        return;
    };
    runtime_loop_with_spec(Agent::Custom(index), spec, command_rx, event_tx, cwd, mcp).await;
}

async fn runtime_loop_with_spec(
    agent: Agent,
    spec: &'static CustomAgentSpec,
    mut command_rx: mpsc::Receiver<CustomCommand>,
    event_tx: mpsc::Sender<AgentOutput>,
    cwd: PathBuf,
    mcp: RunnerMcp,
) {
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
                    AgentEvent::Error(AgentError::provider(format!(
                        "{} is already running a turn; cancel it before sending another prompt",
                        spec.label()
                    ))),
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
                    send(
                        &event_tx,
                        agent,
                        Some(&context),
                        AgentEvent::Error(AgentError::model_unavailable(error)),
                    )
                    .await;
                    continue;
                }
                let policy = request.policy;
                active = Some(spawn_turn(
                    agent,
                    spec,
                    request,
                    selected_model.clone(),
                    policy,
                    cwd.clone(),
                    event_tx.clone(),
                    mcp.clone(),
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
                    Err(error) => AgentEvent::Error(AgentError::model_unavailable(error)),
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
                    send(
                        &event_tx,
                        agent,
                        None,
                        AgentEvent::Error(AgentError::model_unavailable(error)),
                    )
                    .await;
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
    mode: TurnPolicy,
    cwd: PathBuf,
    event_tx: mpsc::Sender<AgentOutput>,
    mcp: RunnerMcp,
    mcp_notice_shown: Arc<AtomicBool>,
) -> ActiveTurn {
    let context = OutputContext::from(&request);
    let pgid = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let settings = dmuxai::settings::DmuxSettings::load(&cwd);
    let uses_acp = is_builtin_grok(spec)
        && settings.grok_protocol == dmuxai::runner_protocol::RunnerProtocol::Acp;
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
            mode,
            &cwd,
            &event_tx,
            &turn_context,
            &turn_pgid,
            &mcp,
            &mcp_notice_shown,
            cancel_for_turn,
        )
        .await
        {
            Ok(()) => AgentEvent::Completed,
            Err(error) if error.kind == AgentErrorKind::Cancelled => AgentEvent::Cancelled,
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

/// Writes content to a private temp file with 0600 permissions (create_new).
fn write_private_temp_file(path: &Path, content: &str, label: &str) -> Result<(), String> {
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
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_turn(
    agent: Agent,
    spec: &CustomAgentSpec,
    request: &AgentRequest,
    model: Option<String>,
    _mode: TurnPolicy,
    cwd: &Path,
    event_tx: &mpsc::Sender<AgentOutput>,
    context: &OutputContext,
    pgid: &std::sync::atomic::AtomicU32,
    mcp: &RunnerMcp,
    mcp_notice_shown: &AtomicBool,
    acp_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<(), AgentError> {
    let settings = dmuxai::settings::DmuxSettings::load(cwd);
    if is_builtin_grok(spec)
        && settings.grok_protocol == dmuxai::runner_protocol::RunnerProtocol::Acp
    {
        let model_id = super::acp_runner::acp_model_arg("grok", model.as_deref())
            .map_err(AgentError::model_unavailable)?
            .ok_or_else(|| AgentError::model_unavailable(grok_no_allowed_model_message()))?;
        let model_id = model_id.as_str();
        let program = spec
            .command
            .first()
            .ok_or_else(|| AgentError::process_exit(format!("{}: empty command", spec.label())))?;
        let effort = spec
            .env
            .get("GROK_REASONING_EFFORT")
            .cloned()
            .or_else(|| std::env::var("GROK_REASONING_EFFORT").ok());
        let read_only = request.policy == TurnPolicy::ReadOnly;
        let args = super::acp_runner::grok_acp_argv(model_id, effort.as_deref(), read_only);
        let permission_ctx = super::acp::PermissionContext {
            workspace: cwd.to_string_lossy().into_owned(),
            sensitive_gate_active: false,
            // FR-1413: grok has no ACP modes; dmuxai denies writes itself.
            read_only_fail_closed: read_only,
        };
        let cancel = acp_cancel.unwrap_or_else(super::acp::new_cancel_flag);
        let user_cancel = cancel.clone();
        let outcome = super::acp_runner::run_agent_acp_turn(
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
            mcp.acp_servers(),
        )
        .await
        .map_err(AgentError::provider)?;
        if outcome.cancelled && !user_cancel.load(std::sync::atomic::Ordering::SeqCst) {
            send(
                event_tx,
                agent,
                Some(&OutputContext::from(request)),
                AgentEvent::Notice(super::acp::PERMISSION_DENIED_NOTICE.to_owned()),
            )
            .await;
        }
        if outcome.cancelled {
            return Err(AgentError::cancelled("ACP turn cancelled"));
        }
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

    // Per-agent system prompt (PR-2.5 part 3): expose text to placeholders.
    let system_prompt_text = request.system_prompt.as_ref().map(|p| p.text.clone());
    ctx.system_prompt = system_prompt_text.clone();

    // Create a temp file containing the prompt text only when {system_prompt_file}
    // is actually used in the spec (command, env, template, or system_prompt_arg).
    let contains_placeholder = |value: &str, needle: &str| value.contains(needle);
    let uses_system_prompt_file = system_prompt_text.is_some()
        && (spec
            .command
            .iter()
            .any(|v| contains_placeholder(v, "{system_prompt_file}"))
            || spec
                .env
                .values()
                .any(|v| contains_placeholder(v, "{system_prompt_file}"))
            || spec
                .temp_config_template
                .as_ref()
                .is_some_and(|t| contains_placeholder(t, "{system_prompt_file}"))
            || spec
                .system_prompt_arg
                .iter()
                .any(|v| contains_placeholder(v, "{system_prompt_file}")));
    let mut temp_system_prompt = TempFile(None);
    if uses_system_prompt_file && let Some(text) = &system_prompt_text {
        let path = std::env::temp_dir().join(format!(
            "dmuxai-{}-{}-system-prompt.txt",
            spec.name,
            uuid::Uuid::new_v4()
        ));
        write_private_temp_file(&path, text, "system prompt").map_err(AgentError::process_exit)?;
        ctx.system_prompt_file = Some(path.display().to_string());
        temp_system_prompt.0 = Some(path);
    }

    // opencode builtin: native delivery via temp config `instructions` array.
    // Fallback when no temp_config_template: prepend the block to the prompt.
    if dmuxai::custom_agent::is_builtin_opencode(spec)
        && let Some(sp) = &request.system_prompt
        && spec.temp_config_template.is_none()
    {
        ctx.prompt = format!(
            "{}{}",
            dmuxai::agent_prompt::prepend_block(&sp.text),
            ctx.prompt
        );
    }

    let mut temp = TempFile(None);
    let mut temp_system_prompt_instructions = TempFile(None);
    if let Some(template) = &spec.temp_config_template {
        let path = std::env::temp_dir().join(format!(
            "dmuxai-{}-{}.json",
            spec.name,
            uuid::Uuid::new_v4()
        ));
        let mut body = substitute(template, &ctx).unwrap_or_else(|| template.clone());

        // opencode: write prompt text to a temp file and merge `instructions`.
        if dmuxai::custom_agent::is_builtin_opencode(spec)
            && let Some(sp) = &request.system_prompt
        {
            let path = std::env::temp_dir().join(format!(
                "dmuxai-{}-{}-instructions.txt",
                spec.name,
                uuid::Uuid::new_v4()
            ));
            write_private_temp_file(&path, &sp.text, "system prompt")
                .map_err(AgentError::process_exit)?;
            let abs = path.display().to_string();
            if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&body)
                && let Some(obj) = json.as_object_mut()
            {
                let entry = obj
                    .entry("instructions".to_owned())
                    .or_insert_with(|| serde_json::json!([]));
                if let Some(arr) = entry.as_array_mut() {
                    arr.push(serde_json::Value::String(abs.clone()));
                } else {
                    // replace non-array with array containing path
                    obj.insert("instructions".to_owned(), serde_json::json!([abs]));
                }
                if let Ok(serialized) = serde_json::to_string(&json) {
                    body = serialized;
                }
            }
            temp_system_prompt_instructions.0 = Some(path);
        }

        if !mcp.is_empty() {
            let (merged, applied) = mcp.opencode_config(&body);
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
        write_private_temp_file(&path, &body, "temp config").map_err(AgentError::process_exit)?;
        ctx.temp_config = Some(path.display().to_string());
        temp.0 = Some(path);
    }

    let mut mcp_temp = TempFile(None);
    if !mcp.is_empty() {
        let path = std::env::temp_dir().join(format!(
            "dmuxai-mcp-{}-{}.json",
            spec.name,
            uuid::Uuid::new_v4()
        ));
        let body = mcp.custom_json();
        write_private_temp_file(&path, &body, "mcp config").map_err(AgentError::process_exit)?;
        ctx.mcp_config = Some(path.display().to_string());
        mcp_temp.0 = Some(path);
    }

    let argv = build_argv(spec, &ctx);
    let program = argv
        .first()
        .ok_or_else(|| AgentError::process_exit(format!("{}: empty command", spec.label())))?;
    let command = dmuxai::custom_agent::build_custom_command(spec, &ctx, cwd)
        .map_err(AgentError::process_exit)?;

    let supervisor_config = dmuxai::process::supervisor::SupervisorConfig {
        label: spec.label().to_owned(),
        idle_timeout: spec.idle_timeout_secs().map(Duration::from_secs),
        total_timeout: Some(Duration::from_secs(spec.timeout_secs())),
        stderr_tail_bytes: STDERR_LIMIT,
        term_grace: TERM_GRACE,
        stderr_drain_grace: STDERR_DRAIN_GRACE,
        ..Default::default()
    };

    let mut supervised =
        dmuxai::process::supervisor::spawn(command, supervisor_config).map_err(|error| {
            AgentError::process_exit(format!(
                "{}: failed to start {program}: {error}",
                spec.label()
            ))
        })?;
    pgid.store(supervised.pgid(), std::sync::atomic::Ordering::SeqCst);

    if let Some(mut stdin) = supervised.take_stdin() {
        let mut prompt = ctx.prompt.clone();
        for image in &ctx.images {
            prompt.push_str(&format!(" @{image}"));
        }
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(|error| AgentError::process_exit(format!("stdin: {error}")))?;
        drop(stdin);
    }

    let effective_output = spec.effective_output();
    let mut reply = String::new();
    let mut jsonl_error: Option<String> = None;
    let mut bound_session: Option<String> = None;
    let mut break_pending = false;
    let mut grok_tools = dmuxai::custom_agent::is_builtin_grok(spec)
        .then(|| dmuxai::grok_activity::GrokToolTracker::new(cwd));
    let mut done = false;
    let mut checked_first_nonempty_stdout = false;

    let status = loop {
        let event = match supervised.next_line().await {
            Ok(event) => event,
            Err(e) => {
                let _ = supervised.stop().await;
                return Err(AgentError::process_exit(e.to_string()));
            }
        };

        match event {
            dmuxai::process::supervisor::LineEvent::Idle(d) => {
                let _ = supervised.stop().await;
                return Err(AgentError::idle(
                    dmuxai::process::supervisor::idle_error_message(spec.label(), d.as_secs()),
                ));
            }
            dmuxai::process::supervisor::LineEvent::Timeout(_) => {
                let _ = supervised.stop().await;
                let (msg, hint) = timeout_parts(spec);
                return Err(AgentError::timeout(msg).with_hint(hint));
            }
            dmuxai::process::supervisor::LineEvent::Eof => {
                break supervised.finish(DONE_EXIT_GRACE).await;
            }
            dmuxai::process::supervisor::LineEvent::Line(line) => {
                if !checked_first_nonempty_stdout && !line.trim().is_empty() {
                    checked_first_nonempty_stdout = true;
                    if effective_output == CustomAgentOutput::Text
                        && serde_json::from_str::<serde_json::Value>(line.trim())
                            .ok()
                            .is_some_and(|v| v.is_object())
                    {
                        send(
                            event_tx,
                            agent,
                            Some(context),
                            AgentEvent::Notice(
                                "output looks like JSONL; set output = \"jsonl\" and text_path in dmuxai.toml (docs/agents/custom-agents.md)"
                                    .to_string(),
                            ),
                        )
                        .await;
                    }
                }
                match effective_output {
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
                            send(event_tx, agent, Some(context), AgentEvent::TextDelta(chunk))
                                .await;
                        }
                        if parsed.text_break {
                            break_pending = true;
                        }
                        if let Some(error) = parsed.error {
                            jsonl_error = Some(error);
                        }
                        if parsed.done {
                            done = true;
                            break supervised.finish(DONE_EXIT_GRACE).await;
                        }
                    }
                }
            }
        }
    };

    let stderr_text = supervised.stderr_tail().await;
    let exit_ok = done || status.success();
    if let Some(reason) =
        failure_reason(spec, exit_ok, &reply, &stderr_text, jsonl_error.as_deref())
    {
        let message = format!("{} failed: {reason}", spec.label());
        let err = if jsonl_error
            .as_deref()
            .map(str::trim)
            .is_some_and(|s| !s.is_empty())
        {
            AgentError::provider(message)
        } else if spec
            .failure_stderr
            .as_deref()
            .and_then(|p| regex::Regex::new(p).ok())
            .is_some_and(|re| re.is_match(&stderr_text))
        {
            AgentError::policy(message)
        } else if !exit_ok {
            AgentError::process_exit(message)
        } else {
            AgentError::provider(message)
        };
        return Err(err);
    }
    Ok(())
}

/// How long the process may take to exit on its own after its `done_when` line or EOF.
const DONE_EXIT_GRACE: Duration = Duration::from_secs(3);
/// How long SIGTERM gets before SIGKILL when stopping a finished turn's group.
const TERM_GRACE: Duration = Duration::from_secs(2);
/// How long stderr may stay open after the process exited.
const STDERR_DRAIN_GRACE: Duration = Duration::from_secs(2);

async fn stop_turn(agent: Agent, turn: Option<ActiveTurn>, event_tx: &mpsc::Sender<AgentOutput>) {
    if let Some(turn) = turn {
        if let Some(cancel) = &turn.acp_cancel {
            cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = tokio::time::timeout(std::time::Duration::from_millis(1600), turn.handle).await;
        } else {
            let pgid = turn.pgid.load(std::sync::atomic::Ordering::SeqCst);
            dmuxai::process::supervisor::signal_group(
                pgid,
                dmuxai::process::supervisor::GroupSignal::Term,
            );
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
    super::emit(event_tx, agent, context, event).await;
}

// The fake agent is a POSIX shell script; keep test-only on unix.
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

    async fn run(spec: &CustomAgentSpec, model: Option<&str>) -> (Result<(), AgentError>, String) {
        run_with_mcp(spec, model, &[]).await
    }

    async fn run_with_mcp(
        spec: &CustomAgentSpec,
        model: Option<&str>,
        mcp_servers: &[dmuxai::mcp::ResolvedMcpServer],
    ) -> (Result<(), AgentError>, String) {
        let (tx, mut rx) = mpsc::channel(64);
        let request = AgentRequest {
            conversation_id: ConversationId::new("c"),
            turn_id: TurnId::new("t"),
            prompt: "hi --model x".to_owned(),
            resume_session: None,
            image_paths: Vec::new(),
            policy: TurnPolicy::Agent,
            require_file_edits: false,
            system_prompt: None,
        };
        let context = OutputContext::from(&request);
        let pgid = std::sync::atomic::AtomicU32::new(0);
        let cwd = std::env::temp_dir();
        let notice_shown = AtomicBool::new(false);
        let mcp = RunnerMcp::for_runner(mcp_servers, &spec.name);
        let result = run_turn(
            Agent::Custom(0),
            spec,
            &request,
            model.map(str::to_owned),
            TurnPolicy::Agent,
            &cwd,
            &tx,
            &context,
            &pgid,
            &mcp,
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
    async fn tc_348_text_mode_json_stdout_emits_jsonl_notice() {
        let spec = spec("jsonl", "output = \"text\"\nlabel = \"json-hint-runner\"");
        let notice_shown = AtomicBool::new(false);
        let (result, events) = run_turn_capture_events(spec, &[], &notice_shown).await;
        assert_eq!(result, Ok(()));
        let jsonl_notices: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::Notice(msg) if msg.contains("output looks like JSONL") => {
                    Some(msg.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(jsonl_notices.len(), 1, "events: {:?}", events);
        let text: String = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TextDelta(delta) => Some(delta.clone()),
                _ => None,
            })
            .collect();
        assert!(
            text.contains("{\"type\":\"step_start\""),
            "text mode must keep raw stdout: {text}"
        );
        assert!(text.contains("echo:hi --model x"));
    }

    #[tokio::test]
    async fn tc_349_inferred_jsonl_stdout_no_jsonl_notice() {
        let spec = spec(
            "jsonl",
            "text_path = \"/part/text\"\nsession_id_path = \"/sessionID\"",
        );
        let notice_shown = AtomicBool::new(false);
        let (result, events) = run_turn_capture_events(spec, &[], &notice_shown).await;
        assert_eq!(result, Ok(()));
        assert!(
            !events.iter().any(|e| {
                matches!(e, AgentEvent::Notice(msg) if msg.contains("output looks like JSONL"))
            }),
            "inferred jsonl must not emit JSONL hint: {:?}",
            events
        );
        let text: String = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TextDelta(delta) => Some(delta.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "echo:hi --model x");
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

    // TC-1689 / FR-1679: runner conditions map to kinds.
    #[tokio::test]
    async fn failures_are_detected() {
        let (empty, _) = run(spec("empty", ""), None).await;
        let empty_err = empty.unwrap_err();
        assert!(empty_err.to_string().contains("empty reply"));
        assert_eq!(empty_err.kind, AgentErrorKind::Provider);

        let (denied, _) = run(spec("denied", ""), None).await;
        let denied_err = denied.unwrap_err();
        assert!(denied_err.to_string().contains("permission denied"));
        assert_eq!(denied_err.kind, AgentErrorKind::Policy);

        let (fail, _) = run(spec("fail", ""), None).await;
        let fail_err = fail.unwrap_err();
        assert!(fail_err.to_string().contains("boom"));
        assert_eq!(fail_err.kind, AgentErrorKind::ProcessExit);

        let (error, _) = run(
            spec(
                "error",
                "output = \"jsonl\"\ntext_path = \"/part/text\"\nerror_path = \"/error/message\"",
            ),
            None,
        )
        .await;
        let error_err = error.unwrap_err();
        assert!(error_err.to_string().contains("model not found"));
        assert_eq!(error_err.kind, AgentErrorKind::Provider);
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

    // TC-1690 / FR-1679: total timeout maps to Timeout.
    #[tokio::test]
    async fn timeout_kills_the_process_group() {
        let spec = spec("sleep", "timeout_secs = 1");
        let started = std::time::Instant::now();
        let (result, _) = run(spec, None).await;
        let err = result.unwrap_err();
        assert_eq!(err.kind, AgentErrorKind::Timeout);
        let err_str = err.to_string();
        assert!(err_str.contains("timed out"));
        assert!(err_str.contains("timeout_secs"));
        assert!(err_str.contains("name = \"fake\""));
        assert!(err_str.contains("partial edits"));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    // TC-1794 (FR-1794): silent custom agent fails idle naming agent and idle_timeout_secs
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn tc_1794_silent_custom_agent_fails_idle_naming_agent() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("child.pid");
        let extra = format!(
            "label = \"my-silent-agent\"\nidle_timeout_secs = 1\ntimeout_secs = 60\nenv = {{ FAKE_AGENT_PIDFILE = \"{}\" }}",
            pidfile.to_str().expect("utf8 path")
        );
        let spec = spec("idle_sleep", &extra);
        let started = std::time::Instant::now();
        let (result, _) = run(spec, None).await;
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(10),
            "took too long: {elapsed:?}"
        );
        let err = result.unwrap_err();
        assert_eq!(err.kind, AgentErrorKind::Idle);
        let msg = err.to_string();
        assert!(
            msg.contains("my-silent-agent"),
            "error message must name agent label: {msg}"
        );
        assert!(
            msg.contains("idle_timeout_secs"),
            "error message must name idle_timeout_secs: {msg}"
        );
        assert!(
            msg.contains("1s") || msg.contains("1"),
            "error message must contain idle timeout seconds: {msg}"
        );
        let child_pid = read_pid(&pidfile).await;
        assert_gone(child_pid).await;
    }

    // TC-1795 (FR-1795): each stdout line resets the idle timer
    #[tokio::test]
    async fn tc_1795_idle_timer_resets_on_each_line() {
        let extra = "idle_timeout_secs = 1\ntimeout_secs = 60\noutput = \"jsonl\"\ntext_path = \"/part/text\"";
        let spec = spec("trickle", extra);
        let started = std::time::Instant::now();
        let (result, text) = run(spec, None).await;
        let elapsed = started.elapsed();
        assert_eq!(result, Ok(()));
        assert!(
            elapsed > Duration::from_millis(1500),
            "trickle should take over 1.5s: {elapsed:?}"
        );
        for i in 1..=6 {
            assert!(
                text.contains(&format!("line{i}")),
                "text should contain line{i}: {text}"
            );
        }
    }

    // TC-1796 (FR-1796): idle_timeout_secs = 0 disables the watchdog
    #[tokio::test]
    async fn tc_1796_idle_timeout_zero_disables_watchdog() {
        let extra = "idle_timeout_secs = 0\ntimeout_secs = 60\noutput = \"jsonl\"\ntext_path = \"/part/text\"";
        let spec = spec("delayed", extra);
        let started = std::time::Instant::now();
        let (result, text) = run(spec, None).await;
        let elapsed = started.elapsed();
        assert_eq!(result, Ok(()));
        assert!(
            elapsed >= Duration::from_secs(2),
            "delayed should take at least 2s: {elapsed:?}"
        );
        assert!(
            text.contains("delayed text"),
            "text should contain delayed text: {text}"
        );
    }

    // TC-1797 (FR-1797): grandchild holding stdout does not block turn completion
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn tc_1797_grandchild_holding_stdout_still_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("grandchild.pid");
        let extra = format!(
            "env = {{ FAKE_AGENT_PIDFILE = \"{}\" }}\noutput = \"jsonl\"\ntext_path = \"/part/text\"",
            pidfile.to_str().expect("utf8 path")
        );
        let spec = spec("orphan_stdout", &extra);
        let started = std::time::Instant::now();
        let (result, text) = run(spec, None).await;
        let elapsed = started.elapsed();
        assert_eq!(result, Ok(()));
        assert!(
            elapsed < Duration::from_secs(10),
            "turn must finish within 10s: {elapsed:?}"
        );
        assert!(
            text.contains("orphan ok"),
            "text must contain orphan ok: {text}"
        );
        let grandchild_pid = read_pid(&pidfile).await;
        assert_gone(grandchild_pid).await;
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
    ) -> (Result<(), AgentError>, Vec<AgentEvent>) {
        let (tx, mut rx) = mpsc::channel(64);
        let request = AgentRequest {
            conversation_id: ConversationId::new("c"),
            turn_id: TurnId::new("t"),
            prompt: "hi --model x".to_owned(),
            resume_session: None,
            image_paths: Vec::new(),
            policy: TurnPolicy::Agent,
            require_file_edits: false,
            system_prompt: None,
        };
        let context = OutputContext::from(&request);
        let pgid = std::sync::atomic::AtomicU32::new(0);
        let cwd = std::env::temp_dir();
        let mcp = RunnerMcp::for_runner(mcp_servers, &spec.name);
        let result = run_turn(
            Agent::Custom(0),
            spec,
            &request,
            None,
            TurnPolicy::Agent,
            &cwd,
            &tx,
            &context,
            &pgid,
            &mcp,
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

    // TC-1724 / FR-1721: e2e agy success line finishes without AgentEvent::Error.
    #[tokio::test]
    async fn tc_1724_e2e_agy_success_line_finishes_without_error() {
        let toml = r#"
name = "agy-e2e"
command = ["sh", "-c", "echo '{\"event\":\"result\",\"conversation_id\":\"c1\",\"result\":{\"status\":\"SUCCESS\",\"response\":\"done\",\"error\":null}}'"]
output = "jsonl"
text_path = "/result/response"
text_when = { path = "/event", equals = "result" }
error_path = "/result/error"
"#;
        let spec: &'static CustomAgentSpec = Box::leak(Box::new(toml::from_str(toml).unwrap()));
        let notice_shown = AtomicBool::new(false);
        let (result, events) = run_turn_capture_events(spec, &[], &notice_shown).await;
        assert_eq!(result, Ok(()), "events: {:?}", events);
        assert!(
            !events.iter().any(|e| matches!(e, AgentEvent::Error(_))),
            "must not emit AgentEvent::Error: {:?}",
            events
        );
        let text: String = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TextDelta(delta) => Some(delta.clone()),
                _ => None,
            })
            .collect();
        assert!(text.contains("done"), "text: {text}");
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
            require_file_edits: false,
            system_prompt: None,
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
            RunnerMcp::default(),
        ));
        std::mem::forget(workspace);
        (command_tx, event_rx, worker)
    }

    async fn until_terminal(rx: &mut mpsc::Receiver<AgentOutput>) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
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
                AgentEvent::Error(error) => Some(error.message.clone()),
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
                AgentEvent::Error(error) => Some(error.to_string()),
                _ => None,
            });
            let list_error = list_error.expect("models error");
            assert!(list_error.contains("grok login"), "{auth}: {list_error}");

            tx.send(CustomCommand::Send(request("hello; $(rm)", None)))
                .await
                .unwrap();
            let sent = until_terminal(&mut rx).await;
            let send_error = sent.iter().find_map(|event| match event {
                AgentEvent::Error(error) => Some(error.to_string()),
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
                AgentEvent::Error(error) => Some(error.to_string()),
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

    // TC-1692 / FR-1679: cancel maps to Cancelled without NUL sentinels.
    #[tokio::test]
    async fn acp_cancel_emits_cancelled_event_without_sentinel() {
        // FR-1416: ACP cancel turns return AgentError with kind Cancelled,
        // which maps directly to AgentEvent::Cancelled without sentinel strings.
        let err = AgentError::cancelled("ACP turn cancelled");
        let event = match Err::<(), AgentError>(err) {
            Ok(()) => AgentEvent::Completed,
            Err(error) if error.kind == AgentErrorKind::Cancelled => AgentEvent::Cancelled,
            Err(error) => AgentEvent::Error(error),
        };
        assert_eq!(event, AgentEvent::Cancelled);
    }
    // TC-1717 (FR-1704): custom placeholders render
    #[tokio::test]
    async fn agent_prompt_custom_placeholders_render() {
        let toml = r#"
name = "sp-agent"
command = ["sh", "-c", "echo sys=[$SYS]; echo file=[$SYSFILE]; echo cfg=[$FAKE_CFG]; if [ -n \"$SYSFILE\" ]; then echo content=[$(cat \"$SYSFILE\")]; fi", "--", "{prompt}"]
temp_config_template = "{\"sp\":\"{system_prompt}\"}"
env = { FAKE_CFG = "{temp_config}", SYS = "{system_prompt}", SYSFILE = "{system_prompt_file}" }
system_prompt_arg = ["--system", "{system_prompt}"]
"#;
        let spec: &'static CustomAgentSpec = Box::leak(Box::new(toml::from_str(toml).unwrap()));
        let sp = dmuxai::agent_prompt::ResolvedPrompt {
            text: "SYS".to_owned(),
            hash: "h".to_owned(),
            source: "s".to_owned(),
            inject: dmuxai::agent_prompt::PromptInject::Session,
        };
        let (tx, mut rx) = mpsc::channel(64);
        let request = AgentRequest {
            conversation_id: ConversationId::new("c"),
            turn_id: TurnId::new("t"),
            prompt: "hi".to_owned(),
            resume_session: None,
            image_paths: Vec::new(),
            policy: TurnPolicy::Agent,
            require_file_edits: false,
            system_prompt: Some(sp),
        };
        let context = OutputContext::from(&request);
        let pgid = std::sync::atomic::AtomicU32::new(0);
        let cwd = std::env::temp_dir();
        let notice_shown = AtomicBool::new(false);
        let result = run_turn(
            Agent::Custom(0),
            spec,
            &request,
            None,
            TurnPolicy::Agent,
            &cwd,
            &tx,
            &context,
            &pgid,
            &RunnerMcp::default(),
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
        assert_eq!(result, Ok(()));
        assert!(text.contains("sys=[SYS]"), "{text}");
        assert!(text.contains("content=[SYS]"), "{text}");
        // file removed after turn
        let file_path = text
            .lines()
            .find_map(|l| l.strip_prefix("file=["))
            .and_then(|l| l.strip_suffix(']'))
            .unwrap();
        assert!(!std::path::Path::new(file_path).exists());
    }

    // TC-1717 (FR-1704): custom placeholders absent without prompt
    #[tokio::test]
    async fn agent_prompt_custom_placeholders_absent_without_prompt() {
        let toml = r#"
name = "sp-agent2"
command = ["sh", "-c", "echo sys=[$SYS]; echo file=[$SYSFILE]; echo cfg=[$FAKE_CFG]", "--", "{prompt}"]
temp_config_template = "{\"sp\":\"{system_prompt}\"}"
env = { FAKE_CFG = "{temp_config}", SYS = "{system_prompt}", SYSFILE = "{system_prompt_file}" }
system_prompt_arg = ["--system", "{system_prompt}"]
"#;
        let spec: &'static CustomAgentSpec = Box::leak(Box::new(toml::from_str(toml).unwrap()));
        let (tx, mut rx) = mpsc::channel(64);
        let request = AgentRequest {
            conversation_id: ConversationId::new("c"),
            turn_id: TurnId::new("t"),
            prompt: "hi".to_owned(),
            resume_session: None,
            image_paths: Vec::new(),
            policy: TurnPolicy::Agent,
            require_file_edits: false,
            system_prompt: None,
        };
        let context = OutputContext::from(&request);
        let pgid = std::sync::atomic::AtomicU32::new(0);
        let cwd = std::env::temp_dir();
        let notice_shown = AtomicBool::new(false);
        let result = run_turn(
            Agent::Custom(0),
            spec,
            &request,
            None,
            TurnPolicy::Agent,
            &cwd,
            &tx,
            &context,
            &pgid,
            &RunnerMcp::default(),
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
        assert_eq!(result, Ok(()));
        assert!(text.contains("sys=[]"), "{text}");
        assert!(text.contains("file=[]"), "{text}");
        assert!(!text.contains("--system"), "{text}");
    }

    // TC-1717 (FR-1704): opencode temp config instructions
    #[tokio::test]
    async fn agent_prompt_opencode_temp_config_instructions() {
        let spec = {
            let mut s = dmuxai::custom_agent::opencode_preset(&fixture());
            s.command = vec![fixture().to_owned(), "env".to_owned()];
            s.output = Some(dmuxai::custom_agent::CustomAgentOutput::Text);
            s.text_path = None;
            s.text_when = None;
            s.error_path = None;
            s.session_id_path = None;
            s.env
                .insert("FAKE_CFG".to_owned(), "{temp_config}".to_owned());
            Box::leak(Box::new(s))
        };
        let sp_text = "SYS instructions";
        let sp = dmuxai::agent_prompt::ResolvedPrompt {
            text: sp_text.to_owned(),
            hash: "h".to_owned(),
            source: "s".to_owned(),
            inject: dmuxai::agent_prompt::PromptInject::Session,
        };
        let (tx, mut rx) = mpsc::channel(64);
        let request = AgentRequest {
            conversation_id: ConversationId::new("c"),
            turn_id: TurnId::new("t"),
            prompt: "hi".to_owned(),
            resume_session: None,
            image_paths: Vec::new(),
            policy: TurnPolicy::Agent,
            require_file_edits: false,
            system_prompt: Some(sp),
        };
        let context = OutputContext::from(&request);
        let pgid = std::sync::atomic::AtomicU32::new(0);
        let cwd = std::env::temp_dir();
        let notice_shown = AtomicBool::new(false);
        let result = run_turn(
            Agent::Custom(0),
            spec,
            &request,
            None,
            TurnPolicy::Agent,
            &cwd,
            &tx,
            &context,
            &pgid,
            &RunnerMcp::default(),
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
        assert_eq!(result, Ok(()));
        assert!(text.contains("instructions"), "{text}");
        // prompt arg has no agent-instructions block
        assert!(!text.contains("<agent-instructions>"), "{text}");
    }

    // TC-1717 (FR-1704): temp files removed
    #[tokio::test]
    async fn agent_prompt_opencode_temp_files_removed() {
        let spec = {
            let mut s = dmuxai::custom_agent::opencode_preset(&fixture());
            s.command = vec![fixture().to_owned(), "env".to_owned()];
            s.output = Some(dmuxai::custom_agent::CustomAgentOutput::Text);
            s.text_path = None;
            s.text_when = None;
            s.error_path = None;
            s.session_id_path = None;
            s.env
                .insert("FAKE_CFG".to_owned(), "{temp_config}".to_owned());
            Box::leak(Box::new(s))
        };
        let sp = dmuxai::agent_prompt::ResolvedPrompt {
            text: "SYS".to_owned(),
            hash: "h".to_owned(),
            source: "s".to_owned(),
            inject: dmuxai::agent_prompt::PromptInject::Session,
        };
        let (tx, mut rx) = mpsc::channel(64);
        let request = AgentRequest {
            conversation_id: ConversationId::new("c"),
            turn_id: TurnId::new("t"),
            prompt: "hi".to_owned(),
            resume_session: None,
            image_paths: Vec::new(),
            policy: TurnPolicy::Agent,
            require_file_edits: false,
            system_prompt: Some(sp),
        };
        let context = OutputContext::from(&request);
        let pgid = std::sync::atomic::AtomicU32::new(0);
        let cwd = std::env::temp_dir();
        let notice_shown = AtomicBool::new(false);
        let _ = run_turn(
            Agent::Custom(0),
            spec,
            &request,
            None,
            TurnPolicy::Agent,
            &cwd,
            &tx,
            &context,
            &pgid,
            &RunnerMcp::default(),
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
        // extract any /tmp dmuxai instructions path and ensure removed
        for token in text.split_whitespace() {
            if token.contains("dmuxai-") && token.contains("instructions.txt") {
                let cleaned =
                    token.trim_matches(|c: char| c == ',' || c == '"' || c == '[' || c == ']');
                if !cleaned.is_empty() {
                    assert!(!std::path::Path::new(cleaned).exists(), "{cleaned}");
                }
            }
        }
    }
}
