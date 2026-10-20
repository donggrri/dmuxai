//! Session open helpers and Cursor fast-model policy (FR-1313).

use std::path::Path;

use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOpenNote {
    New,
    Loaded,
    Resumed,
    NewAfterResumeFailed,
}

#[derive(Debug, Clone, Default)]
pub struct AgentCapabilities {
    pub load_session: bool,
    pub resume_session: bool,
    pub set_model: bool,
    pub set_config_option: bool,
}

pub fn parse_agent_capabilities(init_result: &Value) -> AgentCapabilities {
    let blob = init_result.to_string().to_ascii_lowercase();
    AgentCapabilities {
        load_session: blob.contains("loadsession") || blob.contains("session/load"),
        resume_session: blob.contains("resume") || blob.contains("session/resume"),
        set_model: blob.contains("set_model") || blob.contains("session/set_model"),
        set_config_option: blob.contains("set_config_option")
            || blob.contains("session/set_config_option"),
    }
}

pub fn session_load_params(session_id: &str, cwd: &Path) -> Value {
    json!({
        "sessionId": session_id,
        "cwd": cwd.to_string_lossy(),
        "mcpServers": []
    })
}

pub fn session_resume_params(session_id: &str, cwd: &Path) -> Value {
    session_load_params(session_id, cwd)
}

pub fn is_fast_model_label(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    lower.contains("fast") || lower.contains("[fast=true]")
}

pub fn model_family_base(model: &str) -> String {
    let stem = model.split('[').next().unwrap_or(model).trim();
    stem.strip_suffix("-fast")
        .unwrap_or(stem)
        .trim_end_matches("[fast=true]")
        .to_owned()
}

