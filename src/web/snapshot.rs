use serde::{Deserialize, Serialize};

use crate::graph::{GraphEdge, GraphNodeStatus, GraphRun, GraphRunPhase, NodeRole};
use crate::todo::Todo;
use crate::web::layout::{NodeLayout, layout_graph};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<UiTheme>,
    pub agents: Vec<AgentView>,
    pub graph: Option<GraphView>,
    #[serde(default)]
    pub todos: Vec<Todo>,
    /// Configured `[todo] jira_base_url`, if any; lets the page preview a
    /// resolved link for a typed key without a round trip (FR-564, UX-561).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jira_base_url: Option<String>,
    /// Live chat session of the `dmuxai --web` TUI (SPEC-dashboard-parity
    /// FR-780); absent for `serve` and headless runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionView>,
}

/// Transcript limits for the snapshot (SPEC-dashboard-parity OQ-781).
pub const SESSION_TRANSCRIPT_MAX_MESSAGES: usize = 100;
pub const SESSION_MESSAGE_MAX_BYTES: usize = 32 * 1024;
pub const SESSION_TRANSCRIPT_MAX_BYTES: usize = 512 * 1024;
/// Marker appended to a message cut at `SESSION_MESSAGE_MAX_BYTES`.
pub const SESSION_TRUNCATED_MARKER: &str = "\n… [truncated in the dashboard; full text in the TUI]";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct SessionView {
    pub conversation_id: String,
    pub conversation_title: String,
    pub selected_agent: String,
    pub details_expanded: bool,
    pub agents: Vec<SessionAgent>,
    pub transcript: Vec<SessionMessage>,
    /// Messages dropped from the front of the transcript by the limits.
    #[serde(default)]
    pub transcript_omitted: usize,
    pub conversations: Vec<SessionConversation>,
    pub commands: Vec<SessionCommandInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct SessionAgent {
    pub id: String,
    pub label: String,
    pub mode: String,
    pub modes: Vec<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub busy: bool,
    pub allowed: bool,
    pub models_loading: bool,
    pub models: Vec<SessionModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunStateView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct RunStateView {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct SessionModel {
    pub id: String,
    pub label: String,
    pub efforts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct SessionMessage {
    /// Stable within one conversation: `<conversation id>:<index>`.
    pub id: String,
    /// `user`, `agent`, `system` or `activity`.
    pub role: String,
    pub agent: Option<String>,
    pub content: String,
    pub streaming: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct SessionConversation {
    pub id: String,
    pub title: String,
    pub updated_at_ms: u64,
    #[serde(default)]
    pub last_agent: Option<String>,
    #[serde(default)]
    pub turn_count: u64,
    #[serde(default)]
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct SessionCommandInfo {
    pub name: String,
    pub description: String,
}

/// Cuts `content` to `SESSION_MESSAGE_MAX_BYTES` on a char boundary and
/// appends the marker; returns the input unchanged when it fits.
pub fn cap_session_message(content: &str) -> String {
    if content.len() <= SESSION_MESSAGE_MAX_BYTES {
        return content.to_owned();
    }
    let mut end = SESSION_MESSAGE_MAX_BYTES;
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &content[..end], SESSION_TRUNCATED_MARKER)
}

/// Keeps the newest messages within the count and total byte limits; returns
/// the kept messages and how many were dropped from the front.
pub fn cap_session_transcript(messages: Vec<SessionMessage>) -> (Vec<SessionMessage>, usize) {
    let total = messages.len();
    let mut kept: Vec<SessionMessage> = Vec::new();
    let mut bytes = 0usize;
    for mut message in messages.into_iter().rev() {
        if kept.len() >= SESSION_TRANSCRIPT_MAX_MESSAGES {
            break;
        }
        message.content = cap_session_message(&message.content);
        if !kept.is_empty() && bytes + message.content.len() > SESSION_TRANSCRIPT_MAX_BYTES {
            break;
        }
        bytes += message.content.len();
        kept.push(message);
    }
    kept.reverse();
    let omitted = total - kept.len();
    (kept, omitted)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UiTheme {
    pub theme: String,
    #[serde(default)]
    pub accent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reduced_motion: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentView {
    pub id: String,
    pub label: String,
    pub busy: bool,
    pub phase: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunStateView>,
    /// Bounded activity detail the TUI status bar shows while the turn is
    /// running (FR-981), for example `▸ read src/app.rs`. `None` when the
    /// agent has no live detail, including after the turn ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
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
    /// Number of layers in the layout (0 when the graph has no nodes).
    #[serde(default)]
    pub layers: u32,
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
    /// Incoming node ids, in plan order.
    #[serde(default)]
    pub deps: Vec<String>,
    /// Layered position for the Graph view, when available.
    #[serde(default)]
    pub layout: Option<NodeLayout>,
    /// Per-node details; empty until a publisher fills them (FR-702).
    #[serde(default)]
    pub detail: NodeDetail,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct NodeDetail {
    #[serde(default)]
    pub log_tail: Vec<String>,
    #[serde(default)]
    pub changed_files: Vec<String>,
}

pub fn truncate_with_ellipsis(s: &str, limit: usize) -> String {
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

    let node_ids: Vec<&str> = run.plan.nodes.iter().map(|node| node.id.as_str()).collect();
    let edge_pairs: Vec<(&str, &str)> = run
        .plan
        .edges
        .iter()
        .map(|edge| (edge.from.as_str(), edge.to.as_str()))
        .collect();
    let layout = layout_graph(&node_ids, &edge_pairs);
    let layers = layout
        .values()
        .map(|placed| placed.layer)
        .max()
        .map_or(0, |deepest| deepest + 1);

    let mut plan_position: std::collections::BTreeMap<&str, usize> =
        std::collections::BTreeMap::new();
    for (position, node) in run.plan.nodes.iter().enumerate() {
        plan_position.entry(node.id.as_str()).or_insert(position);
    }
    let mut incoming: std::collections::BTreeMap<&str, Vec<(&str, usize)>> =
        std::collections::BTreeMap::new();
    for edge in &run.plan.edges {
        if edge.from == edge.to {
            continue;
        }
        let (Some(&source), Some(_)) = (
            plan_position.get(edge.from.as_str()),
            plan_position.get(edge.to.as_str()),
        ) else {
            continue;
        };
        incoming
            .entry(edge.to.as_str())
            .or_default()
            .push((edge.from.as_str(), source));
    }
    for sources in incoming.values_mut() {
        sources.sort_by_key(|(_, position)| *position);
        sources.dedup_by(|a, b| a.0 == b.0);
    }

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

        let deps = incoming
            .get(node.id.as_str())
            .map(|sources| sources.iter().map(|(id, _)| (*id).to_string()).collect())
            .unwrap_or_default();

        let started_at_ms = node.started_at_ms;
        let finished_at_ms = node.finished_at_ms;
        let elapsed_ms = match (started_at_ms, finished_at_ms) {
            (Some(start), Some(finish)) => Some(finish.saturating_sub(start)),
            _ => None,
        };

        nodes.push(NodeView {
            id: node.id.clone(),
            role: node.role,
            job: truncate_with_ellipsis(&node.job, 200),
            runner: node.runner.clone(),
            model_id: node.model_id.clone(),
            status: node.status,
            attempt: node.attempt,
            started_at_ms,
            finished_at_ms,
            elapsed_ms,
            deps,
            layout: layout.get(node.id.as_str()).copied(),
            detail: NodeDetail::default(),
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
        layers,
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
                        started_at_ms: None,
                        finished_at_ms: None,
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
                        started_at_ms: None,
                        finished_at_ms: None,
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
                        started_at_ms: None,
                        finished_at_ms: None,
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
            ..Default::default()
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
            ui: None,
            agents: vec![AgentView {
                id: "codex".to_string(),
                label: "Codex".to_string(),
                busy: false,
                phase: None,
                model: Some("o3-mini".to_string()),
                color: None,
                run: None,
                activity: None,
            }],
            graph: Some(view),
            todos: Vec::new(),
            jira_base_url: None,
            session: None,
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

    // TC-702 / FR-701: nodes expose deps and layout, graph exposes layers.
    #[test]
    fn snapshot_exposes_deps_and_layout() {
        let make_node = |id: &str, job: &str| GraphNode {
            id: id.to_string(),
            role: NodeRole::Implement,
            job: job.to_string(),
            runner: "codex".to_string(),
            model_id: "m".to_string(),
            skill: None,
            requires_gate: false,
            status: GraphNodeStatus::Pending,
            attempt: 0,
            started_at_ms: None,
            finished_at_ms: None,
        };
        let run = GraphRun {
            plan: GraphPlan {
                schema: "dmuxai.graph/v1".to_string(),
                id: "run-layout".to_string(),
                orchestrator_model: "gpt-4o".to_string(),
                user_prompt: "prompt".to_string(),
                limits: GraphLimits::default(),
                nodes: vec![
                    make_node("a", "root"),
                    make_node("b", "left"),
                    make_node("c", "right"),
                ],
                // Deliberately out of plan order and with a duplicate.
                edges: vec![
                    GraphEdge {
                        from: "a".to_string(),
                        to: "c".to_string(),
                    },
                    GraphEdge {
                        from: "a".to_string(),
                        to: "b".to_string(),
                    },
                    GraphEdge {
                        from: "a".to_string(),
                        to: "b".to_string(),
                    },
                ],
            },
            conversation_id: "conv-1".to_string(),
            phase: GraphRunPhase::Active,
            waiting_gate_node: None,
            ..Default::default()
        };

        let view = snapshot_from_run(&run);

        assert_eq!(view.layers, 2);
        let find = |id: &str| view.nodes.iter().find(|node| node.id == id).unwrap();
        assert_eq!(find("a").deps, Vec::<String>::new());
        assert_eq!(find("b").deps, vec!["a".to_string()]);
        assert_eq!(find("c").deps, vec!["a".to_string()]);
        assert_eq!(find("a").layout, Some(NodeLayout { layer: 0, order: 0 }));
        assert_eq!(find("b").layout, Some(NodeLayout { layer: 1, order: 0 }));
        assert_eq!(find("c").layout, Some(NodeLayout { layer: 1, order: 1 }));
        assert_eq!(find("a").detail, NodeDetail::default());

        let json = serde_json::to_string(&view).unwrap();
        assert!(json.contains("\"deps\":[\"a\"]"));
        assert!(json.contains("\"layers\":2"));
        assert!(json.contains("\"layout\":{\"layer\":0,\"order\":0}"));
        assert!(json.contains("\"detail\":{\"log_tail\":[],\"changed_files\":[]}"));
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

    // TC-860 / FR-860: SessionConversation new fields serialize and legacy JSON without them deserializes.
    #[test]
    fn session_conversation_fields_roundtrip() {
        let conv = SessionConversation {
            id: "conv-123".to_string(),
            title: "Refactor sidebar".to_string(),
            updated_at_ms: 1_700_000_000_000,
            last_agent: Some("Cursor".to_string()),
            turn_count: 4,
            running: true,
        };
        let json = serde_json::to_string(&conv).unwrap();
        assert!(json.contains("\"last_agent\":\"Cursor\""), "{json}");
        assert!(json.contains("\"turn_count\":4"), "{json}");
        assert!(json.contains("\"running\":true"), "{json}");

        let roundtrip: SessionConversation = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip, conv);

        // Legacy payload without last_agent, turn_count, running still deserializes with defaults.
        let legacy = r#"{"id":"conv-old","title":"Legacy chat","updated_at_ms":42}"#;
        let parsed: SessionConversation = serde_json::from_str(legacy).unwrap();
        assert_eq!(
            parsed,
            SessionConversation {
                id: "conv-old".to_string(),
                title: "Legacy chat".to_string(),
                updated_at_ms: 42,
                last_agent: None,
                turn_count: 0,
                running: false,
            }
        );
    }
}
