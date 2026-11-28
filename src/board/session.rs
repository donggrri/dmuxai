//! Board work-session links: where a board run or ask did its work
//! (SPEC-board-work-session FR-3820..FR-3826, NFR-3820).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::{
    BoardError, BoardItem, BoardStore, SESSION_MD_PATHS_MAX, SessionKind, SessionLink,
    SessionStatus, valid_item_id,
};
use crate::conversation::BindingStatus;
use crate::run_journal::{
    JournalError, JournalStore, RunManifest, data_base, mask_sensitive_tokens, valid_run_id,
};
use crate::state_path::repo_key_for_canonical;

const LINK_ID_MAX: usize = 128;
const LABEL_MAX: usize = 120;
const MD_PATH_MAX: usize = 512;

/// Ids and agent read from the child's `run --json` output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChildLinks {
    pub conversation_id: Option<String>,
    pub graph_id: Option<String>,
    pub agent: Option<String>,
}

/// `{agent, status}` of one provider binding; never the provider session id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BindingView {
    pub agent: String,
    pub status: BindingStatus,
}

/// Response of `board session`, MCP `board_session` and
/// `GET /api/board/{id}/session` (FR-3823).
#[derive(Debug, Clone, Serialize)]
pub struct SessionView {
    pub item_id: String,
    pub work_session: Option<SessionLink>,
    pub sessions: Vec<SessionLink>,
    pub bindings: Vec<BindingView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// Conversation, graph and run ids: `[A-Za-z0-9_-]{1,128}`.
pub fn valid_link_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= LINK_ID_MAX
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn clean_id(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| valid_link_id(v))
        .map(str::to_owned)
}

fn clean_label(value: Option<&str>) -> Option<String> {
    let value = value.map(str::trim).filter(|v| !v.is_empty())?;
    let masked = mask_sensitive_tokens(value);
    let label: String = masked
        .chars()
        .filter(|c| !c.is_control())
        .take(LABEL_MAX)
        .collect();
    (!label.is_empty()).then_some(label)
}

/// Workspace-relative `.md` path without `..`, absolute or `.dmuxai/` parts.
fn safe_md_path(path: &str) -> bool {
    if path.is_empty()
        || path.len() > MD_PATH_MAX
        || path.chars().any(char::is_control)
        || !path.to_ascii_lowercase().ends_with(".md")
    {
        return false;
    }
    let parsed = Path::new(path);
    if parsed.starts_with(".dmuxai") {
        return false;
    }
    parsed
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
}

/// Re-validates a link before it is stored: ids keep their charset, labels are
/// masked and Markdown paths stay workspace-relative and bounded (FR-3821).
pub(crate) fn mask_link(link: &mut SessionLink) {
    link.run_id = clean_id(link.run_id.as_deref());
    link.conversation_id = clean_id(link.conversation_id.as_deref());
    link.graph_id = clean_id(link.graph_id.as_deref());
    link.agent = clean_label(link.agent.as_deref());
    link.model = clean_label(link.model.as_deref());
    link.md_paths.retain(|p| safe_md_path(p));
    link.md_paths.truncate(SESSION_MD_PATHS_MAX);
}

/// A `running` link appended when a board run starts (FR-3820).
pub fn running_link(kind: SessionKind, workspace: &Path, started_ms: u64) -> SessionLink {
    SessionLink {
        kind,
        run_id: None,
        conversation_id: None,
        graph_id: None,
        workspace_path: workspace.to_string_lossy().into_owned(),
        agent: None,
        model: None,
        status: SessionStatus::Running,
        exit_code: None,
        started_ms,
        ended_ms: None,
        md_paths: Vec::new(),
    }
}

/// `git status` entries of a workspace, keyed by workspace-relative path.
#[derive(Debug, Clone, Default)]
pub struct MdSnapshot {
    entries: BTreeMap<String, String>,
}

