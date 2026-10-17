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

/// Whether a runner may take graph nodes (FR-813: the built-in opencode preset
/// is excluded while `allowed_runners` is unset).
pub fn is_runner_allowed(runner: &str, settings: &DmuxSettings) -> bool {
    settings.graph_runner_allowed(runner)
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
        let known = matches!(lower.as_str(), "agy" | "codex" | "cursor")
            || crate::custom_agent::index_of(&lower).is_some()
            || settings
                .custom_agents
                .iter()
                .any(|spec| spec.name.eq_ignore_ascii_case(&lower));
        if known && settings.graph_runner_allowed(&lower) {
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

    #[test]
    fn builtin_opencode_is_not_graph_routed_unless_listed() {
        // FR-813 (SPEC-first-run): the built-in preset takes no graph nodes while
        // allowed_runners is unset, even as the routing default.
        let mut settings = DmuxSettings::default();
        settings
            .custom_agents
            .push(crate::custom_agent::opencode_preset("opencode"));
        settings.routing_default_agent = Some("opencode".to_owned());
        assert!(!is_runner_allowed("opencode", &settings));
        assert_eq!(assign_runner(NodeRole::Implement, &settings), "cursor");

        settings.graph_allowed_runners = Some(vec!["opencode".to_owned()]);
        assert!(is_runner_allowed("opencode", &settings));
        assert_eq!(assign_runner(NodeRole::Implement, &settings), "opencode");
    }
}
