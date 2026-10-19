use std::fmt;

use serde::{Deserialize, Serialize};

use super::judge::{FallbackCause, OverrideKind, Route, TriageSource};
use super::plan_doc::PlanStep;

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
    /// FR-964: the task this step belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_ms: Option<u64>,
}

/// FR-963: one planned task (one runner turn or less).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanTask {
    pub id: String,
    pub title: String,
    pub done_check: String,
    #[serde(default)]
    pub areas: Vec<String>,
    pub size: TaskSize,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub implicit: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskSize {
    Small,
    Medium,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero_usize(value: &usize) -> bool {
    *value == 0
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
    /// FR-958: 0 disables replanning.
    #[serde(default = "default_max_replans")]
    pub max_replans: u32,
    /// FR-958: per-run budget for summed replan prompt lengths.
    #[serde(default = "default_replan_budget_chars")]
    pub replan_budget_chars: usize,
    /// FR-958: executable steps per run, including replan additions.
    #[serde(default = "default_max_nodes")]
    pub max_nodes: usize,
}

fn default_max_replans() -> u32 {
    3
}

fn default_replan_budget_chars() -> usize {
    24_000
}

fn default_max_nodes() -> usize {
    12
}

impl Default for GraphLimits {
    fn default() -> Self {
        Self {
            max_parallel: 2,
            max_retries: 2,
            max_replans: 3,
            replan_budget_chars: 24_000,
            max_nodes: 12,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphPlan {
    pub schema: String,
    pub id: String,
    pub orchestrator_model: String,
    pub user_prompt: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub limits: GraphLimits,
    /// FR-963: task list for this plan; empty for old records.
    #[serde(default)]
    pub tasks: Vec<PlanTask>,
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphRun {
    pub plan: GraphPlan,
    pub conversation_id: String,
    pub phase: GraphRunPhase,
    pub waiting_gate_node: Option<String>,
    /// FR-946: triage decision made for this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage: Option<Triage>,
    /// FR-946: approval state and revisions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<Approval>,
    /// FR-954: 1-based turn number within the conversation (0 for old records).
    #[serde(default)]
    pub turn_index: u32,
    /// FR-960: plan change history; at most 50 entries kept (oldest dropped).
    #[serde(default)]
    pub plan_history: Vec<PlanChange>,
    /// FR-958: planner call counters.
    #[serde(default)]
    pub planner_calls: PlannerCalls,
    /// FR-1008: one entry per counted internal planner adapter call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub planner_io: Vec<PlannerIo>,
    /// FR-958: applied or rejected replan turns sent for this run.
    /// Incremented by the App when a replan turn is sent (P11).
    #[serde(default)]
    pub replans_used: u32,
    /// FR-958: summed replan prompt lengths sent for this run.
    /// Incremented by the App when a replan turn is sent (P11).
    #[serde(default)]
    pub replan_chars_used: usize,
    /// FR-951: judge start (ms since epoch) for the orchestrate node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrate_started_ms: Option<u64>,
    /// FR-951: when the plan (or 1-node plan) was fixed at the gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_fixed_ms: Option<u64>,
    /// FR-959: proposed plan while a sensitive replan waits for approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_replan_plan: Option<GraphPlan>,
}

/// FR-943: at most five plan revisions per run.
pub const MAX_PLAN_REVISIONS: u32 = 5;

impl GraphRun {
    /// Keeps at most [`PLAN_HISTORY_MAX`] entries, dropping the oldest.
    pub fn push_plan_change(&mut self, change: PlanChange) {
        self.plan_history.push(change);
        let excess = self.plan_history.len().saturating_sub(PLAN_HISTORY_MAX);
        self.plan_history.drain(0..excess);
    }
}

/// Cap on `GraphRun.plan_history` length (FR-960).
pub const PLAN_HISTORY_MAX: usize = 50;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphRunPhase {
    #[default]
    Active,
    WaitingGate,
    Completed,
    Failed,
    Triage,
    Planning,
    AwaitingApproval,
    Rejected,
    Cancelled,
}

/// Who made a plan decision (FR-946).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionBy {
    Tui,
    Dashboard,
    Cli,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    NotRequired,
    Pending,
    Approved,
    Rejected,
}

/// One persisted plan proposal (FR-946). `raw` only holds the model text of a
/// failed parse, capped at 4 KiB.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRevision {
    pub n: u32,
    pub source: String,
    pub steps: Vec<PlanStep>,
    /// FR-959: step ids added in a replan revision awaiting approval.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<String>,
    /// FR-959: step ids changed in a replan revision awaiting approval.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

/// Cap for [`PlanRevision::raw`], cut on a char boundary.
const PLAN_REVISION_RAW_MAX_BYTES: usize = 4096;

/// Returns the parsed-model raw text clipped to 4 KiB without splitting a
/// UTF-8 code point.
pub fn cap_revision_raw(raw: &str) -> String {
    if raw.len() <= PLAN_REVISION_RAW_MAX_BYTES {
        return raw.to_owned();
    }
    let mut bytes = 0;
    for (index, ch) in raw.char_indices() {
        let ch_len = ch.len_utf8();
        if bytes + ch_len > PLAN_REVISION_RAW_MAX_BYTES {
            return raw[..index].to_owned();
        }
        bytes += ch_len;
    }
    raw.to_owned()
}

/// What changed in one plan revision (FR-960).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanChangeKind {
    Initial,
    Edit,
    Replan,
}

/// Why the replan was triggered (FR-958).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanChangeTrigger {
    Failure,
    Marker,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanChange {
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
    /// FR-968: new tasks in a replan revision (count only).
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub tasks_added: usize,
    /// FR-968: pending tasks revised in a replan revision (count only).
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub tasks_changed: usize,
    /// FR-968: pending tasks dropped in a replan revision (count only).
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub tasks_removed: usize,
    pub applied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// FR-1007: rejected/invalid replan adapter text (≤2000 chars).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_excerpt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<DecisionBy>,
}

/// Next revision number shared by `plan_history` and approval revisions (FR-1007).
pub fn next_plan_change_rev(run: &GraphRun) -> u32 {
    let from_history = run
        .plan_history
        .last()
        .map(|change| change.rev.saturating_add(1))
        .unwrap_or(1);
    let from_approval = run
        .approval
        .as_ref()
        .and_then(|approval| approval.revisions.last().map(|rev| rev.n.saturating_add(1)))
        .unwrap_or(1);
    from_history.max(from_approval)
}

/// Caps replan rejection excerpts on a char boundary (FR-1007).
pub fn cap_plan_response_excerpt(raw: &str) -> String {
    const MAX: usize = 2000;
    if raw.chars().count() <= MAX {
        return raw.to_owned();
    }
    raw.char_indices()
        .nth(MAX)
        .map(|(index, _)| raw[..index].to_owned())
        .unwrap_or_else(|| raw.to_owned())
}

/// Triage verdict persisted on the run (FR-946).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Triage {
    pub route: Route,
    pub reason: String,
    pub source: TriageSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<FallbackCause>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_kind: Option<OverrideKind>,
    pub runner: String,
    pub model: String,
    pub elapsed_ms: u64,
}

/// Approval panel state persisted on the run (FR-946).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub state: ApprovalState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revisions: Vec<PlanRevision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<DecisionBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at_ms: Option<u64>,
}

/// Kind of internal planner adapter call (FR-1008).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlannerIoKind {
    Judge,
    Plan,
    Repair,
    Edit,
    Replan,
}

