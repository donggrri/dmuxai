use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

use crate::events::{EventDraft, EventSink};
use crate::graph::GraphNodeStatus;
use crate::todo::Todo;
use crate::web::snapshot::{
    AgentView, DashboardSnapshot, GraphView, SNAPSHOT_SCHEMA, SessionView, SnapshotSource,
    TurnView, UiTheme,
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
    switch_config: Option<crate::web::WorkspaceSwitchConfig>,
}

#[derive(Clone)]
pub struct DashboardHub {
    state: Arc<Mutex<HubState>>,
    watch_tx: Arc<watch::Sender<DashboardSnapshot>>,
    events: Option<Arc<EventSink>>,
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
            session: None,
            turn: None,
        };

        let (tx, _) = watch::channel(initial_snapshot.clone());

        Self {
            state: Arc::new(Mutex::new(HubState {
                timings: HashMap::new(),
                current_graph_id: None,
                snapshot: initial_snapshot,
                switch_config: None,
            })),
            watch_tx: Arc::new(tx),
            events: None,
        }
    }

    pub fn set_switch_config(&self, config: crate::web::WorkspaceSwitchConfig) {
        self.state.lock().unwrap().switch_config = Some(config);
    }

    pub fn switch_config(&self) -> Option<crate::web::WorkspaceSwitchConfig> {
        self.state.lock().unwrap().switch_config.clone()
    }

    pub fn publish(
        &self,
        agents: Vec<AgentView>,
        graph: Option<GraphView>,
        turn: Option<TurnView>,
    ) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.publish_at(agents, graph, turn, now_ms);
    }

    pub fn publish_at(
        &self,
        agents: Vec<AgentView>,
        mut graph: Option<GraphView>,
        turn: Option<TurnView>,
        now_ms: u64,
    ) {
        let changed_snapshot = {
            let mut state = self.state.lock().unwrap();

            let new_graph_id = graph.as_ref().map(|g| g.id.clone());
            if new_graph_id != state.current_graph_id {
                state.timings.clear();
                state.current_graph_id = new_graph_id;
            }

            if let Some(ref mut g) = graph {
                for node in &mut g.nodes {
                    let timing =
                        state
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

                    if let Some(started) = node.started_at_ms {
                        timing.started_at_ms = Some(started);
                    } else if node.status == GraphNodeStatus::Running
                        && timing.started_at_ms.is_none()
                    {
                        timing.started_at_ms = Some(now_ms);
                    }
                    let terminal = matches!(
                        node.status,
                        GraphNodeStatus::Completed
                            | GraphNodeStatus::Failed
                            | GraphNodeStatus::Skipped
                    );
                    if let Some(finished) = node.finished_at_ms {
                        timing.finished_at_ms = Some(finished);
                    } else if !terminal {
                        // A node that left a terminal state without an attempt bump
                        // (or whose Pending frame was coalesced) must not keep a stale
                        // finish time from before.
                        timing.finished_at_ms = None;
                    } else if matches!(
                        node.status,
                        GraphNodeStatus::Completed
                            | GraphNodeStatus::Failed
                            | GraphNodeStatus::Skipped
                    ) && timing.finished_at_ms.is_none()
                    {
                        timing.finished_at_ms = Some(now_ms);
                    }

                    node.started_at_ms = node.started_at_ms.or(timing.started_at_ms);
                    node.finished_at_ms = node.finished_at_ms.or(timing.finished_at_ms);
                    node.elapsed_ms = match (node.started_at_ms, node.finished_at_ms) {
                        (Some(start), Some(finish)) => Some(finish.saturating_sub(start)),
                        _ => None,
                    };
                }
            }

            let content_changed = state.snapshot.agents != agents
                || state.snapshot.graph != graph
                || state.snapshot.turn != turn;
            if content_changed {
                state.snapshot.revision += 1;
                state.snapshot.generated_at_ms = now_ms;
                state.snapshot.agents = agents;
                state.snapshot.graph = graph;
                state.snapshot.turn = turn;
                let _ = self.watch_tx.send_replace(state.snapshot.clone());
                Some(state.snapshot.clone())
            } else {
                None
            }
        };

        // State changes are observed after the state mutex is released so event
        // writing can never stall the publisher (FR-768).
        if let Some(snapshot) = changed_snapshot
            && let Some(sink) = &self.events
        {
            sink.observe_snapshot(&snapshot);
        }
    }

    /// Replaces the to-do list, bumping the revision only when it changes
    /// (FR-541). Agent/graph content is preserved.
    pub fn set_todos(&self, todos: Vec<Todo>) {
        let changed_snapshot = {
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
            state.snapshot.clone()
        };

        if let Some(sink) = &self.events {
            sink.observe_snapshot(&changed_snapshot);
        }
    }

    /// Replaces the live chat session (SPEC-dashboard-parity FR-780), bumping
    /// the revision only when it changes. Session changes are not
    /// state-change events, so the event sink is not consulted.
    pub fn set_session(&self, session: Option<SessionView>) {
        let mut state = self.state.lock().unwrap();
        if state.snapshot.session == session {
            return;
        }
        state.snapshot.session = session;
        state.snapshot.revision += 1;
        state.snapshot.generated_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let _ = self.watch_tx.send_replace(state.snapshot.clone());
    }

    /// Appends a system notice to the snapshot session transcript and bumps the revision (FR-874).
    pub fn push_notice(&self, notice: String) {
        let mut state = self.state.lock().unwrap();
        let session = state
            .snapshot
            .session
            .get_or_insert_with(SessionView::default);
        let idx = session.transcript.len();
        session
            .transcript
            .push(crate::web::snapshot::SessionMessage {
                id: format!("notice:{idx}"),
                role: "system".to_owned(),
                agent: None,
                content: notice,
                content_html: None,
                streaming: false,
                images: Vec::new(),
                prompt_id: None,
                queued: false,
            });
        state.snapshot.revision += 1;
        state.snapshot.generated_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let _ = self.watch_tx.send_replace(state.snapshot.clone());
    }

    /// Attaches an event sink so snapshot changes are appended to the log.
    pub fn with_events(mut self, sink: Arc<EventSink>) -> Self {
        self.events = Some(sink);
        self
    }

    pub fn events(&self) -> Option<Arc<EventSink>> {
        self.events.clone()
    }

    pub fn emit_event(&self, draft: EventDraft) {
        if let Some(sink) = &self.events {
            sink.emit(vec![draft]);
        }
    }

    pub fn note_runner_activity(&self, runner: &str) {
        if let Some(sink) = &self.events {
            sink.note_activity(runner);
        }
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
    use crate::graph::{GraphEdge, GraphNodeStatus, GraphRunPhase, NodeRole, PlannerCalls};
    use crate::web::snapshot::{GraphCounts, NodeDetail, NodeView};

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
            layers: 1,
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
                deps: Vec::new(),
                layout: None,
                detail: NodeDetail::default(),
                rev_change: None,
                task: None,
            }],
            edges: vec![GraphEdge {
                from: "a".to_string(),
                to: "b".to_string(),
            }],
            triage: None,
            approval: None,
            plan_history: Vec::new(),
            tasks: Vec::new(),
            planner_calls: PlannerCalls::default(),
            planner_io: Vec::new(),
            turns: Vec::new(),
            earlier_turns: 0,
            follow_up: Vec::new(),
        }
    }

    // TC-526: hub_tracks_node_timings_and_revisions
    #[test]
    fn hub_tracks_node_timings_and_revisions() {
        let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_string());
        assert_eq!(hub.snapshot().revision, 0);

        // 1. Publish running @ 1000
        let graph1 = make_test_graph("run-1", GraphNodeStatus::Running);
        hub.publish_at(Vec::new(), Some(graph1), None, 1000);

        let snap1 = hub.snapshot();
        assert_eq!(snap1.revision, 1);
        let node1 = &snap1.graph.as_ref().unwrap().nodes[0];
        assert_eq!(node1.started_at_ms, Some(1000));
        assert_eq!(node1.finished_at_ms, None);
        assert_eq!(node1.elapsed_ms, None);

        // 2. Publish completed @ 4000
        let graph1_done = make_test_graph("run-1", GraphNodeStatus::Completed);
        hub.publish_at(Vec::new(), Some(graph1_done.clone()), None, 4000);

        let snap2 = hub.snapshot();
        assert_eq!(snap2.revision, 2);
        let node2 = &snap2.graph.as_ref().unwrap().nodes[0];
        assert_eq!(node2.started_at_ms, Some(1000));
        assert_eq!(node2.finished_at_ms, Some(4000));
        assert_eq!(node2.elapsed_ms, Some(3000));

        // 3. Publish identical twice (at 5000 and 6000)
        hub.publish_at(Vec::new(), Some(graph1_done.clone()), None, 5000);
        assert_eq!(hub.snapshot().revision, 2);

        hub.publish_at(Vec::new(), Some(graph1_done), None, 6000);
        assert_eq!(hub.snapshot().revision, 2);

        // 4. Publish new graph id (run-2)
        let graph2 = make_test_graph("run-2", GraphNodeStatus::Running);
        hub.publish_at(Vec::new(), Some(graph2), None, 7000);

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
        hub.publish_at(Vec::new(), Some(graph_retry_run0), None, 8000);
        let snap_retry0 = hub.snapshot();
        let node_retry0 = &snap_retry0.graph.as_ref().unwrap().nodes[0];
        assert_eq!(node_retry0.started_at_ms, Some(8000));
        assert_eq!(node_retry0.finished_at_ms, None);

        let graph_retry_ready1 =
            make_test_graph_with_attempt("run-retry", GraphNodeStatus::Ready, 1);
        hub.publish_at(Vec::new(), Some(graph_retry_ready1), None, 9000);
        let snap_ready1 = hub.snapshot();
        let node_ready1 = &snap_ready1.graph.as_ref().unwrap().nodes[0];
        assert_eq!(node_ready1.started_at_ms, None);
        assert_eq!(node_ready1.finished_at_ms, None);

        let graph_retry_run1 =
            make_test_graph_with_attempt("run-retry", GraphNodeStatus::Running, 1);
        hub.publish_at(Vec::new(), Some(graph_retry_run1), None, 10000);
        let snap_retry1 = hub.snapshot();
        let node_retry1 = &snap_retry1.graph.as_ref().unwrap().nodes[0];
        assert_eq!(node_retry1.started_at_ms, Some(10000));
        assert_eq!(node_retry1.finished_at_ms, None);
    }

    // TC-883: a node that goes back to Running without an attempt bump drops the stale finish
    #[test]
    fn hub_clears_stale_finish_when_node_runs_again() {
        let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_string());
        hub.publish_at(
            Vec::new(),
            Some(make_test_graph("run-1", GraphNodeStatus::Running)),
            None,
            1000,
        );
        hub.publish_at(
            Vec::new(),
            Some(make_test_graph("run-1", GraphNodeStatus::Completed)),
            None,
            4000,
        );
        hub.publish_at(
            Vec::new(),
            Some(make_test_graph("run-1", GraphNodeStatus::Running)),
            None,
            5000,
        );
        let snap = hub.snapshot();
        let node = &snap.graph.as_ref().unwrap().nodes[0];
        assert_eq!(node.finished_at_ms, None);
        assert_eq!(node.elapsed_ms, None);
        assert_eq!(node.started_at_ms, Some(1000));

        // Executor-supplied timestamps win over the hub's own clock.
        let mut g = make_test_graph("run-1", GraphNodeStatus::Completed);
        g.nodes[0].started_at_ms = Some(1500);
        g.nodes[0].finished_at_ms = Some(6500);
        hub.publish_at(Vec::new(), Some(g), None, 7000);
        let snap = hub.snapshot();
        let node = &snap.graph.as_ref().unwrap().nodes[0];
        assert_eq!(node.started_at_ms, Some(1500));
        assert_eq!(node.finished_at_ms, Some(6500));
        assert_eq!(node.elapsed_ms, Some(5000));
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
            reduced_motion: None,
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
                    run: None,
                    activity: None,
                },
                AgentView {
                    id: "cursor".to_string(),
                    label: "Cursor".to_string(),
                    busy: false,
                    phase: None,
                    model: None,
                    color: Some("#000080".to_string()),
                    run: None,
                    activity: None,
                },
            ],
            None,
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

    // TC-769 / FR-768: a content change on the hub feeds the attached event
    // sink, while the baseline publish and repeats emit nothing.
    #[test]
    fn hub_with_events_emits_on_transition() {
        use crate::events::{EventLog, EventSink, events_path, read_since};
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let log = EventLog::open(dir.path(), dir.path().display().to_string());
        let sink = EventSink::new(log.clone(), Duration::ZERO, None);
        let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_string())
            .with_events(sink.clone());

        assert_eq!(hub.events().map(|s| s.log().last_seq()), Some(0));

        // Baseline publish establishes the tracker's starting state.
        hub.publish_at(
            Vec::new(),
            Some(make_test_graph("g1", GraphNodeStatus::Ready)),
            None,
            1000,
        );
        assert_eq!(log.last_seq(), 0, "baseline publish emits no event");

        // Same content again: still no event.
        hub.publish_at(
            Vec::new(),
            Some(make_test_graph("g1", GraphNodeStatus::Ready)),
            None,
            2000,
        );
        assert_eq!(log.last_seq(), 0);

        // Node ready -> running produces exactly one event.
        hub.publish_at(
            Vec::new(),
            Some(make_test_graph("g1", GraphNodeStatus::Running)),
            None,
            3000,
        );

        let events = read_since(&events_path(dir.path()), 0, None);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "node");
        assert_eq!(events[0].task.as_deref(), Some("g1"));
        assert_eq!(events[0].node.as_deref(), Some("node-1"));
        assert_eq!(events[0].to_state, "running");
    }
}
