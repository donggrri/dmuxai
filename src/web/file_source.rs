use std::path::{Path, PathBuf};

use crate::graph::GraphRun;
use crate::web::snapshot::{GraphView, snapshot_from_run};

pub struct FileGraphSource {
    workspace: PathBuf,
    last: Option<GraphView>,
}

impl FileGraphSource {
    pub fn new(workspace: &Path) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            last: None,
        }
    }

    pub fn poll(&mut self) -> Option<GraphView> {
        let dir = self.workspace.join(".dmuxai").join("graphs");
        if !dir.is_dir() {
            self.last = None;
            return None;
        }

        let read_dir = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => return self.last.clone(),
        };

        let mut json_files = Vec::new();
        for entry in read_dir.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() || !file_type.is_file() {
                continue;
            }

            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json")
                && let Ok(metadata) = entry.metadata()
                && let Ok(mtime) = metadata.modified()
            {
                json_files.push((path, mtime));
            }
        }

        if json_files.is_empty() {
            return self.last.clone();
        }

        json_files.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));

        let (newest_path, _) = &json_files[0];
        let newest_run = std::fs::read_to_string(newest_path)
            .ok()
            .and_then(|c| serde_json::from_str::<GraphRun>(&c).ok());

        if let Some(run) = newest_run {
            self.last = Some(snapshot_from_run(&run));
            return self.last.clone();
        }

        // If the newest file fails to parse and self.last is Some, keep and return self.last (do not fall back to older files)
        if self.last.is_some() {
            return self.last.clone();
        }

        // Only when self.last is None, fall back to the newest older file that parses
        for (path, _) in &json_files[1..] {
            if let Ok(contents) = std::fs::read_to_string(path)
                && let Ok(run) = serde_json::from_str::<GraphRun>(&contents)
            {
                self.last = Some(snapshot_from_run(&run));
                return self.last.clone();
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::time::{Duration, SystemTime};

    use crate::graph::{GraphLimits, GraphPlan, GraphRunPhase};

    fn make_run_json(id: &str) -> String {
        let run = GraphRun {
            plan: GraphPlan {
                schema: "dmuxai.graph/v1".to_string(),
                id: id.to_string(),
                orchestrator_model: "gpt-4o".to_string(),
                user_prompt: "prompt".to_string(),
                limits: GraphLimits::default(),
                nodes: Vec::new(),
                edges: Vec::new(),
            },
            conversation_id: "conv-1".to_string(),
            phase: GraphRunPhase::Active,
            waiting_gate_node: None,
        };
        serde_json::to_string(&run).unwrap()
    }

    // TC-528: file_source_picks_latest_valid_run
    #[test]
    fn file_source_picks_latest_valid_run() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace = temp_dir.path();
        let mut source = FileGraphSource::new(workspace);

        // Missing dir -> None
        assert_eq!(source.poll(), None);

        let graphs_dir = workspace.join(".dmuxai").join("graphs");
        fs::create_dir_all(&graphs_dir).unwrap();

        let base_time = SystemTime::now() - Duration::from_secs(100);

        // a.json (older valid)
        let a_path = graphs_dir.join("a.json");
        fs::write(&a_path, make_run_json("run-a")).unwrap();
        let a_file = File::open(&a_path).unwrap();
        a_file.set_modified(base_time).unwrap();

        // b.json (newer valid)
        let b_path = graphs_dir.join("b.json");
        fs::write(&b_path, make_run_json("run-b")).unwrap();
        let b_file = File::open(&b_path).unwrap();
        b_file
            .set_modified(base_time + Duration::from_secs(10))
            .unwrap();

        // Poll after valid newest b exists -> picks b
        let polled_b = source.poll().expect("expected graph from b.json");
        assert_eq!(polled_b.id, "run-b");

        // Write a newer malformed c while an older valid a exists
        let c_path = graphs_dir.join("c.json");
        fs::write(&c_path, "{ malformed json").unwrap();
        let c_file = File::open(&c_path).unwrap();
        c_file
            .set_modified(base_time + Duration::from_secs(20))
            .unwrap();

        // After valid newest b has been polled, poll still returns b (not a)
        let polled_c = source.poll().expect("expected graph kept from b.json");
        assert_eq!(polled_c.id, "run-b");

        // Test fresh source where self.last is None: falls back to newest older file that parses (b)
        let mut fresh_source = FileGraphSource::new(workspace);
        let polled_fresh = fresh_source
            .poll()
            .expect("expected fallback to valid b.json");
        assert_eq!(polled_fresh.id, "run-b");

        // Missing dir clears last and returns None
        fs::remove_dir_all(&graphs_dir).unwrap();
        assert_eq!(source.poll(), None);
        assert_eq!(fresh_source.poll(), None);
    }
}
