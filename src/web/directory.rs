//! Workspace directory for multi-workspace hosting.
//!
//! Tracks loaded workspaces and provides lookup and focused workspace access.
//! Currently always exactly one entry ("default").

use std::sync::Arc;

use crate::web::hub::DashboardHub;
use crate::web::server::DashboardServices;

/// A single loaded workspace entry.
pub struct WorkspaceEntry {
    pub id: String,
    pub hub: DashboardHub,
    pub services: DashboardServices,
}

/// Directory of active workspaces. Currently always exactly one entry.
pub struct WorkspaceDirectory {
    entries: Vec<Arc<WorkspaceEntry>>,
    focused: usize,
}

impl WorkspaceDirectory {
    /// Creates a directory with a single workspace entry ("default").
    ///
    /// Currently always exactly one entry.
    pub fn single(hub: DashboardHub, services: DashboardServices) -> Self {
        let entry = Arc::new(WorkspaceEntry {
            id: "default".to_string(),
            hub,
            services,
        });
        Self {
            entries: vec![entry],
            focused: 0,
        }
    }

    /// Returns the currently focused workspace entry.
    ///
    /// Currently always exactly one entry.
    pub fn focused(&self) -> Arc<WorkspaceEntry> {
        self.entries[self.focused].clone()
    }

    /// Looks up a workspace entry by id.
    ///
    /// Currently always exactly one entry.
    pub fn get(&self, id: &str) -> Option<Arc<WorkspaceEntry>> {
        self.entries.iter().find(|e| e.id == id).cloned()
    }

    /// Returns the number of workspaces in the directory.
    ///
    /// Currently always exactly one entry.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the directory has no workspaces.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::snapshot::SnapshotSource;

    #[test]
    fn single_workspace_directory() {
        let hub = DashboardHub::new(SnapshotSource::Tui, "/tmp".to_string());
        let services = DashboardServices::default();
        let dir = WorkspaceDirectory::single(hub, services);

        assert_eq!(dir.len(), 1);
        assert!(!dir.is_empty());
        assert_eq!(dir.focused().id, "default");
        assert!(dir.get("default").is_some());
        assert_eq!(dir.get("default").unwrap().id, "default");
        assert!(dir.get("other").is_none());
    }
}