/// Snapshot taken before and after a board run child; `None` outside git.
pub fn md_snapshot(workspace: &Path) -> Option<MdSnapshot> {
    let raw = crate::graph::spec_snapshot(workspace)?;
    let prefix = if workspace.join(".git").exists() {
        String::new()
    } else {
        let output = Command::new("git")
            .args(["rev-parse", "--show-prefix"])
            .current_dir(workspace)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    let entries = raw
        .into_iter()
        .filter_map(|(path, status)| {
            path.strip_prefix(prefix.as_str())
                .map(|rel| (rel.to_owned(), status))
        })
        .collect();
    Some(MdSnapshot { entries })
}

/// `.md` paths whose `git status` differs between `before` and `after`,
/// sorted and capped at [`SESSION_MD_PATHS_MAX`] (FR-3821).
pub fn changed_md_paths(before: Option<&MdSnapshot>, after: Option<&MdSnapshot>) -> Vec<String> {
    let (Some(before), Some(after)) = (before, after) else {
        return Vec::new();
    };
    let mut changed = BTreeSet::new();
    for (path, status) in &after.entries {
        if before.entries.get(path) != Some(status) {
            changed.insert(path.clone());
        }
    }
    for path in before.entries.keys() {
        if !after.entries.contains_key(path) {
            changed.insert(path.clone());
        }
    }
    changed
        .into_iter()
        .filter(|p| safe_md_path(p))
        .take(SESSION_MD_PATHS_MAX)
        .collect()
}

fn runs_dir_for(workspace_path: &str) -> Option<PathBuf> {
    Some(
        data_base()
            .ok()?
            .join("repos")
            .join(repo_key_for_canonical(workspace_path))
            .join("runs"),
    )
}

fn load_manifest(workspace_path: &str, run_id: &str) -> Result<RunManifest, JournalError> {
    let dir = runs_dir_for(workspace_path).ok_or_else(|| {
        JournalError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "run journal directory unavailable",
        ))
    })?;
    JournalStore::new(dir).load(run_id)
}

fn status_from_exit(exit_code: i32) -> SessionStatus {
    match exit_code {
        0 => SessionStatus::Completed,
        7 => SessionStatus::AwaitingApproval,
        130 => SessionStatus::Cancelled,
        _ => SessionStatus::Failed,
    }
}

fn status_from_manifest(status: &str) -> Option<SessionStatus> {
    match status {
        "completed" | "approved" => Some(SessionStatus::Completed),
        "failed" => Some(SessionStatus::Failed),
        "cancelled" => Some(SessionStatus::Cancelled),
        "awaiting_approval" => Some(SessionStatus::AwaitingApproval),
        "active" | "running" => Some(SessionStatus::Running),
        _ => None,
    }
}

fn apply_manifest(link: &mut SessionLink, manifest: &RunManifest) {
    if link.conversation_id.is_none() {
        link.conversation_id = clean_id(manifest.links.conversation_id.as_deref());
    }
    if link.graph_id.is_none() {
        link.graph_id = clean_id(manifest.links.graph_id.as_deref());
    }
    if let Some(agent) = clean_label(
        manifest
            .resolved_runner
            .as_deref()
            .or(manifest.requested_runner.as_deref()),
    ) {
        link.agent = Some(agent);
    }
    if let Some(model) = clean_label(
        manifest
            .resolved_model
            .as_deref()
            .or(manifest.requested_model.as_deref()),
    ) {
        link.model = Some(model);
    }
}

/// Fill a link from the finished child: ids from its `--json` output and its
/// run manifest, status from the manifest or the exit code (FR-3821, FR-3822).
pub fn finished_link(
    mut link: SessionLink,
    run_id: Option<&str>,
    exit_code: i32,
    child: &ChildLinks,
    md_paths: Vec<String>,
) -> SessionLink {
    link.run_id = clean_id(run_id);
    link.conversation_id = clean_id(child.conversation_id.as_deref());
    link.graph_id = clean_id(child.graph_id.as_deref());
    if let Some(agent) = clean_label(child.agent.as_deref()) {
        link.agent = Some(agent);
    }
    let mut status = status_from_exit(exit_code);
    if let Some(rid) = link.run_id.clone().filter(|r| valid_run_id(r))
        && let Ok(manifest) = load_manifest(&link.workspace_path, &rid)
    {
        apply_manifest(&mut link, &manifest);
        if let Some(from_manifest) =
            status_from_manifest(&manifest.status).filter(|s| *s != SessionStatus::Running)
        {
            status = from_manifest;
        }
    }
    link.status = status;
    link.exit_code = Some(exit_code);
    link.ended_ms = Some(now_ms());
    link.md_paths = md_paths;
    mask_link(&mut link);
    link
}

