use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRole {
    Orchestrate,
    Research,
    Explore,
    Test,
    Implement,
    Draft,
    Plan,
    Route,
    Review,
}

impl fmt::Display for NodeRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Orchestrate => "orchestrate",
            Self::Research => "research",
            Self::Explore => "explore",
            Self::Test => "test",
            Self::Implement => "implement",
            Self::Draft => "draft",
            Self::Plan => "plan",
            Self::Route => "route",
            Self::Review => "review",
        };
        formatter.write_str(label)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub role: NodeRole,
    pub job: String,
    pub runner: String,
    pub model_id: String,
    pub skill: Option<String>,
    pub requires_gate: bool,
    pub status: GraphNodeStatus,
    pub attempt: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphNodeStatus {
    Pending,
    Ready,
    Running,
    WaitingGate,
    Completed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphLimits {
    pub max_parallel: usize,
    pub max_retries: u32,
}

impl Default for GraphLimits {
    fn default() -> Self {
        Self {
            max_parallel: 2,
            max_retries: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphPlan {
    pub schema: String,
    pub id: String,
    pub orchestrator_model: String,
    pub user_prompt: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub limits: GraphLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SinglePlan {
    pub runner: String,
    pub model_id: String,
    pub role: NodeRole,
    pub job: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionPlan {
    Single(SinglePlan),
    Graph(GraphPlan),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphRun {
    pub plan: GraphPlan,
    pub conversation_id: String,
    pub phase: GraphRunPhase,
    pub waiting_gate_node: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphRunPhase {
    Active,
    WaitingGate,
    Completed,
    Failed,
}
