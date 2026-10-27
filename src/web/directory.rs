//! Workspace directory for multi-workspace hosting.
//!
//! Tracks loaded workspaces and provides lookup and focused workspace access.
//! The dashboard currently creates exactly one entry.

use std::fmt;
use std::sync::{Arc, RwLock};

use crate::web::hub::DashboardHub;
use crate::web::server::DashboardServices;

/// Error returned when opening a workspace in-process fails.
#[derive(Debug, Clone)]
pub struct OpenError {
    pub status: u16,
    pub message: String,
}

/// Request sent to an opener channel to open a workspace in-process.
pub struct OpenRequest {
    pub path: std::path::PathBuf,
    pub reply: tokio::sync::oneshot::Sender<Result<Arc<WorkspaceEntry>, OpenError>>,
}

/// Maximum number of workspaces that can be open simultaneously.
pub const MAX_WORKSPACES: usize = 4;

/// Errors returned by workspace directory operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryError {
    Full,
    Duplicate,
    Missing,
    LastEntry,
}

impl fmt::Display for DirectoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DirectoryError::Full => write!(f, "too many open folders (max 4)"),
            DirectoryError::Duplicate => write!(f, "folder is already open"),
            DirectoryError::Missing => write!(f, "no such open folder"),
            DirectoryError::LastEntry => write!(f, "cannot close the last open folder"),
        }
    }
}

impl std::error::Error for DirectoryError {}

/// A single loaded workspace entry.
pub struct WorkspaceEntry {
    pub id: String,
    pub path: String,
    pub hub: DashboardHub,
    pub services: DashboardServices,
}

struct Inner {
    entries: Vec<Arc<WorkspaceEntry>>,
    focused: usize,
}

/// Directory of active workspaces. The dashboard currently creates exactly one entry.
pub struct WorkspaceDirectory {
    inner: RwLock<Inner>,
    opener: RwLock<Option<tokio::sync::mpsc::Sender<OpenRequest>>>,
}

impl WorkspaceDirectory {
    /// Creates a directory with a single workspace entry ("default").
    ///
    /// The dashboard currently creates exactly one entry.
    pub fn single(hub: DashboardHub, services: DashboardServices) -> Self {
        let path = hub.workspace();
        let entry = Arc::new(WorkspaceEntry {
            id: "default".to_string(),
            path,
            hub,
            services,
        });
        Self {
            inner: RwLock::new(Inner {
                entries: vec![entry],
                focused: 0,
            }),
            opener: RwLock::new(None),
        }
    }

    /// Sets the opener sender for in-process workspace opening.
    pub fn set_opener(&self, tx: tokio::sync::mpsc::Sender<OpenRequest>) {
        let mut opener = self.opener.write().unwrap_or_else(|e| e.into_inner());
        *opener = Some(tx);
    }

    /// Returns a clone of the opener sender, if set.
    pub fn opener(&self) -> Option<tokio::sync::mpsc::Sender<OpenRequest>> {
        let opener = self.opener.read().unwrap_or_else(|e| e.into_inner());
        opener.clone()
    }

