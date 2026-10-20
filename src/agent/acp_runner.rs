//! Shared ACP turn helper for Cursor and grok (FR-1300, FR-1301).

use std::path::Path;

use tokio::sync::mpsc;

use super::acp::{PermissionContext, run_acp_turn};
use super::{Agent, AgentRequest, OutputContext, prompt_with_image_tokens};

pub async fn run_agent_acp_turn(
    program: &Path,
    args: &[String],
    cwd: &Path,
    request: &AgentRequest,
    agent: Agent,
    event_tx: &mpsc::Sender<super::AgentOutput>,
    permission_ctx: PermissionContext,
) -> Result<Option<String>, String> {
    let context = OutputContext::from(request);
    let prompt = prompt_with_image_tokens(&request.prompt, &request.image_paths);
    let outcome = run_acp_turn(
        program,
        args,
        cwd,
        &prompt,
        request.resume_session.as_deref(),
        permission_ctx,
        event_tx,
        agent,
        &context,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(outcome.session_id)
}

pub fn cursor_acp_argv(model: Option<&str>, cwd: &Path) -> Vec<String> {
    let mut args = vec!["acp".to_owned()];
    if let Some(model) = model {
        if dmuxai::model_policy::validate_model_for_runner("cursor", model).is_ok() {
            args.push("--model".to_owned());
            args.push(model.to_owned());
        }
    }
    args.push("--workspace".to_owned());
    args.push(cwd.to_string_lossy().into_owned());
    args
}

pub fn grok_acp_argv(model: &str) -> Vec<String> {
    vec![
        "agent".to_owned(),
        "stdio".to_owned(),
        "-m".to_owned(),
        model.to_owned(),
        "--always-approve".to_owned(),
    ]
}
