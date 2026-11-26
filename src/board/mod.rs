//! User-level coordinator decision board (SPEC-decision-board FR-3100..FR-3115).

mod mcp;
mod plan;
mod rules;
mod store;

pub use mcp::invoke as mcp_invoke;

pub use plan::{PendingPlanFile, edit_plan_from_json, pending_plan_path, read_pending_plan};
pub use rules::{BoardRules, RulesStore, compile_rules_from_set_text, default_rules};
pub use store::{
    AddItemRequest, BoardStore, DecideRequest, ListFilter, SayRequest, UpdateItemRequest,
    WaitFilter, WaitFor, WaitOutcome,
};

use crate::agent_api::exit;
use serde::{Deserialize, Serialize};

pub const ITEM_ID_LEN: usize = 12;
pub const TITLE_MAX: usize = 200;
pub const TEXT_MAX: usize = 8000;
pub const OPTIONS_MAX: usize = 8;
pub const OPTION_MAX: usize = 200;
pub const THREAD_MAX: usize = 500;
pub const PLAN_HISTORY_MAX: usize = 20;
pub const CLOSED_HIDE_DAYS: u64 = 30;

/// `b` + 12 lowercase hex (`^b[0-9a-f]{12}$`).
pub fn valid_item_id(id: &str) -> bool {
    id.len() == 1 + ITEM_ID_LEN
        && id.as_bytes()[0] == b'b'
        && id.as_bytes()[1..]
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
}

pub fn generate_item_id() -> String {
    let raw = uuid::Uuid::new_v4().simple().to_string();
    format!("b{}", &raw[..ITEM_ID_LEN])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    Auto,
    NeedsDecision,
    Approved,
    InProgress,
    Done,
    Rejected,
    Unknown,
}

impl Lane {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Lane::Auto),
            "needs_decision" => Some(Lane::NeedsDecision),
            "approved" => Some(Lane::Approved),
            "in_progress" => Some(Lane::InProgress),
            "done" => Some(Lane::Done),
            "rejected" => Some(Lane::Rejected),
            "unknown" => Some(Lane::Unknown),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Lane::Auto => "auto",
            Lane::NeedsDecision => "needs_decision",
            Lane::Approved => "approved",
            Lane::InProgress => "in_progress",
            Lane::Done => "done",
            Lane::Rejected => "rejected",
            Lane::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Debug,
    Feature,
    Question,
    Review,
    Other,
}

