use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

use crate::graph::GraphNodeStatus;
use crate::todo::Todo;
use crate::web::snapshot::{
    AgentView, DashboardSnapshot, GraphView, SNAPSHOT_SCHEMA, SnapshotSource, UiTheme,
};

#[derive(Debug, Clone, Default)]
struct NodeTiming {
    attempt: u32,
    started_at_ms: Option<u64>,
    finished_at_ms: Option<u64>,
}

struct HubState {
    timings: HashMap<String, NodeTiming>,
    current_graph_id: Option<String>,
    snapshot: DashboardSnapshot,
}

#[derive(Clone)]
pub struct DashboardHub {
    state: Arc<Mutex<HubState>>,
    watch_tx: Arc<watch::Sender<DashboardSnapshot>>,
}

impl DashboardHub {
    pub fn new(source: SnapshotSource, workspace: String) -> Self {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let initial_snapshot = DashboardSnapshot {
            schema: SNAPSHOT_SCHEMA.to_string(),
            revision: 0,
            generated_at_ms: now_ms,
            sent_at_ms: 0,
            source,
            workspace,
            ui: None,
            agents: Vec::new(),
            graph: None,
            todos: Vec::new(),
            jira_base_url: None,
        };

        let (tx, _) = watch::channel(initial_snapshot.clone());

        Self {
            state: Arc::new(Mutex::new(HubState {
                timings: HashMap::new(),
                current_graph_id: None,
                snapshot: initial_snapshot,
            })),
            watch_tx: Arc::new(tx),
        }
    }

