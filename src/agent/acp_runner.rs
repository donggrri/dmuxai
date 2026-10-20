//! Shared ACP turn helper for Cursor, grok, and opencode (FR-1300, FR-1301, FR-1320).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::sync::mpsc;

use super::acp::{
    AcpLifecycle, AcpRunnerKind, AcpTurnConfig, PermissionContext, new_cancel_flag, run_acp_turn,
};

pub fn acp_cancel_for_turn() -> Arc<AtomicBool> {
    new_cancel_flag()
}
use super::{Agent, AgentRequest, OutputContext, prompt_with_image_tokens};

#[allow(clippy::too_many_arguments)]
pub async fn run_agent_acp_turn(
    program: &Path,
    args: &[String],
    cwd: &Path,
    request: &AgentRequest,
    agent: Agent,
    event_tx: &mpsc::Sender<super::AgentOutput>,
    permission_ctx: PermissionContext,
    runner: AcpRunnerKind,
    lifecycle: AcpLifecycle,
    cancel: Arc<AtomicBool>,
    requested_model: Option<String>,
    session_config_model: Option<&str>,
    reasoning_effort: Option<&str>,
) -> Result<Option<String>, String> {
    let context = OutputContext::from(request);
    let prompt = prompt_with_image_tokens(&request.prompt, &request.image_paths);
    let config = AcpTurnConfig {
        runner,
        policy: request.policy,
        lifecycle,
        requested_model,
        session_config_model: session_config_model.map(str::to_owned),
        reasoning_effort: reasoning_effort.map(str::to_owned),
        cancel,
    };
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
        config,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(outcome.session_id)
}

pub fn cursor_acp_argv(model: Option<&str>, cwd: &Path) -> Vec<String> {
    let mut args = vec!["acp".to_owned()];
    if let Some(model) = model
        && dmuxai::model_policy::validate_model_for_runner("cursor", model).is_ok()
    {
        args.push("--model".to_owned());
        args.push(model.to_owned());
    }
    // Best-effort: match legacy stream-json sandbox when the ACP CLI supports it (OQ-1326).
    args.push("--sandbox".to_owned());
    args.push("enabled".to_owned());
    args.push("--workspace".to_owned());
    args.push(cwd.to_string_lossy().into_owned());
    args
}

/// Grok requires global flags before the `stdio` subcommand (FR-1316).
pub fn grok_acp_argv(model: &str, reasoning_effort: Option<&str>) -> Vec<String> {
    let mut args = vec!["agent".to_owned(), "-m".to_owned(), model.to_owned()];
    if let Some(effort) = reasoning_effort.filter(|e| !e.is_empty()) {
        args.push("--reasoning-effort".to_owned());
        args.push(effort.to_owned());
    }
    args.push("stdio".to_owned());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    // TC-1316 / FR-1301: grok global flags precede `stdio`.
    #[test]
    fn grok_acp_argv_puts_model_before_stdio() {
        let args = grok_acp_argv("grok-4.7", Some("high"));
        assert_eq!(
            args,
            vec![
                "agent",
                "-m",
                "grok-4.7",
                "--reasoning-effort",
                "high",
                "stdio"
            ]
        );
        assert!(!args.iter().any(|a| a == "--always-approve"));
    }
}
