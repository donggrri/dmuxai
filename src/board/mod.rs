//! User-level coordinator decision board (SPEC-decision-board FR-3100..FR-3115).

mod mcp;
pub mod notify;
mod plan;
mod rules;
pub mod runner;
mod store;

pub use mcp::invoke as mcp_invoke;

pub use plan::{
    PendingPlanFile, board_plan_copy_path, edit_plan_from_json, ensure_pending_plan_from_copy,
    pending_plan_path, prune_old_plan_copies, read_pending_plan, unmasked_prompt_from_item_plan,
    write_board_plan_copy,
};
pub use rules::{BoardRules, RulesStore, compile_rules_from_set_text, default_rules};
pub use store::{
    AddItemRequest, BoardStore, DecideRequest, ListFilter, PendingItem, PendingList, SayRequest,
    UpdateItemRequest, WaitEvent, WaitEventAny, WaitFilter, WaitFor, WaitOutcome,
};

/// Who is invoking a board write (FR-3108 owner identity).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardCaller {
    CliTty,
    CliNonTty,
    Mcp,
    Dashboard,
}

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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "ref",
        alias = "ref_url"
    )]
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
    /// Steps from the revision immediately before the current one (dashboard diff).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_steps: Option<Vec<crate::graph::PlanStep>>,
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
pub struct CheckedRecord {
    pub by: String,
    pub at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadMessage {
    pub from: String,
    pub text: String,
    pub at_ms: u64,
}

/// Warning shown for an item whose intake plan came from the heuristic
/// fallback (SPEC-board-intake FR-3862, OQ-3860).
pub const FALLBACK_PLAN_WARNING: &str = "planner unavailable — fallback plan";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanningState {
    Planning,
    Planned,
    Failed,
}

/// `item.planning` of a `board add --prompt` intake (SPEC-board-intake FR-3861).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Planning {
    pub state: PlanningState,
    pub started_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<u64>,
    /// `planner` or `fallback`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// [`FALLBACK_PLAN_WARNING`] when the attached plan is a fallback plan (FR-3862).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<CheckedRecord>,
    #[serde(default)]
    pub thread: Vec<ThreadMessage>,
    #[serde(default)]
    pub run_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planning: Option<Planning>,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub version: u32,
}

impl BoardItem {
    /// FR-3862: the fallback-plan warning while the intake plan is attached.
    pub fn planning_warning(&self) -> Option<&str> {
        self.planning
            .as_ref()
            .filter(|_| self.plan.is_some())
            .and_then(|p| p.warning.as_deref())
    }
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

pub fn default_actor_by(caller: BoardCaller) -> String {
    match caller {
        BoardCaller::CliTty | BoardCaller::Dashboard => "owner".to_owned(),
        BoardCaller::Mcp => "bot:mcp".to_owned(),
        BoardCaller::CliNonTty => "bot:coordinator".to_owned(),
    }
}

pub fn resolve_actor(by: Option<&str>, caller: BoardCaller) -> Result<String, BoardError> {
    use std::io::IsTerminal;
    let actor = by
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| default_actor_by(caller));
    if actor == "owner" {
        match caller {
            BoardCaller::Mcp | BoardCaller::CliNonTty => {
                return Err(BoardError::Forbidden(
                    "owner actions need an interactive terminal or the dashboard".to_owned(),
                ));
            }
            BoardCaller::CliTty => {
                if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                    return Err(BoardError::Forbidden(
                        "owner actions need stdin and stdout on a TTY".to_owned(),
                    ));
                }
                if !std::env::var(crate::term::policy::RUN_ID_ENV)
                    .unwrap_or_default()
                    .trim()
                    .is_empty()
                {
                    return Err(BoardError::Forbidden(
                        "owner actions are refused while DMUXAI_RUN_ID is set".to_owned(),
                    ));
                }
            }
            BoardCaller::Dashboard => {}
        }
    }
    if actor == "planner" {
        return Err(BoardError::Forbidden(
            "planner actor is reserved for internal board runner actions".to_owned(),
        ));
    }
    if actor != "owner" && !actor.starts_with("bot:") {
        return Err(BoardError::Usage(format!(
            "invalid --by '{actor}' (expected owner or bot:NAME)"
        )));
    }
    Ok(actor)
}

