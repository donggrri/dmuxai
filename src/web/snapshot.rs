use serde::{Deserialize, Serialize};

use crate::graph::{GraphEdge, GraphNodeStatus, GraphRun, GraphRunPhase, NodeRole};
use crate::todo::Todo;

pub const SNAPSHOT_SCHEMA: &str = "dmuxai.dashboard/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotSource {
    Tui,
    Files,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DashboardSnapshot {
    pub schema: String,
    pub revision: u64,
    pub generated_at_ms: u64,
    /// Server clock when this payload was sent (set per response, not part of
    /// the revision); the page computes live elapsed time from it.
    #[serde(default)]
    pub sent_at_ms: u64,
    pub source: SnapshotSource,
    pub workspace: String,
    pub agents: Vec<AgentView>,
    pub graph: Option<GraphView>,
    #[serde(default)]
    pub todos: Vec<Todo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentView {
    pub id: String,
    pub label: String,
    pub busy: bool,
    pub phase: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GraphView {
    pub id: String,
    pub phase: GraphRunPhase,
    pub orchestrator_model: String,
    pub user_prompt: String,
    pub current_nodes: Vec<String>,
    pub waiting_gate_node: Option<String>,
    pub counts: GraphCounts,
    pub nodes: Vec<NodeView>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GraphCounts {
    pub pending: usize,
    pub ready: usize,
    pub running: usize,
    pub waiting_gate: usize,
    pub completed: usize,
    pub failed: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct NodeView {
    pub id: String,
    pub role: NodeRole,
    pub job: String,
    pub runner: String,
    pub model_id: String,
    pub status: GraphNodeStatus,
    pub attempt: u32,
    pub started_at_ms: Option<u64>,
    pub finished_at_ms: Option<u64>,
    pub elapsed_ms: Option<u64>,
}

fn truncate_with_ellipsis(s: &str, limit: usize) -> String {
    let mut chars = s.chars();
    let prefix: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

/// Milliseconds since the Unix epoch on the server clock.
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

pub fn snapshot_from_run(run: &GraphRun) -> GraphView {
    let mut counts = GraphCounts {
        pending: 0,
        ready: 0,
        running: 0,
        waiting_gate: 0,
        completed: 0,
        failed: 0,
        skipped: 0,
    };
    let mut current_nodes = Vec::new();
    let mut nodes = Vec::with_capacity(run.plan.nodes.len());

    for node in &run.plan.nodes {
        match node.status {
            GraphNodeStatus::Pending => counts.pending += 1,
            GraphNodeStatus::Ready => counts.ready += 1,
            GraphNodeStatus::Running => {
                counts.running += 1;
                current_nodes.push(node.id.clone());
            }
            GraphNodeStatus::WaitingGate => counts.waiting_gate += 1,
            GraphNodeStatus::Completed => counts.completed += 1,
            GraphNodeStatus::Failed => counts.failed += 1,
            GraphNodeStatus::Skipped => counts.skipped += 1,
        }

        nodes.push(NodeView {
            id: node.id.clone(),
            role: node.role,
            job: truncate_with_ellipsis(&node.job, 200),
            runner: node.runner.clone(),
            model_id: node.model_id.clone(),
            status: node.status,
            attempt: node.attempt,
            started_at_ms: None,
            finished_at_ms: None,
            elapsed_ms: None,
        });
    }

    GraphView {
        id: run.plan.id.clone(),
        phase: run.phase,
        orchestrator_model: run.plan.orchestrator_model.clone(),
        user_prompt: truncate_with_ellipsis(&run.plan.user_prompt, 200),
        current_nodes,
        waiting_gate_node: run.waiting_gate_node.clone(),
        counts,
        nodes,
        edges: run.plan.edges.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{GraphEdge, GraphLimits, GraphNode, GraphPlan};

    // TC-525: snapshot_reports_current_node_runner_and_counts
    #[test]
    fn snapshot_reports_current_node_runner_and_counts() {
        let long_job = "a".repeat(300);
        let run = GraphRun {
            plan: GraphPlan {
                schema: "dmuxai.graph/v1".to_string(),
                id: "run-42".to_string(),
                orchestrator_model: "gpt-4o".to_string(),
                user_prompt: "Build dashboard".to_string(),
                limits: GraphLimits::default(),
                nodes: vec![
                    GraphNode {
                        id: "step-1".to_string(),
                        role: NodeRole::Plan,
                        job: "Plan task".to_string(),
                        runner: "codex".to_string(),
                        model_id: "o3-mini".to_string(),
                        skill: None,
                        requires_gate: false,
                        status: GraphNodeStatus::Completed,
                        attempt: 1,
                    },
                    GraphNode {
                        id: "step-2".to_string(),
                        role: NodeRole::Implement,
                        job: long_job.clone(),
                        runner: "opencode".to_string(),
                        model_id: "claude-3-5-sonnet".to_string(),
                        skill: None,
                        requires_gate: false,
                        status: GraphNodeStatus::Running,
                        attempt: 1,
                    },
                    GraphNode {
                        id: "step-3".to_string(),
                        role: NodeRole::Review,
                        job: "Review code".to_string(),
                        runner: "cursor".to_string(),
                        model_id: "cursor-small".to_string(),
                        skill: None,
                        requires_gate: false,
                        status: GraphNodeStatus::Pending,
                        attempt: 0,
                    },
                ],
                edges: vec![GraphEdge {
                    from: "step-1".to_string(),
                    to: "step-2".to_string(),
                }],
            },
            conversation_id: "conv-1".to_string(),
            phase: GraphRunPhase::Active,
            waiting_gate_node: None,
        };

        let view = snapshot_from_run(&run);

        assert_eq!(view.current_nodes, vec!["step-2"]);
        assert_eq!(view.counts.completed, 1);
        assert_eq!(view.counts.running, 1);
        assert_eq!(view.counts.pending, 1);
        assert_eq!(view.counts.ready, 0);

        let running_node = view.nodes.iter().find(|n| n.id == "step-2").unwrap();
        assert_eq!(running_node.runner, "opencode");
        assert_eq!(running_node.model_id, "claude-3-5-sonnet");
        assert_eq!(running_node.job.chars().count(), 201); // 200 chars + ellipsis
        assert!(running_node.job.ends_with('…'));

        let snapshot = DashboardSnapshot {
            schema: SNAPSHOT_SCHEMA.to_string(),
            revision: 1,
            generated_at_ms: 1000,
            sent_at_ms: 0,
            source: SnapshotSource::Files,
            workspace: "/tmp/test".to_string(),
            agents: vec![AgentView {
                id: "codex".to_string(),
                label: "Codex".to_string(),
                busy: false,
                phase: None,
                model: Some("o3-mini".to_string()),
            }],
            graph: Some(view),
            todos: Vec::new(),
        };

        let json = serde_json::to_string(&snapshot).unwrap();

        // NFR-521: access token is never in snapshot JSON
        assert!(!json.contains("token"));
        // Stable field names from spec
        assert!(json.contains("\"schema\":\"dmuxai.dashboard/v1\""));
        assert!(json.contains("\"revision\":1"));
        assert!(json.contains("\"generated_at_ms\":1000"));
        assert!(json.contains("\"source\":\"files\""));
        assert!(json.contains("\"workspace\":\"/tmp/test\""));
        assert!(json.contains("\"current_nodes\":[\"step-2\"]"));
        assert!(json.contains("\"counts\":{"));
        assert!(json.contains("\"started_at_ms\":null"));
        assert!(json.contains("\"finished_at_ms\":null"));
        assert!(json.contains("\"elapsed_ms\":null"));
    }

    #[test]
    fn test_multibyte_truncation() {
        let multibyte = "한".repeat(150) + &"🚀".repeat(150);
        assert_eq!(multibyte.chars().count(), 300);

        let truncated = truncate_with_ellipsis(&multibyte, 200);
        assert_eq!(truncated.chars().count(), 201);
        assert!(truncated.ends_with('…'));
        assert!(std::str::from_utf8(truncated.as_bytes()).is_ok());
    }
}
