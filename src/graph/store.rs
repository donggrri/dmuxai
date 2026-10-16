use std::{
    fs, io,
    path::{Path, PathBuf},
};

use super::types::GraphRun;

pub struct GraphStore {
    root: PathBuf,
}

impl GraphStore {
    pub fn new(workspace: &Path) -> Self {
        Self {
            root: workspace.join(".dmuxai").join("graphs"),
        }
    }

    pub fn save(&self, run: &GraphRun) -> io::Result<PathBuf> {
        fs::create_dir_all(&self.root)?;
        let path = self.root.join(format!("{}.json", run.plan.id));
        let json = serde_json::to_string_pretty(run)?;
        fs::write(&path, json)?;
        Ok(path)
    }
}

pub fn save_run(workspace: &Path, run: &GraphRun) -> io::Result<PathBuf> {
    GraphStore::new(workspace).save(run)
}

pub fn load_run(workspace: &Path, id: &str) -> io::Result<GraphRun> {
    let path = workspace
        .join(".dmuxai")
        .join("graphs")
        .join(format!("{id}.json"));
    let contents = fs::read_to_string(path)?;
    serde_json::from_str(&contents)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
