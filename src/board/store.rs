//! Board item storage and events (FR-3100, FR-3101).

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

use serde::{Deserialize, Serialize};

use super::plan::{
    PendingPlanFile, board_plan_from_pending, expand_pending_plan, pending_hash_for_plan,
    read_pending_plan, write_board_plan_copy,
};
use super::rules::RulesStore;
use super::{
    BoardError, BoardItem, CLOSED_HIDE_DAYS, Decision, Kind, Lane, SESSIONS_MAX, SessionLink,
    SessionStatus, Source, THREAD_MAX, ThreadMessage, Verdict, WorkspaceRef, generate_item_id,
    mask_item_fields, valid_item_id, validate_message, validate_new_text, validate_options,
    validate_ref_url,
};
use crate::graph::spec_review::ReviewStatus;
use crate::state_path::{repo_key, valid_pending_plan_hash};
use crate::workspace::normalize_workspace_path;

/// Read and verify a pending plan, then store it spec-first expanded for the
/// item's workspace so the approved plan is the plan that runs (FR-3715).
fn load_attached_plan(
    state_root: &Path,
    workspace: Option<&str>,
    hash: &str,
) -> Result<(PendingPlanFile, String), BoardError> {
    let pending = read_pending_plan(state_root, hash)?;
    if pending_hash_for_plan(&pending)? != hash {
        return Err(BoardError::Failed("pending plan hash mismatch".to_owned()));
    }
    if let Some(workspace) = workspace
        && let Some(expanded) = expand_pending_plan(Path::new(workspace), state_root, &pending)?
    {
        return Ok(expanded);
    }
    Ok((pending, hash.to_owned()))
}

/// Thread `from` and event `by` of spec review requests (FR-3852).
pub const SPEC_REVIEW_ACTOR: &str = "spec_review";
const SPEC_REVIEW_ID_MAX: usize = 128;