    pub fn publish(&self, agents: Vec<AgentView>, graph: Option<GraphView>) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.publish_at(agents, graph, now_ms);
    }

    pub fn publish_at(&self, agents: Vec<AgentView>, mut graph: Option<GraphView>, now_ms: u64) {
        let mut state = self.state.lock().unwrap();

        let new_graph_id = graph.as_ref().map(|g| g.id.clone());
        if new_graph_id != state.current_graph_id {
            state.timings.clear();
            state.current_graph_id = new_graph_id;
        }

        if let Some(ref mut g) = graph {
            for node in &mut g.nodes {
                let timing = state
                    .timings
                    .entry(node.id.clone())
                    .or_insert_with(|| NodeTiming {
                        attempt: node.attempt,
                        started_at_ms: None,
                        finished_at_ms: None,
                    });

                if timing.attempt != node.attempt {
                    timing.attempt = node.attempt;
                    timing.started_at_ms = None;
                    timing.finished_at_ms = None;
                }

                if node.status == GraphNodeStatus::Running && timing.started_at_ms.is_none() {
                    timing.started_at_ms = Some(now_ms);
                }
                if matches!(
                    node.status,
                    GraphNodeStatus::Completed | GraphNodeStatus::Failed | GraphNodeStatus::Skipped
                ) && timing.finished_at_ms.is_none()
                {
                    timing.finished_at_ms = Some(now_ms);
                }

                node.started_at_ms = timing.started_at_ms;
                node.finished_at_ms = timing.finished_at_ms;
                node.elapsed_ms = match (timing.started_at_ms, timing.finished_at_ms) {
                    (Some(start), Some(finish)) => Some(finish.saturating_sub(start)),
                    _ => None,
                };
            }
        }

        let content_changed = state.snapshot.agents != agents || state.snapshot.graph != graph;
        if content_changed {
            state.snapshot.revision += 1;
            state.snapshot.generated_at_ms = now_ms;
            state.snapshot.agents = agents;
            state.snapshot.graph = graph;
            let _ = self.watch_tx.send_replace(state.snapshot.clone());
        }
    }

    /// Replaces the to-do list, bumping the revision only when it changes
    /// (FR-541). Agent/graph content is preserved.
    pub fn set_todos(&self, todos: Vec<Todo>) {
        let mut state = self.state.lock().unwrap();
        if state.snapshot.todos == todos {
            return;
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        state.snapshot.todos = todos;
        state.snapshot.revision += 1;
        state.snapshot.generated_at_ms = now_ms;
        let _ = self.watch_tx.send_replace(state.snapshot.clone());
    }

    /// Sets or updates the UI theme preferences in the snapshot (FR-603).
    pub fn set_ui(&self, ui: impl Into<Option<UiTheme>>) {
        let ui = ui.into();
        let mut state = self.state.lock().unwrap();
        if state.snapshot.ui == ui {
            return;
        }
        state.snapshot.ui = ui;
        state.snapshot.revision += 1;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        state.snapshot.generated_at_ms = now_ms;
        let _ = self.watch_tx.send_replace(state.snapshot.clone());
    }

    /// Builder helper to set the initial UI theme.
    pub fn with_ui(self, ui: impl Into<Option<UiTheme>>) -> Self {
        self.set_ui(ui);
        self
    }

    pub fn snapshot(&self) -> DashboardSnapshot {
        self.state.lock().unwrap().snapshot.clone()
    }

    /// Records the configured Jira base URL so the snapshot can preview links
    /// (FR-564). Bumps the revision only when the value changes.
    pub fn set_jira_base_url(&self, base: Option<String>) {
        let mut state = self.state.lock().unwrap();
        if state.snapshot.jira_base_url == base {
            return;
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        state.snapshot.jira_base_url = base;
        state.snapshot.revision += 1;
        state.snapshot.generated_at_ms = now_ms;
        let _ = self.watch_tx.send_replace(state.snapshot.clone());
    }

    pub fn subscribe(&self) -> watch::Receiver<DashboardSnapshot> {
        self.watch_tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{GraphEdge, GraphNodeStatus, GraphRunPhase, NodeRole};
    use crate::web::snapshot::{GraphCounts, NodeView};

    fn make_test_graph(id: &str, status: GraphNodeStatus) -> GraphView {
        make_test_graph_with_attempt(id, status, 0)
    }

    fn make_test_graph_with_attempt(id: &str, status: GraphNodeStatus, attempt: u32) -> GraphView {
        GraphView {
            id: id.to_string(),
            phase: GraphRunPhase::Active,
            orchestrator_model: "gpt-4o".to_string(),
            user_prompt: "test prompt".to_string(),
            current_nodes: if status == GraphNodeStatus::Running {
                vec!["node-1".to_string()]
            } else {
                Vec::new()
            },
            waiting_gate_node: None,
            counts: GraphCounts {
                pending: 0,
                ready: if status == GraphNodeStatus::Ready {
                    1
                } else {
                    0
                },
                running: if status == GraphNodeStatus::Running {
                    1
                } else {
                    0
                },
                waiting_gate: 0,
                completed: if status == GraphNodeStatus::Completed {
                    1
                } else {
                    0
                },
                failed: 0,
                skipped: 0,
            },
            nodes: vec![NodeView {
                id: "node-1".to_string(),
                role: NodeRole::Implement,
                job: "do work".to_string(),
                runner: "opencode".to_string(),
                model_id: "m".to_string(),
                status,
                attempt,
                started_at_ms: None,
                finished_at_ms: None,
                elapsed_ms: None,
            }],
            edges: vec![GraphEdge {
                from: "a".to_string(),
                to: "b".to_string(),
            }],
        }
    }

    // TC-526: hub_tracks_node_timings_and_revisions
    #[test]
    fn hub_tracks_node_timings_and_revisions() {
        let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_string());
        assert_eq!(hub.snapshot().revision, 0);

        // 1. Publish running @ 1000
        let graph1 = make_test_graph("run-1", GraphNodeStatus::Running);
        hub.publish_at(Vec::new(), Some(graph1), 1000);

        let snap1 = hub.snapshot();
        assert_eq!(snap1.revision, 1);
        let node1 = &snap1.graph.as_ref().unwrap().nodes[0];
        assert_eq!(node1.started_at_ms, Some(1000));
        assert_eq!(node1.finished_at_ms, None);
        assert_eq!(node1.elapsed_ms, None);

        // 2. Publish completed @ 4000
        let graph1_done = make_test_graph("run-1", GraphNodeStatus::Completed);
        hub.publish_at(Vec::new(), Some(graph1_done.clone()), 4000);

        let snap2 = hub.snapshot();
        assert_eq!(snap2.revision, 2);
        let node2 = &snap2.graph.as_ref().unwrap().nodes[0];
        assert_eq!(node2.started_at_ms, Some(1000));
        assert_eq!(node2.finished_at_ms, Some(4000));
        assert_eq!(node2.elapsed_ms, Some(3000));

        // 3. Publish identical twice (at 5000 and 6000)
        hub.publish_at(Vec::new(), Some(graph1_done.clone()), 5000);
        assert_eq!(hub.snapshot().revision, 2);

        hub.publish_at(Vec::new(), Some(graph1_done), 6000);
        assert_eq!(hub.snapshot().revision, 2);

        // 4. Publish new graph id (run-2)
        let graph2 = make_test_graph("run-2", GraphNodeStatus::Running);
        hub.publish_at(Vec::new(), Some(graph2), 7000);

        let snap3 = hub.snapshot();
        assert_eq!(snap3.revision, 3);
        let node3 = &snap3.graph.as_ref().unwrap().nodes[0];
        // Timings reset for new graph
        assert_eq!(node3.started_at_ms, Some(7000));
        assert_eq!(node3.finished_at_ms, None);
        assert_eq!(node3.elapsed_ms, None);

        // 5. Retry re-run per-attempt timings (finding #1):
        // running(attempt 0) @ 8000 -> ready(attempt 1) @ 9000 -> running(attempt 1) @ 10000
        let graph_retry_run0 =
            make_test_graph_with_attempt("run-retry", GraphNodeStatus::Running, 0);
        hub.publish_at(Vec::new(), Some(graph_retry_run0), 8000);
        let snap_retry0 = hub.snapshot();
        let node_retry0 = &snap_retry0.graph.as_ref().unwrap().nodes[0];
        assert_eq!(node_retry0.started_at_ms, Some(8000));
        assert_eq!(node_retry0.finished_at_ms, None);

        let graph_retry_ready1 =
            make_test_graph_with_attempt("run-retry", GraphNodeStatus::Ready, 1);
        hub.publish_at(Vec::new(), Some(graph_retry_ready1), 9000);
        let snap_ready1 = hub.snapshot();
        let node_ready1 = &snap_ready1.graph.as_ref().unwrap().nodes[0];
        assert_eq!(node_ready1.started_at_ms, None);
        assert_eq!(node_ready1.finished_at_ms, None);

        let graph_retry_run1 =
            make_test_graph_with_attempt("run-retry", GraphNodeStatus::Running, 1);
        hub.publish_at(Vec::new(), Some(graph_retry_run1), 10000);
        let snap_retry1 = hub.snapshot();
        let node_retry1 = &snap_retry1.graph.as_ref().unwrap().nodes[0];
        assert_eq!(node_retry1.started_at_ms, Some(10000));
        assert_eq!(node_retry1.finished_at_ms, None);
    }

    // TC-606 / FR-603: snapshot_exposes_ui_theme
    #[tokio::test]
    async fn snapshot_exposes_ui_theme() {
        use crate::web::server::build_router;
        use crate::web::snapshot::{AgentView, UiTheme};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_string());
        hub.set_ui(UiTheme {
            theme: "dark".to_string(),
            accent: Some("#3366ff".to_string()),
        });
        hub.publish(
            vec![
                AgentView {
                    id: "codex".to_string(),
                    label: "Codex".to_string(),
                    busy: false,
                    phase: None,
                    model: None,
                    color: Some("#123456".to_string()),
                },
                AgentView {
                    id: "cursor".to_string(),
                    label: "Cursor".to_string(),
                    busy: false,
                    phase: None,
                    model: None,
                    color: Some("#000080".to_string()),
                },
            ],
            None,
        );

        let token = "test-token-secret-12345".to_string();
        let allowed = Some(vec!["localhost:7420".to_string()]);
        let router = build_router(hub, token.clone(), allowed);

        let req = axum::http::Request::builder()
            .uri("/api/state")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(json["ui"]["theme"], "dark");
        assert_eq!(json["ui"]["accent"], "#3366ff");
        assert!(json.get("token").is_none());
        assert!(!String::from_utf8_lossy(&bytes).contains("test-token-secret-12345"));

        let agents = json["agents"].as_array().expect("agents array in response");
        let codex = agents
            .iter()
            .find(|a| a["id"] == "codex")
            .expect("codex agent present");
        assert_eq!(codex["color"], "#123456");
        let cursor = agents
            .iter()
            .find(|a| a["id"] == "cursor")
            .expect("cursor agent present");
        assert_eq!(cursor["color"], "#000080");
    }
}