/// Strip trailing CR/LF so stored plan prompts match `dmuxai run -` (`read_prompt`).
pub fn normalize_plan_prompt(prompt: &str) -> String {
    prompt.trim_end_matches(['\r', '\n']).to_owned()
}

/// Read a local file path or stdin when `path` is `-`.
pub fn read_path_or_stdin(path: &str) -> Result<String, BoardError> {
    use std::io::Read;
    if path.trim() == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| BoardError::Usage(format!("could not read stdin: {e}")))?;
        if text.trim().is_empty() {
            return Err(BoardError::Usage("stdin is empty".to_owned()));
        }
        return Ok(text);
    }
    std::fs::read_to_string(path)
        .map_err(|e| BoardError::Usage(format!("could not read '{}': {e}", path.trim())))
}

pub fn actor_may_use_planner(actor: &str) -> bool {
    actor == "owner" || actor == "planner"
}

pub fn actor_is_owner(actor: &str) -> bool {
    actor == "owner"
}

/// Prompt-agnostic owner confirmation check for CLI TTY owner actions (FR-3138).
pub fn confirm_owner_action(
    id: &str,
    verb: &str,
    title: &str,
    read_answer: impl FnOnce(&str) -> Result<String, BoardError>,
) -> Result<(), BoardError> {
    let prompt = format!("Type the item id to confirm {verb} {title}:");
    let answer = read_answer(&prompt)?;
    if answer.trim() != id {
        return Err(BoardError::Forbidden(
            "confirmation did not match item id".to_owned(),
        ));
    }
    Ok(())
}

pub fn validate_item_id_early(id: &str) -> Result<(), BoardError> {
    if !valid_item_id(id) {
        return Err(BoardError::Usage("invalid board item id".to_owned()));
    }
    Ok(())
}

/// JSON body for a successful `board wait` (FR-3112).
pub fn wait_ready_json(event: &WaitEvent, item: &BoardItem) -> serde_json::Value {
    serde_json::json!({
        "timed_out": false,
        "event": event,
        "item": item,
    })
}