impl Kind {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "debug" => Some(Kind::Debug),
            "feature" => Some(Kind::Feature),
            "question" => Some(Kind::Question),
            "review" => Some(Kind::Review),
            "other" => Some(Kind::Other),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Debug => "debug",
            Kind::Feature => "feature",
            Kind::Question => "question",
            Kind::Review => "review",
            Kind::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Grade {
    #[serde(rename = "A")]
    A,
    #[serde(rename = "B")]
    B,
    #[serde(rename = "C")]
    C,
    #[serde(rename = "D")]
    D,
}

impl Grade {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_uppercase().as_str() {
            "A" => Some(Grade::A),
            "B" => Some(Grade::B),
            "C" => Some(Grade::C),
            "D" => Some(Grade::D),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Source {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkspaceRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardPlan {
    pub hash: String,
    pub prompt: String,
    pub tasks: Vec<crate::graph::PlanTask>,
    pub steps: Vec<crate::graph::PlanStep>,
    pub revision: u32,
    #[serde(default)]
    pub history: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Approve,
    Reject,
    RequestChanges,
    Answer,
}

impl Verdict {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "approve" => Some(Verdict::Approve),
            "reject" => Some(Verdict::Reject),
            "request_changes" => Some(Verdict::RequestChanges),
            "answer" => Some(Verdict::Answer),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Approve => "approve",
            Verdict::Reject => "reject",
            Verdict::RequestChanges => "request_changes",
            Verdict::Answer => "answer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub verdict: Verdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_hash: Option<String>,
    pub by: String,
    pub at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadMessage {
    pub from: String,
    pub text: String,
    pub at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardItem {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grade: Option<Grade>,
    pub lane: Lane,
    pub source: Source,
    pub workspace: WorkspaceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommendation: Option<String>,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<BoardPlan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
    #[serde(default)]
    pub thread: Vec<ThreadMessage>,
    #[serde(default)]
    pub run_ids: Vec<String>,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardError {
    Usage(String),
    NotFound(String),
    Conflict(String),
    Forbidden(String),
    Failed(String),
    Internal(String),
}

impl BoardError {
    pub fn exit_code(&self) -> i32 {
        match self {
            BoardError::Usage(_) => exit::USAGE,
            BoardError::NotFound(_) => exit::NOT_FOUND,
            BoardError::Conflict(_) => exit::CONFLICT,
            BoardError::Forbidden(_) => exit::FORBIDDEN,
            BoardError::Failed(_) => exit::FAILED,
            BoardError::Internal(_) => exit::INTERNAL,
        }
    }
}

impl std::fmt::Display for BoardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoardError::Usage(m)
            | BoardError::NotFound(m)
            | BoardError::Conflict(m)
            | BoardError::Forbidden(m)
            | BoardError::Failed(m)
            | BoardError::Internal(m) => f.write_str(m),
        }
    }
}

impl From<BoardError> for crate::agent_api::ApiError {
    fn from(error: BoardError) -> Self {
        crate::agent_api::ApiError::new(error.exit_code(), error.to_string())
    }
}

pub fn default_actor_by(is_tty: bool, is_mcp: bool) -> String {
    if is_tty {
        "owner".to_owned()
    } else if is_mcp {
        "bot:mcp".to_owned()
    } else {
        "bot:coordinator".to_owned()
    }
}

pub fn resolve_actor(by: Option<&str>, is_tty: bool, is_mcp: bool) -> Result<String, BoardError> {
    let actor = by
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| default_actor_by(is_tty, is_mcp));
    if is_mcp && actor == "owner" {
        return Err(BoardError::Forbidden(
            "MCP callers cannot act as owner".to_owned(),
        ));
    }
    if actor != "owner" && actor != "planner" && !actor.starts_with("bot:") {
        return Err(BoardError::Usage(format!(
            "invalid --by '{actor}' (expected owner, planner, or bot:NAME)"
        )));
    }
    Ok(actor)
}

pub fn mask_board_plan(plan: &mut BoardPlan) {
    plan.prompt = crate::run_journal::mask_sensitive_tokens(&plan.prompt);
    for step in &mut plan.steps {
        step.job = crate::run_journal::mask_sensitive_tokens(&step.job);
    }
    for task in &mut plan.tasks {
        task.title = crate::run_journal::mask_sensitive_tokens(&task.title);
        task.done_check = crate::run_journal::mask_sensitive_tokens(&task.done_check);
    }
}

pub fn mask_item_fields(item: &mut BoardItem) {
    item.title = crate::run_journal::mask_sensitive_tokens(&item.title);
    item.summary = crate::run_journal::mask_sensitive_tokens(&item.summary);
    if let Some(rec) = item.recommendation.as_mut() {
        *rec = crate::run_journal::mask_sensitive_tokens(rec);
    }
    item.options = item
        .options
        .iter()
        .map(|o| crate::run_journal::mask_sensitive_tokens(o))
        .collect();
    if let Some(plan) = item.plan.as_mut() {
        mask_board_plan(plan);
    }
    for msg in &mut item.thread {
        msg.text = crate::run_journal::mask_sensitive_tokens(&msg.text);
    }
    if let Some(decision) = item.decision.as_mut()
        && let Some(text) = decision.text.as_mut()
    {
        *text = crate::run_journal::mask_sensitive_tokens(text);
    }
}

pub fn validate_new_text(
    title: &str,
    summary: &str,
    recommendation: Option<&str>,
) -> Result<(), BoardError> {
    if title.trim().is_empty() {
        return Err(BoardError::Usage("title is required".to_owned()));
    }
    if title.chars().count() > TITLE_MAX {
        return Err(BoardError::Usage(format!(
            "title exceeds {TITLE_MAX} characters"
        )));
    }
    if summary.chars().count() > TEXT_MAX {
        return Err(BoardError::Usage(format!(
            "summary exceeds {TEXT_MAX} characters"
        )));
    }
    if let Some(rec) = recommendation
        && rec.chars().count() > TEXT_MAX
    {
        return Err(BoardError::Usage(format!(
            "recommendation exceeds {TEXT_MAX} characters"
        )));
    }
    Ok(())
}

pub fn validate_options(options: &[String]) -> Result<(), BoardError> {
    if options.len() > OPTIONS_MAX {
        return Err(BoardError::Usage(format!(
            "at most {OPTIONS_MAX} options allowed"
        )));
    }
    for opt in options {
        if opt.chars().count() > OPTION_MAX {
            return Err(BoardError::Usage(format!(
                "each option must be at most {OPTION_MAX} characters"
            )));
        }
    }
    Ok(())
}

pub fn validate_message(text: &str) -> Result<(), BoardError> {
    if text.trim().is_empty() {
        return Err(BoardError::Usage("message text is required".to_owned()));
    }
    if text.chars().count() > TEXT_MAX {
        return Err(BoardError::Usage(format!(
            "message exceeds {TEXT_MAX} characters"
        )));
    }
    Ok(())
}

pub fn validate_ref_url(url: Option<&str>) -> Result<(), BoardError> {
    let Some(url) = url else {
        return Ok(());
    };
    let lower = url.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return Err(BoardError::Usage("ref must be an http(s) URL".to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::useless_vec)]
    #[test]
    fn tc_3102_invalid_ids_and_limits_rejected_before_fs_access() {
        assert!(!valid_item_id("b"));
        assert!(!valid_item_id("bZZZZZZZZZZZZZ"));
        assert!(!valid_item_id("../items/x"));
        assert!(valid_item_id(&generate_item_id()));
        assert!(validate_new_text("t", "", None).is_ok());
        assert!(validate_new_text("", "", None).is_err());
        assert!(validate_new_text(&"x".repeat(TITLE_MAX + 1), "", None).is_err());
        assert!(validate_options(&["o".repeat(OPTION_MAX + 1)]).is_err());
        assert!(
            validate_options(&vec!["ok".to_owned(); OPTIONS_MAX + 1]).is_err(),
            "too many options"
        );
        assert!(validate_message(&"m".repeat(TEXT_MAX + 1)).is_err());
        assert!(resolve_actor(Some("owner"), false, true).is_err());
        assert!(resolve_actor(None, true, false).unwrap() == "owner");
        assert!(resolve_actor(None, false, true).unwrap() == "bot:mcp");
    }

    #[test]
    fn tc_3103_free_text_is_masked_on_write() {
        let mut item = BoardItem {
            id: generate_item_id(),
            title: "token sk-abcdefghij token".to_owned(),
            summary: String::new(),
            kind: Kind::Debug,
            grade: None,
            lane: Lane::Auto,
            source: Source::default(),
            workspace: WorkspaceRef::default(),
            recommendation: None,
            options: vec![],
            plan: None,
            decision: None,
            thread: vec![],
            run_ids: vec![],
            created_ms: 0,
            updated_ms: 0,
            version: 1,
        };
        mask_item_fields(&mut item);
        assert!(!item.title.contains("sk-abcdefghij"));
    }

    #[test]
    fn tc_3115_board_never_touches_credentials_or_token() {
        let src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/board/store.rs"));
        for forbidden in [
            ".cursor",
            "credentials.json",
            "dashboard_token",
            "DMUXAI_DASHBOARD_TOKEN",
        ] {
            assert!(
                !src.contains(forbidden),
                "board store must not reference {forbidden}"
            );
        }
    }
}