/// Size record for one internal planner adapter call (FR-1008).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerIo {
    pub kind: PlannerIoKind,
    pub prompt_chars: usize,
    pub response_chars: usize,
    pub at_ms: u64,
    pub ok: bool,
}

/// Planner call counters (FR-958).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerCalls {
    #[serde(default)]
    pub judge: u32,
    #[serde(default)]
    pub plan: u32,
    #[serde(default)]
    pub repair: u32,
    #[serde(default)]
    pub edit: u32,
    #[serde(default)]
    pub replan: u32,
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

    #[test]
    fn graph_plan_and_run_without_tasks_field_load() {
        let plan_json = r#"{
            "schema": "cursor-agent-graph/v1",
            "id": "p1",
            "orchestrator_model": "composer-2.5",
            "user_prompt": "go",
            "nodes": [{
                "id": "a",
                "role": "implement",
                "job": "work",
                "runner": "cursor",
                "model_id": "composer-2.5",
                "requires_gate": false,
                "status": "pending",
                "attempt": 0
            }],
            "edges": [],
            "limits": {
                "max_parallel": 2,
                "max_retries": 2,
                "max_replans": 3,
                "replan_budget_chars": 24000,
                "max_nodes": 12
            }
        }"#;
        let plan: GraphPlan = serde_json::from_str(plan_json).expect("plan");
        assert!(plan.tasks.is_empty());
        assert!(plan.nodes[0].task.is_none());

        let run_json = format!(r#"{{"plan":{plan_json},"conversation_id":"c1","phase":"active"}}"#);
        let run: GraphRun = serde_json::from_str(&run_json).expect("run");
        assert!(run.plan.tasks.is_empty());
        assert!(run.planner_io.is_empty());
    }
}