    /// Returns the currently focused workspace entry.
    ///
    /// The dashboard currently creates exactly one entry.
    pub fn focused(&self) -> Arc<WorkspaceEntry> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner.entries[inner.focused].clone()
    }

    /// Looks up a workspace entry by id.
    ///
    /// The dashboard currently creates exactly one entry.
    pub fn get(&self, id: &str) -> Option<Arc<WorkspaceEntry>> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner.entries.iter().find(|e| e.id == id).cloned()
    }

    /// Returns the number of workspaces in the directory.
    ///
    /// The dashboard currently creates exactly one entry.
    pub fn len(&self) -> usize {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner.entries.len()
    }

    /// Returns true if the directory has no workspaces.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns all workspace entries in insertion order.
    pub fn entries(&self) -> Vec<Arc<WorkspaceEntry>> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner.entries.clone()
    }

    /// Looks up a workspace entry by path.
    pub fn find_by_path(&self, path: &str) -> Option<Arc<WorkspaceEntry>> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner.entries.iter().find(|e| e.path == path).cloned()
    }

    /// Inserts a new workspace entry.
    ///
    /// Returns `DirectoryError::Full` if directory already has `MAX_WORKSPACES`.
    /// Returns `DirectoryError::Duplicate` if id or path already exists.
    pub fn insert(&self, entry: WorkspaceEntry) -> Result<Arc<WorkspaceEntry>, DirectoryError> {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if inner.entries.len() >= MAX_WORKSPACES {
            return Err(DirectoryError::Full);
        }
        if inner
            .entries
            .iter()
            .any(|e| e.id == entry.id || e.path == entry.path)
        {
            return Err(DirectoryError::Duplicate);
        }
        let arc_entry = Arc::new(entry);
        inner.entries.push(arc_entry.clone());
        Ok(arc_entry)
    }

    /// Sets the focused workspace by id. Returns true if found and focused, false otherwise.
    pub fn set_focused(&self, id: &str) -> bool {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if let Some(pos) = inner.entries.iter().position(|e| e.id == id) {
            inner.focused = pos;
            true
        } else {
            false
        }
    }

    /// Removes a workspace entry by id.
    ///
    /// Returns `DirectoryError::Missing` if not found, or `DirectoryError::LastEntry`
    /// if it is the only entry. If the focused entry is removed, focus moves to the
    /// first remaining entry. Maintains valid focus index.
    pub fn remove(&self, id: &str) -> Result<Arc<WorkspaceEntry>, DirectoryError> {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if inner.entries.len() <= 1 {
            // Check if it exists to return Missing vs LastEntry if needed,
            // or if len is 1 and id matches -> LastEntry, id doesn't match -> Missing
            let exists = inner.entries.iter().any(|e| e.id == id);
            if !exists {
                return Err(DirectoryError::Missing);
            }
            return Err(DirectoryError::LastEntry);
        }
        let pos = inner
            .entries
            .iter()
            .position(|e| e.id == id)
            .ok_or(DirectoryError::Missing)?;
        let removed = inner.entries.remove(pos);
        if inner.focused == pos {
            inner.focused = 0;
        } else if inner.focused > pos {
            inner.focused -= 1;
        }
        Ok(removed)
    }

    /// Resolves a workspace entry based on an optional URL query string.
    ///
    /// Parses the query string for `ws=<id>`. If present and non-empty, looks up
    /// that workspace entry by id (returning `None` if unknown). Otherwise,
    /// returns the currently focused workspace entry (`Some(self.focused())`).
    pub fn resolve(&self, query: Option<&str>) -> Option<Arc<WorkspaceEntry>> {
        if let Some(q) = query {
            for part in q.split('&') {
                if let Some((k, v)) = part.split_once('=')
                    && k == "ws"
                    && !v.is_empty()
                {
                    return self.get(v);
                }
            }
        }
        Some(self.focused())
    }
}

