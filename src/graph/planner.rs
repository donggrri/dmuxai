use uuid::Uuid;

use crate::settings::DmuxSettings;
use crate::skills::discover_skills;

use super::policy::{ORCHESTRATOR_MODEL, assign_runner, model_for_role, validate_model_id};
use super::types::{
    ExecutionPlan, GraphEdge, GraphNode, GraphNodeStatus, GraphPlan, NodeRole, SinglePlan,
};

const SCHEMA: &str = "cursor-agent-graph/v1";

pub fn plan_request(
    prompt: &str,
    workspace: &std::path::Path,
    settings: &DmuxSettings,
) -> ExecutionPlan {
    let roles = infer_roles(prompt);
    if roles.len() <= 2
        || settings
            .graph_allowed_runners
            .as_ref()
            .is_some_and(|r| r.is_empty())
    {
        let role = roles.last().copied().unwrap_or(NodeRole::Implement);
        let model_id = model_for_role(role);
        let _ = validate_model_id(model_id);
        return ExecutionPlan::Single(SinglePlan {
            runner: assign_runner(role, settings),
            model_id: model_id.to_owned(),
            role,
            job: prompt.trim().to_owned(),
        });
    }

    let skills = discover_skills(workspace);
    let id = Uuid::new_v4().to_string();
    let mut nodes = Vec::new();

    nodes.push(GraphNode {
        id: "orchestrate".to_owned(),
        role: NodeRole::Orchestrate,
        job: format!("Build execution graph for: {}", summarize(prompt)),
        runner: assign_runner(NodeRole::Orchestrate, settings),
        model_id: ORCHESTRATOR_MODEL.to_owned(),
        skill: Some("agent-graph-engineering".to_owned()),
        requires_gate: false,
        status: GraphNodeStatus::Completed,
        attempt: 0,
    });

    for (index, role) in roles
        .iter()
        .filter(|r| **r != NodeRole::Orchestrate)
        .enumerate()
    {
        let node_id = format!("step-{}", index + 1);
        let skill = pick_skill_for_role(*role, &skills);
        let requires_gate = gate_required(prompt, *role);
        nodes.push(GraphNode {
            id: node_id.clone(),
            role: *role,
            job: job_for_role(*role, prompt),
            runner: assign_runner(*role, settings),
            model_id: model_for_role(*role).to_owned(),
            skill,
            requires_gate,
            status: GraphNodeStatus::Pending,
            attempt: 0,
        });
    }

    let edges = build_edges(&nodes);

    ExecutionPlan::Graph(GraphPlan {
        schema: SCHEMA.to_owned(),
        id,
        orchestrator_model: ORCHESTRATOR_MODEL.to_owned(),
        user_prompt: prompt.to_owned(),
        nodes,
        edges,
        limits: super::policy::default_limits(),
    })
}

fn build_edges(nodes: &[GraphNode]) -> Vec<GraphEdge> {
    let ids = nodes.iter().map(|node| node.id.clone()).collect::<Vec<_>>();
    ids.windows(2)
        .map(|pair| GraphEdge {
            from: pair[0].clone(),
            to: pair[1].clone(),
        })
        .collect()
}

fn infer_roles(prompt: &str) -> Vec<NodeRole> {
    let lower = prompt.to_ascii_lowercase();
    let mut roles = vec![NodeRole::Orchestrate];
    if contains_any(
        &lower,
        &["research", "explore", "investigate", "search", "understand"],
    ) {
        roles.push(NodeRole::Research);
    }
    if contains_any(&lower, &["explore", "trace", "grep"]) && !roles.contains(&NodeRole::Explore) {
        roles.push(NodeRole::Explore);
    }
    if contains_any(&lower, &["test", "verify", "spec", "coverage"]) {
        roles.push(NodeRole::Test);
    }
    if contains_any(&lower, &["plan", "design", "architect", "route"]) {
        roles.push(NodeRole::Plan);
    }
    if contains_any(&lower, &["review", "audit", "checklist"]) {
        roles.push(NodeRole::Review);
    }
    if contains_any(
        &lower,
        &[
            "implement",
            "fix",
            "build",
            "draft",
            "code",
            "refactor",
            "add ",
        ],
    ) {
        roles.push(NodeRole::Implement);
    }
    if roles.len() == 1 {
        roles.push(NodeRole::Implement);
    }
    roles
}

fn gate_required(prompt: &str, role: NodeRole) -> bool {
    let lower = prompt.to_ascii_lowercase();
    let sensitive = contains_any(
        &lower,
        &[
            "merge",
            "database",
            " db",
            "auth",
            "login",
            "credential",
            "migration",
            "deploy",
            "production",
        ],
    );
    sensitive
        && matches!(
            role,
            NodeRole::Implement | NodeRole::Draft | NodeRole::Plan | NodeRole::Review
        )
}

fn pick_skill_for_role(role: NodeRole, skills: &[crate::skills::SkillInfo]) -> Option<String> {
    let want = match role {
        NodeRole::Research | NodeRole::Explore => "agent-graph-engineering",
        NodeRole::Test => "agent-graph-engineering",
        NodeRole::Implement | NodeRole::Draft => "agent-graph-engineering",
        NodeRole::Plan | NodeRole::Route | NodeRole::Review => "agent-graph-engineering",
        NodeRole::Orchestrate => "agent-graph-engineering",
    };
    skills
        .iter()
        .find(|skill| skill.id == want)
        .map(|skill| skill.id.clone())
}

fn job_for_role(role: NodeRole, prompt: &str) -> String {
    let verb = match role {
        NodeRole::Research => "Research",
        NodeRole::Explore => "Explore",
        NodeRole::Test => "Test",
        NodeRole::Plan => "Plan",
        NodeRole::Route => "Route",
        NodeRole::Review => "Review",
        NodeRole::Implement => "Implement",
        NodeRole::Draft => "Draft",
        NodeRole::Orchestrate => "Orchestrate",
    };
    format!("{verb}: {}", summarize(prompt))
}

fn summarize(prompt: &str) -> String {
    let trimmed = prompt.trim();
    if trimmed.len() <= 120 {
        trimmed.to_owned()
    } else {
        format!("{}…", trimmed.chars().take(120).collect::<String>())
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}
