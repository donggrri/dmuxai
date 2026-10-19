use serde::{Deserialize, Serialize};

use crate::graph::{
    DecisionBy, FallbackCause, GraphEdge, GraphNodeStatus, GraphPlan, GraphRun, GraphRunPhase,
    NodeRole, OverrideKind, PlanChange, PlanChangeKind, PlanChangeTrigger, PlanRevision, PlanStep,
    PlannerCalls, PlannerIo, Route, TaskState, Triage, TriageSource, judge_line_text, task_status,
};
use crate::todo::Todo;
use crate::web::layout::{NodeLayout, layout_graph};

pub const SNAPSHOT_SCHEMA: &str = "dmuxai.dashboard/v1";
/// FR-955: at most this many turn bands are sent in each snapshot.
pub const CONVERSATION_TURN_BAND_LIMIT: usize = 20;

const SNAPSHOT_USER_PROMPT_LIMIT: usize = 200;
const SNAPSHOT_PLAN_HISTORY_REASON_LIMIT: usize = 300;
const SNAPSHOT_APPROVAL_NOTE_LIMIT: usize = 300;
const SNAPSHOT_STEP_JOB_LIMIT: usize = 2000;
/// Live swimlane node job text stays short (unchanged contract).
const SNAPSHOT_NODE_JOB_LIMIT: usize = 200;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage: Option<TriageView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plan_history: Vec<PlanChangeView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<TaskChecklistView>,
    #[serde(default, skip_serializing_if = "planner_calls_is_default")]
    pub planner_calls: PlannerCalls,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub planner_io: Vec<PlannerIo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<TurnBandView>,
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub earlier_turns: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub follow_up: Vec<FollowUpEdgeView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TurnBandView {
    pub run_id: String,
    pub turn_index: u32,
    pub phase: GraphRunPhase,
    pub user_prompt: String,
    pub nodes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FollowUpEdgeView {
    pub from: String,
    pub to: String,
    pub from_run: String,
    pub to_run: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TriageView {
    pub route: Route,
    pub reason: String,
    pub source: TriageSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<FallbackCause>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_kind: Option<OverrideKind>,
    pub runner: String,
    pub model: String,
    /// Same plain-text line as the TUI (`triage_view::judge_line_text`).
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ApprovalView {
    pub rev: u32,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<DecisionBy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<ApprovalStepView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ApprovalStepView {
    pub id: String,
    pub role: String,
    pub runner: String,
    pub model: String,
    pub job: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev_change: Option<RevChangeMark>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevChangeMark {
    Added,
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PlanChangeView {
    pub rev: u32,
    pub kind: PlanChangeKind,
    pub at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_node: Option<String>,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<PlanChangeTrigger>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_chars: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_chars: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub tasks_added: usize,
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub tasks_changed: usize,
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub tasks_removed: usize,
    pub applied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<DecisionBy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TaskChecklistView {
    pub id: String,
    pub title: String,
    pub done_check: String,
    pub size: crate::graph::TaskSize,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub status: String,
    pub nodes: Vec<String>,
}

fn is_zero_usize(value: &usize) -> bool {
    *value == 0
}

fn planner_calls_is_default(calls: &PlannerCalls) -> bool {
    *calls == PlannerCalls::default()
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev_change: Option<RevChangeMark>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
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

fn latest_applied_plan_change(run: &GraphRun) -> Option<&PlanChange> {
    run.plan_history
        .iter()
        .rev()
        .find(|change| change.applied && change.kind != PlanChangeKind::Initial)
}

fn rev_change_for_step_id(revision: &PlanRevision, step_id: &str) -> Option<RevChangeMark> {
    if revision.added.iter().any(|id| id == step_id) {
        Some(RevChangeMark::Added)
    } else if revision.changed.iter().any(|id| id == step_id) {
        Some(RevChangeMark::Changed)
    } else {
        None
    }
}

fn rev_change_for_node_id(change: &PlanChange, node_id: &str) -> Option<RevChangeMark> {
    if change.added.iter().any(|id| id == node_id) {
        Some(RevChangeMark::Added)
    } else if change.changed.iter().any(|id| id == node_id) {
        Some(RevChangeMark::Changed)
    } else {
        None
    }
}

fn plan_change_view(change: &PlanChange) -> PlanChangeView {
    PlanChangeView {
        rev: change.rev,
        kind: change.kind,
        at_ms: change.at_ms,
        after_node: change.after_node.clone(),
        reason: truncate_with_ellipsis(&change.reason, SNAPSHOT_PLAN_HISTORY_REASON_LIMIT),
        trigger: change.trigger,
        prompt_chars: change.prompt_chars,
        response_chars: change.response_chars,
        added: change.added.clone(),
        changed: change.changed.clone(),
        removed: change.removed.clone(),
        tasks_added: change.tasks_added,
        tasks_changed: change.tasks_changed,
        tasks_removed: change.tasks_removed,
        applied: change.applied,
        error: change.error.clone(),
        decided_by: change.decided_by,
    }
}

fn triage_view(triage: &Triage) -> TriageView {
    TriageView {
        route: triage.route,
        reason: triage.reason.clone(),
        source: triage.source,
        cause: triage.cause,
        override_kind: triage.override_kind,
        runner: triage.runner.clone(),
        model: triage.model.clone(),
        line: judge_line_text(triage),
    }
}

fn fallback_cause_label(cause: FallbackCause) -> &'static str {
    match cause {
        FallbackCause::Timeout => "timeout",
        FallbackCause::Error => "error",
        FallbackCause::InvalidVerdict => "invalid_verdict",
        FallbackCause::NoReadOnlyMode => "no_read_only_mode",
    }
}

fn approval_fallback_note(run: &GraphRun, revision: &PlanRevision) -> Option<String> {
    if revision.source != "fallback" {
        return None;
    }
    let cause = run
        .triage
        .as_ref()
        .and_then(|triage| triage.cause)
        .map(fallback_cause_label)
        .unwrap_or("error");
    let runner = run
        .triage
        .as_ref()
        .map(|triage| triage.runner.as_str())
        .unwrap_or("agent");
    Some(format!(
        "Fallback plan (heuristic, {cause}), not written by {runner}"
    ))
}

fn approval_step_view(revision: &PlanRevision, step: &PlanStep) -> ApprovalStepView {
    let model = step.model.clone().unwrap_or_default();
    ApprovalStepView {
        id: step.id.clone(),
        role: step.role.clone(),
        runner: step.runner.clone(),
        model,
        job: truncate_with_ellipsis(&step.job, SNAPSHOT_STEP_JOB_LIMIT),
        depends_on: step.depends_on.clone(),
        task: step.task.clone(),
        rev_change: rev_change_for_step_id(revision, &step.id),
    }
}

fn approval_view(run: &GraphRun) -> Option<ApprovalView> {
    let approval = run.approval.as_ref()?;
    let revision = approval.revisions.last()?;
    let notes = revision
        .errors
        .as_ref()
        .map(|errors| {
            errors
                .iter()
                .map(|note| note.trim())
                .filter(|note| !note.is_empty())
                .map(|note| truncate_with_ellipsis(note, SNAPSHOT_APPROVAL_NOTE_LIMIT))
                .collect()
        })
        .unwrap_or_default();
    let include_steps =
        run.phase == GraphRunPhase::AwaitingApproval || run.pending_replan_plan.is_some();
    let steps = if include_steps {
        revision
            .steps
            .iter()
            .map(|step| approval_step_view(revision, step))
            .collect()
    } else {
        Vec::new()
    };
    Some(ApprovalView {
        rev: revision.n,
        source: revision.source.clone(),
        fallback: approval_fallback_note(run, revision),
        notes,
        decided_by: approval.decided_by,
        steps,
    })
}

fn task_checklist_views(run: &GraphRun) -> Vec<TaskChecklistView> {
    if run.plan.tasks.is_empty() {
        return Vec::new();
    }
    task_status(&run.plan)
        .into_iter()
        .map(|(task, state)| {
            let nodes: Vec<String> = run
                .plan
                .nodes
                .iter()
                .filter(|node| node.task.as_deref() == Some(task.id.as_str()))
                .map(|node| node.id.clone())
                .collect();
            TaskChecklistView {
                id: task.id,
                title: task.title,
                done_check: task.done_check,
                size: task.size,
                depends_on: task.depends_on,
                status: task_state_label(state),
                nodes,
            }
        })
        .collect()
}

fn task_state_label(state: TaskState) -> String {
    match state {
        TaskState::Pending => "pending",
        TaskState::Running => "running",
        TaskState::Done => "done",
        TaskState::Failed => "failed",
    }
    .to_owned()
}

pub fn snapshot_from_run(run: &GraphRun) -> GraphView {
    build_graph_view(run, &[], None)
}

/// Like [`snapshot_from_run`] but fills FR-955 `turns` / `follow_up` from
/// `conversation_runs` (same `conversation_id` only; other conversations omitted).
pub fn snapshot_from_run_with_conversation(
    run: &GraphRun,
    conversation_runs: &[GraphRun],
    conversation_run_total: Option<usize>,
) -> GraphView {
    build_graph_view(run, conversation_runs, conversation_run_total)
}

fn build_graph_view(
    run: &GraphRun,
    conversation_runs: &[GraphRun],
    conversation_run_total: Option<usize>,
) -> GraphView {
    let applied_revision = latest_applied_plan_change(run);
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

        let rev_change =
            applied_revision.and_then(|change| rev_change_for_node_id(change, node.id.as_str()));

        nodes.push(NodeView {
            id: node.id.clone(),
            role: node.role,
            job: truncate_with_ellipsis(&node.job, SNAPSHOT_NODE_JOB_LIMIT),
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
            rev_change,
            task: node.task.clone(),
        });
    }

    let triage = run.triage.as_ref().map(triage_view);
    let approval = approval_view(run);
    let plan_history = run
        .plan_history
        .iter()
        .map(plan_change_view)
        .collect::<Vec<_>>();
    let tasks = task_checklist_views(run);

    let turn_fields = conversation_turn_fields(
        &run.conversation_id,
        conversation_runs,
        conversation_run_total,
    );

    GraphView {
        id: run.plan.id.clone(),
        phase: run.phase,
        orchestrator_model: run.plan.orchestrator_model.clone(),
        user_prompt: truncate_with_ellipsis(&run.plan.user_prompt, SNAPSHOT_USER_PROMPT_LIMIT),
        current_nodes,
        waiting_gate_node: run.waiting_gate_node.clone(),
        counts,
        layers,
        nodes,
        edges: run.plan.edges.clone(),
        triage,
        approval,
        plan_history,
        tasks,
        planner_calls: run.planner_calls,
        planner_io: run.planner_io.clone(),
        turns: turn_fields.bands,
        earlier_turns: turn_fields.earlier_turns,
        follow_up: turn_fields.follow_up,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ConversationTurnFields {
    bands: Vec<TurnBandView>,
    earlier_turns: usize,
    follow_up: Vec<FollowUpEdgeView>,
}

fn conversation_turn_fields(
    conversation_id: &str,
    conversation_runs: &[GraphRun],
    conversation_run_total: Option<usize>,
) -> ConversationTurnFields {
    let mut runs: Vec<&GraphRun> = conversation_runs
        .iter()
        .filter(|run| run.conversation_id == conversation_id)
        .collect();
    if runs.is_empty() {
        return ConversationTurnFields::default();
    }
    runs.sort_by(|left, right| {
        left.turn_index
            .cmp(&right.turn_index)
            .then_with(|| left.plan.id.cmp(&right.plan.id))
    });

    let total = conversation_run_total.unwrap_or(runs.len());
    let omitted = total.saturating_sub(CONVERSATION_TURN_BAND_LIMIT);
    let visible = runs[runs.len().saturating_sub(CONVERSATION_TURN_BAND_LIMIT)..].to_vec();

    let bands = visible
        .iter()
        .map(|run| TurnBandView {
            run_id: run.plan.id.clone(),
            turn_index: run.turn_index,
            phase: run.phase,
            user_prompt: truncate_with_ellipsis(&run.plan.user_prompt, SNAPSHOT_USER_PROMPT_LIMIT),
            nodes: executable_node_ids(&run.plan),
        })
        .collect();

    let mut follow_up = Vec::new();
    for window in visible.windows(2) {
        let from_run = window[0];
        let to_run = window[1];
        let from_nodes = last_finished_executable_node_ids(&from_run.plan);
        let to_nodes = root_executable_node_ids(&to_run.plan);
        for from in &from_nodes {
            for to in &to_nodes {
                follow_up.push(FollowUpEdgeView {
                    from: from.clone(),
                    to: to.clone(),
                    from_run: from_run.plan.id.clone(),
                    to_run: to_run.plan.id.clone(),
                    kind: "follow_up".to_owned(),
                });
            }
        }
    }

    ConversationTurnFields {
        bands,
        earlier_turns: omitted,
        follow_up,
    }
}

fn is_executable_node(role: NodeRole) -> bool {
    role != NodeRole::Orchestrate
}

fn is_terminal_node_status(status: GraphNodeStatus) -> bool {
    matches!(
        status,
        GraphNodeStatus::Completed | GraphNodeStatus::Skipped | GraphNodeStatus::Failed
    )
}

fn executable_node_ids(plan: &GraphPlan) -> Vec<String> {
    plan.nodes
        .iter()
        .filter(|node| is_executable_node(node.role))
        .map(|node| node.id.clone())
        .collect()
}

fn root_executable_node_ids(plan: &GraphPlan) -> Vec<String> {
    let executable: std::collections::HashSet<&str> = plan
        .nodes
        .iter()
        .filter(|node| is_executable_node(node.role))
        .map(|node| node.id.as_str())
        .collect();
    plan.nodes
        .iter()
        .filter(|node| executable.contains(node.id.as_str()))
        .filter(|node| {
            plan.edges
                .iter()
                .filter(|edge| edge.to == node.id)
                .all(|edge| !executable.contains(edge.from.as_str()))
        })
        .map(|node| node.id.clone())
        .collect()
}

fn last_finished_executable_node_ids(plan: &GraphPlan) -> Vec<String> {
    let executable: std::collections::HashSet<&str> = plan
        .nodes
        .iter()
        .filter(|node| is_executable_node(node.role))
        .map(|node| node.id.as_str())
        .collect();
    let finished: std::collections::HashSet<&str> = plan
        .nodes
        .iter()
        .filter(|node| {
            executable.contains(node.id.as_str()) && is_terminal_node_status(node.status)
        })
        .map(|node| node.id.as_str())
        .collect();
    plan.nodes
        .iter()
        .filter(|node| finished.contains(node.id.as_str()))
        .filter(|node| {
            !plan
                .edges
                .iter()
                .any(|edge| edge.from == node.id && finished.contains(edge.to.as_str()))
        })
        .map(|node| node.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{
        Approval, ApprovalState, DecisionBy, GraphEdge, GraphLimits, GraphNode, GraphPlan,
        PlanChange, PlanChangeKind, PlanChangeTrigger, PlanRevision, PlanStep, PlannerIo,
        PlannerIoKind, Route, TaskSize, Triage, TriageSource,
    };

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
                tasks: Vec::new(),
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
                        task: None,
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
                        task: None,
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
                        task: None,
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
            task: None,
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
                tasks: Vec::new(),
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

    // TC-957 snapshot part (UX-930): `graph.triage` serializes route metadata and
    // `line` matches `triage_view::judge_line_text`; FR-1008 planner_calls/io on the view.
    #[test]
    fn snapshot_carries_triage() {
        let triage = Triage {
            route: Route::Complex,
            reason: "multi-file refactor".to_owned(),
            source: TriageSource::Judge,
            cause: None,
            override_kind: None,
            runner: "cursor".to_owned(),
            model: "composer-2.5".to_owned(),
            elapsed_ms: 42,
        };
        let run = GraphRun {
            plan: GraphPlan {
                schema: "dmuxai.graph/v1".to_string(),
                id: "run-triage".to_string(),
                orchestrator_model: "m".to_string(),
                user_prompt: "go".to_string(),
                limits: GraphLimits::default(),
                tasks: Vec::new(),
                nodes: Vec::new(),
                edges: Vec::new(),
            },
            conversation_id: "conv".to_string(),
            phase: GraphRunPhase::Planning,
            triage: Some(triage.clone()),
            planner_calls: PlannerCalls {
                judge: 1,
                plan: 0,
                repair: 0,
                edit: 0,
                replan: 0,
            },
            planner_io: vec![PlannerIo {
                kind: PlannerIoKind::Judge,
                prompt_chars: 10,
                response_chars: 5,
                at_ms: 1,
                ok: true,
            }],
            ..Default::default()
        };

        let view = snapshot_from_run(&run);
        let triage_view = view.triage.as_ref().expect("triage");
        assert_eq!(triage_view.line, judge_line_text(&triage));
        assert_eq!(triage_view.runner, "cursor");

        let json = serde_json::to_string(&view).unwrap();
        assert!(json.contains("\"triage\""));
        assert!(json.contains("\"line\":"));
        assert!(json.contains("\"planner_calls\""));
        assert!(json.contains("\"planner_io\""));
    }

    // TC-977 snapshot part (FR-960): the initial approval revision is not a replan
    // badge source; only later applied edit/replan revisions mark nodes.
    #[test]
    fn initial_plan_has_no_rev_change_badges() {
        let make_node = |id: &str| GraphNode {
            id: id.to_string(),
            role: NodeRole::Implement,
            job: id.to_string(),
            runner: "cursor".to_string(),
            model_id: "m".to_string(),
            skill: None,
            requires_gate: false,
            status: GraphNodeStatus::Pending,
            attempt: 0,
            task: None,
            started_at_ms: None,
            finished_at_ms: None,
        };
        let initial = PlanChange {
            rev: 1,
            kind: PlanChangeKind::Initial,
            at_ms: 1,
            after_node: None,
            reason: "approved".to_owned(),
            trigger: None,
            prompt_chars: None,
            response_chars: None,
            added: vec!["node-a".to_owned(), "node-b".to_owned()],
            changed: vec![],
            removed: vec![],
            tasks_added: 0,
            tasks_changed: 0,
            tasks_removed: 0,
            applied: true,
            error: None,
            response_excerpt: None,
            decided_by: Some(DecisionBy::Tui),
        };
        let fresh = GraphRun {
            plan: GraphPlan {
                schema: "dmuxai.graph/v1".to_string(),
                id: "run-initial".to_string(),
                orchestrator_model: "m".to_string(),
                user_prompt: "p".to_string(),
                limits: GraphLimits::default(),
                tasks: Vec::new(),
                nodes: vec![make_node("node-a"), make_node("node-b")],
                edges: Vec::new(),
            },
            conversation_id: "conv".to_string(),
            phase: GraphRunPhase::Active,
            plan_history: vec![initial],
            ..Default::default()
        };
        let fresh_view = snapshot_from_run(&fresh);
        for node in &fresh_view.nodes {
            assert_eq!(node.rev_change, None, "node {}", node.id);
        }

        let replan = PlanChange {
            rev: 2,
            kind: PlanChangeKind::Replan,
            at_ms: 2,
            after_node: Some("node-a".to_owned()),
            reason: "failure".to_owned(),
            trigger: Some(PlanChangeTrigger::Failure),
            prompt_chars: None,
            response_chars: None,
            added: vec!["node-c".to_owned()],
            changed: vec!["node-a".to_owned()],
            removed: vec![],
            tasks_added: 0,
            tasks_changed: 0,
            tasks_removed: 0,
            applied: true,
            error: None,
            response_excerpt: None,
            decided_by: None,
        };
        let mut after_replan = fresh;
        after_replan.plan.nodes.push(make_node("node-c"));
        after_replan.plan_history.push(replan);
        let replan_view = snapshot_from_run(&after_replan);
        let node_a = replan_view.nodes.iter().find(|n| n.id == "node-a").unwrap();
        let node_b = replan_view.nodes.iter().find(|n| n.id == "node-b").unwrap();
        let node_c = replan_view.nodes.iter().find(|n| n.id == "node-c").unwrap();
        assert_eq!(node_a.rev_change, Some(RevChangeMark::Changed));
        assert_eq!(node_b.rev_change, None);
        assert_eq!(node_c.rev_change, Some(RevChangeMark::Added));
    }

    // TC-977 snapshot part (FR-960, FR-962): applied replan sets node `rev_change`;
    // rejected replan stays in `plan_history` without `response_excerpt`; legacy
    // NodeView JSON without `rev_change`/`task` still deserializes.
    #[test]
    fn snapshot_marks_revision_changes() {
        let make_node = |id: &str| GraphNode {
            id: id.to_string(),
            role: NodeRole::Implement,
            job: id.to_string(),
            runner: "cursor".to_string(),
            model_id: "m".to_string(),
            skill: None,
            requires_gate: false,
            status: GraphNodeStatus::Completed,
            attempt: 1,
            task: None,
            started_at_ms: None,
            finished_at_ms: None,
        };
        let applied = PlanChange {
            rev: 2,
            kind: PlanChangeKind::Replan,
            at_ms: 1,
            after_node: Some("node-a".to_owned()),
            reason: "failure".to_owned(),
            trigger: Some(PlanChangeTrigger::Failure),
            prompt_chars: None,
            response_chars: None,
            added: vec!["node-b".to_owned()],
            changed: vec!["node-a".to_owned()],
            removed: vec!["node-c".to_owned()],
            tasks_added: 0,
            tasks_changed: 0,
            tasks_removed: 0,
            applied: true,
            error: None,
            response_excerpt: None,
            decided_by: None,
        };
        let rejected = PlanChange {
            rev: 3,
            kind: PlanChangeKind::Replan,
            at_ms: 2,
            after_node: None,
            reason: "bad replan".to_owned(),
            trigger: None,
            prompt_chars: None,
            response_chars: None,
            added: vec!["node-x".to_owned()],
            changed: vec![],
            removed: vec![],
            tasks_added: 0,
            tasks_changed: 0,
            tasks_removed: 0,
            applied: false,
            error: Some("invalid".to_owned()),
            response_excerpt: Some("secret excerpt must not leak".to_owned()),
            decided_by: Some(DecisionBy::Tui),
        };
        let run = GraphRun {
            plan: GraphPlan {
                schema: "dmuxai.graph/v1".to_string(),
                id: "run-rev".to_string(),
                orchestrator_model: "m".to_string(),
                user_prompt: "p".to_string(),
                limits: GraphLimits::default(),
                tasks: Vec::new(),
                nodes: vec![make_node("node-a"), make_node("node-b")],
                edges: Vec::new(),
            },
            conversation_id: "conv".to_string(),
            phase: GraphRunPhase::Active,
            plan_history: vec![applied, rejected],
            ..Default::default()
        };

        let view = snapshot_from_run(&run);
        assert_eq!(view.plan_history.len(), 2);
        assert!(!view.plan_history[1].applied);

        let node_a = view.nodes.iter().find(|n| n.id == "node-a").unwrap();
        let node_b = view.nodes.iter().find(|n| n.id == "node-b").unwrap();
        assert_eq!(node_a.rev_change, Some(RevChangeMark::Changed));
        assert_eq!(node_b.rev_change, Some(RevChangeMark::Added));

        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("response_excerpt"));
        assert!(!json.contains("secret excerpt"));

        let legacy_node = r#"{
            "id": "n1",
            "role": "implement",
            "job": "j",
            "runner": "cursor",
            "model_id": "m",
            "status": "pending",
            "attempt": 0,
            "deps": [],
            "detail": {"log_tail": [], "changed_files": []}
        }"#;
        let parsed: NodeView = serde_json::from_str(legacy_node).unwrap();
        assert_eq!(parsed.rev_change, None);
        assert_eq!(parsed.task, None);
    }

    // TC-996 snapshot part (UX-936, FR-966): `graph.tasks` lists three tasks with
    // derived status (done, running, failed) and node ids; steps expose `task`.
    #[test]
    fn snapshot_carries_task_checklist() {
        use crate::graph::PlanTask;

        let tasks = vec![
            PlanTask {
                id: "task-1".to_string(),
                title: "First".to_string(),
                done_check: "tests pass".to_string(),
                areas: Vec::new(),
                size: TaskSize::Small,
                depends_on: Vec::new(),
                implicit: false,
            },
            PlanTask {
                id: "task-2".to_string(),
                title: "Second".to_string(),
                done_check: "lint clean".to_string(),
                areas: Vec::new(),
                size: TaskSize::Medium,
                depends_on: vec!["task-1".to_string()],
                implicit: false,
            },
            PlanTask {
                id: "task-3".to_string(),
                title: "Third".to_string(),
                done_check: "shipped".to_string(),
                areas: Vec::new(),
                size: TaskSize::Small,
                depends_on: Vec::new(),
                implicit: false,
            },
        ];
        let run = GraphRun {
            plan: GraphPlan {
                schema: "dmuxai.graph/v1".to_string(),
                id: "run-tasks".to_string(),
                orchestrator_model: "m".to_string(),
                user_prompt: "p".to_string(),
                limits: GraphLimits::default(),
                tasks,
                nodes: vec![
                    GraphNode {
                        id: "s1".to_string(),
                        role: NodeRole::Implement,
                        job: "j1".to_string(),
                        runner: "cursor".to_string(),
                        model_id: "m".to_string(),
                        skill: None,
                        requires_gate: false,
                        status: GraphNodeStatus::Completed,
                        attempt: 1,
                        task: Some("task-1".to_string()),
                        started_at_ms: None,
                        finished_at_ms: None,
                    },
                    GraphNode {
                        id: "s2".to_string(),
                        role: NodeRole::Implement,
                        job: "j2".to_string(),
                        runner: "cursor".to_string(),
                        model_id: "m".to_string(),
                        skill: None,
                        requires_gate: false,
                        status: GraphNodeStatus::Running,
                        attempt: 1,
                        task: Some("task-2".to_string()),
                        started_at_ms: None,
                        finished_at_ms: None,
                    },
                    GraphNode {
                        id: "s3".to_string(),
                        role: NodeRole::Implement,
                        job: "j3".to_string(),
                        runner: "cursor".to_string(),
                        model_id: "m".to_string(),
                        skill: None,
                        requires_gate: false,
                        status: GraphNodeStatus::Failed,
                        attempt: 1,
                        task: Some("task-3".to_string()),
                        started_at_ms: None,
                        finished_at_ms: None,
                    },
                ],
                edges: Vec::new(),
            },
            conversation_id: "conv".to_string(),
            phase: GraphRunPhase::Active,
            ..Default::default()
        };

        let view = snapshot_from_run(&run);
        assert_eq!(view.tasks.len(), 3);
        assert_eq!(view.tasks[0].id, "task-1");
        assert_eq!(view.tasks[0].title, "First");
        assert_eq!(view.tasks[0].done_check, "tests pass");
        assert_eq!(view.tasks[0].status, "done");
        assert_eq!(view.tasks[0].nodes, vec!["s1"]);
        assert_eq!(view.tasks[1].status, "running");
        assert_eq!(view.tasks[1].nodes, vec!["s2"]);
        assert_eq!(view.tasks[2].status, "failed");
        assert_eq!(view.tasks[2].nodes, vec!["s3"]);

        let s2 = view.nodes.iter().find(|n| n.id == "s2").unwrap();
        assert_eq!(s2.task.as_deref(), Some("task-2"));

        let json = serde_json::to_string(&view).unwrap();
        assert!(json.contains("\"tasks\""));
    }

    // TC-990 (FR-955): conv-A runs form ordered turn bands with `follow_up` edges;
    // conv-B absent; 22 runs → 20 bands plus `earlier_turns: 2`.
    #[test]
    fn snapshot_conversation_turns_append() {
        fn turn_run(
            conversation_id: &str,
            turn_index: u32,
            run_id: &str,
            node_id: &str,
            status: GraphNodeStatus,
        ) -> GraphRun {
            GraphRun {
                plan: GraphPlan {
                    schema: "dmuxai.graph/v1".to_string(),
                    id: run_id.to_string(),
                    orchestrator_model: "m".to_string(),
                    user_prompt: format!("prompt turn {turn_index}"),
                    limits: GraphLimits::default(),
                    tasks: Vec::new(),
                    nodes: vec![GraphNode {
                        id: node_id.to_string(),
                        role: NodeRole::Implement,
                        job: "job".to_string(),
                        runner: "cursor".to_string(),
                        model_id: "m".to_string(),
                        skill: None,
                        requires_gate: false,
                        status,
                        attempt: 1,
                        task: None,
                        started_at_ms: None,
                        finished_at_ms: None,
                    }],
                    edges: Vec::new(),
                },
                conversation_id: conversation_id.to_string(),
                turn_index,
                phase: GraphRunPhase::Completed,
                waiting_gate_node: None,
                ..Default::default()
            }
        }

        let runs_a = [
            turn_run("conv-a", 1, "run-a-1", "a1", GraphNodeStatus::Completed),
            turn_run("conv-a", 2, "run-a-2", "a2", GraphNodeStatus::Completed),
            turn_run("conv-a", 3, "run-a-3", "a3", GraphNodeStatus::Completed),
        ];
        let runs_b = [turn_run(
            "conv-b",
            1,
            "run-b-1",
            "b1",
            GraphNodeStatus::Completed,
        )];
        let all_runs: Vec<GraphRun> = runs_a.iter().chain(runs_b.iter()).cloned().collect();

        let view = snapshot_from_run_with_conversation(&runs_a[2], &all_runs, None);
        assert_eq!(view.turns.len(), 3);
        assert!(
            !view.turns.iter().any(|band| band.run_id == "run-b-1"),
            "other conversations must not appear in graph.turns"
        );
        assert_eq!(view.turns[0].run_id, "run-a-1");
        assert_eq!(view.turns[2].run_id, "run-a-3");
        assert_eq!(view.earlier_turns, 0);
        assert_eq!(
            view.follow_up,
            vec![
                FollowUpEdgeView {
                    from: "a1".to_string(),
                    to: "a2".to_string(),
                    from_run: "run-a-1".to_string(),
                    to_run: "run-a-2".to_string(),
                    kind: "follow_up".to_string(),
                },
                FollowUpEdgeView {
                    from: "a2".to_string(),
                    to: "a3".to_string(),
                    from_run: "run-a-2".to_string(),
                    to_run: "run-a-3".to_string(),
                    kind: "follow_up".to_string(),
                },
            ]
        );

        let dup_runs = [
            turn_run(
                "conv-dup",
                1,
                "run-dup-1",
                "step-1",
                GraphNodeStatus::Completed,
            ),
            turn_run(
                "conv-dup",
                2,
                "run-dup-2",
                "step-1",
                GraphNodeStatus::Completed,
            ),
        ];
        let dup_view = snapshot_from_run_with_conversation(&dup_runs[1], &dup_runs, None);
        assert_eq!(dup_view.follow_up.len(), 1);
        assert_eq!(dup_view.follow_up[0].from, "step-1");
        assert_eq!(dup_view.follow_up[0].to, "step-1");
        assert_eq!(dup_view.follow_up[0].from_run, "run-dup-1");
        assert_eq!(dup_view.follow_up[0].to_run, "run-dup-2");

        let long_korean = "가".repeat(250);
        let mut korean_run = turn_run("conv-ko", 1, "run-ko", "k1", GraphNodeStatus::Completed);
        korean_run.plan.user_prompt = long_korean.clone();
        let ko_view = snapshot_from_run_with_conversation(
            &korean_run,
            std::slice::from_ref(&korean_run),
            None,
        );
        let band_prompt = &ko_view.turns[0].user_prompt;
        assert_eq!(band_prompt.chars().count(), SNAPSHOT_USER_PROMPT_LIMIT + 1);
        assert!(band_prompt.ends_with('…'));
        assert!(std::str::from_utf8(band_prompt.as_bytes()).is_ok());
        assert!(
            view.turns
                .iter()
                .all(|band| band.user_prompt.chars().count() <= 201)
        );

        let many: Vec<GraphRun> = (1..=22u32)
            .map(|turn| {
                let run_id = format!("run-{turn}");
                let node_id = format!("n{turn}");
                turn_run(
                    "conv-many",
                    turn,
                    &run_id,
                    &node_id,
                    GraphNodeStatus::Completed,
                )
            })
            .collect();
        let tail = many.last().unwrap();
        let packed = snapshot_from_run_with_conversation(tail, &many, None);
        assert_eq!(packed.turns.len(), 20);
        assert_eq!(packed.earlier_turns, 2);
        assert_eq!(packed.turns[0].turn_index, 3);
        assert_eq!(packed.turns[19].turn_index, 22);

        let partial: Vec<GraphRun> = many[many.len().saturating_sub(21)..].to_vec();
        let packed_partial = snapshot_from_run_with_conversation(tail, &partial, Some(many.len()));
        assert_eq!(packed_partial.turns.len(), 20);
        assert_eq!(packed_partial.earlier_turns, 2);
        assert_eq!(packed_partial.turns[0].turn_index, 3);
    }

    #[test]
    fn snapshot_carries_approval_when_awaiting() {
        let run = GraphRun {
            plan: GraphPlan {
                schema: "dmuxai.graph/v1".to_string(),
                id: "run-approval".to_string(),
                orchestrator_model: "m".to_string(),
                user_prompt: "p".to_string(),
                limits: GraphLimits::default(),
                tasks: Vec::new(),
                nodes: Vec::new(),
                edges: Vec::new(),
            },
            conversation_id: "conv".to_string(),
            phase: GraphRunPhase::AwaitingApproval,
            approval: Some(Approval {
                state: ApprovalState::Pending,
                revisions: vec![PlanRevision {
                    n: 1,
                    source: "planner".to_string(),
                    steps: vec![PlanStep {
                        id: "step-1".to_string(),
                        role: "implement".to_string(),
                        job: "do it".to_string(),
                        runner: "cursor".to_string(),
                        model: Some("m".to_string()),
                        depends_on: Vec::new(),
                        sensitive: None,
                        task: None,
                        sensitive_invalid: false,
                        depends_on_invalid: false,
                        model_invalid: false,
                        id_invalid: false,
                        not_object: false,
                    }],
                    added: vec!["step-1".to_string()],
                    changed: Vec::new(),
                    feedback: None,
                    errors: Some(vec![" note ".to_string()]),
                    raw: None,
                }],
                decided_by: None,
                decided_at_ms: None,
            }),
            ..Default::default()
        };

        let view = snapshot_from_run(&run);
        let approval = view.approval.as_ref().expect("approval");
        assert_eq!(approval.rev, 1);
        assert_eq!(approval.steps.len(), 1);
        assert_eq!(approval.steps[0].rev_change, Some(RevChangeMark::Added));
        assert_eq!(approval.notes, vec!["note"]);
    }
}
