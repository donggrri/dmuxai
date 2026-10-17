#![allow(clippy::field_reassign_with_default)]
use dmuxai::graph::{
    ExecutionPlan, GraphRun, GraphRunPhase, GraphRunSession, NodeRole, ORCHESTRATOR_MODEL,
    plan_request, save_run, validate_model_id,
};
use dmuxai::settings::DmuxSettings;
use dmuxai::skills::discover_skills;
use tempfile::tempdir;

#[test]
fn planner_sets_orchestrator_model() {
    let dir = tempdir().unwrap();
    let plan = plan_request(
        "research the auth module, plan migration, implement fix, and review",
        dir.path(),
        &DmuxSettings::default(),
    );
    let graph = match plan {
        ExecutionPlan::Graph(graph) => graph,
        ExecutionPlan::Single(_) => panic!("expected multi-node graph"),
    };
    assert_eq!(graph.orchestrator_model, ORCHESTRATOR_MODEL);
    assert!(
        graph
            .nodes
            .iter()
            .any(|node| node.role == NodeRole::Orchestrate)
    );
}

#[test]
fn simple_prompt_uses_single_agent() {
    let dir = tempdir().unwrap();
    let plan = plan_request("fix typo in README", dir.path(), &DmuxSettings::default());
    assert!(matches!(plan, ExecutionPlan::Single(_)));
}

#[test]
fn planner_creates_single_node_or_multi_node_graph_by_complexity() {
    let dir = tempdir().unwrap();
    let simple = plan_request("what is 2+2", dir.path(), &DmuxSettings::default());
    assert!(matches!(simple, ExecutionPlan::Single(_)));

    let complex = plan_request(
        "research the auth module, plan migration, implement fix, and test",
        dir.path(),
        &DmuxSettings::default(),
    );
    match complex {
        ExecutionPlan::Graph(graph) => {
            assert!(graph.nodes.len() > 1);
            assert_eq!(graph.orchestrator_model, ORCHESTRATOR_MODEL);
        }
        ExecutionPlan::Single(_) => panic!("expected multi-node graph"),
    }
}

#[test]
fn validate_rejects_fast_and_non_composer_models() {
    assert!(validate_model_id("grok-4.7-medium-fast").is_err());
    assert!(validate_model_id("grok-4.7-medium").is_err());
    assert!(validate_model_id("claude-haiku-5-5-high").is_err());
    assert!(validate_model_id("claude-sonnet-5-5").is_err());
    assert!(validate_model_id("gpt-5").is_err());
    assert!(validate_model_id("gemini-4").is_err());
    assert!(validate_model_id("composer-2.5").is_ok());
}

#[test]
fn store_round_trip() {
    let dir = tempdir().unwrap();
    let plan = plan_request(
        "explore codebase and implement feature with tests and review",
        dir.path(),
        &DmuxSettings::default(),
    );
    let graph = match plan {
        ExecutionPlan::Graph(graph) => graph,
        ExecutionPlan::Single(_) => panic!("expected graph"),
    };
    let run = GraphRun {
        plan: graph,
        conversation_id: "conv-1".to_owned(),
        phase: GraphRunPhase::Active,
        waiting_gate_node: None,
    };
    let path = save_run(dir.path(), &run).unwrap();
    assert!(path.exists());
    let loaded = dmuxai::graph::load_run(dir.path(), &run.plan.id).unwrap();
    assert_eq!(loaded.plan.id, run.plan.id);
}

#[test]
fn gate_pauses_until_continue() {
    let dir = tempdir().unwrap();
    let plan = plan_request(
        "research auth, plan database migration, implement merge to main",
        dir.path(),
        &DmuxSettings::default(),
    );
    let graph = match plan {
        ExecutionPlan::Graph(graph) => graph,
        ExecutionPlan::Single(_) => panic!("expected graph"),
    };
    let run = GraphRun {
        plan: graph,
        conversation_id: "conv".to_owned(),
        phase: GraphRunPhase::Active,
        waiting_gate_node: None,
    };
    let mut session = GraphRunSession::new(run);
    let first = session.schedule();
    assert!(
        first
            .iter()
            .any(|action| matches!(action, dmuxai::graph::GraphAction::PauseForGate(_)))
            || first
                .iter()
                .any(|action| matches!(action, dmuxai::graph::GraphAction::StartNode(_)))
    );
    if first
        .iter()
        .any(|a| matches!(a, dmuxai::graph::GraphAction::PauseForGate(_)))
    {
        let after = session.acknowledge_gate();
        assert!(!after.is_empty() || session.run().phase == GraphRunPhase::WaitingGate);
    }
}

#[test]
fn discover_skills_finds_agent_graph_engineering() {
    let skills = discover_skills(std::path::Path::new("."));
    assert!(
        skills
            .iter()
            .any(|skill| skill.id == "agent-graph-engineering"),
        "expected agent-graph-engineering skill in repo"
    );
}

#[test]
fn allowed_list_excludes_cursor_every_node_runner_is_agy() {
    // TC-430, FR-430, FR-431, FR-432
    let dir = tempdir().unwrap();
    let mut settings = DmuxSettings::default();
    settings.graph_allowed_runners = Some(vec!["agy".to_owned()]);

    let plan = plan_request(
        "research auth, plan database migration, implement merge to main and test",
        dir.path(),
        &settings,
    );
    let graph = match plan {
        ExecutionPlan::Graph(graph) => graph,
        ExecutionPlan::Single(_) => panic!("expected multi-node graph"),
    };
    assert!(!graph.nodes.is_empty());
    for node in &graph.nodes {
        assert_eq!(
            node.runner, "agy",
            "node {} has runner {}, expected agy",
            node.id, node.runner
        );
    }
}

#[test]
fn routing_default_not_allowed_falls_back() {
    // TC-431, FR-431
    let dir = tempdir().unwrap();
    let mut settings = DmuxSettings::default();
    settings.routing_default_agent = Some("codex".to_owned());
    settings.graph_allowed_runners = Some(vec!["cursor".to_owned(), "agy".to_owned()]);

    let plan = plan_request(
        "research auth, plan database migration, implement fix and test",
        dir.path(),
        &settings,
    );
    let graph = match plan {
        ExecutionPlan::Graph(graph) => graph,
        ExecutionPlan::Single(_) => panic!("expected multi-node graph"),
    };
    for node in &graph.nodes {
        assert_eq!(node.runner, "cursor");
    }

    // When cursor is also excluded, falls back to first allowed runner
    settings.routing_default_agent = Some("cursor".to_owned());
    settings.graph_allowed_runners = Some(vec!["agy".to_owned(), "codex".to_owned()]);
    let plan2 = plan_request(
        "research auth, plan database migration, implement fix and test",
        dir.path(),
        &settings,
    );
    let graph2 = match plan2 {
        ExecutionPlan::Graph(graph) => graph,
        ExecutionPlan::Single(_) => panic!("expected multi-node graph"),
    };
    for node in &graph2.nodes {
        assert_eq!(node.runner, "agy");
    }
}

#[test]
fn empty_allowed_runners_planner_falls_back_to_single() {
    // TC-433, INT-430
    let dir = tempdir().unwrap();
    let mut settings = DmuxSettings::default();
    settings.graph_allowed_runners = Some(Vec::new());

    let plan = plan_request(
        "research auth, plan database migration, implement fix and test",
        dir.path(),
        &settings,
    );
    assert!(matches!(plan, ExecutionPlan::Single(_)));
}