pub fn wait_ready_json_any(event: &WaitEventAny, item: &BoardItem) -> serde_json::Value {
    serde_json::json!({
        "timed_out": false,
        "event": event,
        "item": item,
    })
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
    if let Some(bot) = item.source.bot.as_mut() {
        *bot = crate::run_journal::mask_sensitive_tokens(bot);
    }
    if let Some(channel) = item.source.channel.as_mut() {
        *channel = crate::run_journal::mask_sensitive_tokens(channel);
    }
    if let Some(ref_url) = item.source.ref_url.as_mut() {
        *ref_url = crate::run_journal::mask_sensitive_tokens(ref_url);
    }
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
    if let Some(checked) = item.checked.as_mut()
        && let Some(text) = checked.text.as_mut()
    {
        *text = crate::run_journal::mask_sensitive_tokens(text);
    }
    if let Some(error) = item.planning.as_mut().and_then(|p| p.error.as_mut()) {
        *error = crate::run_journal::mask_sensitive_tokens(error);
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

pub fn validate_text_limit(label: &str, text: &str, max: usize) -> Result<(), BoardError> {
    if text.chars().count() > max {
        return Err(BoardError::Usage(format!(
            "{label} exceeds {max} characters"
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
        assert!(resolve_actor(Some("owner"), BoardCaller::Mcp).is_err());
        assert!(resolve_actor(Some("owner"), BoardCaller::CliNonTty).is_err());
        assert!(matches!(
            resolve_actor(Some("planner"), BoardCaller::Mcp),
            Err(BoardError::Forbidden(_))
        ));
        assert!(matches!(
            resolve_actor(Some("planner"), BoardCaller::CliNonTty),
            Err(BoardError::Forbidden(_))
        ));
        match resolve_actor(None, BoardCaller::CliTty) {
            Ok(actor) => assert_eq!(actor, "owner"),
            Err(BoardError::Forbidden(_)) => {}
            Err(other) => panic!("unexpected resolve_actor error: {other}"),
        }
        assert!(resolve_actor(None, BoardCaller::CliNonTty).unwrap() == "bot:coordinator");
        assert!(resolve_actor(None, BoardCaller::Mcp).unwrap() == "bot:mcp");
        assert!(resolve_actor(None, BoardCaller::Dashboard).unwrap() == "owner");

        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let _guard = crate::state_path::test_env_lock();
        // SAFETY: test-only mutation of the process environment.
        unsafe { std::env::set_var("DMUXAI_STATE_DIR", &state) };
        let bad_id = super::mcp_invoke(
            std::path::Path::new("/tmp"),
            "board_show",
            &serde_json::json!({ "id": "../items/x" }),
        );
        let bad_lane = super::mcp_invoke(
            std::path::Path::new("/tmp"),
            "board_move",
            &serde_json::json!({ "id": "b000000000000", "lane": "not-a-lane" }),
        );
        let bad_add_lane = super::mcp_invoke(
            std::path::Path::new("/tmp"),
            "board_add",
            &serde_json::json!({ "title": "t", "kind": "debug", "lane": "not-a-lane" }),
        );
        let bad_kind = super::mcp_invoke(
            std::path::Path::new("/tmp"),
            "board_add",
            &serde_json::json!({ "title": "t", "kind": "not-a-kind" }),
        );
        unsafe { std::env::remove_var("DMUXAI_STATE_DIR") };
        assert_eq!(bad_id["exit_code"], exit::USAGE);
        assert_eq!(bad_lane["exit_code"], exit::USAGE);
        assert_eq!(bad_add_lane["exit_code"], exit::USAGE);
        assert_eq!(bad_kind["exit_code"], exit::USAGE);
        assert!(
            !state.join("board").exists(),
            "invalid input must not create the board directory"
        );
    }

    #[test]
    fn tc_3103_free_text_is_masked_on_write() {
        use crate::board::plan::{PendingPlanFile, write_pending_plan};
        use crate::board::store::{AddItemRequest, BoardStore, ensure_dir};
        use crate::graph::{PlanStep, PlanTask, TaskSize};
        use tempfile::tempdir;

        let secret = "sk-abcdefghijklmnop";
        let mut item = BoardItem {
            id: generate_item_id(),
            title: format!("title {secret}"),
            summary: format!("summary {secret}"),
            kind: Kind::Debug,
            grade: None,
            lane: Lane::Auto,
            source: Source {
                bot: Some(format!("bot {secret}")),
                channel: Some(format!("channel {secret}")),
                ref_url: Some(format!("https://example.com/x?token={secret}")),
            },
            workspace: WorkspaceRef::default(),
            recommendation: Some(format!("recommend {secret}")),
            options: vec![format!("option {secret}")],
            plan: Some(BoardPlan {
                hash: "0123456789abcdef".to_owned(),
                prompt: format!("prompt {secret}"),
                tasks: vec![PlanTask {
                    id: "task-1".to_owned(),
                    title: format!("task {secret}"),
                    done_check: format!("done {secret}"),
                    areas: vec![],
                    size: TaskSize::Small,
                    depends_on: vec![],
                    implicit: false,
                }],
                steps: vec![PlanStep {
                    id: "s1".to_owned(),
                    role: "implement".to_owned(),
                    job: format!("job {secret}"),
                    runner: "cursor".to_owned(),
                    model: None,
                    depends_on: vec![],
                    sensitive: None,
                    task: Some("task-1".to_owned()),
                    ..Default::default()
                }],
                revision: 1,
                history: vec![],
                previous_steps: None,
            }),
            decision: Some(Decision {
                verdict: Verdict::RequestChanges,
                text: Some(format!("decision {secret}")),
                option: None,
                plan_hash: None,
                by: "owner".to_owned(),
                at_ms: 0,
            }),
            checked: None,
            thread: vec![ThreadMessage {
                from: "owner".to_owned(),
                text: format!("thread {secret}"),
                at_ms: 0,
            }],
            run_ids: vec![],
            key: None,
            planning: Some(Planning {
                state: PlanningState::Failed,
                started_ms: 0,
                ended_ms: Some(1),
                plan_source: None,
                error: Some(format!("error {secret}")),
                warning: None,
            }),
            created_ms: 0,
            updated_ms: 0,
            version: 1,
        };
        mask_item_fields(&mut item);
        let serialized = serde_json::to_string(&item).unwrap();
        assert!(!serialized.contains(secret), "masked item leaked secret");
        for text in [
            &item.title,
            &item.summary,
            item.source.bot.as_ref().unwrap(),
            item.source.channel.as_ref().unwrap(),
            item.source.ref_url.as_ref().unwrap(),
            item.recommendation.as_ref().unwrap(),
            &item.options[0],
            &item.thread[0].text,
            item.decision.as_ref().unwrap().text.as_ref().unwrap(),
        ] {
            assert!(!text.contains(secret), "unmasked text: {text}");
        }
        let plan = item.plan.as_ref().unwrap();
        assert!(!plan.prompt.contains(secret));
        assert!(!plan.steps[0].job.contains(secret));
        assert!(!plan.tasks[0].title.contains(secret));
        assert!(!plan.tasks[0].done_check.contains(secret));

        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        ensure_dir(&state).unwrap();
        let store = BoardStore::open(state.join("board")).unwrap();
        let unmasked_prompt = format!("deploy {secret} now");
        let pending = PendingPlanFile {
            prompt: unmasked_prompt.clone(),
            tasks: vec![],
            steps: vec![],
        };
        let hash = write_pending_plan(&state, &pending).unwrap();
        let created = store
            .add(AddItemRequest {
                title: "plan".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: None,
                source_bot: None,
                source_channel: None,
                source_ref: None,
                workspace: None,
                recommendation: None,
                options: vec![],
                plan_hash: Some(hash),
                key: None,
                by: "bot:test".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        let stored = store.get(&created.id).unwrap();
        assert!(!stored.plan.as_ref().unwrap().prompt.contains(secret));
        assert_eq!(
            store.unmasked_prompt(&created.id, None).unwrap(),
            unmasked_prompt
        );

        let _guard = crate::state_path::test_env_lock();
        // SAFETY: test-only mutation of the process environment.
        unsafe { std::env::set_var("DMUXAI_STATE_DIR", &state) };
        let reply = super::mcp_invoke(
            std::path::Path::new("/tmp"),
            "board_prompt",
            &serde_json::json!({ "id": created.id }),
        );
        unsafe { std::env::remove_var("DMUXAI_STATE_DIR") };
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["prompt"], unmasked_prompt);
    }

    #[test]
    fn tc_3115_board_never_touches_credentials_or_token() {
        let files = [
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/board/store.rs")),
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/board/mcp.rs")),
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/board/plan.rs")),
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/board/rules.rs")),
        ]
        .join("\n");
        let web_token = format!("DMUXAI_{}", "WEB_TOKEN");
        let dash_token = format!("DMUXAI_{}", "DASHBOARD_TOKEN");
        for forbidden in [
            web_token.as_str(),
            &format!("{}=", "token"),
            ".cursor",
            "credentials.json",
            dash_token.as_str(),
        ] {
            assert!(
                !files.contains(forbidden),
                "board sources must not reference {forbidden}"
            );
        }
    }

    // TC-3237 / FR-3237: README board section and SPEC-decision-board §7 document auto-run
    // on approve and instruct bots not to call `board run` when lane is already `in_progress`.
    #[test]
    fn tc_3237_readme_documents_auto_run_on_approve() {
        let readme = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md"));
        let board_spec = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/docs/specs/SPEC-decision-board.md"
        ));
        let board_js = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/web/assets/board.js"
        ));
        assert!(readme.contains("### Decision board"));
        assert!(readme.contains("auto_run_on_approve"));
        assert!(readme.contains("board wait"));
        assert!(readme.contains("in_progress"));
        assert!(readme.contains("board run"));
        assert!(board_spec.contains("in_progress"));
        assert!(board_spec.contains("auto_run_on_approve"));
        assert!(board_js.contains("Approved — run started"));
        assert!(board_js.contains("Approved — auto-run skipped: "));
    }
}