pub fn extract_reported_model(session_result: &Value) -> Option<String> {
    session_result
        .pointer("/models/currentModelId")
        .or_else(|| session_result.pointer("/models/current_model_id"))
        .or_else(|| session_result.get("model"))
        .or_else(|| session_result.pointer("/config/model"))
        .or_else(|| session_result.pointer("/currentModel"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

pub fn available_model_ids(session_result: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let arrays = [
        session_result.pointer("/models/availableModels"),
        session_result.pointer("/models/available_models"),
        session_result.get("availableModels"),
    ];
    for arr in arrays.into_iter().flatten() {
        collect_id_list(arr, &mut out);
    }
    out
}

pub fn available_mode_ids(session_result: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let arrays = [
        session_result.pointer("/modes/availableModes"),
        session_result.pointer("/modes/available_modes"),
        session_result.get("availableModes"),
    ];
    for arr in arrays.into_iter().flatten() {
        collect_id_list(arr, &mut out);
    }
    out
}

fn collect_id_list(arr: &Value, out: &mut Vec<String>) {
    if let Some(items) = arr.as_array() {
        for item in items {
            if let Some(s) = item.as_str() {
                out.push(s.to_owned());
            } else if let Some(id) = item.get("id").and_then(Value::as_str) {
                out.push(id.to_owned());
            } else if let Some(id) = item.get("modeId").and_then(Value::as_str) {
                out.push(id.to_owned());
            } else if let Some(id) = item.get("modelId").and_then(Value::as_str) {
                out.push(id.to_owned());
            }
        }
    }
}

/// Pick a non-fast model id in the same family as `requested` (e.g. composer-2.5 only).
pub fn pick_non_fast_model(candidates: &[String], requested: Option<&str>) -> Option<String> {
    let base = requested.map(model_family_base)?;
    candidates
        .iter()
        .find(|id| !is_fast_model_label(id) && model_family_base(id) == base)
        .cloned()
}

const READ_ONLY_MODE_PREFERENCES: &[&str] = &[
    "ask",
    "plan",
    "read-only",
    "readonly",
    "read_only",
    "review",
];

/// Choose a read-only `modeId` from session payload (`modes.availableModes`).
pub fn pick_read_only_mode_id(session_result: &Value) -> Option<String> {
    let modes = available_mode_ids(session_result);
    if modes.is_empty() {
        return None;
    }
    for preferred in READ_ONLY_MODE_PREFERENCES {
        if let Some(found) = modes.iter().find(|id| id.eq_ignore_ascii_case(preferred)) {
            return Some(found.clone());
        }
    }
    None
}

pub fn read_only_turn_refusal(agent: &str) -> String {
    format!(
        "ACP runner `{agent}` has no read-only mode available for this internal turn; refusing to run writable agent mode."
    )
}

/// Inspect session payload; does not call ACP (used after open when no model switch is needed).
pub fn cursor_acp_model_check(
    session_result: &Value,
    requested_model: Option<&str>,
) -> Result<String, String> {
    let effective = extract_reported_model(session_result)
        .or_else(|| requested_model.map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned());

    if is_fast_model_label(&effective) {
        return Err(fast_model_refusal(&effective));
    }
    Ok(effective)
}

pub fn fast_model_refusal(effective: &str) -> String {
    format!(
        "Cursor ACP session is using fast model `{effective}` and no non-fast composer model could be selected. \
Set `[cursor] protocol = \"legacy\"` in dmuxai.toml or choose a non-fast model in Cursor."
    )
}

pub fn session_set_mode_params(session_id: &str, mode_id: &str) -> Value {
    json!({
        "sessionId": session_id,
        "modeId": mode_id
    })
}

pub fn session_set_model_params(session_id: &str, model_id: &str) -> Value {
    json!({
        "sessionId": session_id,
        "modelId": model_id
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    // TC-1318 / FR-1313: reject Cursor ACP fast-only models.
    #[test]
    fn cursor_acp_rejects_fast_model_report() {
        let payload = json!({ "models": { "currentModelId": "composer-2.5[fast=true]" } });
        let err = cursor_acp_model_check(&payload, Some("composer-2.5")).unwrap_err();
        assert!(err.contains("fast"), "{err}");
    }

    // TC-1319 / FR-1313: read currentModelId not requested id.
    #[test]
    fn cursor_acp_reads_models_current_model_id() {
        let payload = json!({
            "models": {
                "currentModelId": "composer-2.5",
                "availableModels": ["composer-2.5", "composer-2.5-fast"]
            }
        });
        assert_eq!(
            cursor_acp_model_check(&payload, Some("composer-2.5[fast=true]")).unwrap(),
            "composer-2.5"
        );
    }

    // TC-1328 / FR-1313: do not switch to unrelated non-fast models.
    #[test]
    fn pick_non_fast_model_stays_in_requested_family() {
        let candidates = vec![
            "auto-smart".to_owned(),
            "claude-opus-5-5".to_owned(),
            "composer-2.5".to_owned(),
        ];
        assert_eq!(
            pick_non_fast_model(&candidates, Some("composer-2.5[fast=true]")).as_deref(),
            Some("composer-2.5")
        );
        assert!(pick_non_fast_model(&candidates, Some("grok-4.7")).is_none());
    }

    // TC-1323 / FR-1316: session/load params include cwd and mcpServers.
    #[test]
    fn session_load_params_schema() {
        let params = session_load_params("sid-1", Path::new("/proj"));
        assert_eq!(params["sessionId"], "sid-1");
        assert_eq!(params["cwd"], "/proj");
        assert_eq!(params["mcpServers"], json!([]));
    }

    // TC-1324: session/set_mode uses modeId.
    #[test]
    fn session_set_mode_uses_mode_id() {
        let params = session_set_mode_params("sid", "ask");
        assert_eq!(params["modeId"], "ask");
        assert!(params.get("mode").is_none());
    }

    #[test]
    fn pick_read_only_mode_from_available_modes() {
        let payload = json!({
            "modes": { "availableModes": ["agent", "plan", "debug"] }
        });
        assert_eq!(pick_read_only_mode_id(&payload).as_deref(), Some("plan"));
    }
}