/// Computes whether a workspace is currently running active tasks or turns.
pub fn snapshot_is_running(snapshot: &crate::web::snapshot::DashboardSnapshot) -> bool {
    if snapshot.agents.iter().any(|a| a.busy) {
        return true;
    }
    if let Some(turn) = &snapshot.turn {
        match turn.state.as_str() {
            "idle" | "done" | "failed" | "cancelled" => {}
            _ => return true,
        }
    }
    if let Some(graph) = &snapshot.graph {
        match graph.phase {
            crate::graph::GraphRunPhase::Completed
            | crate::graph::GraphRunPhase::Failed
            | crate::graph::GraphRunPhase::Cancelled
            | crate::graph::GraphRunPhase::Rejected => {}
            _ => return true,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::snapshot::SnapshotSource;

    fn make_entry(id: &str, path: &str) -> WorkspaceEntry {
        let hub = DashboardHub::new(SnapshotSource::Tui, path.to_string());
        let services = DashboardServices::default();
        WorkspaceEntry {
            id: id.to_string(),
            path: path.to_string(),
            hub,
            services,
        }
    }

    #[test]
    fn single_workspace_directory() {
        let hub = DashboardHub::new(SnapshotSource::Tui, "/tmp".to_string());
        let services = DashboardServices::default();
        let dir = WorkspaceDirectory::single(hub, services);

        assert_eq!(dir.len(), 1);
        assert!(!dir.is_empty());
        assert_eq!(dir.focused().id, "default");
        assert_eq!(dir.focused().path, "/tmp");
        assert!(dir.get("default").is_some());
        assert_eq!(dir.get("default").unwrap().id, "default");
        assert!(dir.get("other").is_none());
    }

    #[test]
    fn insert_up_to_max_and_full_error() {
        let hub = DashboardHub::new(SnapshotSource::Tui, "/w1".to_string());
        let dir = WorkspaceDirectory::single(hub, DashboardServices::default());

        assert_eq!(dir.len(), 1);
        let e2 = dir.insert(make_entry("w2", "/w2")).unwrap();
        assert_eq!(e2.id, "w2");
        let e3 = dir.insert(make_entry("w3", "/w3")).unwrap();
        assert_eq!(e3.id, "w3");
        let e4 = dir.insert(make_entry("w4", "/w4")).unwrap();
        assert_eq!(e4.id, "w4");
        assert_eq!(dir.len(), 4);

        let err = match dir.insert(make_entry("w5", "/w5")) {
            Ok(_) => panic!("expected Full error"),
            Err(e) => e,
        };
        assert_eq!(err, DirectoryError::Full);
        assert_eq!(err.to_string(), "too many open folders (max 4)");
        assert_eq!(dir.len(), 4);

        let entries = dir.entries();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].id, "default");
        assert_eq!(entries[1].id, "w2");
        assert_eq!(entries[2].id, "w3");
        assert_eq!(entries[3].id, "w4");
    }

    #[test]
    fn duplicate_by_id_and_by_path() {
        let hub = DashboardHub::new(SnapshotSource::Tui, "/path1".to_string());
        let dir = WorkspaceDirectory::single(hub, DashboardServices::default());

        // Duplicate id
        let err1 = match dir.insert(make_entry("default", "/path2")) {
            Ok(_) => panic!("expected Duplicate error"),
            Err(e) => e,
        };
        assert_eq!(err1, DirectoryError::Duplicate);
        assert_eq!(err1.to_string(), "folder is already open");

        // Duplicate path
        let err2 = match dir.insert(make_entry("w2", "/path1")) {
            Ok(_) => panic!("expected Duplicate error"),
            Err(e) => e,
        };
        assert_eq!(err2, DirectoryError::Duplicate);

        // Both duplicate
        let err3 = match dir.insert(make_entry("default", "/path1")) {
            Ok(_) => panic!("expected Duplicate error"),
            Err(e) => e,
        };
        assert_eq!(err3, DirectoryError::Duplicate);
    }

    #[test]
    fn set_focused() {
        let hub = DashboardHub::new(SnapshotSource::Tui, "/w1".to_string());
        let dir = WorkspaceDirectory::single(hub, DashboardServices::default());
        dir.insert(make_entry("w2", "/w2")).unwrap();

        assert_eq!(dir.focused().id, "default");
        assert!(dir.set_focused("w2"));
        assert_eq!(dir.focused().id, "w2");

        assert!(!dir.set_focused("nonexistent"));
        assert_eq!(dir.focused().id, "w2");
    }

    #[test]
    fn remove_focused_moves_focus_and_keeps_index_valid() {
        let hub = DashboardHub::new(SnapshotSource::Tui, "/w1".to_string());
        let dir = WorkspaceDirectory::single(hub, DashboardServices::default());
        dir.insert(make_entry("w2", "/w2")).unwrap();
        dir.insert(make_entry("w3", "/w3")).unwrap();

        // Focus w2 (index 1)
        dir.set_focused("w2");
        assert_eq!(dir.focused().id, "w2");

        // Remove w2 (the focused entry) -> focus should move to first remaining entry ("default")
        let removed = dir.remove("w2").unwrap();
        assert_eq!(removed.id, "w2");
        assert_eq!(dir.len(), 2);
        assert_eq!(dir.focused().id, "default");

        // Now entries are ["default", "w3"]
        // Focus w3 (index 1)
        dir.set_focused("w3");
        assert_eq!(dir.focused().id, "w3");

        // Remove "default" (index 0). Since focused was index 1 (> 0), focused index adjusts to 0 ("w3")
        let removed = dir.remove("default").unwrap();
        assert_eq!(removed.id, "default");
        assert_eq!(dir.len(), 1);
        assert_eq!(dir.focused().id, "w3");
    }

    #[test]
    fn remove_last_entry_and_missing() {
        let hub = DashboardHub::new(SnapshotSource::Tui, "/w1".to_string());
        let dir = WorkspaceDirectory::single(hub, DashboardServices::default());

        let err_missing = match dir.remove("missing") {
            Ok(_) => panic!("expected Missing error"),
            Err(e) => e,
        };
        assert_eq!(err_missing, DirectoryError::Missing);
        assert_eq!(err_missing.to_string(), "no such open folder");

        let err_last = match dir.remove("default") {
            Ok(_) => panic!("expected LastEntry error"),
            Err(e) => e,
        };
        assert_eq!(err_last, DirectoryError::LastEntry);
        assert_eq!(err_last.to_string(), "cannot close the last open folder");
        assert_eq!(dir.len(), 1);
    }

    #[test]
    fn find_by_path() {
        let hub = DashboardHub::new(SnapshotSource::Tui, "/repo/project1".to_string());
        let dir = WorkspaceDirectory::single(hub, DashboardServices::default());
        dir.insert(make_entry("w2", "/repo/project2")).unwrap();

        assert_eq!(
            dir.find_by_path("/repo/project1").map(|e| e.id.clone()),
            Some("default".to_string())
        );
        assert_eq!(
            dir.find_by_path("/repo/project2").map(|e| e.id.clone()),
            Some("w2".to_string())
        );
        assert!(dir.find_by_path("/repo/other").is_none());
    }

    #[test]
    fn tc_2300_resolve_ws_query() {
        let hub = DashboardHub::new(SnapshotSource::Tui, "/w1".to_string());
        let dir = WorkspaceDirectory::single(hub, DashboardServices::default());
        dir.insert(make_entry("w2", "/w2")).unwrap();

        // no ws -> focused ("default")
        assert_eq!(dir.resolve(None).unwrap().id, "default");
        assert_eq!(dir.resolve(Some("")).unwrap().id, "default");
        assert_eq!(dir.resolve(Some("foo=bar")).unwrap().id, "default");

        // ws=default -> default
        assert_eq!(dir.resolve(Some("ws=default")).unwrap().id, "default");
        assert_eq!(
            dir.resolve(Some("foo=1&ws=default&bar=2")).unwrap().id,
            "default"
        );

        // ws=w2 -> w2
        assert_eq!(dir.resolve(Some("ws=w2")).unwrap().id, "w2");

        // ws=nope -> None
        assert!(dir.resolve(Some("ws=nope")).is_none());
        assert!(dir.resolve(Some("foo=1&ws=nope")).is_none());

        // empty ws -> focused
        assert_eq!(dir.resolve(Some("ws=")).unwrap().id, "default");

        // change focused to w2, then no ws -> w2
        dir.set_focused("w2");
        assert_eq!(dir.resolve(None).unwrap().id, "w2");
        assert_eq!(dir.resolve(Some("other=param")).unwrap().id, "w2");
    }

    #[test]
    fn tc_2301_snapshot_is_running() {
        use crate::graph::GraphRunPhase;
        use crate::web::snapshot::{AgentView, DashboardSnapshot, GraphView, TurnView};

        let mut snapshot = DashboardSnapshot {
            schema: "test".to_string(),
            revision: 1,
            generated_at_ms: 1000,
            sent_at_ms: 1000,
            source: SnapshotSource::Tui,
            workspace: "/tmp".to_string(),
            ui: None,
            agents: vec![],
            graph: None,
            todos: vec![],
            jira_base_url: None,
            session: None,
            turn: None,
        };

        // All empty/idle -> not running
        assert!(!snapshot_is_running(&snapshot));

        // Agent busy -> running
        snapshot.agents.push(AgentView {
            id: "codex".to_string(),
            label: "Codex".to_string(),
            busy: true,
            phase: None,
            model: None,
            color: None,
            run: None,
            activity: None,
        });
        assert!(snapshot_is_running(&snapshot));
        snapshot.agents[0].busy = false;
        assert!(!snapshot_is_running(&snapshot));

        // Turn in finished states -> not running
        for state in ["idle", "done", "failed", "cancelled"] {
            snapshot.turn = Some(TurnView {
                state: state.to_string(),
                conversation_id: "c1".to_string(),
                graph_id: None,
                since_ms: 1000,
            });
            assert!(
                !snapshot_is_running(&snapshot),
                "state {state} should not be running"
            );
        }

        // Turn in non-finished states -> running
        for state in [
            "running",
            "pending",
            "started",
            "awaiting_approval",
            "needs_input",
        ] {
            snapshot.turn = Some(TurnView {
                state: state.to_string(),
                conversation_id: "c1".to_string(),
                graph_id: None,
                since_ms: 1000,
            });
            assert!(
                snapshot_is_running(&snapshot),
                "state {state} should be running"
            );
        }
        snapshot.turn = None;

        // Graph in finished phases -> not running
        let make_graph_view = |phase| GraphView {
            id: "g1".to_string(),
            phase,
            orchestrator_model: "m1".to_string(),
            user_prompt: "p1".to_string(),
            current_nodes: vec![],
            waiting_gate_node: None,
            counts: crate::web::snapshot::GraphCounts {
                pending: 0,
                ready: 0,
                running: 0,
                waiting_gate: 0,
                completed: 0,
                failed: 0,
                skipped: 0,
            },
            layers: 0,
            nodes: vec![],
            edges: vec![],
            triage: None,
            approval: None,
            plan_history: vec![],
            tasks: vec![],
            planner_calls: Default::default(),
            planner_io: vec![],
            turns: vec![],
            earlier_turns: 0,
            follow_up: vec![],
        };

        for phase in [
            GraphRunPhase::Completed,
            GraphRunPhase::Failed,
            GraphRunPhase::Cancelled,
            GraphRunPhase::Rejected,
        ] {
            snapshot.graph = Some(make_graph_view(phase));
            assert!(
                !snapshot_is_running(&snapshot),
                "phase {phase:?} should not be running"
            );
        }

        // Graph in non-finished phases -> running
        for phase in [
            GraphRunPhase::Active,
            GraphRunPhase::WaitingGate,
            GraphRunPhase::Triage,
            GraphRunPhase::Planning,
            GraphRunPhase::AwaitingApproval,
        ] {
            snapshot.graph = Some(make_graph_view(phase));
            assert!(
                snapshot_is_running(&snapshot),
                "phase {phase:?} should be running"
            );
        }
    }

    #[tokio::test]
    async fn tc_2303_opener_roundtrip() {
        let (hub, services) = (
            DashboardHub::new(SnapshotSource::Tui, "/test".to_string()),
            DashboardServices::default(),
        );
        let dir = WorkspaceDirectory::single(hub, services);
        assert!(dir.opener().is_none());

        let (tx, mut rx) = tokio::sync::mpsc::channel::<OpenRequest>(4);
        dir.set_opener(tx);
        let opener = dir.opener();
        assert!(opener.is_some());

        let req_opener = opener.unwrap();
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let path = std::path::PathBuf::from("/test/path");
        req_opener
            .send(OpenRequest {
                path: path.clone(),
                reply: reply_tx,
            })
            .await
            .unwrap();

        let received = rx.recv().await.expect("received open request");
        assert_eq!(received.path, path);
        let _ = received.reply.send(Err(OpenError {
            status: 404,
            message: "not found".to_string(),
        }));
        let res = reply_rx.await.unwrap();
        assert!(res.is_err());
    }
}
