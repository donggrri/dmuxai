//! Shared ACP turn helper for Cursor and grok (FR-1300, FR-1301, FR-1412).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::sync::mpsc;

use super::acp::{
    AcpLifecycle, AcpRunnerKind, AcpTurnConfig, PermissionContext, new_cancel_flag, run_acp_turn,
};
use super::{Agent, AgentRequest, OutputContext, prompt_with_image_tokens};

pub fn acp_cancel_for_turn() -> Arc<AtomicBool> {
    new_cancel_flag()
}

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
    mcp_servers: serde_json::Value,
) -> Result<AcpTurnResult, String> {
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
        mcp_servers,
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
    Ok(AcpTurnResult {
        cancelled: outcome.cancelled(),
        session_id: outcome.session_id,
    })
}

/// Result of one ACP turn for the caller's lifecycle handling (FR-1416).
#[derive(Debug, Clone, Default)]
pub struct AcpTurnResult {
    pub session_id: Option<String>,
    pub cancelled: bool,
}

/// Validate a requested model for an ACP runner; never drop it silently (FR-1415).
pub fn acp_model_arg(runner: &str, model: Option<&str>) -> Result<Option<String>, String> {
    match model.map(str::trim).filter(|m| !m.is_empty()) {
        None => Ok(None),
        Some(model) => dmuxai::model_policy::validate_model_for_runner(runner, model)
            .map(|()| Some(model.to_owned()))
            .map_err(|error| format!("{runner} ACP refused model `{model}`: {error}")),
    }
}

/// Cursor ACP argv: root options before the `acp` subcommand (FR-1300, FR-1415).
pub fn cursor_acp_argv(model: Option<&str>, cwd: &Path) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    if let Some(model) = acp_model_arg("cursor", model)? {
        args.push("--model".to_owned());
        args.push(model);
    }
    // Best-effort: match legacy stream-json sandbox when the ACP CLI supports it (OQ-1326).
    args.push("--sandbox".to_owned());
    args.push("enabled".to_owned());
    args.push("--workspace".to_owned());
    args.push(cwd.to_string_lossy().into_owned());
    args.push("acp".to_owned());
    Ok(args)
}

/// Grok ACP argv (FR-1412): root flags (`--permission-mode`) precede `agent`;
/// agent flags (`-m`, `--reasoning-effort`) precede `stdio`.
pub fn grok_acp_argv(model: &str, reasoning_effort: Option<&str>, read_only: bool) -> Vec<String> {
    let mut args = Vec::new();
    if read_only {
        args.push("--permission-mode".to_owned());
        args.push("plan".to_owned());
    }
    args.extend(["agent".to_owned(), "-m".to_owned(), model.to_owned()]);
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

    // TC-1316, TC-1414 / FR-1301, FR-1412: agent-mode grok argv.
    #[test]
    fn grok_acp_argv_puts_model_before_stdio() {
        let args = grok_acp_argv("grok-4.7", Some("high"), false);
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
        assert!(
            !args
                .iter()
                .any(|a| a == "--always-approve" || a == "--permission-mode")
        );
    }

    // TC-1413 / FR-1412: read-only grok puts the root flag before `agent`.
    #[test]
    fn tc_1413_grok_read_only_root_flag_before_agent() {
        let args = grok_acp_argv("grok-4.7", None, true);
        assert_eq!(
            args,
            vec![
                "--permission-mode",
                "plan",
                "agent",
                "-m",
                "grok-4.7",
                "stdio"
            ]
        );
    }

    // FR-1300: cursor root options precede `acp`.
    #[test]
    fn cursor_acp_argv_root_options_before_acp() {
        let args = cursor_acp_argv(Some("composer-2.5"), std::path::Path::new("/p")).unwrap();
        assert_eq!(args.last().map(String::as_str), Some("acp"));
        assert_eq!(&args[..2], ["--model", "composer-2.5"]);
    }

    // TC-1419 / FR-1415: disallowed model fails instead of being dropped.
    #[test]
    fn tc_1419_disallowed_model_is_an_error() {
        assert!(cursor_acp_argv(Some("composer-2.5-fast"), std::path::Path::new("/p")).is_err());
        assert!(acp_model_arg("grok", Some("grok-4.7-build-fast")).is_err());
        assert_eq!(
            acp_model_arg("grok", Some("grok-4.7")).unwrap().as_deref(),
            Some("grok-4.7")
        );
        assert_eq!(acp_model_arg("cursor", None).unwrap(), None);
    }
}