#[derive(Debug, Clone)]
pub struct AddItemRequest {
    pub title: String,
    pub summary: String,
    pub kind: Kind,
    pub grade: Option<super::Grade>,
    pub lane: Option<Lane>,
    pub source_bot: Option<String>,
    pub source_channel: Option<String>,
    pub source_ref: Option<String>,
    pub workspace: Option<PathBuf>,
    pub recommendation: Option<String>,
    pub options: Vec<String>,
    pub plan_hash: Option<String>,
    pub key: Option<String>,
    pub by: String,
    pub state_root: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateItemRequest {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub recommendation: Option<String>,
    pub options: Option<Vec<String>>,
    pub grade: Option<Option<super::Grade>>,
    pub kind: Option<Kind>,
    pub run_id: Option<String>,
    pub if_version: Option<u32>,
    pub by: String,
}

#[derive(Debug, Clone)]
pub struct DecideRequest {
    pub verdict: Verdict,
    pub text: Option<String>,
    pub option: Option<usize>,
    pub plan_hash: Option<String>,
    pub by: String,
}

#[derive(Debug, Clone)]
pub struct SayRequest {
    pub text: String,
    pub from: String,
}

#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub lanes: Vec<Lane>,
    pub workspace: Option<PathBuf>,
    pub include_all: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitFor {
    Decision,
    Message,
    Any,
}

#[derive(Debug, Clone)]
pub struct WaitFilter {
    pub since_version: u32,
    pub timeout_secs: u64,
    pub wait_for: WaitFor,
    pub waiter: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WaitEvent {
    pub seq: u64,
    pub item_id: String,
    pub kind: String,
    pub by: String,
    pub version: u32,
    #[serde(flatten)]
    pub detail: EventDetail,
}

#[derive(Debug, Clone, Serialize)]
pub struct WaitEventAny {
    pub seq: u64,
    pub item_id: String,
    pub kind: String,
    pub by: String,
    pub version: u32,
    #[serde(flatten)]
    pub detail: EventDetail,
}

/// Optional fields of coordinator observability events
/// (SPEC-coordinator-observability FR-3850..FR-3852).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventDetail {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_revision: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spec_paths: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum WaitOutcome {
    Ready {
        item: Box<BoardItem>,
        event: WaitEvent,
    },
    TimedOut {
        version: u32,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingItem {
    pub id: String,
    pub reason: String,
    pub lane: Lane,
    pub workspace_path: Option<String>,
    pub repo_key: Option<String>,
    pub title: String,
    pub kind: Kind,
    pub grade: Option<super::Grade>,
    pub age_ms: u64,
    pub updated_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_review: Option<super::BoardSpecReview>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingList {
    pub items: Vec<PendingItem>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EventKind {
    Created,
    Updated,
    Lane,
    Plan,
    Decision,
    Message,
    Checked,
    RunStarted,
    AwaitingApproval,
    SpecReviewRequested,
    Session,
}

impl EventKind {
    /// Coordinator observability kinds wake `wait --for any` whoever wrote
    /// them (FR-3854); the waiter's own writes are still ignored.
    fn is_observability(&self) -> bool {
        matches!(
            self,
            EventKind::RunStarted | EventKind::AwaitingApproval | EventKind::SpecReviewRequested
        )
    }

    fn matches_wait(&self, wait_for: WaitFor) -> bool {
        match wait_for {
            WaitFor::Decision => matches!(self, EventKind::Decision),
            WaitFor::Message => matches!(self, EventKind::Message),
            WaitFor::Any => {
                matches!(
                    self,
                    EventKind::Decision | EventKind::Message | EventKind::Plan | EventKind::Lane
                ) || self.is_observability()
            }
        }
    }

    fn as_str(&self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".to_owned())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BoardEvent {
    seq: u64,
    item_id: String,
    kind: EventKind,
    version: u32,
    at_ms: u64,
    by: String,
    #[serde(flatten)]
    detail: EventDetail,
}

/// Only `seq` of an event line, so lines with kinds or fields this binary
/// does not know still count when allocating the next `seq`.
#[derive(Deserialize)]
struct EventSeq {
    seq: u64,
}

pub struct BoardStore {
    base: PathBuf,
    items_dir: PathBuf,
    events_path: PathBuf,
    lock_path: PathBuf,
}

pub fn ensure_dir(path: &Path) -> Result<(), BoardError> {
    crate::run_journal::verify_no_symlinks(path).map_err(|e| BoardError::Usage(e.to_string()))?;
    if crate::run_journal::is_symlink(path).unwrap_or(false) {
        return Err(BoardError::Usage(format!(
            "refusing symlinked board path at {}",
            path.display()
        )));
    }
    if path.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if path.is_dir() {
                let mode = fs::metadata(path)
                    .map_err(|e| BoardError::Internal(e.to_string()))?
                    .permissions()
                    .mode()
                    & 0o777;
                if mode != 0o700 {
                    let mut perms = fs::metadata(path)
                        .map_err(|e| BoardError::Internal(e.to_string()))?
                        .permissions();
                    perms.set_mode(0o700);
                    fs::set_permissions(path, perms)
                        .map_err(|e| BoardError::Internal(e.to_string()))?;
                }
            }
        }
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder
        .create(path)
        .map_err(|e| BoardError::Internal(e.to_string()))
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), BoardError> {
    let parent = path.parent().unwrap_or(Path::new("."));
    ensure_dir(parent)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_owned());
    let temp = parent.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let result = (|| -> Result<(), BoardError> {
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        opts.mode(0o600);
        let mut file = opts
            .open(&temp)
            .map_err(|e| BoardError::Internal(e.to_string()))?;
        file.write_all(bytes)
            .map_err(|e| BoardError::Internal(e.to_string()))?;
        file.sync_all()
            .map_err(|e| BoardError::Internal(e.to_string()))?;
        drop(file);
        fs::rename(&temp, path).map_err(|e| BoardError::Internal(e.to_string()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl BoardStore {
    pub fn open(base: PathBuf) -> Result<Self, BoardError> {
        let items_dir = base.join("items");
        ensure_dir(&base)?;
        ensure_dir(&items_dir)?;
        let events_path = base.join("events.jsonl");
        let lock_path = base.join(".lock");
        Ok(Self {
            base: base.clone(),
            items_dir,
            events_path,
            lock_path,
        })
    }

    pub fn base(&self) -> &Path {
        &self.base
    }

    pub(crate) fn item_path(&self, id: &str) -> Result<PathBuf, BoardError> {
        if !valid_item_id(id) {
            return Err(BoardError::Usage("invalid board item id".to_owned()));
        }
        Ok(self.items_dir.join(format!("{id}.json")))
    }

    fn with_lock<T>(
        &self,
        work: impl FnOnce(&Self) -> Result<T, BoardError>,
    ) -> Result<T, BoardError> {
        let mut opts = OpenOptions::new();
        opts.create(true).read(true).write(true);
        #[cfg(unix)]
        opts.mode(0o600);
        let lock_file = opts
            .open(&self.lock_path)
            .map_err(|e| BoardError::Internal(e.to_string()))?;
        lock_file
            .lock()
            .map_err(|e| BoardError::Internal(format!("board lock: {e}")))?;
        work(self)
    }

    fn read_last_seq(&self) -> Result<u64, BoardError> {
        if !self.events_path.exists() {
            return Ok(0);
        }
        let file =
            File::open(&self.events_path).map_err(|e| BoardError::Internal(e.to_string()))?;
        let mut handle = file;
        let len = handle
            .metadata()
            .map_err(|e| BoardError::Internal(e.to_string()))?
            .len();
        if len == 0 {
            return Ok(0);
        }
        let scan = len.min(65536);
        handle
            .seek(SeekFrom::End(-(scan as i64)))
            .map_err(|e| BoardError::Internal(e.to_string()))?;
        let mut tail = Vec::new();
        handle
            .read_to_end(&mut tail)
            .map_err(|e| BoardError::Internal(e.to_string()))?;
        let mut last = 0u64;
        let mut scan_text = String::from_utf8_lossy(&tail).into_owned();
        if len > scan && !tail.ends_with(b"\n") {
            scan_text = fs::read_to_string(&self.events_path)
                .map_err(|e| BoardError::Internal(e.to_string()))?;
        }
        for line in scan_text.lines() {
            if let Ok(event) = serde_json::from_str::<EventSeq>(line) {
                last = last.max(event.seq);
            }
        }
        Ok(last)
    }

    fn append_event(
        &self,
        item_id: &str,
        kind: EventKind,
        version: u32,
        by: &str,
    ) -> Result<(), BoardError> {
        self.append_event_detail(item_id, kind, version, by, EventDetail::default())
    }

    fn append_event_detail(
        &self,
        item_id: &str,
        kind: EventKind,
        version: u32,
        by: &str,
        detail: EventDetail,
    ) -> Result<(), BoardError> {
        let seq = self.read_last_seq()? + 1;
        let event = BoardEvent {
            seq,
            item_id: item_id.to_owned(),
            kind,
            version,
            at_ms: now_ms(),
            by: by.to_owned(),
            detail,
        };
        let line =
            serde_json::to_string(&event).map_err(|e| BoardError::Internal(e.to_string()))?;
        let mut opts = OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        opts.mode(0o600);
        let mut file = opts
            .open(&self.events_path)
            .map_err(|e| BoardError::Internal(e.to_string()))?;
        writeln!(file, "{line}").map_err(|e| BoardError::Internal(e.to_string()))?;
        file.sync_all()
            .map_err(|e| BoardError::Internal(e.to_string()))?;
        Ok(())
    }

    fn load_item_inner(&self, id: &str) -> Result<BoardItem, BoardError> {
        let path = self.item_path(id)?;
        let bytes = fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                BoardError::NotFound("board item not found".to_owned())
            } else {
                BoardError::Internal(e.to_string())
            }
        })?;
        serde_json::from_slice(&bytes)
            .map_err(|e| BoardError::Failed(format!("invalid item JSON: {e}")))
    }

    fn save_item_inner(&self, item: &BoardItem) -> Result<(), BoardError> {
        let path = self.item_path(&item.id)?;
        let mut copy = item.clone();
        mask_item_fields(&mut copy);
        let bytes =
            serde_json::to_vec_pretty(&copy).map_err(|e| BoardError::Internal(e.to_string()))?;
        atomic_write(&path, &bytes)
    }

    pub fn get(&self, id: &str) -> Result<BoardItem, BoardError> {
        if !valid_item_id(id) {
            return Err(BoardError::Usage("invalid board item id".to_owned()));
        }
        self.load_item_inner(id)
    }

    pub fn add(&self, req: AddItemRequest) -> Result<BoardItem, BoardError> {
        validate_new_text(&req.title, &req.summary, req.recommendation.as_deref())?;
        validate_options(&req.options)?;
        validate_ref_url(req.source_ref.as_deref())?;
        if let Some(hash) = &req.plan_hash
            && !valid_pending_plan_hash(hash)
        {
            return Err(BoardError::Usage(
                "plan hash must be 16 lowercase hex characters".to_owned(),
            ));
        }
        let (item, notify_needs_decision, awaiting) = self.with_lock(|store| {
            if let Some(key) = &req.key
                && let Some(existing) = store.find_item_by_key(key, req.source_bot.as_deref())?
            {
                return Ok((existing, false, None));
            }
            let rules = RulesStore::new(&store.base).load();
            let lane = if let Some(lane) = req.lane {
                if matches!(lane, Lane::Approved | Lane::Rejected) {
                    return Err(BoardError::Forbidden(
                        "cannot create item in approved or rejected (use board decide)".to_owned(),
                    ));
                }
                if req.by != "owner" && lane == Lane::Auto {
                    return Err(BoardError::Forbidden(
                        "only the owner may set lane auto".to_owned(),
                    ));
                }
                lane
            } else {
                rules.initial_lane(req.kind, req.grade, &req.title)?
            };
            let workspace = workspace_from_path(req.workspace.as_deref())?;
            let mut item = BoardItem {
                id: generate_item_id(),
                title: req.title,
                summary: req.summary,
                kind: req.kind,
                grade: req.grade,
                lane,
                source: Source {
                    bot: req.source_bot,
                    channel: req.source_channel,
                    ref_url: req.source_ref,
                },
                workspace,
                recommendation: req.recommendation,
                options: req.options,
                plan: None,
                decision: None,
                checked: None,
                thread: Vec::new(),
                run_ids: Vec::new(),
                sessions: Vec::new(),
                key: req.key.clone(),
                spec_review: None,
                awaiting_approval_revision: None,
                created_ms: now_ms(),
                updated_ms: now_ms(),
                version: 1,
            };
            if let Some(hash) = req.plan_hash {
                let (pending, hash) =
                    load_attached_plan(&req.state_root, item.workspace.path.as_deref(), &hash)?;
                write_board_plan_copy(&req.state_root, &store.base, &hash)?;
                item.plan = Some(board_plan_from_pending(pending, hash, None));
                store.clear_decision_on_plan_change(&mut item);
            }
            let awaiting = mark_awaiting_approval(&mut item, None, None);
            mask_item_fields(&mut item);
            store.save_item_inner(&item)?;
            store.append_event(&item.id, EventKind::Created, item.version, &req.by)?;
            store.append_awaiting_approval(&item, &req.by, awaiting.as_ref())?;
            let notify = item.lane == Lane::NeedsDecision;
            Ok((item, notify, awaiting))
        })?;
        self.dispatch_lane_events(&item, &req.by, notify_needs_decision, awaiting.as_ref());
        Ok(item)
    }

    fn append_awaiting_approval(
        &self,
        item: &BoardItem,
        by: &str,
        awaiting: Option<&EventDetail>,
    ) -> Result<(), BoardError> {
        match awaiting {
            Some(detail) => self.append_event_detail(
                &item.id,
                EventKind::AwaitingApproval,
                item.version,
                by,
                detail.clone(),
            ),
            None => Ok(()),
        }
    }

    /// Dispatch `needs_decision` and `awaiting_approval` notifications after
    /// the write, reading `[board]` once (FR-3135, FR-3851).
    fn dispatch_lane_events(
        &self,
        item: &BoardItem,
        by: &str,
        needs_decision: bool,
        awaiting: Option<&EventDetail>,
    ) {
        if !needs_decision && awaiting.is_none() {
            return;
        }
        let settings = crate::settings::BoardSettings::load_user_only();
        if needs_decision {
            super::notify::dispatch_board_event_with_settings(
                &self.base,
                item,
                "needs_decision",
                by,
                &settings,
            );
        }
        if let Some(detail) = awaiting {
            super::notify::dispatch_board_event_detail_with_settings(
                &self.base,
                item,
                "awaiting_approval",
                by,
                detail,
                &settings,
            );
        }
    }

    pub fn list(&self, filter: &ListFilter) -> Result<Vec<BoardItem>, BoardError> {
        let mut items = Vec::new();
        let cutoff = now_ms().saturating_sub(CLOSED_HIDE_DAYS * 86_400 * 1000);
        let workspace_key = filter
            .workspace
            .as_ref()
            .map(|p| normalize_workspace_path(p))
            .transpose()
            .map_err(|e| BoardError::Usage(e.to_string()))?;
        for entry in
            fs::read_dir(&self.items_dir).map_err(|e| BoardError::Internal(e.to_string()))?
        {
            let entry = entry.map_err(|e| BoardError::Internal(e.to_string()))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs::read(&path).map_err(|e| BoardError::Internal(e.to_string()))?;
            let item: BoardItem = serde_json::from_slice(&bytes)
                .map_err(|e| BoardError::Internal(format!("invalid item: {e}")))?;
            if !filter.lanes.is_empty() && !filter.lanes.contains(&item.lane) {
                continue;
            }
            if !filter.include_all
                && matches!(item.lane, Lane::Done | Lane::Rejected)
                && item.updated_ms < cutoff
            {
                continue;
            }
            if let Some(ref ws) = workspace_key
                && item.workspace.path.as_deref() != Some(ws.as_str())
            {
                continue;
            }
            items.push(item);
        }
        items.sort_by_key(|item| std::cmp::Reverse(item.updated_ms));
        Ok(items)
    }

    /// List items waiting on human action, oldest first (FR-3137).
    pub fn pending(&self, workspace: Option<&Path>) -> Result<PendingList, BoardError> {
        let now = now_ms();
        let cutoff = now.saturating_sub(CLOSED_HIDE_DAYS * 86_400 * 1000);
        let workspace_key = workspace
            .map(normalize_workspace_path)
            .transpose()
            .map_err(|e| BoardError::Usage(e.to_string()))?;
        let mut entries = Vec::new();
        for entry in
            fs::read_dir(&self.items_dir).map_err(|e| BoardError::Internal(e.to_string()))?
        {
            let entry = entry.map_err(|e| BoardError::Internal(e.to_string()))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs::read(&path).map_err(|e| BoardError::Internal(e.to_string()))?;
            let item: BoardItem = serde_json::from_slice(&bytes)
                .map_err(|e| BoardError::Internal(format!("invalid item: {e}")))?;
            if let Some(ref ws) = workspace_key
                && item.workspace.path.as_deref() != Some(ws.as_str())
            {
                continue;
            }
            let spec_review_pending = item
                .spec_review
                .as_ref()
                .is_some_and(|review| review.status == ReviewStatus::Pending);
            let reason = match item.lane {
                _ if spec_review_pending => "spec_review",
                Lane::NeedsDecision => "decide",
                Lane::Unknown if item.checked.is_none() && item.updated_ms >= cutoff => "failed",
                Lane::Done if item.checked.is_none() && item.updated_ms >= cutoff => "check",
                _ => continue,
            };
            entries.push(PendingItem {
                id: item.id,
                reason: reason.to_owned(),
                lane: item.lane,
                workspace_path: item.workspace.path,
                repo_key: item.workspace.repo_key,
                title: item.title,
                kind: item.kind,
                grade: item.grade,
                age_ms: now.saturating_sub(item.updated_ms),
                updated_ms: item.updated_ms,
                spec_review: item.spec_review,
            });
        }
        entries.sort_by(|a, b| {
            a.updated_ms
                .cmp(&b.updated_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        let count = entries.len();
        Ok(PendingList {
            items: entries,
            count,
        })
    }

    /// Mark a `done` or `unknown` item as checked by the owner (FR-3137, FR-3138).
    pub fn check(&self, id: &str, text: Option<String>, by: &str) -> Result<BoardItem, BoardError> {
        if !valid_item_id(id) {
            return Err(BoardError::Usage("invalid board item id".to_owned()));
        }
        if by != "owner" {
            return Err(BoardError::Forbidden(
                "only the owner may check items".to_owned(),
            ));
        }
        if let Some(t) = text.as_deref() {
            super::validate_text_limit("text", t, super::TEXT_MAX)?;
        }
        let clean_text = text.and_then(|t| {
            let trimmed = t.trim().to_owned();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        });
        self.with_lock(|store| {
            let mut item = store.load_item_inner(id)?;
            if !matches!(item.lane, Lane::Done | Lane::Unknown) {
                return Err(BoardError::Conflict(format!(
                    "item in lane '{}' cannot be checked (must be done or unknown)",
                    item.lane.as_str()
                )));
            }
            let now = now_ms();
            item.checked = Some(super::CheckedRecord {
                by: by.to_owned(),
                at_ms: now,
                text: clean_text,
            });
            item.version += 1;
            item.updated_ms = now;
            store.save_item_inner(&item)?;
            store.append_event(id, EventKind::Checked, item.version, by)?;
            Ok(item)
        })
    }

    pub fn update(&self, id: &str, req: UpdateItemRequest) -> Result<BoardItem, BoardError> {
        if !valid_item_id(id) {
            return Err(BoardError::Usage("invalid board item id".to_owned()));
        }
        self.with_lock(|store| {
            let mut item = store.load_item_inner(id)?;
            if let Some(expected) = req.if_version
                && item.version != expected
            {
                return Err(BoardError::Conflict(format!(
                    "version mismatch (expected {expected}, found {})",
                    item.version
                )));
            }
            if let Some(title) = req.title {
                validate_new_text(&title, "", None)?;
                item.title = title;
            }
            if let Some(summary) = req.summary {
                validate_new_text(&item.title, &summary, None)?;
                item.summary = summary;
            }
            if let Some(rec) = req.recommendation {
                validate_new_text(&item.title, "", Some(&rec))?;
                item.recommendation = Some(rec);
            }
            if let Some(options) = req.options {
                validate_options(&options)?;
                item.options = options;
            }
            if let Some(grade) = req.grade {
                item.grade = grade;
            }
            if let Some(kind) = req.kind {
                item.kind = kind;
            }
            if let Some(run_id) = req.run_id
                && !run_id.trim().is_empty()
            {
                item.run_ids.push(run_id);
            }
            item.version += 1;
            item.updated_ms = now_ms();
            store.save_item_inner(&item)?;
            store.append_event(id, EventKind::Updated, item.version, &req.by)?;
            Ok(item)
        })
    }

    pub fn move_lane(
        &self,
        id: &str,
        lane: Lane,
        by: &str,
        if_version: Option<u32>,
    ) -> Result<BoardItem, BoardError> {
        if !valid_item_id(id) {
            return Err(BoardError::Usage("invalid board item id".to_owned()));
        }
        enforce_sensitive_lane_move(by, lane)?;
        let (item, notify_needs_decision, awaiting) = self.with_lock(|store| {
            let mut item = store.load_item_inner(id)?;
            if let Some(expected) = if_version
                && item.version != expected
            {
                return Err(BoardError::Conflict(format!(
                    "version mismatch (expected {expected}, found {})",
                    item.version
                )));
            }
            enforce_in_progress_move(by, lane, &item)?;
            let prev_lane = item.lane;
            let prev_revision = item.plan.as_ref().map(|p| p.revision);
            item.lane = lane;
            if prev_lane != lane || matches!(lane, Lane::Done | Lane::Unknown) {
                item.checked = None;
            }
            item.version += 1;
            item.updated_ms = now_ms();
            let awaiting = mark_awaiting_approval(&mut item, Some(prev_lane), prev_revision);
            store.save_item_inner(&item)?;
            store.append_event(id, EventKind::Lane, item.version, by)?;
            store.append_awaiting_approval(&item, by, awaiting.as_ref())?;
            let notify = prev_lane != Lane::NeedsDecision && item.lane == Lane::NeedsDecision;
            Ok((item, notify, awaiting))
        })?;
        self.dispatch_lane_events(&item, by, notify_needs_decision, awaiting.as_ref());
        Ok(item)
    }

    pub fn attach_plan(
        &self,
        id: &str,
        hash: &str,
        state_root: &Path,
        by: &str,
    ) -> Result<BoardItem, BoardError> {
        if !valid_item_id(id) || !valid_pending_plan_hash(hash) {
            return Err(BoardError::Usage("invalid id or plan hash".to_owned()));
        }
        let (item, notify_needs_decision, awaiting) = self.with_lock(|store| {
            let mut item = store.load_item_inner(id)?;
            store.ensure_no_run_in_flight(&item)?;
            let (pending, hash) =
                load_attached_plan(state_root, item.workspace.path.as_deref(), hash)?;
            write_board_plan_copy(state_root, &store.base, &hash)?;
            let prev_lane = item.lane;
            let prev_revision = item.plan.as_ref().map(|p| p.revision);
            let plan = board_plan_from_pending(pending, hash, item.plan.as_ref());
            item.plan = Some(plan);
            store.clear_decision_on_plan_change(&mut item);
            item.version += 1;
            item.updated_ms = now_ms();
            let awaiting = mark_awaiting_approval(&mut item, Some(prev_lane), prev_revision);
            store.save_item_inner(&item)?;
            store.append_event(id, EventKind::Plan, item.version, by)?;
            store.append_awaiting_approval(&item, by, awaiting.as_ref())?;
            let notify = prev_lane != Lane::NeedsDecision && item.lane == Lane::NeedsDecision;
            Ok((item, notify, awaiting))
        })?;
        self.dispatch_lane_events(&item, by, notify_needs_decision, awaiting.as_ref());
        Ok(item)
    }

    pub fn edit_plan(
        &self,
        id: &str,
        hash: &str,
        _workspace: &Path,
        _state_root: &Path,
        by: &str,
    ) -> Result<BoardItem, BoardError> {
        if !valid_item_id(id) || !valid_pending_plan_hash(hash) {
            return Err(BoardError::Usage("invalid id or plan hash".to_owned()));
        }
        let (item, notify_needs_decision, awaiting) = self.with_lock(|store| {
            let mut item = store.load_item_inner(id)?;
            store.ensure_no_run_in_flight(&item)?;
            let (pending, hash) =
                load_attached_plan(_state_root, item.workspace.path.as_deref(), hash)?;
            write_board_plan_copy(_state_root, &store.base, &hash)?;
            let prev_lane = item.lane;
            let prev_revision = item.plan.as_ref().map(|p| p.revision);
            let plan = board_plan_from_pending(pending, hash, item.plan.as_ref());
            item.plan = Some(plan);
            store.clear_decision_on_plan_change(&mut item);
            item.version += 1;
            item.updated_ms = now_ms();
            let awaiting = mark_awaiting_approval(&mut item, Some(prev_lane), prev_revision);
            store.save_item_inner(&item)?;
            store.append_event(id, EventKind::Plan, item.version, by)?;
            store.append_awaiting_approval(&item, by, awaiting.as_ref())?;
            let notify = prev_lane != Lane::NeedsDecision && item.lane == Lane::NeedsDecision;
            Ok((item, notify, awaiting))
        })?;
        self.dispatch_lane_events(&item, by, notify_needs_decision, awaiting.as_ref());
        Ok(item)
    }

    pub fn unmasked_prompt(&self, id: &str, hash: Option<&str>) -> Result<String, BoardError> {
        if !valid_item_id(id) {
            return Err(BoardError::Usage("invalid board item id".to_owned()));
        }
        let item = self.load_item_inner(id)?;
        let plan_hash = if let Some(hash) = hash {
            if !valid_pending_plan_hash(hash) {
                return Err(BoardError::Usage(
                    "plan hash must be 16 lowercase hex characters".to_owned(),
                ));
            }
            let current = item.plan.as_ref().map(|p| p.hash.as_str());
            let in_history = item
                .plan
                .as_ref()
                .map(|p| p.history.iter().any(|h| h == hash))
                .unwrap_or(false);
            if current != Some(hash) && !in_history {
                return Err(BoardError::NotFound(
                    "plan hash is not current or in history".to_owned(),
                ));
            }
            hash.to_owned()
        } else {
            item.plan
                .as_ref()
                .map(|p| p.hash.clone())
                .ok_or_else(|| BoardError::NotFound("item has no plan".to_owned()))?
        };
        let state_root = self
            .base
            .parent()
            .ok_or_else(|| BoardError::Internal("board store has no state parent".to_owned()))?;
        let pending = super::plan::read_pending_or_board_copy(state_root, &self.base, &plan_hash)?;
        Ok(pending.prompt)
    }

    /// FR-3807: no plan attach or edit while a live run holds the item.
    fn ensure_no_run_in_flight(&self, item: &BoardItem) -> Result<(), BoardError> {
        if item.lane == Lane::InProgress
            && super::runner::BoardLockGuard::is_active(&self.base, "runs", &item.id)
        {
            return Err(BoardError::Conflict(
                "cannot change the plan while a run is in progress for this item".to_owned(),
            ));
        }
        Ok(())
    }

    fn clear_decision_on_plan_change(&self, item: &mut BoardItem) {
        item.decision = None;
        if item.lane == Lane::Approved || item.lane != Lane::Auto {
            item.lane = Lane::NeedsDecision;
        }
    }

    fn find_item_by_key(
        &self,
        key: &str,
        bot: Option<&str>,
    ) -> Result<Option<BoardItem>, BoardError> {
        for entry in
            fs::read_dir(&self.items_dir).map_err(|e| BoardError::Internal(e.to_string()))?
        {
            let entry = entry.map_err(|e| BoardError::Internal(e.to_string()))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs::read(&path).map_err(|e| BoardError::Internal(e.to_string()))?;
            let item: BoardItem = serde_json::from_slice(&bytes)
                .map_err(|e| BoardError::Internal(format!("invalid item: {e}")))?;
            if item.key.as_deref() == Some(key)
                && bot
                    .map(|b| item.source.bot.as_deref() == Some(b))
                    .unwrap_or(true)
            {
                return Ok(Some(item));
            }
        }
        Ok(None)
    }

    pub fn list_events(&self, since_seq: u64) -> Result<Vec<WaitEventAny>, BoardError> {
        if !self.events_path.exists() {
            return Ok(Vec::new());
        }
        let text = fs::read_to_string(&self.events_path)
            .map_err(|e| BoardError::Internal(e.to_string()))?;
        let mut out = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(event) = serde_json::from_str::<BoardEvent>(line) else {
                continue;
            };
            if event.seq <= since_seq {
                continue;
            }
            out.push(WaitEventAny {
                seq: event.seq,
                item_id: event.item_id,
                kind: event.kind.as_str(),
                by: event.by,
                version: event.version,
                detail: event.detail,
            });
        }
        Ok(out)
    }

    pub fn with_rules_lock<T>(
        &self,
        work: impl FnOnce() -> Result<T, BoardError>,
    ) -> Result<T, BoardError> {
        self.with_lock(|_| work())
    }

    /// Serializes run lock read-modify-write against other board writers.
    pub(crate) fn with_store_lock<T>(
        &self,
        work: impl FnOnce() -> Result<T, BoardError>,
    ) -> Result<T, BoardError> {
        self.with_lock(|_| work())
    }

    pub fn wait_any(
        &self,
        since_seq: u64,
        filter: WaitFilter,
    ) -> Result<Option<(WaitEventAny, BoardItem)>, BoardError> {
        let deadline = Instant::now() + Duration::from_secs(filter.timeout_secs);
        let mut file_offset = 0u64;
        loop {
            if let Some(event) =
                self.poll_events_any(&filter, since_seq, file_offset, &mut file_offset)?
            {
                let item = self.get(&event.item_id)?;
                return Ok(Some((event, item)));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            thread::sleep(Duration::from_millis(500));
        }
    }

    fn poll_events_any(
        &self,
        filter: &WaitFilter,
        since_seq: u64,
        read_from: u64,
        file_offset: &mut u64,
    ) -> Result<Option<WaitEventAny>, BoardError> {
        if !self.events_path.exists() {
            return Ok(None);
        }
        let len = fs::metadata(&self.events_path)
            .map_err(|e| BoardError::Internal(e.to_string()))?
            .len();
        if len <= read_from {
            return Ok(None);
        }
        let mut file =
            File::open(&self.events_path).map_err(|e| BoardError::Internal(e.to_string()))?;
        file.seek(SeekFrom::Start(read_from))
            .map_err(|e| BoardError::Internal(e.to_string()))?;
        let byte_count = (len - read_from) as usize;
        let mut buf = vec![0u8; byte_count];
        file.read_exact(&mut buf)
            .map_err(|e| BoardError::Internal(e.to_string()))?;
        let Some(last_nl) = buf.iter().rposition(|&b| b == b'\n') else {
            return Ok(None);
        };
        *file_offset = read_from + (last_nl as u64) + 1;
        let text = String::from_utf8_lossy(&buf[..=last_nl]);
        let mut last_match = None;
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(event) = serde_json::from_str::<BoardEvent>(line) else {
                continue;
            };
            if event.seq <= since_seq {
                continue;
            }
            if let Some(trigger) = wait_trigger_for_event_any(&event, filter) {
                last_match = Some(trigger);
            }
        }
        Ok(last_match)
    }

    pub fn decide(&self, id: &str, req: DecideRequest) -> Result<BoardItem, BoardError> {
        let by = req.by.clone();
        let item = self.decide_without_notify(id, req)?;
        super::notify::dispatch_decision_event(&self.base, &item, &by, false);
        Ok(item)
    }

    /// Record a decision and append `EventKind::Decision` to `events.jsonl`
    /// without dispatching `[board]` notify yet (used when owner approval
    /// attempts auto-run first so notify can carry `auto_run: true`).
    pub fn decide_without_notify(
        &self,
        id: &str,
        req: DecideRequest,
    ) -> Result<BoardItem, BoardError> {
        if !valid_item_id(id) {
            return Err(BoardError::Usage("invalid board item id".to_owned()));
        }
        if req.by != "owner" {
            return Err(BoardError::Forbidden(
                "only the owner may record decisions".to_owned(),
            ));
        }
        self.with_lock(|store| {
            let mut item = store.load_item_inner(id)?;
            if matches!(req.verdict, Verdict::Approve)
                && let Some(plan) = &item.plan
            {
                let Some(hash) = req.plan_hash.as_deref() else {
                    return Err(BoardError::Conflict(
                        "plan_hash is required to approve an item with a plan".to_owned(),
                    ));
                };
                if hash != plan.hash {
                    return Err(BoardError::Conflict(
                        "plan hash does not match the current plan".to_owned(),
                    ));
                }
            } else if let Some(hash) = &req.plan_hash {
                let current = item.plan.as_ref().map(|p| p.hash.as_str());
                if current != Some(hash.as_str()) {
                    return Err(BoardError::Conflict(
                        "plan hash does not match the current plan".to_owned(),
                    ));
                }
            }
            let verdict = req.verdict;
            match verdict {
                Verdict::RequestChanges => {
                    let text = req
                        .text
                        .as_deref()
                        .filter(|t| !t.trim().is_empty())
                        .ok_or_else(|| {
                            BoardError::Usage("request_changes needs --text".to_owned())
                        })?;
                    validate_message(text)?;
                }
                Verdict::Answer => {
                    if req.text.as_deref().is_none_or(|t| t.trim().is_empty())
                        && req.option.is_none()
                    {
                        return Err(BoardError::Usage(
                            "answer needs --text or --option".to_owned(),
                        ));
                    }
                    if let Some(text) = req.text.as_deref() {
                        validate_message(text)?;
                    }
                    if let Some(idx) = req.option
                        && idx >= item.options.len()
                    {
                        return Err(BoardError::Usage("option index out of range".to_owned()));
                    }
                }
                Verdict::Approve | Verdict::Reject => {}
            }
            item.decision = Some(Decision {
                verdict,
                text: req.text,
                option: req.option,
                plan_hash: req
                    .plan_hash
                    .or_else(|| item.plan.as_ref().map(|p| p.hash.clone())),
                by: req.by.clone(),
                at_ms: now_ms(),
            });
            item.lane = match verdict {
                Verdict::Approve => Lane::Approved,
                Verdict::Reject => Lane::Rejected,
                Verdict::RequestChanges | Verdict::Answer => Lane::NeedsDecision,
            };
            item.version += 1;
            item.updated_ms = now_ms();
            store.save_item_inner(&item)?;
            store.append_event(id, EventKind::Decision, item.version, &req.by)?;
            Ok(item)
        })
    }

    /// Store the item's spec review and optionally append a masked thread
    /// message; never changes the lane or the decision (SPEC-spec-review-gate
    /// FR-3711).
    pub fn set_spec_review(
        &self,
        id: &str,
        review: super::BoardSpecReview,
        message: Option<(&str, &str)>,
    ) -> Result<BoardItem, BoardError> {
        if !valid_item_id(id) {
            return Err(BoardError::Usage("invalid board item id".to_owned()));
        }
        self.with_lock(|store| {
            let mut item = store.load_item_inner(id)?;
            if item.spec_review.as_ref() == Some(&review) && message.is_none() {
                return Ok(item);
            }
            item.spec_review = Some(review.clone());
            let by = match message {
                Some((from, text)) => {
                    let masked = crate::run_journal::mask_sensitive_tokens(text);
                    if item.thread.len() >= THREAD_MAX {
                        item.thread.remove(0);
                    }
                    item.thread.push(ThreadMessage {
                        from: from.to_owned(),
                        text: masked.chars().take(super::TEXT_MAX).collect(),
                        at_ms: now_ms(),
                    });
                    from
                }
                None => "owner",
            };
            item.version += 1;
            item.updated_ms = now_ms();
            store.save_item_inner(&item)?;
            let kind = if message.is_some() {
                EventKind::Message
            } else {
                EventKind::Updated
            };
            store.append_event(id, kind, item.version, by)?;
            Ok(item)
        })
    }

    /// Append a masked `run` thread message without changing the item's lane or
    /// writing a decision/checked record (SPEC-board-approve-autorun FR-3234, FR-3235).
    pub fn append_run_message(&self, id: &str, text: &str) -> Result<BoardItem, BoardError> {
        if !valid_item_id(id) {
            return Err(BoardError::Usage("invalid board item id".to_owned()));
        }
        let masked = crate::run_journal::mask_sensitive_tokens(text);
        let msg: String = masked.chars().take(super::TEXT_MAX).collect();
        self.with_lock(|store| {
            let mut item = store.load_item_inner(id)?;
            if item.thread.len() >= THREAD_MAX {
                item.thread.remove(0);
            }
            item.thread.push(ThreadMessage {
                from: "run".to_owned(),
                text: msg,
                at_ms: now_ms(),
            });
            item.version += 1;
            item.updated_ms = now_ms();
            store.save_item_inner(&item)?;
            store.append_event(id, EventKind::Message, item.version, "run")?;
            Ok(item)
        })
    }

    /// Append a session link and log a `session` event; the oldest links are
    /// dropped past [`SESSIONS_MAX`] (SPEC-board-work-session FR-3820, FR-3822).
    pub fn append_session(
        &self,
        id: &str,
        link: SessionLink,
        by: &str,
    ) -> Result<BoardItem, BoardError> {
        self.update_sessions(id, by, |sessions| {
            sessions.push(link);
            true
        })
    }

    /// Replace the `running` link of `link.kind` started at `started_ms` with
    /// its finished form, or append it when it was already dropped (FR-3821).
    pub fn finish_session(
        &self,
        id: &str,
        started_ms: u64,
        link: SessionLink,
        by: &str,
    ) -> Result<BoardItem, BoardError> {
        self.update_sessions(id, by, |sessions| {
            match sessions.iter_mut().rev().find(|existing| {
                existing.kind == link.kind
                    && existing.started_ms == started_ms
                    && existing.status == SessionStatus::Running
            }) {
                Some(existing) => *existing = link,
                None => sessions.push(link),
            }
            true
        })
    }

    /// Edit `sessions` under the store lock. `edit` returns `false` to leave the
    /// item untouched (no version bump, no event).
    pub(crate) fn update_sessions(
        &self,
        id: &str,
        by: &str,
        edit: impl FnOnce(&mut Vec<SessionLink>) -> bool,
    ) -> Result<BoardItem, BoardError> {
        if !valid_item_id(id) {
            return Err(BoardError::Usage("invalid board item id".to_owned()));
        }
        self.with_lock(|store| {
            let mut item = store.load_item_inner(id)?;
            if !edit(&mut item.sessions) {
                return Ok(item);
            }
            let excess = item.sessions.len().saturating_sub(SESSIONS_MAX);
            if excess > 0 {
                item.sessions.drain(..excess);
            }
            item.version += 1;
            item.updated_ms = now_ms();
            store.save_item_inner(&item)?;
            store.append_event(id, EventKind::Session, item.version, by)?;
            Ok(item)
        })
    }

    pub fn say(&self, id: &str, req: SayRequest) -> Result<BoardItem, BoardError> {
        if !valid_item_id(id) {
            return Err(BoardError::Usage("invalid board item id".to_owned()));
        }
        validate_message(&req.text)?;
        let (item, from) = self.with_lock(|store| {
            let mut item = store.load_item_inner(id)?;
            if item.thread.len() >= THREAD_MAX {
                return Err(BoardError::Usage(format!(
                    "thread exceeds {THREAD_MAX} messages"
                )));
            }
            let from = req.from.clone();
            item.thread.push(ThreadMessage {
                from: from.clone(),
                text: req.text,
                at_ms: now_ms(),
            });
            item.version += 1;
            item.updated_ms = now_ms();
            store.save_item_inner(&item)?;
            store.append_event(id, EventKind::Message, item.version, &from)?;
            Ok((item, from))
        })?;
        if from == "owner" {
            super::notify::dispatch_board_event(&self.base, &item, "message", &from);
        }
        Ok(item)
    }

    /// Record the outcome of `board run` (FR-3133, FR-3135). The lane moves to
    /// `done`/`unknown` only while the item is still `in_progress` on the plan
    /// that ran; otherwise only a `run` thread note is added
    /// (SPEC-board-approval-hardening FR-3806).
    pub fn record_run_result(
        &self,
        id: &str,
        run_id: Option<&str>,
        exit_code: i32,
        summary: Option<&str>,
        by: &str,
        plan_hash: &str,
    ) -> Result<BoardItem, BoardError> {
        if !valid_item_id(id) {
            return Err(BoardError::Usage("invalid board item id".to_owned()));
        }
        let item = self.with_lock(|store| {
            let mut item = store.load_item_inner(id)?;
            if let Some(rid) = run_id.map(str::trim).filter(|s| !s.is_empty())
                && !item.run_ids.iter().any(|existing| existing == rid)
            {
                item.run_ids.push(rid.to_owned());
            }
            let run_label = run_id
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("unknown");
            let mut msg = if exit_code == 0 {
                format!("Run {run_label} completed (exit 0).")
            } else {
                let mut m = format!("Run {run_label} failed (exit {exit_code}).");
                if let Some(summary) = summary.map(str::trim).filter(|s| !s.is_empty()) {
                    let masked = crate::run_journal::mask_sensitive_tokens(summary);
                    let clipped: String = masked.chars().take(2000).collect();
                    m.push(' ');
                    m.push_str(&clipped);
                }
                m
            };
            let applied = item.lane == Lane::InProgress
                && item.plan.as_ref().map(|p| p.hash.as_str()) == Some(plan_hash);
            if !applied {
                msg = format!(
                    "Run {run_label} finished (exit {exit_code}) but its result was not applied: the item changed while it ran (lane '{}').",
                    item.lane.as_str()
                );
            }
            if msg.chars().count() > super::TEXT_MAX {
                msg = msg.chars().take(super::TEXT_MAX).collect();
            }
            if item.thread.len() >= THREAD_MAX {
                item.thread.remove(0);
            }
            item.thread.push(ThreadMessage {
                from: "run".to_owned(),
                text: msg,
                at_ms: now_ms(),
            });
            if applied {
                item.lane = if exit_code == 0 {
                    Lane::Done
                } else {
                    Lane::Unknown
                };
                item.checked = None;
            }
            item.version += 1;
            item.updated_ms = now_ms();
            store.save_item_inner(&item)?;
            let kind = if applied {
                EventKind::Lane
            } else {
                EventKind::Message
            };
            store.append_event(id, kind, item.version, by)?;
            Ok(item)
        })?;
        super::notify::dispatch_board_event(&self.base, &item, "run_finished", by);
        Ok(item)
    }

    /// Log `run_started` for an `in_progress` item whose child run is about
    /// to start and dispatch the notification (FR-3850). Appends one event
    /// line and leaves the item unchanged (NFR-3850).
    pub fn record_run_started(&self, id: &str, by: &str) -> Result<BoardItem, BoardError> {
        if !valid_item_id(id) {
            return Err(BoardError::Usage("invalid board item id".to_owned()));
        }
        let (item, detail) = self.with_lock(|store| {
            let item = store.load_item_inner(id)?;
            let detail = EventDetail {
                plan_hash: item.plan.as_ref().map(|p| p.hash.clone()),
                ..EventDetail::default()
            };
            store.append_event_detail(
                id,
                EventKind::RunStarted,
                item.version,
                by,
                detail.clone(),
            )?;
            Ok((item, detail))
        })?;
        super::notify::dispatch_board_event_detail(&self.base, &item, "run_started", by, &detail);
        Ok(item)
    }

    /// Post a `spec_review` thread message and log `spec_review_requested` on
    /// `item_id` when it names an `in_progress` item, else on the item whose
    /// `run_ids` holds `run_id` (FR-3852). Returns `None` and writes nothing
    /// when no item matches. `spec_paths` must already be workspace-relative
    /// and capped.
    pub fn record_spec_review_request(
        &self,
        item_id: Option<&str>,
        run_id: &str,
        review_id: &str,
        spec_paths: &[String],
    ) -> Result<Option<BoardItem>, BoardError> {
        if !crate::run_journal::valid_run_id(run_id) {
            return Err(BoardError::Usage("invalid run id".to_owned()));
        }
        let review_id = crate::run_journal::mask_sensitive_tokens(review_id.trim());
        let review_id: String = review_id.chars().take(SPEC_REVIEW_ID_MAX).collect();
        if review_id.is_empty() {
            return Err(BoardError::Usage("review id is required".to_owned()));
        }
        let spec_paths: Vec<String> = spec_paths
            .iter()
            .map(|p| crate::run_journal::mask_sensitive_tokens(p))
            .collect();
        let found = self.with_lock(|store| {
            let in_progress = item_id
                .filter(|id| valid_item_id(id))
                .and_then(|id| store.load_item_inner(id).ok())
                .filter(|item| item.lane == Lane::InProgress);
            let target = match in_progress {
                Some(item) => Some(item),
                None => store.find_item_by_run_id(run_id)?,
            };
            let Some(mut item) = target else {
                return Ok(None);
            };
            let listed = if spec_paths.is_empty() {
                "no spec paths".to_owned()
            } else {
                spec_paths.join(", ")
            };
            let text: String =
                format!("Spec review {review_id} requested for run {run_id}: {listed}")
                    .chars()
                    .take(super::TEXT_MAX)
                    .collect();
            if item.thread.len() >= THREAD_MAX {
                item.thread.remove(0);
            }
            item.thread.push(ThreadMessage {
                from: SPEC_REVIEW_ACTOR.to_owned(),
                text,
                at_ms: now_ms(),
            });
            item.version += 1;
            item.updated_ms = now_ms();
            store.save_item_inner(&item)?;
            let detail = EventDetail {
                review_id: Some(review_id.clone()),
                spec_paths: spec_paths.clone(),
                ..EventDetail::default()
            };
            store.append_event_detail(
                &item.id,
                EventKind::SpecReviewRequested,
                item.version,
                SPEC_REVIEW_ACTOR,
                detail.clone(),
            )?;
            Ok(Some((item, detail)))
        })?;
        let Some((item, detail)) = found else {
            return Ok(None);
        };
        super::notify::dispatch_board_event_detail(
            &self.base,
            &item,
            "spec_review_requested",
            SPEC_REVIEW_ACTOR,
            &detail,
        );
        Ok(Some(item))
    }

    fn find_item_by_run_id(&self, run_id: &str) -> Result<Option<BoardItem>, BoardError> {
        for entry in
            fs::read_dir(&self.items_dir).map_err(|e| BoardError::Internal(e.to_string()))?
        {
            let entry = entry.map_err(|e| BoardError::Internal(e.to_string()))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs::read(&path).map_err(|e| BoardError::Internal(e.to_string()))?;
            let Ok(item) = serde_json::from_slice::<BoardItem>(&bytes) else {
                continue;
            };
            if item.run_ids.iter().any(|existing| existing == run_id) {
                return Ok(Some(item));
            }
        }
        Ok(None)
    }

    pub fn wait(&self, id: &str, filter: WaitFilter) -> Result<WaitOutcome, BoardError> {
        if !valid_item_id(id) {
            return Err(BoardError::Usage("invalid board item id".to_owned()));
        }
        if self.load_item_inner(id).is_err() {
            return Err(BoardError::NotFound("board item not found".to_owned()));
        }
        let deadline = Instant::now() + Duration::from_secs(filter.timeout_secs);
        let mut file_offset = 0u64;
        let mut history_scanned = false;
        loop {
            let read_from = if history_scanned { file_offset } else { 0 };
            if let Some(event) = self.poll_events(id, &filter, read_from, &mut file_offset)? {
                let item = self.get(id)?;
                return Ok(WaitOutcome::Ready {
                    item: Box::new(item),
                    event,
                });
            }
            history_scanned = true;
            if Instant::now() >= deadline {
                let version = self
                    .get(id)
                    .map(|i| i.version)
                    .unwrap_or(filter.since_version);
                return Ok(WaitOutcome::TimedOut { version });
            }
            thread::sleep(Duration::from_millis(500));
        }
    }

    fn poll_events(
        &self,
        id: &str,
        filter: &WaitFilter,
        read_from: u64,
        file_offset: &mut u64,
    ) -> Result<Option<WaitEvent>, BoardError> {
        if !self.events_path.exists() {
            return Ok(None);
        }
        let len = fs::metadata(&self.events_path)
            .map_err(|e| BoardError::Internal(e.to_string()))?
            .len();
        if len <= read_from {
            return Ok(None);
        }
        let mut file =
            File::open(&self.events_path).map_err(|e| BoardError::Internal(e.to_string()))?;
        file.seek(SeekFrom::Start(read_from))
            .map_err(|e| BoardError::Internal(e.to_string()))?;
        let byte_count = (len - read_from) as usize;
        let mut buf = vec![0u8; byte_count];
        file.read_exact(&mut buf)
            .map_err(|e| BoardError::Internal(e.to_string()))?;
        let Some(last_nl) = buf.iter().rposition(|&b| b == b'\n') else {
            return Ok(None);
        };
        *file_offset = read_from + (last_nl as u64) + 1;
        let text = String::from_utf8_lossy(&buf[..=last_nl]);
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(event) = serde_json::from_str::<BoardEvent>(line) else {
                continue;
            };
            if let Some(trigger) = wait_trigger_for_event(&event, id, filter) {
                return Ok(Some(trigger));
            }
        }
        Ok(None)
    }
}

fn actor_wakes_waiter(by: &str) -> bool {
    by == "owner" || by == "planner"
}

fn wait_trigger_for_event(event: &BoardEvent, id: &str, filter: &WaitFilter) -> Option<WaitEvent> {
    if event.item_id != id || event.version <= filter.since_version {
        return None;
    }
    if !event_wakes_waiter(event, filter) {
        return None;
    }
    Some(WaitEvent {
        seq: event.seq,
        item_id: event.item_id.clone(),
        kind: event.kind.as_str(),
        by: event.by.clone(),
        version: event.version,
        detail: event.detail.clone(),
    })
}

fn wait_trigger_for_event_any(event: &BoardEvent, filter: &WaitFilter) -> Option<WaitEventAny> {
    if !event_wakes_waiter(event, filter) {
        return None;
    }
    Some(WaitEventAny {
        seq: event.seq,
        item_id: event.item_id.clone(),
        kind: event.kind.as_str(),
        by: event.by.clone(),
        version: event.version,
        detail: event.detail.clone(),
    })
}

fn event_wakes_waiter(event: &BoardEvent, filter: &WaitFilter) -> bool {
    event.by != filter.waiter
        && (actor_wakes_waiter(&event.by) || event.kind.is_observability())
        && event.kind.matches_wait(filter.wait_for)
}

/// Record that the item's current plan revision is announced as
/// `awaiting_approval` when the item enters `needs_decision` with a plan or a
/// new revision lands there, at most once per revision (FR-3851).
fn mark_awaiting_approval(
    item: &mut BoardItem,
    prev_lane: Option<Lane>,
    prev_revision: Option<u32>,
) -> Option<EventDetail> {
    let plan = item.plan.as_ref()?;
    if item.lane != Lane::NeedsDecision || item.awaiting_approval_revision == Some(plan.revision) {
        return None;
    }
    if prev_lane == Some(Lane::NeedsDecision) && prev_revision == Some(plan.revision) {
        return None;
    }
    let detail = EventDetail {
        plan_hash: Some(plan.hash.clone()),
        plan_revision: Some(plan.revision),
        ..EventDetail::default()
    };
    item.awaiting_approval_revision = Some(plan.revision);
    Some(detail)
}

fn enforce_sensitive_lane_move(by: &str, lane: Lane) -> Result<(), BoardError> {
    if matches!(lane, Lane::Approved | Lane::Rejected) {
        return Err(BoardError::Forbidden(
            "cannot move item into approved or rejected (use board decide)".to_owned(),
        ));
    }
    if lane == Lane::Auto && by != "owner" {
        return Err(BoardError::Forbidden(
            "only the owner may move items into auto".to_owned(),
        ));
    }
    Ok(())
}

/// FR-3803: a non-owner may move an item into `in_progress` only when it is in
/// the `auto` lane or already approved by the owner for its current plan, which
/// is exactly what `board run` checks before it moves the item.
fn enforce_in_progress_move(by: &str, lane: Lane, item: &BoardItem) -> Result<(), BoardError> {
    if lane != Lane::InProgress || by == "owner" || item.lane == Lane::Auto {
        return Ok(());
    }
    let approved_for_plan = item.decision.as_ref().is_some_and(|d| {
        d.verdict == Verdict::Approve
            && d.by == "owner"
            && d.plan_hash.as_deref() == item.plan.as_ref().map(|p| p.hash.as_str())
    });
    if approved_for_plan {
        return Ok(());
    }
    Err(BoardError::Forbidden(
        "only an approved or auto lane item can move into in_progress; use `board run`".to_owned(),
    ))
}

fn workspace_from_path(path: Option<&Path>) -> Result<WorkspaceRef, BoardError> {
    let Some(path) = path else {
        return Ok(WorkspaceRef::default());
    };
    let canonical = normalize_workspace_path(path)
        .map_err(|e| BoardError::Usage(format!("invalid workspace: {e}")))?;
    crate::workspace::check_workspace_denied(std::path::Path::new(&canonical))
        .map_err(|e| BoardError::Usage(e.to_string()))?;
    let key = repo_key(path).map_err(|e| BoardError::Internal(e.to_string()))?;
    Ok(WorkspaceRef {
        path: Some(canonical),
        repo_key: Some(key),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::plan::{PendingPlanFile, write_pending_plan};
    use crate::board::{Kind, Verdict};
    use crate::graph::{PlanStep, PlanTask, TaskSize};
    use std::sync::Arc;
    use std::thread;
    use tempfile::tempdir;

    fn test_store() -> (tempfile::TempDir, BoardStore, PathBuf) {
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        let board = state.join("board");
        ensure_dir(&board).unwrap();
        let store = BoardStore::open(board).unwrap();
        (dir, store, state)
    }

    #[test]
    fn tc_3100_item_roundtrip_atomic_write_and_permissions() {
        let (_dir, store, state) = test_store();
        let ref_url = "https://example.com/issues/1";
        let item = store
            .add(AddItemRequest {
                title: "hello".to_owned(),
                summary: "sum".to_owned(),
                kind: Kind::Debug,
                grade: None,
                lane: None,
                source_bot: Some("coord".to_owned()),
                source_channel: None,
                source_ref: Some(ref_url.to_owned()),
                workspace: None,
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:test".to_owned(),
                state_root: state,
            })
            .unwrap();
        let loaded = store.get(&item.id).unwrap();
        assert_eq!(loaded.title, item.title);
        let path = store.item_path(&item.id).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            let dir_mode = fs::metadata(store.base()).unwrap().permissions().mode() & 0o777;
            assert_eq!(dir_mode, 0o700);
            let items_mode = fs::metadata(store.base().join("items"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(items_mode, 0o700);
            let events_mode = fs::metadata(&store.events_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(events_mode, 0o600);
        }
        let json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(json["source"]["ref"], ref_url);
        assert!(json["source"].get("ref_url").is_none());
        let mut with_unknown = json.clone();
        with_unknown["future_field"] = serde_json::json!({"new": true});
        atomic_write(&path, &serde_json::to_vec_pretty(&with_unknown).unwrap()).unwrap();
        let reloaded = store.get(&item.id).unwrap();
        assert_eq!(reloaded.title, item.title);
        assert_eq!(reloaded.source.ref_url.as_deref(), Some(ref_url));
        let names: Vec<_> = fs::read_dir(store.base().join("items"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("tmp"))
            .collect();
        assert!(names.is_empty());
    }

    #[test]
    fn tc_3101_events_seq_monotonic_under_concurrent_writers() {
        let (_dir, store, state) = test_store();
        let store = Arc::new(store);
        let item = store
            .add(AddItemRequest {
                title: "base".to_owned(),
                summary: String::new(),
                kind: Kind::Debug,
                grade: None,
                lane: None,
                source_bot: None,
                source_channel: None,
                source_ref: None,
                workspace: None,
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:test".to_owned(),
                state_root: state,
            })
            .unwrap();
        let mut handles = Vec::new();
        for i in 0..8 {
            let store = Arc::clone(&store);
            let id = item.id.clone();
            handles.push(thread::spawn(move || {
                store
                    .update(
                        &id,
                        UpdateItemRequest {
                            title: Some(format!("title-{i}")),
                            summary: None,
                            recommendation: None,
                            options: None,
                            grade: None,
                            kind: None,
                            run_id: None,
                            if_version: None,
                            by: format!("bot:{i}"),
                        },
                    )
                    .unwrap();
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        let text = fs::read_to_string(store.events_path.clone()).unwrap();
        let mut seqs = Vec::new();
        for line in text.lines() {
            let event: BoardEvent = serde_json::from_str(line).unwrap();
            if event.item_id == item.id {
                seqs.push(event.seq);
            }
        }
        seqs.sort_unstable();
        assert_eq!(seqs.len(), 9);
        for (expected, got) in (1u64..=9).zip(seqs.iter().copied()) {
            assert_eq!(expected, got);
        }
    }

    #[test]
    fn tc_3108_decide_verbs_lanes_stale_hash_conflict_and_bot_approve_forbidden() {
        let (_dir, store, state) = test_store();
        let item = store
            .add(AddItemRequest {
                title: "f".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: Some(Lane::NeedsDecision),
                source_bot: None,
                source_channel: None,
                source_ref: None,
                workspace: None,
                recommendation: None,
                options: vec!["a".to_owned()],
                plan_hash: None,
                key: None,
                by: "bot:test".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        let plan = PendingPlanFile {
            prompt: "p".to_owned(),
            tasks: vec![PlanTask {
                id: "task-1".to_owned(),
                title: "t".to_owned(),
                done_check: "d".to_owned(),
                areas: vec![],
                size: TaskSize::Small,
                depends_on: vec![],
                implicit: false,
            }],
            steps: vec![PlanStep {
                id: "s1".to_owned(),
                role: "implement".to_owned(),
                job: "j".to_owned(),
                runner: "cursor".to_owned(),
                model: None,
                depends_on: vec![],
                sensitive: None,
                task: Some("task-1".to_owned()),
                ..Default::default()
            }],
            dynamic_cap: None,
        };
        let hash = write_pending_plan(&state, &plan).unwrap();
        let item = store
            .attach_plan(&item.id, &hash, &state, "bot:test")
            .unwrap();
        let stale = store
            .decide(
                &item.id,
                DecideRequest {
                    verdict: Verdict::Approve,
                    text: None,
                    option: None,
                    plan_hash: Some("0000000000000000".to_owned()),
                    by: "owner".to_owned(),
                },
            )
            .unwrap_err();
        assert_eq!(stale.exit_code(), crate::agent_api::exit::CONFLICT);
        let missing_hash = store
            .decide(
                &item.id,
                DecideRequest {
                    verdict: Verdict::Approve,
                    text: None,
                    option: None,
                    plan_hash: None,
                    by: "owner".to_owned(),
                },
            )
            .unwrap_err();
        assert_eq!(missing_hash.exit_code(), crate::agent_api::exit::CONFLICT);
        let bot_forbidden = store
            .decide(
                &item.id,
                DecideRequest {
                    verdict: Verdict::Approve,
                    text: None,
                    option: None,
                    plan_hash: None,
                    by: "bot:x".to_owned(),
                },
            )
            .unwrap_err();
        assert_eq!(bot_forbidden.exit_code(), crate::agent_api::exit::FORBIDDEN);
        let approved = store
            .decide(
                &item.id,
                DecideRequest {
                    verdict: Verdict::Approve,
                    text: None,
                    option: None,
                    plan_hash: Some(hash),
                    by: "owner".to_owned(),
                },
            )
            .unwrap();
        assert_eq!(approved.lane, Lane::Approved);
        assert_eq!(
            approved.decision.as_ref().unwrap().verdict,
            Verdict::Approve
        );

        let rejected = store
            .decide(
                &item.id,
                DecideRequest {
                    verdict: Verdict::Reject,
                    text: None,
                    option: None,
                    plan_hash: None,
                    by: "owner".to_owned(),
                },
            )
            .unwrap();
        assert_eq!(rejected.lane, Lane::Rejected);
        assert_eq!(rejected.decision.as_ref().unwrap().verdict, Verdict::Reject);

        let changes = store
            .decide(
                &item.id,
                DecideRequest {
                    verdict: Verdict::RequestChanges,
                    text: Some("please fix".to_owned()),
                    option: None,
                    plan_hash: None,
                    by: "owner".to_owned(),
                },
            )
            .unwrap();
        assert_eq!(changes.lane, Lane::NeedsDecision);
        assert_eq!(
            changes.decision.as_ref().unwrap().verdict,
            Verdict::RequestChanges
        );
        assert_eq!(
            changes.decision.as_ref().unwrap().text.as_deref(),
            Some("please fix")
        );

        let answer = store
            .decide(
                &item.id,
                DecideRequest {
                    verdict: Verdict::Answer,
                    text: None,
                    option: Some(0),
                    plan_hash: None,
                    by: "owner".to_owned(),
                },
            )
            .unwrap();
        assert_eq!(answer.lane, Lane::NeedsDecision);
        assert_eq!(answer.decision.as_ref().unwrap().verdict, Verdict::Answer);
        assert_eq!(answer.decision.as_ref().unwrap().option, Some(0));

        let planner = store
            .decide(
                &item.id,
                DecideRequest {
                    verdict: Verdict::Reject,
                    text: None,
                    option: None,
                    plan_hash: None,
                    by: "planner".to_owned(),
                },
            )
            .unwrap_err();
        assert_eq!(planner.exit_code(), crate::agent_api::exit::FORBIDDEN);

        let bot_move = store
            .move_lane(&item.id, Lane::Approved, "bot:x", None)
            .unwrap_err();
        assert_eq!(bot_move.exit_code(), crate::agent_api::exit::FORBIDDEN);

        let owner_move_approved = store
            .move_lane(&item.id, Lane::Approved, "owner", None)
            .unwrap_err();
        assert_eq!(
            owner_move_approved.exit_code(),
            crate::agent_api::exit::FORBIDDEN
        );

        let auto = store
            .add(AddItemRequest {
                title: "dbg".to_owned(),
                summary: String::new(),
                kind: Kind::Debug,
                grade: None,
                lane: None,
                source_bot: None,
                source_channel: None,
                source_ref: None,
                workspace: None,
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:test".to_owned(),
                state_root: state,
            })
            .unwrap();
        assert_eq!(auto.lane, Lane::Auto);
        let auto_bot_err = store
            .decide(
                &auto.id,
                DecideRequest {
                    verdict: Verdict::Approve,
                    text: None,
                    option: None,
                    plan_hash: None,
                    by: "bot:x".to_owned(),
                },
            )
            .unwrap_err();
        assert_eq!(auto_bot_err.exit_code(), crate::agent_api::exit::FORBIDDEN);

        use crate::board::{BoardCaller, resolve_actor};
        assert!(resolve_actor(Some("owner"), BoardCaller::CliNonTty).is_err());
        assert!(resolve_actor(Some("owner"), BoardCaller::Dashboard).is_ok());
    }

    #[test]
    fn tc_3109_say_appends_thread_message() {
        let (_dir, store, state) = test_store();
        let item = store
            .add(AddItemRequest {
                title: "chat".to_owned(),
                summary: String::new(),
                kind: Kind::Question,
                grade: None,
                lane: None,
                source_bot: None,
                source_channel: None,
                source_ref: None,
                workspace: None,
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:test".to_owned(),
                state_root: state,
            })
            .unwrap();
        let updated = store
            .say(
                &item.id,
                SayRequest {
                    text: "hello".to_owned(),
                    from: "owner".to_owned(),
                },
            )
            .unwrap();
        assert_eq!(updated.thread.len(), 1);
        assert_eq!(updated.thread[0].text, "hello");
    }

    #[test]
    fn tc_3110_move_update_and_if_version_conflict() {
        let (_dir, store, state) = test_store();
        let item = store
            .add(AddItemRequest {
                title: "u".to_owned(),
                summary: String::new(),
                kind: Kind::Debug,
                grade: None,
                lane: None,
                source_bot: None,
                source_channel: None,
                source_ref: None,
                workspace: None,
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:test".to_owned(),
                state_root: state,
            })
            .unwrap();
        let moved = store
            .move_lane(&item.id, Lane::InProgress, "owner", None)
            .unwrap();
        assert_eq!(moved.lane, Lane::InProgress);
        assert!(
            store
                .update(
                    &item.id,
                    UpdateItemRequest {
                        title: Some("new".to_owned()),
                        summary: None,
                        recommendation: None,
                        options: None,
                        grade: None,
                        kind: None,
                        run_id: None,
                        if_version: Some(1),
                        by: "owner".to_owned(),
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn tc_3111_list_filters_and_hides_old_closed_items() {
        let (_dir, store, state) = test_store();
        let item = store
            .add(AddItemRequest {
                title: "old".to_owned(),
                summary: String::new(),
                kind: Kind::Debug,
                grade: None,
                lane: Some(Lane::Done),
                source_bot: None,
                source_channel: None,
                source_ref: None,
                workspace: None,
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:test".to_owned(),
                state_root: state,
            })
            .unwrap();
        let mut old = store.get(&item.id).unwrap();
        old.updated_ms = 0;
        store.save_item_inner(&old).unwrap();
        let hidden = store.list(&ListFilter::default()).unwrap();
        assert!(hidden.is_empty());
        let all = store
            .list(&ListFilter {
                include_all: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn tc_3112_wait_returns_on_owner_decision_and_times_out() {
        let (_dir, store, state) = test_store();
        let item = store
            .add(AddItemRequest {
                title: "wait".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: Some(Lane::NeedsDecision),
                source_bot: None,
                source_channel: None,
                source_ref: None,
                workspace: None,
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:test".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        let store2 = BoardStore::open(store.base().to_path_buf()).unwrap();
        let id = item.id.clone();
        let handle = thread::spawn(move || {
            store2
                .wait(
                    &id,
                    WaitFilter {
                        since_version: item.version,
                        timeout_secs: 3,
                        wait_for: WaitFor::Any,
                        waiter: "bot:coordinator".to_owned(),
                    },
                )
                .unwrap()
        });
        thread::sleep(Duration::from_millis(300));
        store
            .decide(
                &item.id,
                DecideRequest {
                    verdict: Verdict::Reject,
                    text: None,
                    option: None,
                    plan_hash: None,
                    by: "owner".to_owned(),
                },
            )
            .unwrap();
        match handle.join().unwrap() {
            WaitOutcome::Ready { item: got, event } => {
                assert_eq!(got.lane, Lane::Rejected);
                assert_eq!(event.kind, "decision");
                assert_eq!(event.by, "owner");
            }
            WaitOutcome::TimedOut { .. } => panic!("expected ready"),
        }
        store
            .decide(
                &item.id,
                DecideRequest {
                    verdict: Verdict::Answer,
                    text: Some("done".to_owned()),
                    option: None,
                    plan_hash: None,
                    by: "owner".to_owned(),
                },
            )
            .unwrap();
        let immediate = store
            .wait(
                &item.id,
                WaitFilter {
                    since_version: 1,
                    timeout_secs: 2,
                    wait_for: WaitFor::Decision,
                    waiter: "bot:coordinator".to_owned(),
                },
            )
            .unwrap();
        match immediate {
            WaitOutcome::Ready { event, .. } => {
                assert_eq!(event.kind, "decision");
                assert_eq!(event.by, "owner");
            }
            WaitOutcome::TimedOut { .. } => panic!("expected immediate ready from history scan"),
        }
        let events_path = store.events_path.clone();
        let partial = format!(
            r#"{{"seq":90001,"item_id":"{}","kind":"message","version":9001,"at_ms":1,"by":"planner""#,
            item.id
        );
        let mut append = OpenOptions::new();
        append.append(true);
        append
            .open(&events_path)
            .unwrap()
            .write_all(partial.as_bytes())
            .unwrap();
        let stuck = store
            .wait(
                &item.id,
                WaitFilter {
                    since_version: 9000,
                    timeout_secs: 1,
                    wait_for: WaitFor::Message,
                    waiter: "bot:coordinator".to_owned(),
                },
            )
            .unwrap();
        assert!(matches!(stuck, WaitOutcome::TimedOut { .. }));
        append
            .open(&events_path)
            .unwrap()
            .write_all(b"}\n")
            .unwrap();
        match store
            .wait(
                &item.id,
                WaitFilter {
                    since_version: 9000,
                    timeout_secs: 2,
                    wait_for: WaitFor::Message,
                    waiter: "bot:coordinator".to_owned(),
                },
            )
            .unwrap()
        {
            WaitOutcome::Ready { event, .. } => {
                assert_eq!(event.kind, "message");
                assert_eq!(event.by, "planner");
                assert_eq!(event.version, 9001);
            }
            WaitOutcome::TimedOut { .. } => panic!("expected ready after partial line completed"),
        }
        let timeout = store
            .wait(
                &item.id,
                WaitFilter {
                    since_version: 99,
                    timeout_secs: 1,
                    wait_for: WaitFor::Decision,
                    waiter: "bot:coordinator".to_owned(),
                },
            )
            .unwrap();
        assert!(matches!(timeout, WaitOutcome::TimedOut { .. }));

        // Events from the waiter itself are ignored.
        let base_version = store.get(&item.id).unwrap().version;
        let id = item.id.clone();
        let store2 = BoardStore::open(store.base().to_path_buf()).unwrap();
        let handle = thread::spawn(move || {
            store2
                .wait(
                    &id,
                    WaitFilter {
                        since_version: base_version,
                        timeout_secs: 3,
                        wait_for: WaitFor::Message,
                        waiter: "planner".to_owned(),
                    },
                )
                .unwrap()
        });
        thread::sleep(Duration::from_millis(300));
        store
            .say(
                &item.id,
                SayRequest {
                    text: "self message".to_owned(),
                    from: "planner".to_owned(),
                },
            )
            .unwrap();
        thread::sleep(Duration::from_millis(300));
        store
            .say(
                &item.id,
                SayRequest {
                    text: "owner message".to_owned(),
                    from: "owner".to_owned(),
                },
            )
            .unwrap();
        match handle.join().unwrap() {
            WaitOutcome::Ready { item: got, event } => {
                assert_eq!(event.kind, "message");
                assert_eq!(event.by, "owner");
                assert!(got.thread.iter().any(|m| m.text == "owner message"));
            }
            WaitOutcome::TimedOut { .. } => panic!("self events must not wake the waiter"),
        }

        // A missing item fails with not-found immediately, without waiting.
        let started = Instant::now();
        let missing = store
            .wait(
                "b000000000000",
                WaitFilter {
                    since_version: 0,
                    timeout_secs: 5,
                    wait_for: WaitFor::Any,
                    waiter: "bot:coordinator".to_owned(),
                },
            )
            .unwrap_err();
        assert_eq!(missing.exit_code(), crate::agent_api::exit::NOT_FOUND);
        assert!(started.elapsed() < Duration::from_secs(2));

        // `wait --any` returns the next owner event on another item.
        let other = store
            .add(AddItemRequest {
                title: "other".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: Some(Lane::NeedsDecision),
                source_bot: None,
                source_channel: None,
                source_ref: None,
                workspace: None,
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:test".to_owned(),
                state_root: state,
            })
            .unwrap();
        let since_seq = store
            .list_events(0)
            .unwrap()
            .last()
            .map(|event| event.seq)
            .unwrap_or(0);
        let store2 = BoardStore::open(store.base().to_path_buf()).unwrap();
        let handle = thread::spawn(move || {
            store2
                .wait_any(
                    since_seq,
                    WaitFilter {
                        since_version: 0,
                        timeout_secs: 3,
                        wait_for: WaitFor::Any,
                        waiter: "bot:coordinator".to_owned(),
                    },
                )
                .unwrap()
        });
        thread::sleep(Duration::from_millis(300));
        store
            .say(
                &other.id,
                SayRequest {
                    text: "any event".to_owned(),
                    from: "owner".to_owned(),
                },
            )
            .unwrap();
        match handle.join().unwrap() {
            Some((event, got)) => {
                assert_eq!(event.item_id, other.id);
                assert_eq!(event.kind, "message");
                assert_eq!(event.by, "owner");
                assert_eq!(got.id, other.id);
            }
            None => panic!("wait_any should return the owner event on another item"),
        }
    }

    fn pending_plan(prompt: &str) -> PendingPlanFile {
        PendingPlanFile {
            prompt: prompt.to_owned(),
            tasks: vec![PlanTask {
                id: "task-1".to_owned(),
                title: "t".to_owned(),
                done_check: "d".to_owned(),
                areas: vec![],
                size: TaskSize::Small,
                depends_on: vec![],
                implicit: false,
            }],
            steps: vec![PlanStep {
                id: "s1".to_owned(),
                role: "implement".to_owned(),
                job: "j".to_owned(),
                runner: "cursor".to_owned(),
                model: None,
                depends_on: vec![],
                sensitive: None,
                task: Some("task-1".to_owned()),
                ..Default::default()
            }],
            dynamic_cap: None,
        }
    }

    fn add_request(
        title: &str,
        lane: Option<Lane>,
        plan_hash: Option<String>,
        state: &Path,
    ) -> AddItemRequest {
        AddItemRequest {
            title: title.to_owned(),
            summary: String::new(),
            kind: Kind::Feature,
            grade: None,
            lane,
            source_bot: None,
            source_channel: None,
            source_ref: None,
            workspace: None,
            recommendation: None,
            options: vec![],
            plan_hash,
            key: None,
            by: "bot:test".to_owned(),
            state_root: state.to_path_buf(),
        }
    }

    fn awaiting_events(store: &BoardStore, item_id: &str) -> Vec<WaitEventAny> {
        store
            .list_events(0)
            .unwrap()
            .into_iter()
            .filter(|e| e.item_id == item_id && e.kind == "awaiting_approval")
            .collect()
    }

    // TC-3851 / FR-3851: `awaiting_approval` is emitted once per plan
    // revision while the item is in needs_decision, never without a plan.
    #[test]
    fn tc_3851_awaiting_approval_event() {
        let (_dir, store, state) = test_store();

        // Created in needs_decision with a plan → one event.
        let hash1 = write_pending_plan(&state, &pending_plan("first")).unwrap();
        let item = store
            .add(add_request(
                "with plan",
                Some(Lane::NeedsDecision),
                Some(hash1.clone()),
                &state,
            ))
            .unwrap();
        let events = awaiting_events(&store, &item.id);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].detail.plan_hash.as_deref(), Some(hash1.as_str()));
        let rev1 = item.plan.as_ref().unwrap().revision;
        assert_eq!(events[0].detail.plan_revision, Some(rev1));
        assert_eq!(events[0].version, item.version);
        assert_eq!(item.awaiting_approval_revision, Some(rev1));

        // Same revision: re-entering needs_decision does not repeat it.
        store
            .move_lane(&item.id, Lane::NeedsDecision, "bot:test", None)
            .unwrap();
        store
            .move_lane(&item.id, Lane::Unknown, "bot:test", None)
            .unwrap();
        store
            .move_lane(&item.id, Lane::NeedsDecision, "bot:test", None)
            .unwrap();
        assert_eq!(awaiting_events(&store, &item.id).len(), 1);

        // A new revision while in needs_decision → one more event.
        let hash2 = write_pending_plan(&state, &pending_plan("second")).unwrap();
        let item = store
            .attach_plan(&item.id, &hash2, &state, "bot:test")
            .unwrap();
        let events = awaiting_events(&store, &item.id);
        assert_eq!(events.len(), 2, "{events:?}");
        let rev2 = item.plan.as_ref().unwrap().revision;
        assert!(rev2 > rev1);
        assert_eq!(events[1].detail.plan_hash.as_deref(), Some(hash2.as_str()));
        assert_eq!(events[1].detail.plan_revision, Some(rev2));
        assert_eq!(events[1].by, "bot:test");

        // A plan attached to an item outside needs_decision moves it there
        // and announces it.
        let later = store
            .add(add_request(
                "later plan",
                Some(Lane::InProgress),
                None,
                &state,
            ))
            .unwrap();
        assert!(awaiting_events(&store, &later.id).is_empty());
        let hash3 = write_pending_plan(&state, &pending_plan("third")).unwrap();
        let later = store
            .attach_plan(&later.id, &hash3, &state, "bot:test")
            .unwrap();
        assert_eq!(later.lane, Lane::NeedsDecision);
        assert_eq!(awaiting_events(&store, &later.id).len(), 1);

        // Without a plan only the existing events are written.
        let plain = store
            .add(add_request(
                "no plan",
                Some(Lane::NeedsDecision),
                None,
                &state,
            ))
            .unwrap();
        store
            .move_lane(&plain.id, Lane::Unknown, "bot:test", None)
            .unwrap();
        store
            .move_lane(&plain.id, Lane::NeedsDecision, "bot:test", None)
            .unwrap();
        assert!(awaiting_events(&store, &plain.id).is_empty());
        assert_eq!(
            store.get(&plain.id).unwrap().awaiting_approval_revision,
            None
        );
    }

    // TC-3854 / FR-3854: `board wait --any` wakes on the new kinds; the
    // decision filter does not; `board events` lists them with detail.
    #[test]
    fn tc_3854_wait_any_sees_new_kinds() {
        let (_dir, store, state) = test_store();
        let hash = write_pending_plan(&state, &pending_plan("run me")).unwrap();
        let item = store
            .add(add_request(
                "run",
                Some(Lane::NeedsDecision),
                Some(hash.clone()),
                &state,
            ))
            .unwrap();
        let since_seq = store.list_events(0).unwrap().last().map_or(0, |e| e.seq);

        let spawn_waiter = |wait_for: WaitFor, timeout_secs: u64| {
            let waiter_store = BoardStore::open(store.base().to_path_buf()).unwrap();
            thread::spawn(move || {
                waiter_store
                    .wait_any(
                        since_seq,
                        WaitFilter {
                            since_version: 0,
                            timeout_secs,
                            wait_for,
                            waiter: "bot:coordinator".to_owned(),
                        },
                    )
                    .unwrap()
            })
        };
        let any = spawn_waiter(WaitFor::Any, 3);
        let decision = spawn_waiter(WaitFor::Decision, 1);
        thread::sleep(Duration::from_millis(300));
        store.record_run_started(&item.id, "bot:runner").unwrap();

        match any.join().unwrap() {
            Some((event, got)) => {
                assert_eq!(event.kind, "run_started");
                assert_eq!(event.item_id, item.id);
                assert_eq!(event.by, "bot:runner");
                assert_eq!(event.detail.plan_hash.as_deref(), Some(hash.as_str()));
                assert_eq!(got.id, item.id);
            }
            None => panic!("wait --any must wake on run_started"),
        }
        assert!(
            decision.join().unwrap().is_none(),
            "a decision waiter must not wake on run_started"
        );

        // The waiter's own observability writes are still filtered out.
        let own = BoardStore::open(store.base().to_path_buf())
            .unwrap()
            .wait_any(
                since_seq,
                WaitFilter {
                    since_version: 0,
                    timeout_secs: 1,
                    wait_for: WaitFor::Any,
                    waiter: "bot:runner".to_owned(),
                },
            )
            .unwrap();
        assert!(own.is_none(), "self events must not wake the waiter");

        let listed = store.list_events(since_seq).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].kind, "run_started");
        assert_eq!(listed[0].detail.plan_hash.as_deref(), Some(hash.as_str()));
        let unchanged = store.get(&item.id).unwrap();
        assert_eq!(unchanged.version, item.version);
    }

    #[test]
    fn tc_3116_list_500_items_under_budget() {
        let (_dir, store, state) = test_store();
        for i in 0..500 {
            store
                .add(AddItemRequest {
                    title: format!("item-{i}"),
                    summary: String::new(),
                    kind: Kind::Debug,
                    grade: None,
                    lane: None,
                    source_bot: None,
                    source_channel: None,
                    source_ref: None,
                    workspace: None,
                    recommendation: None,
                    options: vec![],
                    plan_hash: None,
                    key: None,
                    by: "bot:test".to_owned(),
                    state_root: state.clone(),
                })
                .unwrap();
        }
        let start = Instant::now();
        let list = store.list(&ListFilter::default()).unwrap();
        let elapsed = start.elapsed();
        assert_eq!(list.len(), 500);
        assert!(
            elapsed.as_millis() < 200,
            "list took {} ms",
            elapsed.as_millis()
        );
    }

    // TC-3137 / FR-3137: board pending lists decide, failed, check (oldest first, excluding >30d
    // done/unknown), and board check records checked + event and removes from pending.
    #[test]
    fn tc_3137_board_pending_lists_decide_failed_check_and_check_clears() {
        let (_dir, store, state) = test_store();
        let ws1 = _dir.path().join("ws-one");
        let ws2 = _dir.path().join("ws-two");
        fs::create_dir_all(&ws1).unwrap();
        fs::create_dir_all(&ws2).unwrap();

        let make = |title: &str, lane: Lane, ws: &Path| {
            store
                .add(AddItemRequest {
                    title: title.to_owned(),
                    summary: String::new(),
                    kind: Kind::Feature,
                    grade: None,
                    lane: Some(lane),
                    source_bot: None,
                    source_channel: None,
                    source_ref: None,
                    workspace: Some(ws.to_path_buf()),
                    recommendation: None,
                    options: vec![],
                    plan_hash: None,
                    key: None,
                    by: "owner".to_owned(),
                    state_root: state.clone(),
                })
                .unwrap()
        };

        let i_decide = make("needs decision", Lane::NeedsDecision, &ws1);
        let i_failed = make("failed run", Lane::Unknown, &ws2);
        let i_done = make("done unchecked", Lane::Done, &ws1);
        let i_old_done = make("old done", Lane::Done, &ws1);
        let _i_auto = make("auto item", Lane::Auto, &ws1);

        // Set explicit updated_ms to verify oldest-first ordering and 30d cutoff.
        let now = now_ms();
        for (id, updated) in [
            (&i_done.id, now - 3 * 3_600_000),
            (&i_failed.id, now - 2 * 3_600_000),
            (&i_decide.id, now - 600_000),
            (&i_old_done.id, now - 35 * 86_400 * 1000),
        ] {
            let mut it = store.get(id).unwrap();
            it.updated_ms = updated;
            store.save_item_inner(&it).unwrap();
        }

        let all_pending = store.pending(None).unwrap();
        assert_eq!(all_pending.count, 3);
        let got: Vec<(&str, &str)> = all_pending
            .items
            .iter()
            .map(|p| (p.id.as_str(), p.reason.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (i_done.id.as_str(), "check"),
                (i_failed.id.as_str(), "failed"),
                (i_decide.id.as_str(), "decide"),
            ]
        );

        let ws1_pending = store.pending(Some(&ws1)).unwrap();
        assert_eq!(ws1_pending.count, 2);

        // Check on needs_decision -> Conflict (exit 4).
        let conflict = store.check(&i_decide.id, None, "owner").unwrap_err();
        assert_eq!(conflict.exit_code(), crate::agent_api::exit::CONFLICT);

        // Bot cannot check -> Forbidden (exit 5).
        let forbidden = store.check(&i_done.id, None, "bot:coord").unwrap_err();
        assert_eq!(forbidden.exit_code(), crate::agent_api::exit::FORBIDDEN);

        // Owner checks done item -> records checked, appends checked event, removes from pending.
        let checked_done = store
            .check(&i_done.id, Some("looks good".to_owned()), "owner")
            .unwrap();
        assert_eq!(checked_done.checked.as_ref().unwrap().by, "owner");
        assert_eq!(
            checked_done.checked.as_ref().unwrap().text.as_deref(),
            Some("looks good")
        );
        let events = store.list_events(0).unwrap();
        assert!(
            events
                .iter()
                .any(|e| e.item_id == i_done.id && e.kind == "checked" && e.by == "owner")
        );

        let after = store.pending(None).unwrap();
        assert_eq!(after.count, 2);
        assert!(!after.items.iter().any(|p| p.id == i_done.id));

        let reopened = store
            .move_lane(&i_done.id, Lane::NeedsDecision, "owner", None)
            .unwrap();
        assert!(reopened.checked.is_none());
        let re_done = store
            .move_lane(&i_done.id, Lane::Done, "bot:coord", None)
            .unwrap();
        assert!(re_done.checked.is_none());
    }

    // TC-3230 / FR-3230: owner decision appends decision event before return so blocked wait
    // and wait --any wake within <= 1 s (assert <= 1.5 s for scheduler jitter).
    #[test]
    fn tc_3230_owner_decision_wakes_blocked_wait_within_one_second() {
        let (_dir, store, state) = test_store();
        let item = store
            .add(AddItemRequest {
                title: "wake wait".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: Some(Lane::NeedsDecision),
                source_bot: Some("coordinator".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: None,
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:coordinator".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();

        let store_wait = BoardStore::open(store.base().to_path_buf()).unwrap();
        let id = item.id.clone();
        let handle = thread::spawn(move || {
            store_wait
                .wait(
                    &id,
                    WaitFilter {
                        since_version: item.version,
                        timeout_secs: 10,
                        wait_for: WaitFor::Decision,
                        waiter: "bot:coordinator".to_owned(),
                    },
                )
                .unwrap()
        });

        thread::sleep(Duration::from_millis(150));
        let decided_at = Instant::now();
        store
            .decide(
                &item.id,
                DecideRequest {
                    verdict: Verdict::Approve,
                    text: None,
                    option: None,
                    plan_hash: None,
                    by: "owner".to_owned(),
                },
            )
            .unwrap();

        match handle.join().unwrap() {
            WaitOutcome::Ready { item: got, event } => {
                let wake_elapsed = decided_at.elapsed();
                assert!(
                    wake_elapsed <= Duration::from_millis(1500),
                    "blocked wait took {:?} after decide",
                    wake_elapsed
                );
                assert_eq!(event.kind, "decision");
                assert_eq!(event.by, "owner");
                assert_eq!(
                    got.decision.as_ref().map(|d| d.verdict),
                    Some(Verdict::Approve)
                );
            }
            WaitOutcome::TimedOut { .. } => panic!("expected wait to wake on decision"),
        }

        // wait_any --for decision likewise wakes within <= 1.5 s.
        let item2 = store
            .add(AddItemRequest {
                title: "wake wait_any".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: Some(Lane::NeedsDecision),
                source_bot: Some("coordinator".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: None,
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:coordinator".to_owned(),
                state_root: state,
            })
            .unwrap();
        let since_seq = store
            .list_events(0)
            .unwrap()
            .last()
            .map(|e| e.seq)
            .unwrap_or(0);
        let store_any = BoardStore::open(store.base().to_path_buf()).unwrap();
        let handle_any = thread::spawn(move || {
            store_any
                .wait_any(
                    since_seq,
                    WaitFilter {
                        since_version: 0,
                        timeout_secs: 10,
                        wait_for: WaitFor::Decision,
                        waiter: "bot:coordinator".to_owned(),
                    },
                )
                .unwrap()
        });

        thread::sleep(Duration::from_millis(150));
        let decided_any_at = Instant::now();
        store
            .decide(
                &item2.id,
                DecideRequest {
                    verdict: Verdict::Approve,
                    text: None,
                    option: None,
                    plan_hash: None,
                    by: "owner".to_owned(),
                },
            )
            .unwrap();

        match handle_any.join().unwrap() {
            Some((event, got)) => {
                let wake_elapsed = decided_any_at.elapsed();
                assert!(
                    wake_elapsed <= Duration::from_millis(1500),
                    "blocked wait_any took {:?} after decide",
                    wake_elapsed
                );
                assert_eq!(event.item_id, item2.id);
                assert_eq!(event.kind, "decision");
                assert_eq!(
                    got.decision.as_ref().map(|d| d.verdict),
                    Some(Verdict::Approve)
                );
            }
            None => panic!("expected wait_any to wake on decision"),
        }
    }

    const HARDENING_PLAN_JSON: &str = r#"{"tasks":[{"id":"task-1","title":"t","done_check":"d","size":"small","depends_on":[]}],"steps":[{"id":"s1","role":"implement","job":"j","runner":"cursor","depends_on":[],"task":"task-1"}]}"#;

    /// An item with a plan in `lane` (`Approved` records an owner approval).
    fn item_with_plan(
        store: &BoardStore,
        state: &Path,
        ws: &Path,
        lane: Lane,
    ) -> (BoardItem, String) {
        let (_, hash) =
            crate::board::edit_plan_from_json(ws, state, "ship", HARDENING_PLAN_JSON, None)
                .unwrap();
        let add_lane = if lane == Lane::Auto {
            None
        } else {
            Some(Lane::NeedsDecision)
        };
        let item = store
            .add(AddItemRequest {
                title: format!("hardening {}", lane.as_str()),
                summary: String::new(),
                kind: if lane == Lane::Auto {
                    Kind::Debug
                } else {
                    Kind::Feature
                },
                grade: None,
                lane: add_lane,
                source_bot: Some("coord".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(ws.to_path_buf()),
                recommendation: None,
                options: vec![],
                plan_hash: Some(hash.clone()),
                key: None,
                by: "bot:coord".to_owned(),
                state_root: state.to_path_buf(),
            })
            .unwrap();
        assert_eq!(item.lane, lane_after_add(lane));
        let item = if lane == Lane::Approved {
            store
                .decide_without_notify(
                    &item.id,
                    DecideRequest {
                        verdict: Verdict::Approve,
                        text: None,
                        option: None,
                        plan_hash: Some(hash.clone()),
                        by: "owner".to_owned(),
                    },
                )
                .unwrap()
        } else {
            item
        };
        (item, hash)
    }

    fn lane_after_add(lane: Lane) -> Lane {
        if lane == Lane::Auto {
            Lane::Auto
        } else {
            Lane::NeedsDecision
        }
    }

    fn hardening_store() -> (tempfile::TempDir, BoardStore, PathBuf, PathBuf) {
        let (dir, store, state) = test_store();
        let ws = dir.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        (dir, store, state, ws)
    }

    // TC-3803 / FR-3803: a bot cannot move a `needs_decision` item into `in_progress`
    // (store and MCP refuse with exit 5 and a `board run` hint); approved and auto
    // items and the owner are unaffected.
    #[test]
    fn tc_3803_move_into_in_progress_restricted() {
        let _env = crate::state_path::test_env_lock();
        let (_dir, store, state, ws) = hardening_store();
        let (pending, _) = item_with_plan(&store, &state, &ws, Lane::NeedsDecision);

        let err = store
            .move_lane(&pending.id, Lane::InProgress, "bot:coord", None)
            .unwrap_err();
        assert_eq!(err.exit_code(), crate::agent_api::exit::FORBIDDEN);
        assert!(err.to_string().contains("board run"), "hint: {err}");
        assert_eq!(store.get(&pending.id).unwrap(), pending);

        let prev_state = std::env::var_os("DMUXAI_STATE_DIR");
        unsafe { std::env::set_var("DMUXAI_STATE_DIR", &state) };
        let mcp = crate::board::mcp_invoke(
            &ws,
            "board_move",
            &serde_json::json!({ "id": pending.id, "lane": "in_progress" }),
        );
        unsafe {
            match prev_state {
                Some(v) => std::env::set_var("DMUXAI_STATE_DIR", v),
                None => std::env::remove_var("DMUXAI_STATE_DIR"),
            }
        }
        assert_eq!(mcp["ok"], false, "mcp: {mcp}");
        assert_eq!(
            mcp["exit_code"],
            crate::agent_api::exit::FORBIDDEN,
            "mcp: {mcp}"
        );
        assert_eq!(store.get(&pending.id).unwrap().lane, Lane::NeedsDecision);

        let (approved, _) = item_with_plan(&store, &state, &ws, Lane::Approved);
        let (auto, _) = item_with_plan(&store, &state, &ws, Lane::Auto);
        assert_eq!(
            store
                .move_lane(&approved.id, Lane::InProgress, "bot:coord", None)
                .unwrap()
                .lane,
            Lane::InProgress
        );
        assert_eq!(
            store
                .move_lane(&auto.id, Lane::InProgress, "bot:coord", None)
                .unwrap()
                .lane,
            Lane::InProgress
        );
        assert_eq!(
            store
                .move_lane(&pending.id, Lane::InProgress, "owner", None)
                .unwrap()
                .lane,
            Lane::InProgress
        );
    }

    // TC-3806 / FR-3806: a run result moves the lane only while the item is still
    // `in_progress` on the plan that ran; a rejected or re-planned item keeps its
    // lane and plan and gets a `run` note with the exit code.
    #[test]
    fn tc_3806_run_result_respects_newer_state() {
        let _env = crate::state_path::test_env_lock();
        let (_dir, store, state, ws) = hardening_store();

        // Success: still in_progress on the same plan -> done.
        let (ok_item, ok_hash) = item_with_plan(&store, &state, &ws, Lane::Approved);
        store
            .move_lane(&ok_item.id, Lane::InProgress, "bot:coord", None)
            .unwrap();
        let done = store
            .record_run_result(
                &ok_item.id,
                Some("0123456789abcdef"),
                0,
                None,
                "bot:coord",
                &ok_hash,
            )
            .unwrap();
        assert_eq!(done.lane, Lane::Done);

        // Rejected while running -> lane stays rejected.
        let (rejected, hash) = item_with_plan(&store, &state, &ws, Lane::Approved);
        store
            .move_lane(&rejected.id, Lane::InProgress, "bot:coord", None)
            .unwrap();
        store
            .decide_without_notify(
                &rejected.id,
                DecideRequest {
                    verdict: Verdict::Reject,
                    text: None,
                    option: None,
                    plan_hash: None,
                    by: "owner".to_owned(),
                },
            )
            .unwrap();
        let after = store
            .record_run_result(
                &rejected.id,
                Some("1111111111111111"),
                0,
                None,
                "bot:coord",
                &hash,
            )
            .unwrap();
        assert_eq!(after.lane, Lane::Rejected);
        assert_eq!(after.plan.as_ref().unwrap().hash, hash);
        let note = after.thread.last().unwrap();
        assert_eq!(note.from, "run");
        assert!(note.text.contains("exit 0"), "note: {}", note.text);
        assert!(note.text.contains("not applied"), "note: {}", note.text);

        // Re-planned while running (no live run lock) -> new plan and lane are kept.
        let (replanned, old_hash) = item_with_plan(&store, &state, &ws, Lane::Approved);
        store
            .move_lane(&replanned.id, Lane::InProgress, "bot:coord", None)
            .unwrap();
        let (_, new_hash) = crate::board::edit_plan_from_json(
            &ws,
            &state,
            "ship v2",
            &HARDENING_PLAN_JSON.replace("\"j\"", "\"j2\""),
            None,
        )
        .unwrap();
        store
            .attach_plan(&replanned.id, &new_hash, &state, "bot:coord")
            .unwrap();
        let after = store
            .record_run_result(&replanned.id, None, 3, Some("boom"), "bot:coord", &old_hash)
            .unwrap();
        assert_eq!(after.lane, Lane::NeedsDecision);
        assert_eq!(after.plan.as_ref().unwrap().hash, new_hash);
        let note = &after.thread.last().unwrap().text;
        assert!(
            note.contains("exit 3") && note.contains("not applied"),
            "note: {note}"
        );

        // Failure on the same plan still moves to unknown.
        let (failed, failed_hash) = item_with_plan(&store, &state, &ws, Lane::Auto);
        store
            .move_lane(&failed.id, Lane::InProgress, "bot:coord", None)
            .unwrap();
        let after = store
            .record_run_result(&failed.id, None, 2, Some("err"), "bot:coord", &failed_hash)
            .unwrap();
        assert_eq!(after.lane, Lane::Unknown);
    }

    // TC-3807 / FR-3807: while a live run holds an `in_progress` item, plan attach,
    // plan edit and `ask --revise` are refused with conflict; after the run ends they
    // work again. The HTTP `POST /plan` path is covered in `src/web/board_api.rs`.
    #[test]
    fn tc_3807_plan_change_refused_while_running() {
        let _env = crate::state_path::test_env_lock();
        let (_dir, store, state, ws) = hardening_store();
        let (item, hash) = item_with_plan(&store, &state, &ws, Lane::Approved);
        let prepared = crate::board::runner::run_item_prepare(
            &store,
            crate::board::runner::RunParams {
                id: item.id.clone(),
                expected_plan_hash: Some(hash.clone()),
                by: "bot:coord".to_owned(),
                state_root: state.clone(),
            },
        )
        .unwrap();
        let (_, new_hash) = crate::board::edit_plan_from_json(
            &ws,
            &state,
            "ship v2",
            &HARDENING_PLAN_JSON.replace("\"j\"", "\"j2\""),
            None,
        )
        .unwrap();

        let attach = store
            .attach_plan(&item.id, &new_hash, &state, "bot:coord")
            .unwrap_err();
        assert_eq!(attach.exit_code(), crate::agent_api::exit::CONFLICT);
        let edit = store
            .edit_plan(&item.id, &new_hash, &ws, &state, "bot:coord")
            .unwrap_err();
        assert_eq!(edit.exit_code(), crate::agent_api::exit::CONFLICT);
        let revise = crate::board::runner::ask_item_prepare(
            &store,
            crate::board::runner::AskParams {
                id: item.id.clone(),
                text: "change it".to_owned(),
                revise: true,
                agent: None,
                model: None,
                by: "bot:coord".to_owned(),
                state_root: state.clone(),
            },
        )
        .err()
        .expect("revise must be refused while running");
        assert_eq!(revise.exit_code(), crate::agent_api::exit::CONFLICT);
        let current = store.get(&item.id).unwrap();
        assert_eq!(current.lane, Lane::InProgress);
        assert_eq!(current.plan.as_ref().unwrap().hash, hash);

        drop(prepared);
        let reattached = store
            .attach_plan(&item.id, &new_hash, &state, "bot:coord")
            .unwrap();
        assert_eq!(reattached.plan.as_ref().unwrap().hash, new_hash);
    }

    // TC-3823 / FR-3823, NFR-3820: `board session` reports the latest run link
    // with a conversation and its binding agents/statuses, read-only and
    // without provider session ids, under 200 ms on a 10k-turn database; a
    // missing database yields `bindings: []` and a warning and is not created.
    #[test]
    fn tc_3823_board_session_reports_bindings() {
        use crate::board::session::{self, BindingView, test_support};
        use crate::board::{SessionKind, SessionStatus};
        use crate::conversation::BindingStatus;
        let (dir, store, state) = test_store();
        let ws = dir.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        let item = store
            .add(AddItemRequest {
                title: "work session".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: None,
                source_bot: Some("coord".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(ws.clone()),
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:coord".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        let ws_path = item.workspace.path.clone().unwrap();

        let empty = session::session_view(&store, &item.id, &state).unwrap();
        assert!(empty.work_session.is_none());
        assert!(empty.bindings.is_empty() && empty.warning.is_none());

        let mut run = session::running_link(SessionKind::Run, Path::new(&ws_path), 1);
        run.status = SessionStatus::Completed;
        run.run_id = Some("0123456789abcdef".to_owned());
        run.conversation_id = Some("conv-work".to_owned());
        store.append_session(&item.id, run, "bot:coord").unwrap();
        let mut ask = session::running_link(SessionKind::Ask, Path::new(&ws_path), 2);
        ask.status = SessionStatus::Completed;
        ask.conversation_id = Some("conv-ask".to_owned());
        store.append_session(&item.id, ask, "planner").unwrap();

        let db = state
            .join("repos")
            .join(crate::state_path::repo_key_for_canonical(&ws_path))
            .join("conversations.sqlite3");
        let missing = session::session_view(&store, &item.id, &state).unwrap();
        assert_eq!(
            missing
                .work_session
                .as_ref()
                .unwrap()
                .conversation_id
                .as_deref(),
            Some("conv-work")
        );
        assert!(missing.bindings.is_empty());
        assert!(missing.warning.is_some());
        assert!(!db.exists(), "board session must not create the database");

        test_support::seed_conversation_db(&state, &ws_path, "conv-work", 10_000);
        let bytes = fs::read(&db).unwrap();
        let started = Instant::now();
        let view = session::session_view(&store, &item.id, &state).unwrap();
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(200),
            "board session took {elapsed:?}"
        );
        assert_eq!(
            view.bindings,
            vec![
                BindingView {
                    agent: "agy".to_owned(),
                    status: BindingStatus::Stale,
                },
                BindingView {
                    agent: "cursor".to_owned(),
                    status: BindingStatus::Active,
                },
            ]
        );
        assert!(view.warning.is_none());
        assert_eq!(view.sessions.len(), 2);
        let json = serde_json::to_value(&view).unwrap();
        for key in ["item_id", "work_session", "sessions", "bindings"] {
            assert!(json.get(key).is_some(), "missing {key}");
        }
        assert!(!json.to_string().contains("SECRET"));
        assert_eq!(fs::read(&db).unwrap(), bytes, "database left untouched");

        let unknown = session::session_view(&store, "b000000000000", &state).unwrap_err();
        assert_eq!(unknown.exit_code(), crate::agent_api::exit::NOT_FOUND);
        let invalid = session::session_view(&store, "nope", &state).unwrap_err();
        assert_eq!(invalid.exit_code(), crate::agent_api::exit::USAGE);
    }
}
