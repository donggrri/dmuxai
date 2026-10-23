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

/// Origin of a resolved model selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSource {
    Explicit,
    Selected,
    ConfigDefault,
    RunnerDefault,
    ProviderDefault,
}

impl ModelSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelSource::Explicit => "explicit",
            ModelSource::Selected => "selected",
            ModelSource::ConfigDefault => "config_default",
            ModelSource::RunnerDefault => "runner_default",
            ModelSource::ProviderDefault => "provider_default",
        }
    }
}

/// Candidate model strings gathered from the various sources.
pub struct ModelCandidates<'a> {
    pub explicit: Option<&'a str>,
    pub selected: Option<&'a str>,
    pub config_default: Option<&'a str>,
}

/// Result of resolving a single runner's effective model.
pub struct ModelResolution {
    pub model: Option<String>,
    pub source: ModelSource,
}

impl ModelResolution {
    pub fn label(&self) -> String {
        self.model
            .clone()
            .unwrap_or_else(|| PROVIDER_DEFAULT.to_owned())
    }

    pub fn resolve(runner: &str, candidates: ModelCandidates) -> Result<Self, String> {
        // 1. explicit
        if let Some(raw) = candidates.explicit {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                match validate_model_for_runner(runner, trimmed) {
                    Ok(()) => {
                        let model = if trimmed.eq_ignore_ascii_case(PROVIDER_DEFAULT) {
                            None
                        } else {
                            Some(trimmed.to_owned())
                        };
                        return Ok(ModelResolution {
                            model,
                            source: ModelSource::Explicit,
                        });
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        // 2. selected (ModelCatalog.selected)
        if let Some(raw) = candidates.selected {
            let trimmed = raw.trim();
            if !trimmed.is_empty() && validate_model_for_runner(runner, trimmed).is_ok() {
                let model = if trimmed.eq_ignore_ascii_case(PROVIDER_DEFAULT) {
                    None
                } else {
                    Some(trimmed.to_owned())
                };
                return Ok(ModelResolution {
                    model,
                    source: ModelSource::Selected,
                });
            }
        }

        // 3. config_default (custom default_model or [runners.<id>] model)
        if let Some(raw) = candidates.config_default {
            let trimmed = raw.trim();
            if !trimmed.is_empty() && validate_model_for_runner(runner, trimmed).is_ok() {
                let model = if trimmed.eq_ignore_ascii_case(PROVIDER_DEFAULT) {
                    None
                } else {
                    Some(trimmed.to_owned())
                };
                return Ok(ModelResolution {
                    model,
                    source: ModelSource::ConfigDefault,
                });
            }
        }

        // 4. runner default
        if let Some(m) = default_model_for_runner(runner) {
            return Ok(ModelResolution {
                model: Some(m.to_owned()),
                source: ModelSource::RunnerDefault,
            });
        }

        // 5. provider default
        Ok(ModelResolution {
            model: None,
            source: ModelSource::ProviderDefault,
        })
    }
}

/// Result of checking a resolved model against a runner's model catalog.
#[derive(Debug)]
pub enum CatalogCheck {
    NoCatalog,
    Listed,
    NotListed { hint: String },
}

pub fn check_catalog(
    model: Option<&str>,
    catalog: Option<&[String]>,
    catalog_command: Option<&str>,
) -> CatalogCheck {
    let cat = match catalog {
        Some(c) if !c.is_empty() => c,
        _ => return CatalogCheck::NoCatalog,
    };
    let m = match model {
        Some(raw) => {
            let t = raw.trim();
            if t.is_empty() {
                return CatalogCheck::NoCatalog;
            }
            t
        }
        None => return CatalogCheck::NoCatalog,
    };
    for item in cat {
        if item.trim() == m {
            return CatalogCheck::Listed;
        }
    }
    let hint = match catalog_command {
        Some(cmd) => format!(
            "model {} is not offered by '{}'; pick another model",
            m, cmd
        ),
        None => format!(
            "model {} is not offered by the runner's model list; pick another model",
            m
        ),
    };
    CatalogCheck::NotListed { hint }
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

    // TC-1734 / FR-1725: precedence explicit > selected > config > runner > provider
    #[test]
    fn tc_1734_precedence_chain() {
        let cands = ModelCandidates {
            explicit: Some("opencode/beam-free"),
            selected: Some("opencode/glm-5.3-flash-free"),
            config_default: Some("opencode/step-5-preview-free"),
        };
        let r = ModelResolution::resolve("opencode", cands).unwrap();
        assert_eq!(r.source, ModelSource::Explicit);
        assert_eq!(r.model.as_deref(), Some("opencode/beam-free"));

        let cands = ModelCandidates {
            explicit: None,
            selected: Some("opencode/glm-5.3-flash-free"),
            config_default: Some("opencode/beam-free"),
        };
        let r = ModelResolution::resolve("opencode", cands).unwrap();
        assert_eq!(r.source, ModelSource::Selected);
        assert_eq!(r.model.as_deref(), Some("opencode/glm-5.3-flash-free"));

        let cands = ModelCandidates {
            explicit: None,
            selected: None,
            config_default: Some("opencode/beam-free"),
        };
        let r = ModelResolution::resolve("opencode", cands).unwrap();
        assert_eq!(r.source, ModelSource::ConfigDefault);
        assert_eq!(r.model.as_deref(), Some("opencode/beam-free"));

        let r = ModelResolution::resolve(
            "cursor",
            ModelCandidates {
                explicit: None,
                selected: None,
                config_default: None,
            },
        )
        .unwrap();
        assert_eq!(r.source, ModelSource::RunnerDefault);
        assert_eq!(r.model.as_deref(), Some("composer-2.5"));

        let r = ModelResolution::resolve(
            "agy",
            ModelCandidates {
                explicit: None,
                selected: None,
                config_default: None,
            },
        )
        .unwrap();
        assert_eq!(r.source, ModelSource::ProviderDefault);
        assert_eq!(r.model, None);
    }

    // TC-1735 / FR-1726: cursor fall-through and explicit error
    #[test]
    fn tc_1735_cursor_fallthrough_and_explicit_error() {
        let r = ModelResolution::resolve(
            "cursor",
            ModelCandidates {
                explicit: None,
                selected: Some("claude-x"),
                config_default: None,
            },
        )
        .unwrap();
        assert_eq!(r.source, ModelSource::RunnerDefault);
        assert_eq!(r.model.as_deref(), Some("composer-2.5"));

        let r = ModelResolution::resolve(
            "cursor",
            ModelCandidates {
                explicit: None,
                selected: None,
                config_default: None,
            },
        )
        .unwrap();
        assert_eq!(r.source, ModelSource::RunnerDefault);
        assert_eq!(r.model.as_deref(), Some("composer-2.5"));

        assert!(
            ModelResolution::resolve(
                "cursor",
                ModelCandidates {
                    explicit: Some("gpt-5"),
                    selected: None,
                    config_default: None,
                }
            )
            .is_err()
        );
    }

    // TC-1736 / FR-1732: agy flash-only and fast block
    #[test]
    fn tc_1736_agy_flash_and_fast_block() {
        let r = ModelResolution::resolve(
            "agy",
            ModelCandidates {
                explicit: Some("gemini-3.8-flash-high"),
                selected: None,
                config_default: None,
            },
        )
        .unwrap();
        assert_eq!(r.source, ModelSource::Explicit);
        assert_eq!(r.model.as_deref(), Some("gemini-3.8-flash-high"));

        assert!(
            ModelResolution::resolve(
                "agy",
                ModelCandidates {
                    explicit: Some("gemini-4-argon"),
                    selected: None,
                    config_default: None,
                }
            )
            .is_err()
        );

        let runners = ["cursor", "agy", "opencode", "grok", "codex"];
        for runner in runners {
            assert!(
                ModelResolution::resolve(
                    runner,
                    ModelCandidates {
                        explicit: Some("something-fast-here"),
                        selected: None,
                        config_default: None,
                    }
                )
                .is_err(),
                "{runner}"
            );
        }
    }

    // TC-1737 / FR-1725: config default and provider default label
    #[test]
    fn tc_1737_config_default_and_provider_label() {
        let r = ModelResolution::resolve(
            "opencode",
            ModelCandidates {
                explicit: None,
                selected: None,
                config_default: Some("opencode/beam-free"),
            },
        )
        .unwrap();
        assert_eq!(r.source, ModelSource::ConfigDefault);
        assert_eq!(r.model.as_deref(), Some("opencode/beam-free"));

        let r = ModelResolution::resolve(
            "agy",
            ModelCandidates {
                explicit: None,
                selected: None,
                config_default: None,
            },
        )
        .unwrap();
        assert_eq!(r.source, ModelSource::ProviderDefault);
        assert_eq!(r.model, None);
        assert_eq!(r.label(), PROVIDER_DEFAULT);
    }

    // TC-1738 / FR-1729: catalog check no-catalog and listed
    #[test]
    fn tc_1738_catalog_no_catalog_and_listed() {
        assert!(matches!(
            check_catalog(None, None, None),
            CatalogCheck::NoCatalog
        ));
        assert!(matches!(
            check_catalog(Some("m"), None, None),
            CatalogCheck::NoCatalog
        ));
        assert!(matches!(
            check_catalog(Some("m"), Some(&[]), None),
            CatalogCheck::NoCatalog
        ));
        assert!(matches!(
            check_catalog(None, Some(&["a".to_string()]), None),
            CatalogCheck::NoCatalog
        ));

        let cat = vec![
            "opencode/beam-free".to_string(),
            "opencode/glm-5.3-flash-free".to_string(),
        ];
        match check_catalog(
            Some("opencode/beam-free"),
            Some(&cat),
            Some("opencode models"),
        ) {
            CatalogCheck::Listed => {}
            other => panic!("expected Listed, got {:?}", other),
        }
    }

    // TC-1739 / FR-1729: catalog check not-listed hint exact text
    #[test]
    fn tc_1739_catalog_not_listed_hint() {
        let cat = vec!["opencode/beam-free".to_string()];
        match check_catalog(
            Some("opencode/step-5-preview-free"),
            Some(&cat),
            Some("opencode models"),
        ) {
            CatalogCheck::NotListed { hint } => {
                assert_eq!(
                    hint,
                    "model opencode/step-5-preview-free is not offered by 'opencode models'; pick another model"
                );
            }
            other => panic!("expected NotListed, got {:?}", other),
        }
        match check_catalog(Some("opencode/step-5-preview-free"), Some(&cat), None) {
            CatalogCheck::NotListed { hint } => {
                assert_eq!(
                    hint,
                    "model opencode/step-5-preview-free is not offered by the runner's model list; pick another model"
                );
            }
            other => panic!("expected NotListed, got {:?}", other),
        }
    }
}
