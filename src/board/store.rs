//! Board item storage and events (FR-3100, FR-3101).

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

use serde::{Deserialize, Serialize};

use super::plan::{board_plan_from_pending, read_pending_plan};
use super::rules::RulesStore;
use super::{
    BoardError, BoardItem, CLOSED_HIDE_DAYS, Decision, Kind, Lane, Source, THREAD_MAX,
    ThreadMessage, Verdict, WorkspaceRef, generate_item_id, mask_item_fields, valid_item_id,
    validate_message, validate_new_text, validate_options, validate_ref_url,
};
use crate::state_path::{repo_key, valid_pending_plan_hash};
use crate::workspace::normalize_workspace_path;

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
    pub kind: String,
    pub by: String,
    pub version: u32,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EventKind {
    Created,
    Updated,
    Lane,
    Plan,
    Decision,
    Message,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BoardEvent {
    seq: u64,
    item_id: String,
    kind: EventKind,
    version: u32,
    at_ms: u64,
    by: String,
}

pub struct BoardStore {
    base: PathBuf,
    items_dir: PathBuf,
    events_path: PathBuf,
    lock_path: PathBuf,
}

pub fn ensure_dir(path: &Path) -> Result<(), BoardError> {
    if path.exists() {
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
        let text = String::from_utf8_lossy(&tail);
        let mut last = 0u64;
        for line in text.lines() {
            if let Ok(event) = serde_json::from_str::<BoardEvent>(line) {
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
        let seq = self.read_last_seq()? + 1;
        let event = BoardEvent {
            seq,
            item_id: item_id.to_owned(),
            kind,
            version,
            at_ms: now_ms(),
            by: by.to_owned(),
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
        self.with_lock(|store| {
            let rules = RulesStore::new(&store.base).load();
            let lane = if let Some(lane) = req.lane {
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
                thread: Vec::new(),
                run_ids: Vec::new(),
                created_ms: now_ms(),
                updated_ms: now_ms(),
                version: 1,
            };
            if let Some(hash) = req.plan_hash {
                let pending = read_pending_plan(&req.state_root, &hash)?;
                item.plan = Some(board_plan_from_pending(pending, hash, None));
            }
            mask_item_fields(&mut item);
            store.save_item_inner(&item)?;
            store.append_event(&item.id, EventKind::Created, item.version, &req.by)?;
            Ok(item)
        })
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

    pub fn move_lane(&self, id: &str, lane: Lane, by: &str) -> Result<BoardItem, BoardError> {
        if !valid_item_id(id) {
            return Err(BoardError::Usage("invalid board item id".to_owned()));
        }
        self.with_lock(|store| {
            let mut item = store.load_item_inner(id)?;
            item.lane = lane;
            item.version += 1;
            item.updated_ms = now_ms();
            store.save_item_inner(&item)?;
            store.append_event(id, EventKind::Lane, item.version, by)?;
            Ok(item)
        })
    }

    pub fn attach_plan(
        &self,
        id: &str,
        hash: &str,
        state_root: &Path,
    ) -> Result<BoardItem, BoardError> {
        if !valid_item_id(id) || !valid_pending_plan_hash(hash) {
            return Err(BoardError::Usage("invalid id or plan hash".to_owned()));
        }
        self.with_lock(|store| {
            let mut item = store.load_item_inner(id)?;
            let pending = read_pending_plan(state_root, hash)?;
            let plan = board_plan_from_pending(pending, hash.to_owned(), item.plan.as_ref());
            item.plan = Some(plan);
            item.version += 1;
            item.updated_ms = now_ms();
            store.save_item_inner(&item)?;
            store.append_event(id, EventKind::Plan, item.version, "bot:coordinator")?;
            Ok(item)
        })
    }

    pub fn edit_plan(
        &self,
        id: &str,
        hash: &str,
        _workspace: &Path,
        _state_root: &Path,
    ) -> Result<BoardItem, BoardError> {
        if !valid_item_id(id) || !valid_pending_plan_hash(hash) {
            return Err(BoardError::Usage("invalid id or plan hash".to_owned()));
        }
        self.with_lock(|store| {
            let mut item = store.load_item_inner(id)?;
            let prompt = item
                .plan
                .as_ref()
                .map(|p| p.prompt.clone())
                .unwrap_or_default();
            let pending = read_pending_plan(_state_root, hash)?;
            if pending.prompt != prompt && !prompt.is_empty() && pending.prompt != prompt {
                // keep item prompt as source of truth
            }
            let mut pending = pending;
            if !prompt.is_empty() {
                pending.prompt = prompt.clone();
            }
            let plan = board_plan_from_pending(pending, hash.to_owned(), item.plan.as_ref());
            item.plan = Some(plan);
            if item.lane != Lane::Auto {
                item.lane = Lane::NeedsDecision;
            }
            item.version += 1;
            item.updated_ms = now_ms();
            store.save_item_inner(&item)?;
            store.append_event(id, EventKind::Plan, item.version, "bot:coordinator")?;
            Ok(item)
        })
    }

    pub fn decide(&self, id: &str, req: DecideRequest) -> Result<BoardItem, BoardError> {
        if !valid_item_id(id) {
            return Err(BoardError::Usage("invalid board item id".to_owned()));
        }
        self.with_lock(|store| {
            let mut item = store.load_item_inner(id)?;
            if matches!(req.verdict, Verdict::Approve)
                && req.by.starts_with("bot:")
                && item.lane != Lane::Auto
            {
                return Err(BoardError::Forbidden(
                    "bots may approve only items in the auto lane".to_owned(),
                ));
            }
            if let Some(hash) = &req.plan_hash {
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

    pub fn say(&self, id: &str, req: SayRequest) -> Result<BoardItem, BoardError> {
        if !valid_item_id(id) {
            return Err(BoardError::Usage("invalid board item id".to_owned()));
        }
        validate_message(&req.text)?;
        self.with_lock(|store| {
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
            Ok(item)
        })
    }

    pub fn wait(&self, id: &str, filter: WaitFilter) -> Result<WaitOutcome, BoardError> {
        if !valid_item_id(id) {
            return Err(BoardError::Usage("invalid board item id".to_owned()));
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

fn wait_trigger_for_event(event: &BoardEvent, id: &str, filter: &WaitFilter) -> Option<WaitEvent> {
    if event.item_id != id || event.version <= filter.since_version {
        return None;
    }
    if event.by == filter.waiter {
        return None;
    }
    let matches = match filter.wait_for {
        WaitFor::Decision => matches!(event.kind, EventKind::Decision),
        WaitFor::Message => matches!(event.kind, EventKind::Message),
        WaitFor::Any => matches!(
            event.kind,
            EventKind::Decision | EventKind::Message | EventKind::Plan
        ),
    };
    if !matches {
        return None;
    }
    let kind = serde_json::to_value(&event.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned());
    Some(WaitEvent {
        kind,
        by: event.by.clone(),
        version: event.version,
    })
}

fn workspace_from_path(path: Option<&Path>) -> Result<WorkspaceRef, BoardError> {
    let Some(path) = path else {
        return Ok(WorkspaceRef::default());
    };
    let canonical = normalize_workspace_path(path)
        .map_err(|e| BoardError::Usage(format!("invalid workspace: {e}")))?;
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
        let item = store
            .add(AddItemRequest {
                title: "hello".to_owned(),
                summary: "sum".to_owned(),
                kind: Kind::Debug,
                grade: None,
                lane: None,
                source_bot: Some("coord".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: None,
                recommendation: None,
                options: vec![],
                plan_hash: None,
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
        }
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
        };
        let hash = write_pending_plan(&state, &plan).unwrap();
        let item = store.attach_plan(&item.id, &hash, &state).unwrap();
        assert!(
            store
                .decide(
                    &item.id,
                    DecideRequest {
                        verdict: Verdict::Approve,
                        text: None,
                        option: None,
                        plan_hash: Some("0000000000000000".to_owned()),
                        by: "owner".to_owned(),
                    }
                )
                .is_err()
        );
        assert!(
            store
                .decide(
                    &item.id,
                    DecideRequest {
                        verdict: Verdict::Approve,
                        text: None,
                        option: None,
                        plan_hash: None,
                        by: "bot:x".to_owned(),
                    }
                )
                .is_err()
        );
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
                by: "bot:test".to_owned(),
                state_root: state,
            })
            .unwrap();
        let moved = store
            .move_lane(&item.id, Lane::InProgress, "owner")
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
                by: "bot:test".to_owned(),
                state_root: state,
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
}
