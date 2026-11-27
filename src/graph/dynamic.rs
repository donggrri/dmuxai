//! Dynamic nodes addition (SPEC-graph-dynamic-nodes, FR-2940..FR-2947).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::fanout::{FanoutLegSpec, fanout_runner_is_free};
use crate::model_policy::validate_model_for_runner;
use crate::settings::DmuxSettings;

use super::plan_doc::{parse_role, valid_step_id};
use super::planner::sensitive_keyword_match;
use super::policy::{is_runner_allowed, model_for_role};
use super::types::{GraphNode, GraphNodeStatus, GraphPlan, NodeRole};

/// One proposed node parsed from a `DMUXAI_ADD_NODES` marker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicNodeSpec {
    pub id: String,
    pub role: String,
    pub job: String,
    pub runner: String,
    #[serde(alias = "model", default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub sensitive: Option<bool>,
}

fn strip_leading_marker_decoration(line: &str) -> &str {
    let mut rest = line.trim_start();
    while !rest.is_empty() {
        if let Some(next) = rest.strip_prefix("**") {
            rest = next;
            continue;
        }
        if let Some(next) = rest.strip_prefix("__") {
            rest = next;
            continue;
        }
        if let Some(next) = rest.strip_prefix('`') {
            rest = next;
            continue;
        }
        if let Some(next) = rest.strip_prefix('*') {
            rest = next;
            continue;
        }
        if let Some(next) = rest.strip_prefix('_') {
            rest = next;
            continue;
        }
        if let Some(next) = rest.strip_prefix("- ") {
            rest = next;
            continue;
        }
        break;
    }
    rest
}

fn is_add_nodes_marker_line(line: &str) -> bool {
    let stripped = strip_leading_marker_decoration(line);
    stripped.starts_with("DMUXAI_ADD_NODES:")
        || ["**", "__", "`", "*", "_"].iter().any(|prefix| {
            stripped.starts_with(&format!("DMUXAI_ADD_NODES{prefix}:"))
                || stripped.starts_with(&format!("DMUXAI_ADD_NODES:{prefix}"))
        })
}

fn parse_add_nodes_marker_line(line: &str) -> Option<&str> {
    let stripped = strip_leading_marker_decoration(line);
    let after = stripped.strip_prefix("DMUXAI_ADD_NODES:").or_else(|| {
        ["**", "__", "`", "*", "_"].iter().find_map(|close| {
            stripped
                .strip_prefix("DMUXAI_ADD_NODES")
                .and_then(|rest| rest.strip_prefix(close))
                .and_then(|rest| rest.strip_prefix(':'))
        })
    })?;
    let after = after.trim_start_matches(['*', '_', '`']).trim();
    let payload = after.trim_end_matches(['*', '_', '`']).trim();
    if payload.is_empty() {
        None
    } else {
        Some(payload)
    }
}

/// Parses `DMUXAI_ADD_NODES: [ ... ]` from a node's answer.
/// Returns:
/// - `None` if no marker was found.
/// - `Some(Err(msg))` if a marker line exists but JSON is malformed or not an array of objects.
/// - `Some(Ok(nodes))` if valid dynamic node specs were parsed.
pub fn parse_add_nodes_marker(text: &str) -> Option<Result<Vec<DynamicNodeSpec>, String>> {
    let marker_lines: Vec<&str> = text
        .lines()
        .filter(|line| is_add_nodes_marker_line(line))
        .collect();
    if marker_lines.is_empty() {
        return None;
    }
    if marker_lines.len() > 1 {
        return Some(Err(
            "multiple DMUXAI_ADD_NODES markers in one reply".to_string()
        ));
    }
    let line = marker_lines[0];
    let payload = match parse_add_nodes_marker_line(line) {
        Some(p) => p,
        None => return Some(Err("empty DMUXAI_ADD_NODES marker".to_string())),
    };
    let val: serde_json::Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(e) => return Some(Err(format!("invalid JSON in DMUXAI_ADD_NODES marker: {e}"))),
    };
    let items = match val {
        serde_json::Value::Array(items) => items,
        _ => {
            return Some(Err(
                "DMUXAI_ADD_NODES marker must be a JSON array".to_string()
            ));
        }
    };
    let mut specs = Vec::with_capacity(items.len());
    for item in items {
        if !item.is_object() {
            return Some(Err(
                "DMUXAI_ADD_NODES array elements must be objects".to_string()
            ));
        }
        match serde_json::from_value::<DynamicNodeSpec>(item) {
            Ok(spec) => specs.push(spec),
            Err(e) => return Some(Err(format!("invalid dynamic node item: {e}"))),
        }
    }
    Some(Ok(specs))
}

