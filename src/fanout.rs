//! Pure fan-out planning types and helpers (SPEC-fanout-readonly FR-2910..2917).

use crate::custom_agent;
use crate::settings::DmuxSettings;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// One runner entry from `--agents a,b[:model],c`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanoutLegSpec {
    pub runner: String,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegStatus {
    Queued,
    Running,
    Ok,
    Failed,
    Skipped,
}

/// One leg in the headless JSON/text output (FR-2915).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FanoutLegReport {
    pub agent: String,
    pub model: Option<String>,
    pub status: LegStatus,
    #[serde(default)]
    pub text: String,
    pub error: Option<String>,
    pub error_kind: Option<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FanoutReport {
    pub min_ok: usize,
    pub legs: Vec<FanoutLegReport>,
}

/// Parsed `--agents` value must list at least two distinct runners (FR-2910).
pub fn parse_agents_flag(raw: &str) -> Result<Vec<FanoutLegSpec>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("--agents needs at least two distinct runners".to_owned());
    }
    let mut legs = Vec::new();
    for entry in trimmed.split(',') {
        let piece = entry.trim();
        if piece.is_empty() {
            return Err("--agents has an empty entry".to_owned());
        }
        let (runner, model) = match piece.split_once(':') {
            Some((runner, model)) => {
                let model = model.trim();
                if model.is_empty() {
                    return Err(format!("--agents entry '{piece}' has an empty model"));
                }
                (runner.trim(), Some(model.to_owned()))
            }
            None => (piece, None),
        };
        if runner.is_empty() {
            return Err("--agents has an empty runner id".to_owned());
        }
        legs.push(FanoutLegSpec {
            runner: runner.to_ascii_lowercase(),
            model,
        });
    }
    validate_agents_list(&legs)?;
    Ok(legs)
}

pub fn validate_agents_list(legs: &[FanoutLegSpec]) -> Result<(), String> {
    if legs.len() < 2 {
        return Err("--agents needs at least two distinct runners".to_owned());
    }
    let mut seen = std::collections::HashSet::new();
    for leg in legs {
        if !seen.insert(leg.runner.as_str()) {
            return Err(format!(
                "--agents lists runner '{}' more than once",
                leg.runner
            ));
        }
    }
    Ok(())
}

pub fn parse_min_ok(raw: Option<&str>, leg_count: usize) -> Result<usize, String> {
    let value = match raw {
        None => 1,
        Some(text) => text
            .trim()
            .parse::<usize>()
            .map_err(|_| "--min-ok must be a positive integer".to_owned())?,
    };
    if value == 0 {
        return Err("--min-ok must be at least 1".to_owned());
    }
    if value > leg_count {
        return Err(format!(
            "--min-ok cannot exceed the number of legs ({leg_count})"
        ));
    }
    Ok(value)
}

/// Quorum: skipped and failed legs do not count; `ok` legs must meet `min_ok` (FR-2913).
pub fn fanout_run_ok(ok_legs: usize, min_ok: usize) -> bool {
    ok_legs >= min_ok
}

pub fn fanout_exit_code(ok: bool, usage_error: bool) -> i32 {
    if usage_error {
        crate::agent_api::exit::USAGE
    } else if ok {
        crate::agent_api::exit::OK
    } else {
        crate::agent_api::exit::FAILED
    }
}

pub fn is_free_runner(resolved_model: Option<&str>, custom_free: bool) -> bool {
    custom_free
        || resolved_model
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .is_some_and(|m| m.to_ascii_lowercase().ends_with("-free"))
}

/// Resolved model for a fan-out leg: `:model` > `[runners.<id>]` > custom default (FR-2916).
pub fn resolved_fanout_leg_model(leg: &FanoutLegSpec, settings: &DmuxSettings) -> Option<String> {
    if let Some(model) = leg
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        return Some(model.to_owned());
    }
    if let Some(model) = settings.runner_models.get(&leg.runner) {
        return Some(model.clone());
    }
    if let Some(spec) = custom_agent::index_of(&leg.runner).and_then(custom_agent::spec_at) {
        return spec.default_model.clone();
    }
    None
}

/// Whether a fan-out leg targets a free runner after model resolution (FR-2916).
pub fn fanout_runner_is_free(leg: &FanoutLegSpec, settings: &DmuxSettings) -> bool {
    if let Some(spec) = custom_agent::index_of(&leg.runner).and_then(custom_agent::spec_at) {
        if spec.free {
            return true;
        }
        if spec.builtin && spec.name.eq_ignore_ascii_case("opencode") {
            return true;
        }
    }
    is_free_runner(resolved_fanout_leg_model(leg, settings).as_deref(), false)
}