/// A run link built from an existing run manifest (`board link`, FR-3826).
pub fn link_from_manifest(workspace_path: &str, manifest: &RunManifest) -> SessionLink {
    let mut link = SessionLink {
        kind: SessionKind::Run,
        run_id: clean_id(Some(&manifest.run_id)),
        conversation_id: None,
        graph_id: None,
        workspace_path: workspace_path.to_owned(),
        agent: None,
        model: None,
        status: status_from_manifest(&manifest.status).unwrap_or(SessionStatus::Failed),
        exit_code: manifest.dmuxai_exit_code.or(manifest.child_exit_code),
        started_ms: manifest.created_at_ms,
        ended_ms: manifest.completed_at_ms,
        md_paths: Vec::new(),
    };
    apply_manifest(&mut link, manifest);
    mask_link(&mut link);
    link
}

/// Append a `run` link for an existing run in the item's workspace. Linking a
/// run that is already linked leaves the item unchanged (FR-3826).
pub fn link_run(
    store: &BoardStore,
    id: &str,
    run_id: &str,
    by: &str,
) -> Result<BoardItem, BoardError> {
    if !valid_item_id(id) {
        return Err(BoardError::Usage("invalid board item id".to_owned()));
    }
    if !valid_run_id(run_id) {
        return Err(BoardError::Usage(
            "run id must be 16 lowercase hex characters".to_owned(),
        ));
    }
    let item = store.get(id)?;
    let Some(workspace) = item.workspace.path.clone() else {
        return Err(BoardError::Usage("board item has no workspace".to_owned()));
    };
    let manifest = match load_manifest(&workspace, run_id) {
        Ok(manifest) => manifest,
        Err(JournalError::InvalidId(_)) => {
            return Err(BoardError::Usage("invalid run id".to_owned()));
        }
        Err(_) => {
            return Err(BoardError::NotFound(format!(
                "run {run_id} not found in the item's workspace"
            )));
        }
    };
    let link = link_from_manifest(&workspace, &manifest);
    store.update_sessions(id, by, |sessions| {
        if sessions
            .iter()
            .any(|l| l.kind == SessionKind::Run && l.run_id.as_deref() == Some(run_id))
        {
            return false;
        }
        sessions.push(link);
        true
    })
}

/// The latest `run` link with a conversation id (FR-3823).
pub fn work_session(item: &BoardItem) -> Option<&SessionLink> {
    item.sessions
        .iter()
        .rev()
        .find(|l| l.kind == SessionKind::Run && l.conversation_id.is_some())
}

