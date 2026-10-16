//! Enforces the repository-wide composer-2.5-only model policy.

pub const ALLOWED_MODEL: &str = "composer-2.5";

/// Role bucket aliases (all resolve to the only permitted model).
pub const ORCHESTRATOR_MODEL: &str = ALLOWED_MODEL;
pub const RESEARCH_MODEL: &str = ALLOWED_MODEL;
pub const IMPLEMENT_MODEL: &str = ALLOWED_MODEL;
pub const PLAN_REVIEW_MODEL: &str = ALLOWED_MODEL;

/// Returns true when `model_id` is exactly the allowed model (case-insensitive, trimmed).
pub fn is_allowed_model(model_id: &str) -> bool {
    model_id.trim().eq_ignore_ascii_case(ALLOWED_MODEL)
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
            "model {trimmed} is not allowed; only {ALLOWED_MODEL} is permitted"
        ));
    }
    Ok(())
}

/// Runner-aware validation: Cursor stays composer-2.5 only; AGY may use
/// `gemini-*-flash-*` models (the dogfooding runner); opencode may use `*-free`.
pub fn validate_model_for_runner(runner: &str, model_id: &str) -> Result<(), String> {
    let lower = model_id.trim().to_ascii_lowercase();
    match runner.trim().to_ascii_lowercase().as_str() {
        "agy" if lower.starts_with("gemini-") && lower.contains("-flash") => Ok(()),
        "opencode" if lower.starts_with("opencode/") && lower.ends_with("-free") => Ok(()),
        _ => validate_model_id(model_id),
    }
}

/// Default model for graph nodes, subagents, and auto-routing.
pub fn default_model() -> &'static str {
    ALLOWED_MODEL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_composer_2_5_is_allowed() {
        assert!(is_allowed_model("composer-2.5"));
        assert!(is_allowed_model(" Composer-2.5 "));
        assert!(!is_allowed_model("claude-haiku-5-5-high"));
        assert!(!is_allowed_model("grok-4.7-medium-fast"));
    }

    #[test]
    fn validate_rejects_fast_and_foreign_models() {
        assert!(validate_model_id("composer-2.5").is_ok());
        assert!(validate_model_id("composer-2.5-fast").is_err());
        assert!(validate_model_id("grok-4.7-medium").is_err());
        assert!(validate_model_id("claude-haiku-5-5-high").is_err());
        assert!(validate_model_id("claude-sonnet-5-5").is_err());
        assert!(validate_model_id("gpt-5").is_err());
        assert!(validate_model_id("gemini-4").is_err());
    }

    #[test]
    fn runner_aware_policy_allows_agy_flash_only_on_agy() {
        assert!(validate_model_for_runner("agy", "gemini-3.8-flash-medium").is_ok());
        assert!(validate_model_for_runner("agy", "composer-2.5").is_ok());
        assert!(validate_model_for_runner("agy", "gemini-4-pro").is_err());
        assert!(validate_model_for_runner("cursor", "gemini-3.8-flash-medium").is_err());
        assert!(validate_model_for_runner("opencode", "opencode/glm-5.3-flash-free").is_ok());
        assert!(validate_model_for_runner("cursor", "opencode/glm-5.3-flash-free").is_err());
    }
}
