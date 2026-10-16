use std::collections::{HashMap, HashSet};

use super::types::{GraphNode, GraphNodeStatus, GraphPlan, GraphRun, GraphRunPhase, NodeRole};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeAssignment {
    pub node_id: String,
    pub runner: String,
    pub model_id: String,
    pub prompt: String,
    pub role: NodeRole,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphAction {
    StartNode(NodeAssignment),
    PauseForGate(NodeAssignment),
    Completed,
    Failed(String),
}

pub struct GraphRunSession {
    run: GraphRun,
    active: HashSet<String>,
    node_turns: HashMap<String, String>,
}

impl GraphRunSession {
    pub fn new(run: GraphRun) -> Self {
        Self {
            run,
            active: HashSet::new(),
            node_turns: HashMap::new(),
        }
    }

    pub fn run(&self) -> &GraphRun {
        &self.run
    }

    pub fn run_mut(&mut self) -> &mut GraphRun {
        &mut self.run
    }

    pub fn register_turn(&mut self, node_id: &str, turn_id: &str) {
        self.node_turns
            .insert(node_id.to_owned(), turn_id.to_owned());
    }

    pub fn turn_for_node(&self, node_id: &str) -> Option<&str> {
        self.node_turns.get(node_id).map(String::as_str)
    }

    pub fn acknowledge_gate(&mut self) -> Vec<GraphAction> {
        if self.run.phase != GraphRunPhase::WaitingGate {
            return Vec::new();
        }
        if let Some(node_id) = self.run.waiting_gate_node.clone() {
            if let Some(node) = self.run.plan.nodes.iter_mut().find(|n| n.id == node_id) {
                node.status = GraphNodeStatus::Ready;
            }
            self.run.waiting_gate_node = None;
            self.run.phase = GraphRunPhase::Active;
        }
        self.schedule()
    }

    pub fn on_node_finished(&mut self, node_id: &str, success: bool) -> Vec<GraphAction> {
        self.active.remove(node_id);
        if let Some(node) = self.run.plan.nodes.iter_mut().find(|n| n.id == node_id) {
            if success {
                node.status = GraphNodeStatus::Completed;
            } else if node.attempt + 1 < self.run.plan.limits.max_retries {
                node.attempt += 1;
                node.status = GraphNodeStatus::Ready;
                return self.schedule();
            } else {
                node.status = GraphNodeStatus::Failed;
            }
        }
        if !success {
            self.run.phase = GraphRunPhase::Failed;
            return vec![GraphAction::Failed(format!("graph node {node_id} failed"))];
        }
        self.schedule()
    }

    pub fn schedule(&mut self) -> Vec<GraphAction> {
        if self.run.phase == GraphRunPhase::Completed || self.run.phase == GraphRunPhase::Failed {
            return Vec::new();
        }
        if self.run.phase == GraphRunPhase::WaitingGate {
            return Vec::new();
        }

        let mut actions = Vec::new();
        let max_parallel = self.run.plan.limits.max_parallel;
        let running = self.active.len();
        if running >= max_parallel {
            return actions;
        }

        let plan = self.run.plan.clone();
        let completed: HashSet<String> = plan
            .nodes
            .iter()
            .filter(|node| node.status == GraphNodeStatus::Completed)
            .map(|node| node.id.clone())
            .collect();

        let mut index = 0usize;
        while index < self.run.plan.nodes.len() {
            let node_id = self.run.plan.nodes[index].id.clone();
            let role = self.run.plan.nodes[index].role;
            let status = self.run.plan.nodes[index].status;
            let requires_gate = self.run.plan.nodes[index].requires_gate;
            if status != GraphNodeStatus::Pending && status != GraphNodeStatus::Ready {
                index += 1;
                continue;
            }
            if role == NodeRole::Orchestrate {
                index += 1;
                continue;
            }
            if !dependencies_met(&plan, &node_id, &completed) {
                index += 1;
                continue;
            }
            if self.active.len() + actions.len() >= max_parallel {
                break;
            }

            let assignment = NodeAssignment {
                node_id: node_id.clone(),
                runner: self.run.plan.nodes[index].runner.clone(),
                model_id: self.run.plan.nodes[index].model_id.clone(),
                prompt: self.run.plan.nodes[index].job.clone(),
                role,
            };

            if requires_gate {
                self.run.plan.nodes[index].status = GraphNodeStatus::WaitingGate;
                self.run.phase = GraphRunPhase::WaitingGate;
                self.run.waiting_gate_node = Some(node_id);
                actions.push(GraphAction::PauseForGate(assignment));
                return actions;
            }

            self.run.plan.nodes[index].status = GraphNodeStatus::Running;
            self.active.insert(node_id);
            actions.push(GraphAction::StartNode(assignment));
            index += 1;
        }

        if actions.is_empty() && self.active.is_empty() && all_work_done(&self.run.plan) {
            self.run.phase = GraphRunPhase::Completed;
            actions.push(GraphAction::Completed);
        }
        actions
    }

    pub fn assignment_line(&self) -> String {
        self.run
            .plan
            .nodes
            .iter()
            .filter(|node| node.role != NodeRole::Orchestrate)
            .map(|node| format!("{}:{}→{}", node.id, node.role_short(), node.model_id))
            .collect::<Vec<_>>()
            .join(" | ")
    }

    pub fn status_line(&self) -> Option<String> {
        self.status_line_with(|_runner, model| model.to_owned())
    }

    pub fn status_line_with<F>(&self, mut resolve_model: F) -> Option<String>
    where
        F: FnMut(&str, &str) -> String,
    {
        let non_orch: Vec<&GraphNode> = self
            .run
            .plan
            .nodes
            .iter()
            .filter(|node| node.role != NodeRole::Orchestrate)
            .collect();
        if non_orch.is_empty() {
            return None;
        }
        let m = non_orch.len();

        if let Some(pos) = non_orch
            .iter()
            .position(|n| self.active.contains(&n.id) || n.status == GraphNodeStatus::Running)
        {
            let current_node = non_orch[pos];
            let n = pos + 1;
            let model = resolve_model(&current_node.runner, &current_node.model_id);
            return Some(format!(
                "Graph {n}/{m} · {} {} · {} {model} · running",
                current_node.id, current_node.role, current_node.runner
            ));
        }

        if let Some(pos) = non_orch
            .iter()
            .position(|n| n.status == GraphNodeStatus::Failed)
        {
            let current_node = non_orch[pos];
            let n = pos + 1;
            let model = resolve_model(&current_node.runner, &current_node.model_id);
            return Some(format!(
                "Graph {n}/{m} · {} {} · {} {model} · failed",
                current_node.id, current_node.role, current_node.runner
            ));
        }

        if non_orch.iter().all(|n| {
            matches!(
                n.status,
                GraphNodeStatus::Completed | GraphNodeStatus::Skipped
            )
        }) {
            let current_node = non_orch.last().unwrap();
            let n = m;
            let model = resolve_model(&current_node.runner, &current_node.model_id);
            return Some(format!(
                "Graph {n}/{m} · {} {} · {} {model} · done",
                current_node.id, current_node.role, current_node.runner
            ));
        }

        let pos = non_orch
            .iter()
            .position(|n| {
                !matches!(
                    n.status,
                    GraphNodeStatus::Completed | GraphNodeStatus::Skipped
                )
            })
            .unwrap_or(0);
        let current_node = non_orch[pos];
        let n = pos + 1;
        let model = resolve_model(&current_node.runner, &current_node.model_id);
        Some(format!(
            "Graph {n}/{m} · {} {} · {} {model} · pending",
            current_node.id, current_node.role, current_node.runner
        ))
    }
}

fn dependencies_met(plan: &GraphPlan, node_id: &str, completed: &HashSet<String>) -> bool {
    let parents = plan
        .edges
        .iter()
        .filter(|edge| edge.to == node_id)
        .map(|edge| edge.from.clone())
        .collect::<Vec<_>>();
    if parents.is_empty() {
        return true;
    }
    parents.iter().all(|parent| completed.contains(parent))
}

fn all_work_done(plan: &GraphPlan) -> bool {
    plan.nodes
        .iter()
        .filter(|node| node.role != NodeRole::Orchestrate)
        .all(|node| {
            matches!(
                node.status,
                GraphNodeStatus::Completed | GraphNodeStatus::Skipped | GraphNodeStatus::Failed
            )
        })
}

trait RoleShort {
    fn role_short(&self) -> &'static str;
}

impl RoleShort for GraphNode {
    fn role_short(&self) -> &'static str {
        match self.role {
            NodeRole::Orchestrate => "orch",
            NodeRole::Research => "research",
            NodeRole::Explore => "explore",
            NodeRole::Test => "test",
            NodeRole::Implement => "impl",
            NodeRole::Draft => "draft",
            NodeRole::Plan => "plan",
            NodeRole::Route => "route",
            NodeRole::Review => "review",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::types::{GraphEdge, GraphLimits};

    #[test]
    fn graph_status_line_updates_across_node_lifecycle_states() {
        let plan = GraphPlan {
            schema: "cursor-agent-graph/v1".to_owned(),
            id: "test-run".to_owned(),
            orchestrator_model: "composer-2.5".to_owned(),
            user_prompt: "multi step prompt".to_owned(),
            nodes: vec![
                GraphNode {
                    id: "orchestrate".to_owned(),
                    role: NodeRole::Orchestrate,
                    job: "Build execution graph".to_owned(),
                    runner: "cursor".to_owned(),
                    model_id: "composer-2.5".to_owned(),
                    skill: None,
                    requires_gate: false,
                    status: GraphNodeStatus::Completed,
                    attempt: 0,
                },
                GraphNode {
                    id: "step-1".to_owned(),
                    role: NodeRole::Research,
                    job: "Research task".to_owned(),
                    runner: "cursor".to_owned(),
                    model_id: "composer-2.5".to_owned(),
                    skill: None,
                    requires_gate: false,
                    status: GraphNodeStatus::Pending,
                    attempt: 0,
                },
                GraphNode {
                    id: "step-2".to_owned(),
                    role: NodeRole::Implement,
                    job: "Implement task".to_owned(),
                    runner: "agy".to_owned(),
                    model_id: "gemini-3.8-flash-medium".to_owned(),
                    skill: None,
                    requires_gate: false,
                    status: GraphNodeStatus::Pending,
                    attempt: 0,
                },
                GraphNode {
                    id: "step-3".to_owned(),
                    role: NodeRole::Test,
                    job: "Test task".to_owned(),
                    runner: "cursor".to_owned(),
                    model_id: "composer-2.5".to_owned(),
                    skill: None,
                    requires_gate: false,
                    status: GraphNodeStatus::Pending,
                    attempt: 0,
                },
                GraphNode {
                    id: "step-4".to_owned(),
                    role: NodeRole::Review,
                    job: "Review task".to_owned(),
                    runner: "cursor".to_owned(),
                    model_id: "composer-2.5".to_owned(),
                    skill: None,
                    requires_gate: false,
                    status: GraphNodeStatus::Pending,
                    attempt: 0,
                },
            ],
            edges: vec![
                GraphEdge {
                    from: "step-1".to_owned(),
                    to: "step-2".to_owned(),
                },
                GraphEdge {
                    from: "step-2".to_owned(),
                    to: "step-3".to_owned(),
                },
                GraphEdge {
                    from: "step-3".to_owned(),
                    to: "step-4".to_owned(),
                },
            ],
            limits: GraphLimits {
                max_parallel: 1,
                max_retries: 1,
            },
        };

        let run = GraphRun {
            plan,
            conversation_id: "conv-1".to_owned(),
            phase: GraphRunPhase::Active,
            waiting_gate_node: None,
        };
        let mut session = GraphRunSession::new(run);

        // 1. Pending step-1
        assert_eq!(
            session.status_line().as_deref(),
            Some("Graph 1/4 · step-1 research · cursor composer-2.5 · pending")
        );

        // 2. Schedule step-1 -> Running
        let actions = session.schedule();
        assert_eq!(actions.len(), 1);
        assert_eq!(
            session.status_line().as_deref(),
            Some("Graph 1/4 · step-1 research · cursor composer-2.5 · running")
        );

        // 3. Complete step-1 -> step-2 is scheduled and Running
        let actions = session.on_node_finished("step-1", true);
        assert_eq!(actions.len(), 1);
        assert_eq!(
            session.status_line().as_deref(),
            Some("Graph 2/4 · step-2 implement · agy gemini-3.8-flash-medium · running")
        );

        // 4. Fail step-2 with max_retries = 1 -> Failed
        let _ = session.on_node_finished("step-2", false);
        assert_eq!(
            session.status_line().as_deref(),
            Some("Graph 2/4 · step-2 implement · agy gemini-3.8-flash-medium · failed")
        );
    }
}
