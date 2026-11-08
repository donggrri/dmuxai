#![allow(clippy::field_reassign_with_default)]
use dmuxai::graph::{
    GraphEdge, GraphNode, GraphNodeStatus, GraphPlan, GraphRun, GraphRunPhase, GraphRunSession,
    NodeRole, ORCHESTRATOR_MODEL, save_run, validate_model_id,
};
use dmuxai::skills::discover_skills;
use tempfile::tempdir;

fn node(id: &str, role: NodeRole, runner: &str, requires_gate: bool) -> GraphNode {
    GraphNode {
        id: id.to_owned(),
        role,
        job: format!("{role} the work"),
        runner: runner.to_owned(),
        model_id: "composer-2.5".to_owned(),
        skill: None,
        requires_gate,
        status: GraphNodeStatus::Pending,
        attempt: 0,
        task: None,
        started_at_ms: None,
        finished_at_ms: None,
    }
}

/// A minimal two-node plan where the first node is gated. Built directly so
/// the orchestration tests do not depend on the removed keyword planner.
fn gated_plan() -> GraphPlan {
    let mut plan = GraphPlan {
        schema: "dmuxai.graph.plan.v1".to_owned(),
        id: "plan-gate-test".to_owned(),
        orchestrator_model: ORCHESTRATOR_MODEL.to_owned(),
        user_prompt: "research auth then implement".to_owned(),
        nodes: vec![
            node("step-1", NodeRole::Research, "cursor", true),
            node("step-2", NodeRole::Implement, "codex", false),
        ],
        edges: vec![GraphEdge {
            from: "step-1".to_owned(),
            to: "step-2".to_owned(),
        }],
        ..Default::default()
    };
    plan.limits.max_parallel = 2;
    plan
}

// TC-1881 / FR-1881: All role buckets use composer-2.5 only.
#[test]
fn tc_1881_validate_rejects_fast_and_non_composer_models() {
    assert!(validate_model_id("grok-4.7-medium-fast").is_err());
    assert!(validate_model_id("grok-4.7-medium").is_err());
    assert!(validate_model_id("claude-haiku-5-5-high").is_err());
    assert!(validate_model_id("claude-sonnet-5-5").is_err());
    assert!(validate_model_id("gpt-5").is_err());
    assert!(validate_model_id("gemini-4").is_err());
    assert!(validate_model_id("composer-2.5").is_ok());
}

// TC-1887 / FR-1887: Persist and reload graph run JSON.
#[test]
fn tc_1887_store_round_trip() {
    let dir = tempdir().unwrap();
    let run = GraphRun {
        plan: gated_plan(),
        conversation_id: "conv-1".to_owned(),
        phase: GraphRunPhase::Active,
        waiting_gate_node: None,
        ..Default::default()
    };
    let path = save_run(dir.path(), &run).unwrap();
    assert!(path.exists());
    let loaded = dmuxai::graph::load_run(dir.path(), &run.plan.id).unwrap();
    assert_eq!(loaded.plan.id, run.plan.id);
}

// TC-1885 / FR-1885: Gates pause on merge/DB/auth-class steps until /graph continue.
#[test]
fn tc_1885_gate_pauses_until_continue() {
    let dir = tempdir().unwrap();
    let run = GraphRun {
        plan: gated_plan(),
        conversation_id: "conv".to_owned(),
        phase: GraphRunPhase::Active,
        waiting_gate_node: None,
        ..Default::default()
    };
    let mut session = GraphRunSession::new(run);
    let first = session.schedule();
    assert!(
        first
            .iter()
            .any(|action| matches!(action, dmuxai::graph::GraphAction::PauseForGate(_))),
        "a gated node must pause the run"
    );
    assert_eq!(session.run().phase, GraphRunPhase::WaitingGate);
    assert_eq!(session.run().waiting_gate_node.as_deref(), Some("step-1"));

    let after = session.acknowledge_gate();
    assert!(
        after
            .iter()
            .any(|action| matches!(action, dmuxai::graph::GraphAction::StartNode(_))),
        "continuing the gate starts the approved node"
    );
    assert_ne!(session.run().phase, GraphRunPhase::WaitingGate);

    let _ = dir;
}

// TC-1888 / FR-1888: Discover .agents/skills/*/SKILL.md for node skill binding.
#[test]
fn tc_1888_discover_skills_finds_agent_graph_engineering() {
    let skills = discover_skills(std::path::Path::new("."));
    assert!(
        skills
            .iter()
            .any(|skill| skill.id == "agent-graph-engineering"),
        "expected agent-graph-engineering skill in repo"
    );
}

// TC-1886 / NFR-1886: Cap parallel nodes (default 2) and retries (default 2).
#[test]
fn tc_1886_executor_and_plan_limits_defaults() {
    let limits = dmuxai::graph::default_limits();
    assert_eq!(limits.max_parallel, 2, "default max_parallel must be 2");
    assert_eq!(limits.max_retries, 2, "default max_retries must be 2");
}
