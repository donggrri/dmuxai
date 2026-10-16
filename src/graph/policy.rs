use crate::model_policy::{ALLOWED_MODEL, validate_model_id as validate_policy_model};
use crate::settings::DmuxSettings;

use super::types::NodeRole;

pub const ORCHESTRATOR_MODEL: &str = ALLOWED_MODEL;
pub const RESEARCH_MODEL: &str = ALLOWED_MODEL;
pub const IMPLEMENT_MODEL: &str = ALLOWED_MODEL;
pub const PLAN_REVIEW_MODEL: &str = ALLOWED_MODEL;

/// Rejects any model other than composer-2.5 (and blocks `fast`).
pub fn validate_model_id(model_id: &str) -> Result<(), String> {
    validate_policy_model(model_id)
}

pub fn model_for_role(role: NodeRole) -> &'static str {
    match role {
        NodeRole::Orchestrate => ORCHESTRATOR_MODEL,
        NodeRole::Plan | NodeRole::Route | NodeRole::Review => PLAN_REVIEW_MODEL,
        NodeRole::Research | NodeRole::Explore | NodeRole::Test => RESEARCH_MODEL,
        NodeRole::Implement | NodeRole::Draft => IMPLEMENT_MODEL,
    }
}

pub fn default_limits() -> super::types::GraphLimits {
    super::types::GraphLimits::default()
}

pub fn assign_runner(_role: NodeRole, settings: &DmuxSettings) -> String {
    if let Some(default) = settings.routing_default_agent.as_deref() {
        let lower = default.trim().to_ascii_lowercase();
        if matches!(lower.as_str(), "agy" | "codex" | "cursor") {
            return lower;
        }
    }
    "cursor".to_owned()
}