/// Read-only session view of one item. Bindings come from the workspace
/// conversation database opened read-only; a missing workspace or database
/// yields `bindings: []` and a `warning` (FR-3823, NFR-3820).
pub fn session_view(
    store: &BoardStore,
    id: &str,
    state_root: &Path,
) -> Result<SessionView, BoardError> {
    if !valid_item_id(id) {
        return Err(BoardError::Usage("invalid board item id".to_owned()));
    }
    let item = store.get(id)?;
    let work = work_session(&item).cloned();
    let mut bindings = Vec::new();
    let mut warning = None;
    if let Some(link) = &work
        && let Some(conversation_id) = link.conversation_id.as_deref()
    {
        let workspace = Path::new(&link.workspace_path);
        let db_path = state_root
            .join("repos")
            .join(repo_key_for_canonical(&link.workspace_path))
            .join("conversations.sqlite3");
        if !workspace.is_dir() {
            warning = Some("workspace folder not found; bindings unavailable".to_owned());
        } else {
            match crate::conversation::read_binding_statuses(&db_path, conversation_id) {
                Ok(Some(rows)) => {
                    bindings = rows
                        .into_iter()
                        .map(|(agent, status)| BindingView { agent, status })
                        .collect();
                }
                Ok(None) => {
                    warning =
                        Some("conversation database not found; bindings unavailable".to_owned());
                }
                Err(_) => {
                    warning = Some(
                        "conversation database could not be read; bindings unavailable".to_owned(),
                    );
                }
            }
        }
    }
    Ok(SessionView {
        item_id: item.id.clone(),
        work_session: work,
        sessions: item.sessions,
        bindings,
        warning,
    })
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};

    use rusqlite::params;

    /// Workspace conversation database under `state_root` holding one
    /// conversation with `turns` turns, an active cursor binding and a stale
    /// agy binding whose provider session ids must never be reported.
    pub fn seed_conversation_db(
        state_root: &Path,
        workspace_path: &str,
        conversation_id: &str,
        turns: usize,
    ) -> PathBuf {
        let db = state_root
            .join("repos")
            .join(crate::state_path::repo_key_for_canonical(workspace_path))
            .join("conversations.sqlite3");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        drop(
            crate::conversation::ConversationEngine::open(&db, Path::new(workspace_path)).unwrap(),
        );
        let mut conn = rusqlite::Connection::open(&db).unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute(
            "INSERT INTO conversations (id, workspace_root, title, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, 'board work', 1, 1)",
            params![conversation_id, workspace_path],
        )
        .unwrap();
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO turns (id, conversation_id, seq, requested_agent, status, created_at_ms)
                     VALUES (?1, ?2, ?3, 'cursor', 'completed', 1)",
                )
                .unwrap();
            for seq in 0..turns {
                stmt.execute(params![format!("t{seq}"), conversation_id, seq as i64])
                    .unwrap();
            }
        }
        for (agent, psid, status) in [
            ("cursor", "psid-cursor-SECRET", "active"),
            ("agy", "psid-agy-SECRET", "stale"),
        ] {
            tx.execute(
                "INSERT INTO provider_bindings
                    (conversation_id, agent, provider_session_id, status, known_through_seq,
                     created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, 0, 1, 1)",
                params![conversation_id, agent, psid, status],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        db
    }

    /// Writes `<data_dir>/repos/<key>/runs/<run_id>/manifest.json`.
    pub fn write_manifest(
        data_dir: &Path,
        workspace_path: &str,
        run_id: &str,
        status: &str,
        conversation_id: Option<&str>,
        graph_id: Option<&str>,
    ) {
        let dir = data_dir
            .join("repos")
            .join(crate::state_path::repo_key_for_canonical(workspace_path))
            .join("runs")
            .join(run_id);
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = serde_json::json!({
            "schema_version": 1,
            "run_id": run_id,
            "origin": "cli",
            "status": status,
            "created_at_ms": 1_000,
            "completed_at_ms": 2_000,
            "requested_runner": "cursor-agent",
            "resolved_model": "gpt-5",
            "dmuxai_exit_code": 0,
            "links": { "conversation_id": conversation_id, "graph_id": graph_id },
            "diagnostics": { "stderr_bytes": 0 },
            "workspace_root": workspace_path,
        });
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // FR-3821: only workspace-relative `.md` paths survive.
    #[test]
    fn tc_3821_md_paths_are_bounded_and_workspace_relative() {
        assert!(safe_md_path("docs/specs/SPEC-x.md"));
        assert!(safe_md_path("README.MD"));
        assert!(!safe_md_path("/etc/x.md"));
        assert!(!safe_md_path("../x.md"));
        assert!(!safe_md_path("docs/../../x.md"));
        assert!(!safe_md_path(".dmuxai/graphs/x.md"));
        assert!(!safe_md_path("docs/x.txt"));
        let before = MdSnapshot::default();
        let mut after = MdSnapshot::default();
        for i in 0..80 {
            after
                .entries
                .insert(format!("docs/n{i:03}.md"), "??".to_owned());
        }
        after.entries.insert("src/a.rs".to_owned(), " M".to_owned());
        let changed = changed_md_paths(Some(&before), Some(&after));
        assert_eq!(changed.len(), SESSION_MD_PATHS_MAX);
        assert_eq!(changed[0], "docs/n000.md");
        assert!(changed.windows(2).all(|w| w[0] < w[1]));
        assert!(changed_md_paths(None, Some(&after)).is_empty());
    }
}
