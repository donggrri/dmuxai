//! Graph-era model policy: role buckets + fast-model rejection.

pub const ORCHESTRATOR_MODEL: &str = "grok-4.7-medium";
pub const RESEARCH_MODEL: &str = "claude-haiku-5-5-high";
pub const IMPLEMENT_MODEL: &str = "composer-2.5";
pub const PLAN_REVIEW_MODEL: &str = "grok-4.7-medium";

/// Legacy alias used in docs/tests that still mention a single default implement model.
pub const ALLOWED_MODEL: &str = IMPLEMENT_MODEL;

const ALLOWED: &[&str] = &[
    ORCHESTRATOR_MODEL,
    RESEARCH_MODEL,
    IMPLEMENT_MODEL,
    PLAN_REVIEW_MODEL,
];

/// Returns true when `model_id` is on the graph allow-list (case-insensitive).
pub fn is_allowed_model(model_id: &str) -> bool {
    let trimmed = model_id.trim();
    ALLOWED
        .iter()
        .any(|candidate| trimmed.eq_ignore_ascii_case(candidate))
}

/// Rejects disallowed model ids used in routing, graphs, or agent defaults.
pub fn validate_model_id(model_id: &str) -> Result<(), String> {
    let trimmed = model_id.trim();
    if trimmed.is_empty() {
        return Err("model id must not be empty".to_owned());
    }
    if trimmed.to_ascii_lowercase().contains("fast") {
        return Err(format!("fast models are blocked by policy: {trimmed}"));
    }
    if !is_allowed_model(trimmed) {
        return Err(format!(
            "model {trimmed} is not allowed; permitted: {}",
            ALLOWED.join(", ")
        ));
    }
    Ok(())
}

/// Default model for implement/draft graph nodes.
pub fn default_model() -> &'static str {
    IMPLEMENT_MODEL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_models_are_allowed() {
        assert!(is_allowed_model("composer-2.5"));
        assert!(is_allowed_model("grok-4.7-medium"));
        assert!(is_allowed_model("claude-haiku-5-5-high"));
    }

    #[test]
    fn validate_rejects_fast_and_foreign_models() {
        assert!(validate_model_id("composer-2.5").is_ok());
        assert!(validate_model_id("composer-2.5-fast").is_err());
        assert!(validate_model_id("gpt-5").is_err());
    }
}