/// Removes the `DMUXAI_ADD_NODES: [ ... ]` line from the given text.
pub fn strip_add_nodes_marker(text: &str) -> String {
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| !is_add_nodes_marker_line(line))
        .collect();
    if kept.len() == text.lines().count() {
        return text.to_owned();
    }
    kept.join("\n").trim_end().to_owned()
}

/// Resolved model for a dynamic node: explicit `model_id` > `[runners.<id>]` >
/// custom default > role preset (FR-2946, same order as fan-out).
pub fn resolved_dynamic_node_model(
    spec: &DynamicNodeSpec,
    role: NodeRole,
    settings: &DmuxSettings,
) -> String {
    if let Some(model) = spec
        .model_id
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        return model.to_owned();
    }
    let runner = spec.runner.trim().to_ascii_lowercase();
    if let Some(model) = settings.runner_models.get(&runner) {
        return model.clone();
    }
    if let Some(agent) =
        crate::custom_agent::index_of(&runner).and_then(crate::custom_agent::spec_at)
        && let Some(default) = &agent.default_model
    {
        return default.clone();
    }
    model_for_role(role).to_owned()
}

/// Pure validation function implementing every rule of FR-2943, FR-2945, FR-2946:
/// - non-empty marker
/// - within remaining cap
/// - total executable nodes within max_nodes
/// - unique ids not already existing in plan
/// - id format matches ^[a-z0-9][a-z0-9-]{0,31}$
/// - valid role and not orchestrate
/// - non-empty job
/// - runner allowed by allowed_runners (or built-in defaults)
/// - validate_model_id rules
/// - depends_on names only existing nodes or earlier entries in the marker
/// - no cycles
/// - free-runner rejection when company_workspace is true
/// - preserves requires_gate if sensitive keywords match or sensitive: true
pub fn validate_add_nodes(
    plan: &GraphPlan,
    new_nodes: &[DynamicNodeSpec],
    cap_remaining: usize,
    settings: &DmuxSettings,
    max_nodes: usize,
    company_workspace: bool,
) -> Result<Vec<GraphNode>, String> {
    if new_nodes.is_empty() {
        return Err("marker node list cannot be empty".to_string());
    }
    if new_nodes.len() > cap_remaining {
        return Err(format!(
            "added node count {} exceeds remaining cap {}",
            new_nodes.len(),
            cap_remaining
        ));
    }
    let current_exec = plan
        .nodes
        .iter()
        .filter(|n| n.role != NodeRole::Orchestrate)
        .count();
    if current_exec + new_nodes.len() > max_nodes {
        return Err(format!(
            "total executable nodes {} would exceed max_nodes {}",
            current_exec + new_nodes.len(),
            max_nodes
        ));
    }

    let mut seen_ids = HashSet::new();
    for node in &plan.nodes {
        seen_ids.insert(node.id.as_str());
    }

    for spec in new_nodes {
        if !valid_step_id(&spec.id) {
            return Err(format!(
                "invalid node id '{}': must match ^[a-z0-9][a-z0-9-]{{0,31}}$",
                spec.id
            ));
        }
        if !seen_ids.insert(spec.id.as_str()) {
            return Err(format!("duplicate or existing node id '{}'", spec.id));
        }
        if spec.role.eq_ignore_ascii_case("orchestrate") {
            return Err("role cannot be orchestrate".to_string());
        }
        if parse_role(&spec.role).is_none() {
            return Err(format!("invalid role '{}'", spec.role));
        }
        if spec.job.trim().is_empty() {
            return Err(format!("job cannot be empty for node '{}'", spec.id));
        }
        let runner = spec.runner.trim().to_ascii_lowercase();
        if !is_runner_allowed(&runner, settings) {
            return Err(format!("runner '{}' is not allowed", spec.runner));
        }

        let role = parse_role(&spec.role).unwrap();
        if let Some(model) = spec.model_id.as_deref()
            && model.trim().is_empty()
        {
            return Err(format!("model_id cannot be empty for node '{}'", spec.id));
        }
        let model_id = resolved_dynamic_node_model(spec, role, settings);
        if let Err(err) = validate_model_for_runner(&runner, &model_id) {
            return Err(format!(
                "invalid model '{}' for node '{}': {}",
                model_id, spec.id, err
            ));
        }

        if company_workspace {
            let settings_free = settings.custom_agents.iter().any(|s| {
                s.name.eq_ignore_ascii_case(&runner)
                    && (s.free || (s.builtin && s.name.eq_ignore_ascii_case("opencode")))
            });
            let leg = FanoutLegSpec {
                runner: runner.clone(),
                model: Some(model_id.clone()),
            };
            if settings_free || fanout_runner_is_free(&leg, settings) {
                return Err(format!(
                    "warning: free runner '{}' rejected in company workspace",
                    spec.runner
                ));
            }
        }
    }

    let mut edge_keys: HashSet<(String, String)> = plan
        .edges
        .iter()
        .map(|edge| (edge.from.clone(), edge.to.clone()))
        .collect();
    let mut seen_earlier: HashSet<&str> = plan.nodes.iter().map(|n| n.id.as_str()).collect();
    for spec in new_nodes {
        for dep in &spec.depends_on {
            if !seen_earlier.contains(dep.as_str()) {
                return Err(format!(
                    "node '{}' depends_on references unknown or future node '{}'",
                    spec.id, dep
                ));
            }
            if let Some(node) = plan.nodes.iter().find(|node| node.id == *dep)
                && matches!(
                    node.status,
                    GraphNodeStatus::Failed | GraphNodeStatus::Skipped
                )
            {
                return Err(format!(
                    "node '{}' cannot depend on {} node '{}'",
                    spec.id,
                    match node.status {
                        GraphNodeStatus::Failed => "failed",
                        GraphNodeStatus::Skipped => "skipped",
                        _ => "unavailable",
                    },
                    dep
                ));
            }
            let edge = (dep.clone(), spec.id.clone());
            if !edge_keys.insert(edge) {
                return Err(format!("duplicate edge from '{}' to '{}'", dep, spec.id));
            }
        }
        seen_earlier.insert(spec.id.as_str());
    }

    let mut validated = Vec::with_capacity(new_nodes.len());
    for spec in new_nodes {
        let role = parse_role(&spec.role).unwrap();
        let runner = spec.runner.trim().to_ascii_lowercase();
        let model_id = resolved_dynamic_node_model(spec, role, settings);
        let keyword =
            sensitive_keyword_match(&plan.user_prompt) || sensitive_keyword_match(&spec.job);
        let requires_gate = spec.sensitive == Some(true) || keyword;
        validated.push(GraphNode {
            id: spec.id.clone(),
            role,
            job: spec.job.clone(),
            runner,
            model_id,
            skill: None,
            requires_gate,
            status: GraphNodeStatus::Pending,
            attempt: 0,
            task: None,
            started_at_ms: None,
            finished_at_ms: None,
        });
    }

    Ok(validated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::types::GraphLimits;
    use crate::settings::DmuxSettings;

    fn sample_settings() -> DmuxSettings {
        DmuxSettings::default()
    }

    fn sample_plan() -> GraphPlan {
        GraphPlan {
            schema: "cursor-agent-graph/v1".to_string(),
            id: "plan-1".to_string(),
            orchestrator_model: "composer-2.5".to_string(),
            user_prompt: "build app".to_string(),
            nodes: vec![GraphNode {
                id: "step-1".to_string(),
                role: NodeRole::Implement,
                job: "initial code".to_string(),
                runner: "codex".to_string(),
                model_id: "composer-2.5".to_string(),
                skill: None,
                requires_gate: false,
                status: GraphNodeStatus::Completed,
                attempt: 0,
                task: None,
                started_at_ms: None,
                finished_at_ms: None,
            }],
            edges: vec![],
            limits: GraphLimits {
                max_parallel: 2,
                max_retries: 2,
                max_replans: 3,
                replan_budget_chars: 24_000,
                max_nodes: 5,
            },
            tasks: vec![],
            dynamic_cap: Some(3),
        }
    }

    #[test]
    fn tc_2943_marker_validation_rejects_each_violation() {
        let plan = sample_plan();
        let settings = sample_settings();

        // 1. Invalid JSON marker parsing
        let parsed_err = parse_add_nodes_marker("DMUXAI_ADD_NODES: not a json array").unwrap();
        assert!(parsed_err.is_err(), "malformed JSON should error");

        // 2. Not an array of objects
        let not_obj = parse_add_nodes_marker("DMUXAI_ADD_NODES: [\"not-an-object\"]").unwrap();
        assert!(not_obj.is_err(), "non-object elements should error");

        // 3. Valid parse
        let valid_text = "Here is my answer.\nDMUXAI_ADD_NODES: [{\"id\":\"step-2\",\"role\":\"test\",\"job\":\"run tests\",\"runner\":\"codex\",\"model_id\":\"composer-2.5\",\"depends_on\":[\"step-1\"]}]";
        let parsed_ok = parse_add_nodes_marker(valid_text).unwrap().unwrap();
        assert_eq!(parsed_ok.len(), 1);
        assert_eq!(parsed_ok[0].id, "step-2");

        // 4. Role orchestrate rejected
        let mut orch_spec = parsed_ok.clone();
        orch_spec[0].role = "orchestrate".to_string();
        let err = validate_add_nodes(&plan, &orch_spec, 3, &settings, 5, false).unwrap_err();
        assert!(err.contains("orchestrate"), "must reject orchestrate role");

        // 5. Unknown role rejected
        let mut unknown_role = parsed_ok.clone();
        unknown_role[0].role = "invalid_role".to_string();
        let err = validate_add_nodes(&plan, &unknown_role, 3, &settings, 5, false).unwrap_err();
        assert!(err.contains("invalid role"), "must reject unknown role");

        // 6. Duplicate / existing id rejected
        let mut dup_id = parsed_ok.clone();
        dup_id[0].id = "step-1".to_string(); // already in plan
        let err = validate_add_nodes(&plan, &dup_id, 3, &settings, 5, false).unwrap_err();
        assert!(
            err.contains("duplicate or existing"),
            "must reject existing id"
        );

        // 6b. Intra-marker duplicate ids rejected
        let intra_dup = vec![
            parsed_ok[0].clone(),
            DynamicNodeSpec {
                id: "step-2".to_string(),
                ..parsed_ok[0].clone()
            },
        ];
        let err = validate_add_nodes(&plan, &intra_dup, 3, &settings, 5, false).unwrap_err();
        assert!(err.contains("duplicate or existing"));

        // 7. Malformed id rejected
        let mut bad_id = parsed_ok.clone();
        bad_id[0].id = "Step_2!".to_string();
        let err = validate_add_nodes(&plan, &bad_id, 3, &settings, 5, false).unwrap_err();
        assert!(err.contains("invalid node id"), "must reject bad id format");

        // 8. Runner not allowed rejected
        let mut bad_runner = parsed_ok.clone();
        bad_runner[0].runner = "unauthorized_runner".to_string();
        let mut allow_codex = settings.clone();
        allow_codex.graph_allowed_runners = Some(vec!["codex".to_string()]);
        let err = validate_add_nodes(&plan, &bad_runner, 3, &allow_codex, 5, false).unwrap_err();
        assert!(err.contains("runner 'unauthorized_runner' is not allowed"));

        // 8b. Custom runner allowed when graph_runner_allowed admits it
        let mut custom_settings = settings.clone();
        custom_settings
            .custom_agents
            .push(crate::custom_agent::CustomAgentSpec {
                name: "pi".to_string(),
                default_model: Some("composer-2.5".to_string()),
                ..Default::default()
            });
        let mut custom_runner = parsed_ok.clone();
        custom_runner[0].runner = "pi".to_string();
        assert!(validate_add_nodes(&plan, &custom_runner, 3, &custom_settings, 5, false).is_ok());

        // 9. Model invalid / empty rejected
        let mut bad_model = parsed_ok.clone();
        bad_model[0].model_id = Some("gpt-4-invalid".to_string());
        let err = validate_add_nodes(&plan, &bad_model, 3, &settings, 5, false).unwrap_err();
        assert!(err.contains("invalid model"));
        let mut empty_model = parsed_ok.clone();
        empty_model[0].model_id = Some("".to_string());
        let err = validate_add_nodes(&plan, &empty_model, 3, &settings, 5, false).unwrap_err();
        assert!(err.contains("empty"));

        // 9b. Runner-specific model (agy) accepted when valid (FR-3000)
        let mut agy_spec = parsed_ok.clone();
        agy_spec[0].runner = "agy".to_string();
        agy_spec[0].model_id = Some("gemini-3.8-flash-high".to_string());
        assert!(validate_add_nodes(&plan, &agy_spec, 3, &settings, 5, false).is_ok());
        // Fast ids stay blocked unless the user opts in (FR-3002, allow_fast off).
        let mut agy_bad = agy_spec.clone();
        agy_bad[0].model_id = Some("gemini-4-carbon-fast".to_string());
        let err = validate_add_nodes(&plan, &agy_bad, 3, &settings, 5, false).unwrap_err();
        assert!(err.contains("invalid model"));
        assert!(err.contains("fast models are blocked by policy"), "{err}");

        // 10. depends_on unknown id rejected
        let mut bad_dep = parsed_ok.clone();
        bad_dep[0].depends_on = vec!["nonexistent-step".to_string()];
        let err = validate_add_nodes(&plan, &bad_dep, 3, &settings, 5, false).unwrap_err();
        assert!(err.contains("unknown or future"));

        // 10b. depends_on failed/skipped rejected
        let mut failed_plan = plan.clone();
        failed_plan.nodes.push(GraphNode {
            id: "step-failed".to_string(),
            role: NodeRole::Test,
            job: "x".to_string(),
            runner: "cursor".to_string(),
            model_id: "composer-2.5".to_string(),
            skill: None,
            requires_gate: false,
            status: GraphNodeStatus::Failed,
            attempt: 0,
            task: None,
            started_at_ms: None,
            finished_at_ms: None,
        });
        let mut dep_failed = parsed_ok.clone();
        dep_failed[0].depends_on = vec!["step-failed".to_string()];
        let err =
            validate_add_nodes(&failed_plan, &dep_failed, 3, &settings, 5, false).unwrap_err();
        assert!(err.contains("failed"));

        // 11. depends_on forward / cycle reference rejected
        let forward_specs = vec![
            DynamicNodeSpec {
                id: "step-2".to_string(),
                role: "test".to_string(),
                job: "run tests".to_string(),
                runner: "codex".to_string(),
                model_id: None,
                depends_on: vec!["step-3".to_string()], // step-3 is later!
                sensitive: None,
            },
            DynamicNodeSpec {
                id: "step-3".to_string(),
                role: "test".to_string(),
                job: "more tests".to_string(),
                runner: "codex".to_string(),
                model_id: None,
                depends_on: vec!["step-2".to_string()],
                sensitive: None,
            },
        ];
        let err = validate_add_nodes(&plan, &forward_specs, 3, &settings, 5, false).unwrap_err();
        assert!(err.contains("unknown or future"));

        // 12. Exceeding max_nodes rejected
        // Current plan has 1 node, max_nodes is 5. Adding 5 nodes = 6 > 5.
        let five_specs: Vec<_> = (2..=6)
            .map(|i| DynamicNodeSpec {
                id: format!("step-{i}"),
                role: "test".to_string(),
                job: "test".to_string(),
                runner: "codex".to_string(),
                model_id: None,
                depends_on: vec![],
                sensitive: None,
            })
            .collect();
        let err = validate_add_nodes(&plan, &five_specs, 10, &settings, 5, false).unwrap_err();
        assert!(err.contains("would exceed max_nodes"));

        // 13. Exceeding cap_remaining rejected
        let err = validate_add_nodes(&plan, &parsed_ok, 0, &settings, 10, false).unwrap_err();
        assert!(err.contains("exceeds remaining cap"));

        // 14. Free runner rejected when company_workspace = true (resolved model)
        let mut free_settings = settings.clone();
        free_settings.graph_allowed_runners = Some(vec!["opencode".to_string()]);
        free_settings.fanout_company_workspace = true;
        free_settings
            .custom_agents
            .push(crate::custom_agent::CustomAgentSpec {
                name: "opencode".to_string(),
                builtin: true,
                free: true,
                ..Default::default()
            });
        let mut free_runner_spec = parsed_ok.clone();
        free_runner_spec[0].runner = "opencode".to_string();
        free_runner_spec[0].model_id = None;
        let err =
            validate_add_nodes(&plan, &free_runner_spec, 3, &free_settings, 5, true).unwrap_err();
        assert!(err.contains("company workspace"));

        // 14b. Resolved runner_models -free triggers free rejection
        let mut resolved_free = settings.clone();
        resolved_free.fanout_company_workspace = true;
        resolved_free.runner_models.insert(
            "cursor".to_string(),
            "opencode/glm-5.3-flash-free".to_string(),
        );
        let mut omit_model = parsed_ok.clone();
        omit_model[0].runner = "cursor".to_string();
        omit_model[0].model_id = None;
        let err = validate_add_nodes(&plan, &omit_model, 3, &resolved_free, 5, true).unwrap_err();
        assert!(err.contains("company workspace"));

        // 15. Multiple markers rejected at parse time
        let two_markers = "DMUXAI_ADD_NODES: []\nDMUXAI_ADD_NODES: []";
        let parsed = parse_add_nodes_marker(two_markers).unwrap();
        assert!(parsed.is_err());
        assert!(parsed.unwrap_err().contains("multiple"));

        // 16. All rules passed: returns Ok
        let ok = validate_add_nodes(&plan, &parsed_ok, 3, &settings, 5, false);
        assert!(ok.is_ok());
        let validated = ok.unwrap();
        assert_eq!(validated.len(), 1);
        assert_eq!(validated[0].status, GraphNodeStatus::Pending);
    }
}
