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

pub fn is_runner_allowed(runner: &str, settings: &DmuxSettings) -> bool {
    match &settings.graph_allowed_runners {
        Some(allowed) => allowed
            .iter()
            .any(|r| r.eq_ignore_ascii_case(runner.trim())),
        None => true,
    }
}

pub fn assign_runner(_role: NodeRole, settings: &DmuxSettings) -> String {
    if let Some(allowed) = &settings.graph_allowed_runners {
        if allowed.is_empty() {
            return String::new();
        }
        if let Some(default) = settings.routing_default_agent.as_deref() {
            let lower = default.trim().to_ascii_lowercase();
            if allowed.contains(&lower) {
                return lower;
            }
        }
        if allowed.iter().any(|r| r == "cursor") {
            return "cursor".to_owned();
        }
        return allowed[0].clone();
    }

    if let Some(default) = settings.routing_default_agent.as_deref() {
        let lower = default.trim().to_ascii_lowercase();
        if matches!(lower.as_str(), "agy" | "codex" | "cursor")
            || crate::custom_agent::index_of(&lower).is_some()
        {
            return lower;
        }
    }
    "cursor".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assign_runner_honors_allowed_runners_and_order() {
        // TC-430, TC-431, FR-431
        let mut settings = DmuxSettings::default();

        // 1. None: baseline default is cursor
        assert_eq!(assign_runner(NodeRole::Implement, &settings), "cursor");

        // 2. Routing default agent when allowed
        settings.routing_default_agent = Some("agy".to_owned());
        settings.graph_allowed_runners = Some(vec!["agy".to_owned(), "cursor".to_owned()]);
        assert_eq!(assign_runner(NodeRole::Implement, &settings), "agy");

        // 3. Routing default agent NOT allowed -> fallback to cursor if cursor is allowed
        settings.routing_default_agent = Some("codex".to_owned());
        settings.graph_allowed_runners = Some(vec!["cursor".to_owned(), "agy".to_owned()]);
        assert_eq!(assign_runner(NodeRole::Implement, &settings), "cursor");

        // 4. Neither default nor cursor allowed -> fallback to first allowed runner
        settings.routing_default_agent = Some("cursor".to_owned());
        settings.graph_allowed_runners = Some(vec!["agy".to_owned(), "opencode".to_owned()]);
        assert_eq!(assign_runner(NodeRole::Implement, &settings), "agy");

        // 5. Empty allowed list returns empty string
        settings.graph_allowed_runners = Some(Vec::new());
        assert_eq!(assign_runner(NodeRole::Implement, &settings), "");

        // is_runner_allowed checks
        settings.graph_allowed_runners = Some(vec!["agy".to_owned()]);
        assert!(is_runner_allowed("agy", &settings));
        assert!(is_runner_allowed("AGY", &settings));
        assert!(!is_runner_allowed("cursor", &settings));
        assert!(!is_runner_allowed("codex", &settings));

        settings.graph_allowed_runners = None;
        assert!(is_runner_allowed("cursor", &settings));
        assert!(is_runner_allowed("agy", &settings));
    }
}
