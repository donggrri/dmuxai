use crate::settings::DmuxSettings;

use super::types::NodeRole;

pub const ORCHESTRATOR_MODEL: &str = "grok-4.7-medium";
pub const RESEARCH_MODEL: &str = "claude-haiku-5-5-high";
pub const IMPLEMENT_MODEL: &str = "composer-2.5";
pub const PLAN_REVIEW_MODEL: &str = "grok-4.7-medium";

/// Rejects disallowed model ids (fast substring or unknown bucket).
pub fn validate_model_id(model_id: &str) -> Result<(), String> {
    let trimmed = model_id.trim();
    if trimmed.is_empty() {
        return Err("model id must not be empty".to_owned());
    }
    if trimmed.to_ascii_lowercase().contains("fast") {
        return Err(format!("fast models are blocked by policy: {trimmed}"));
    }
    let allowed = [
        ORCHESTRATOR_MODEL,
        RESEARCH_MODEL,
        IMPLEMENT_MODEL,
        PLAN_REVIEW_MODEL,
    ];
    if allowed
        .iter()
        .any(|candidate| trimmed.eq_ignore_ascii_case(candidate))
    {
        Ok(())
    } else {
        Err(format!(
            "model {trimmed} is not in the graph policy allow-list"
        ))
    }
}

pub fn model_for_role(role: NodeRole) -> &'static str {
    match role {
        NodeRole::Orchestrate | NodeRole::Plan | NodeRole::Route | NodeRole::Review => {
            PLAN_REVIEW_MODEL
        }
        NodeRole::Research | NodeRole::Explore | NodeRole::Test => RESEARCH_MODEL,
        NodeRole::Implement | NodeRole::Draft => IMPLEMENT_MODEL,
    }
}

pub fn default_limits() -> super::types::GraphLimits {
    super::types::GraphLimits::default()
}

pub fn assign_runner(role: NodeRole, settings: &DmuxSettings) -> String {
    if let Some(default) = settings.routing_default_agent.as_deref() {
        let lower = default.trim().to_ascii_lowercase();
        if matches!(lower.as_str(), "agy" | "codex" | "cursor") {
            return lower;
        }
    }
    match role {
        NodeRole::Implement | NodeRole::Draft => "cursor".to_owned(),
        NodeRole::Research | NodeRole::Explore | NodeRole::Test => "cursor".to_owned(),
        NodeRole::Orchestrate | NodeRole::Plan | NodeRole::Route | NodeRole::Review => {
            "cursor".to_owned()
        }
    }
}
