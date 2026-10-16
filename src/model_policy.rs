//! Enforces the repository-wide composer-2.5-only model policy.

pub const ALLOWED_MODEL: &str = "composer-2.5";

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
        assert!(validate_model_id("gpt-5").is_err());
    }
}
