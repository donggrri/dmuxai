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
/// `gemini-*-flash-*` models (the dogfooding runner); opencode may use `*-free`;
/// grok may use `grok-*` ids that do not contain `fast` (FR-910, FR-911).
pub fn validate_model_for_runner(runner: &str, model_id: &str) -> Result<(), String> {
    let trimmed = model_id.trim();
    if trimmed.to_ascii_lowercase().contains("fast") {
        return Err(format!("fast models are blocked by policy: {trimmed}"));
    }
    let runner_key = runner.trim().to_ascii_lowercase();
    // FR-1460: the provider-default sentinel (no `--model`) is every
    // non-Cursor runner's own default; Cursor stays pinned to composer-2.5.
    if trimmed.eq_ignore_ascii_case(PROVIDER_DEFAULT) {
        return if default_model_for_runner(&runner_key).is_none() {
            Ok(())
        } else {
            Err(format!(
                "model {trimmed} is not allowed for {runner_key}; only {ALLOWED_MODEL} is permitted"
            ))
        };
    }
    if crate::custom_agent::is_grok_model_id(trimmed) {
        return if runner_key == "grok" {
            Ok(())
        } else {
            Err(format!(
                "model {trimmed} is not allowed for {runner_key}; only the grok runner accepts grok models"
            ))
        };
    }
    let lower = trimmed.to_ascii_lowercase();
    match runner_key.as_str() {
        "agy" if lower.starts_with("gemini-") && lower.contains("-flash") => Ok(()),
        "opencode" if lower.starts_with("opencode/") && lower.ends_with("-free") => Ok(()),
        "cursor" | "agy" | "codex" => validate_model_id(model_id),
        "grok" => Err(format!(
            "model {trimmed} is not allowed for grok; only grok-* models without fast are permitted"
        )),
        other => {
            match crate::custom_agent::index_of(other).and_then(crate::custom_agent::spec_at) {
                Some(spec) => crate::custom_agent::validate_custom_model(spec, model_id),
                None => validate_model_id(model_id),
            }
        }
    }
}

/// Default model for graph nodes, subagents, and auto-routing.
pub fn default_model() -> &'static str {
    ALLOWED_MODEL
}

/// Sentinel for "let the provider pick its own default" (no `--model`).
pub const PROVIDER_DEFAULT: &str = "provider default";

/// Per-runner default when no model was selected (SPEC FR-1460): Cursor gets
/// the pinned non-fast `composer-2.5`; every other runner gets `None`
/// (provider default). Never another runner's model, never a fast model.
pub fn default_model_for_runner(runner: &str) -> Option<&'static str> {
    if runner.trim().eq_ignore_ascii_case("cursor") {
        Some(ALLOWED_MODEL)
    } else {
        None
    }
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

    // TC-910 / FR-910
    #[test]
    fn fast_models_are_rejected_for_every_runner() {
        let runners = ["cursor", "agy", "opencode", "grok", "codex"];
        let fast_ids = [
            "composer-2.5-fast",
            "grok-4.7-build-fast",
            "gemini-3-flash-fast",
            "opencode/glm-fast-free",
        ];
        for runner in runners {
            for id in fast_ids {
                let error = validate_model_for_runner(runner, id).unwrap_err();
                assert!(
                    error.contains("fast models are blocked by policy"),
                    "{runner} {id}: {error}"
                );
                assert!(error.contains(id), "{runner} {id}: {error}");
            }
        }
        assert!(validate_model_for_runner("cursor", "composer-2.5").is_ok());
        assert!(validate_model_for_runner("agy", "gemini-3.8-flash-medium").is_ok());
        assert!(validate_model_for_runner("opencode", "opencode/glm-5.3-flash-free").is_ok());
    }

    // TC-911 / FR-911
    #[test]
    fn grok_models_are_allowed_only_for_the_grok_runner() {
        for id in ["grok-4.7", "grok-4.6", "Grok-4.7"] {
            assert!(validate_model_for_runner("grok", id).is_ok(), "{id}");
            assert!(validate_model_for_runner("GROK", id).is_ok(), "{id}");
            for runner in ["cursor", "agy", "codex", "opencode"] {
                assert!(
                    validate_model_for_runner(runner, id).is_err(),
                    "{runner} {id}"
                );
            }
        }
        assert!(validate_model_for_runner("cursor", "composer-2.5").is_ok());
        assert!(validate_model_for_runner("cursor", "grok-4.7").is_err());
        assert!(validate_model_for_runner("grok", "composer-2.5").is_err());
        assert!(validate_model_for_runner("grok", "grok").is_err());
        assert!(validate_model_for_runner("grok", "opencode/glm-5.3-flash-free").is_err());
        assert!(validate_model_for_runner("opencode", "opencode/glm-5.3-flash-free").is_ok());

        // Grok ids are rejected before any custom-agent lookup, so an unknown
        // or custom runner never accepts them (no global registry mutation).
        assert!(validate_model_for_runner("zz-grok-policy", "grok-4.7").is_err());
        assert!(validate_model_for_runner("grok", "composer-2.5").is_err());
    }

    #[test]
    fn tc_1461_default_model_for_runner_is_per_runner() {
        assert_eq!(default_model_for_runner("cursor"), Some("composer-2.5"));
        assert_eq!(default_model_for_runner("Cursor"), Some("composer-2.5"));
        for runner in ["grok", "agy", "opencode", "codex", "custom-x"] {
            assert_eq!(default_model_for_runner(runner), None, "{runner}");
        }
        assert!(validate_model_for_runner("grok", PROVIDER_DEFAULT).is_ok());
        assert!(validate_model_for_runner("cursor", PROVIDER_DEFAULT).is_err());
        let cursor = default_model_for_runner("cursor").unwrap();
        assert!(!cursor.contains("fast"));
        assert!(validate_model_for_runner("cursor", cursor).is_ok());
    }
}