/// FR-2916: mark free runners skipped when company workspace mode is on.
pub fn apply_company_workspace_skip(
    legs: Vec<FanoutLegSpec>,
    company_workspace: bool,
    is_free: impl Fn(&FanoutLegSpec) -> bool,
) -> (Vec<FanoutLegSpec>, Vec<(String, String)>) {
    if !company_workspace {
        return (legs, Vec::new());
    }
    let mut warnings = Vec::new();
    let mut kept = Vec::new();
    for leg in legs {
        if is_free(&leg) {
            warnings.push((
                leg.runner.clone(),
                format!(
                    "warning: skipping free runner '{}' in company workspace",
                    leg.runner
                ),
            ));
        } else {
            kept.push(leg);
        }
    }
    (kept, warnings)
}

pub fn fanout_json_value(ok: bool, min_ok: usize, legs: &[FanoutLegReport]) -> Value {
    json!({
        "ok": ok,
        "fanout": {
            "min_ok": min_ok,
            "legs": legs,
        }
    })
}

pub fn fanout_text_output(legs: &[FanoutLegReport]) -> String {
    let mut parts = Vec::new();
    for leg in legs {
        let title = match leg.model.as_deref() {
            Some(model) => format!("=== {} ({model}) ===", leg.agent),
            None => format!("=== {} ===", leg.agent),
        };
        let body = if leg.status == LegStatus::Ok && !leg.text.is_empty() {
            leg.text.clone()
        } else if let Some(err) = &leg.error {
            err.clone()
        } else {
            format!("{:?}", leg.status)
        };
        parts.push(format!("{title}\n{body}"));
    }
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // TC-2915 / FR-2915
    #[test]
    fn tc_2915_json_output_shape_and_leg_order() {
        let legs = vec![
            FanoutLegReport {
                agent: "cursor".into(),
                model: Some("m1".into()),
                status: LegStatus::Skipped,
                text: String::new(),
                error: None,
                error_kind: None,
                duration_ms: 0,
            },
            FanoutLegReport {
                agent: "codex".into(),
                model: None,
                status: LegStatus::Failed,
                text: String::new(),
                error: Some("boom".into()),
                error_kind: Some("provider".into()),
                duration_ms: 20,
            },
        ];
        let serialized = serde_json::to_value(&legs[1]).expect("leg json");
        assert!(serialized.get("text").is_some());
        assert!(serialized.get("error").is_some());
        assert!(serialized.get("error_kind").is_some());
        let arr = serde_json::to_value(&legs).expect("legs");
        assert_eq!(arr[0]["agent"], "cursor");
        assert_eq!(arr[1]["agent"], "codex");
    }

    // TC-2916 / FR-2916
    #[test]
    fn tc_2916_company_workspace_skips_free_runners_with_warning() {
        let mut settings = DmuxSettings::default();
        settings.runner_models.insert("codex".into(), "o3".into());
        let legs = [
            FanoutLegSpec {
                runner: "cursor".into(),
                model: Some("composer-2.5".into()),
            },
            FanoutLegSpec {
                runner: "codex".into(),
                model: None,
            },
            FanoutLegSpec {
                runner: "agy".into(),
                model: Some("kimi-k2.5-free".into()),
            },
        ];
        assert!(!fanout_runner_is_free(&legs[0], &settings));
        assert!(!fanout_runner_is_free(&legs[1], &settings));
        assert!(fanout_runner_is_free(&legs[2], &settings));
        assert_eq!(
            resolved_fanout_leg_model(&legs[1], &settings).as_deref(),
            Some("o3")
        );
        let free = FanoutLegSpec {
            runner: "custom".into(),
            model: None,
        };
        assert!(!fanout_runner_is_free(&free, &settings));
    }

    #[test]
    fn fanout_quorum_helper_counts_ok_legs() {
        assert!(fanout_run_ok(1, 1));
        assert!(!fanout_run_ok(0, 1));
        assert!(fanout_run_ok(2, 2));
    }

    #[test]
    fn is_free_runner_detects_suffix_and_custom_flag() {
        assert!(is_free_runner(Some("opencode/kimi-k2.5-free"), false));
        assert!(!is_free_runner(Some("composer-2.5"), false));
        assert!(is_free_runner(None, true));
    }
}
