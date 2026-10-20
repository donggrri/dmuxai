//! Session open helpers and Cursor fast-model policy (FR-1313).

use serde_json::{Value, json};

use crate::agent::TurnPolicy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOpenNote {
    New,
    Loaded,
    Resumed,
    NewAfterResumeFailed, // used when resume id was provided but session/new ran
}

#[derive(Debug, Clone, Default)]
pub struct AgentCapabilities {
    pub load_session: bool,
    pub resume_session: bool,
}

pub fn parse_agent_capabilities(init_result: &Value) -> AgentCapabilities {
    let blob = init_result.to_string().to_ascii_lowercase();
    AgentCapabilities {
        load_session: blob.contains("loadsession") || blob.contains("\"load\""),
        resume_session: blob.contains("resume"),
    }
}

pub fn is_fast_model_label(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    lower.contains("fast") || lower.contains("[fast=true]")
}

/// After `session/new` or load, inspect reported model and reject stuck fast variants.
pub fn cursor_acp_model_check(
    session_result: &Value,
    requested_model: Option<&str>,
) -> Result<String, String> {
    let effective = session_result
        .get("model")
        .or_else(|| session_result.pointer("/config/model"))
        .or_else(|| session_result.pointer("/currentModel"))
        .and_then(Value::as_str)
        .or(requested_model)
        .map(str::to_owned)
        .unwrap_or_else(|| "unknown".to_owned());

    if is_fast_model_label(&effective) {
        return Err(format!(
            "Cursor ACP session is using fast model `{effective}` and no non-fast composer model is available. \
Set `[cursor] protocol = \"legacy\"` in dmuxai.toml or choose a non-fast model in Cursor."
        ));
    }
    Ok(effective)
}

pub fn read_only_acp_mode(policy: TurnPolicy) -> Option<&'static str> {
    match policy {
        TurnPolicy::ReadOnly => Some("ask"),
        TurnPolicy::Agent => None,
    }
}

pub fn session_set_mode_params(session_id: &str, mode: &str) -> Value {
    json!({
        "sessionId": session_id,
        "mode": mode
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // TC-1318 / FR-1313: reject Cursor ACP fast-only models.
    #[test]
    fn cursor_acp_rejects_fast_model_report() {
        let payload = json!({ "model": "composer-2.5[fast=true]" });
        let err = cursor_acp_model_check(&payload, Some("composer-2.5")).unwrap_err();
        assert!(err.contains("fast"), "{err}");
    }
}
