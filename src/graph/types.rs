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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_ms: Option<u64>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_node_timestamps_roundtrip_and_back_compat() {
        let legacy_json = r#"{
            "id": "plan",
            "role": "plan",
            "job": "Split CSV export",
            "runner": "agy",
            "model_id": "gemini-3.8-flash-high",
            "skill": null,
            "requires_gate": false,
            "status": "completed",
            "attempt": 1
        }"#;
        let parsed_legacy: GraphNode = serde_json::from_str(legacy_json).expect("parse legacy");
        assert_eq!(parsed_legacy.started_at_ms, None);
        assert_eq!(parsed_legacy.finished_at_ms, None);

        let reserialized_legacy = serde_json::to_string(&parsed_legacy).expect("serialize legacy");
        assert!(!reserialized_legacy.contains("started_at_ms"));
        assert!(!reserialized_legacy.contains("finished_at_ms"));

        let timed = GraphNode {
            started_at_ms: Some(1_700_000_000_000),
            finished_at_ms: Some(1_700_000_120_000),
            ..parsed_legacy
        };
        let json = serde_json::to_string(&timed).expect("serialize timed");
        assert!(json.contains("\"started_at_ms\":1700000000000"));
        assert!(json.contains("\"finished_at_ms\":1700000120000"));
        let roundtrip: GraphNode = serde_json::from_str(&json).expect("roundtrip timed");
        assert_eq!(roundtrip, timed);
    }
}
