//! Per-turn changed files (FR-748). OWNER: deepseek.
//!
//! Contract:
//! - `pub struct WorkspaceScan { .. }`; `pub fn scan_workspace(root: &Path, limit: usize) -> WorkspaceScan`
//!   (path -> `(size, mtime_ns, content_fingerprint)` skipping IGNORED_DIRS, `truncated` flag).
//!   For files smaller than 256 KiB, `content_fingerprint` is an FNV-1a hash of the first 4 KiB;
//!   larger files use `0` for the fingerprint slot.
//! - `#[serde(rename_all="snake_case")] pub enum ChangeKind { Created, Modified, Deleted }`
//! - `pub struct FileChange { pub path: String, pub change: ChangeKind }`
//! - `pub fn diff_scans(before: &WorkspaceScan, after: &WorkspaceScan, max: usize) -> (Vec<FileChange>, bool)`
//! - `pub struct TurnChanges { pub id: String, pub agent: String, pub graph_id: Option<String>,
//!    pub node_id: Option<String>, pub started_ms: u64, pub finished_ms: u64,
//!    pub files: Vec<FileChange>, pub truncated: bool, pub output_tail: Vec<String>, pub links: Vec<String> }`
//! - `#[derive(Clone, Default)] pub struct ChangeLog(Arc<Mutex<VecDeque<TurnChanges>>>)` with
//!   `push(TurnChanges)` (keep newest 50), `list(node: Option<&str>) -> Vec<TurnChanges>` newest first.
//! - `pub fn output_tail(text: &str) -> Vec<String>` last 40 lines, each <= 500 chars on a char boundary.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::Serialize;

use super::IGNORED_DIRS;

/// How many turns the log keeps (FR-748).
const MAX_TURNS: usize = 50;
/// How many lines of agent output are kept (NFR-752).
const MAX_TAIL_LINES: usize = 40;
/// Maximum characters per kept output line (NFR-752).
const MAX_TAIL_CHARS: usize = 500;
/// Files at or above this size are compared by size/mtime only.
const FINGERPRINT_MAX_BYTES: u64 = 256 * 1024;
/// Bytes hashed for the content fingerprint on small files.
const FINGERPRINT_READ_BYTES: usize = 4 * 1024;

/// A point-in-time snapshot of the workspace (FR-748). Values are
/// `(size, mtime_ns, content_fingerprint)` keyed by `/`-joined workspace path.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceScan {
    pub files: HashMap<String, (u64, u64, u64)>,
    pub truncated: bool,
}

/// Scans `root` up to `limit` files, skipping `IGNORED_DIRS` (FR-748).
pub fn scan_workspace(root: &Path, limit: usize) -> WorkspaceScan {
    let mut files = HashMap::new();
    let mut truncated = false;
    let mut stack = vec![(root.to_path_buf(), String::new())];
    'walk: while let Some((dir, prefix)) = stack.pop() {
        let Ok(reader) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in reader.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if file_type.is_dir() {
                if !IGNORED_DIRS.contains(&name.as_str()) {
                    stack.push((entry.path(), relative));
                }
                continue;
            }
            let metadata = if file_type.is_symlink() {
                match std::fs::metadata(entry.path()) {
                    Ok(meta) if meta.is_file() => meta,
                    _ => continue,
                }
            } else if file_type.is_file() {
                match entry.metadata() {
                    Ok(meta) => meta,
                    Err(_) => continue,
                }
            } else {
                continue;
            };
            if files.len() >= limit {
                truncated = true;
                break 'walk;
            }
            let size = metadata.len();
            let fingerprint = content_fingerprint(&entry.path(), size);
            files.insert(relative, (size, mtime_ns(&metadata), fingerprint));
        }
    }
    WorkspaceScan { files, truncated }
}

fn content_fingerprint(path: &Path, size: u64) -> u64 {
    if size >= FINGERPRINT_MAX_BYTES {
        return 0;
    }
    let Ok(mut file) = File::open(path) else {
        return 0;
    };
    let mut buf = vec![0u8; FINGERPRINT_READ_BYTES];
    let read = file.read(&mut buf).unwrap_or(0);
    fnv1a64(&buf[..read])
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3u64);
    }
    hash
}

fn mtime_ns(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
}

/// How a file changed between two scans (FR-748).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Created,
    Modified,
    Deleted,
}

/// One changed file in a turn (FR-748).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileChange {
    pub path: String,
    pub change: ChangeKind,
}

/// Diffs two scans, sorted by path and capped at `max` (FR-748, NFR-752).
pub fn diff_scans(
    before: &WorkspaceScan,
    after: &WorkspaceScan,
    max: usize,
) -> (Vec<FileChange>, bool) {
    let mut changes = Vec::new();
    for (path, current) in &after.files {
        match before.files.get(path) {
            None => changes.push(FileChange {
                path: path.clone(),
                change: ChangeKind::Created,
            }),
            Some(previous) if previous != current => changes.push(FileChange {
                path: path.clone(),
                change: ChangeKind::Modified,
            }),
            Some(_) => {}
        }
    }
    for path in before.files.keys() {
        if !after.files.contains_key(path) {
            changes.push(FileChange {
                path: path.clone(),
                change: ChangeKind::Deleted,
            });
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    let mut truncated = before.truncated || after.truncated;
    if changes.len() > max {
        changes.truncate(max);
        truncated = true;
    }
    (changes, truncated)
}

/// The record of one agent turn's file changes (FR-748).
#[derive(Debug, Clone, Serialize)]
pub struct TurnChanges {
    pub id: String,
    pub agent: String,
    pub graph_id: Option<String>,
    pub node_id: Option<String>,
    pub started_ms: u64,
    pub finished_ms: u64,
    pub files: Vec<FileChange>,
    pub truncated: bool,
    pub output_tail: Vec<String>,
    pub links: Vec<String>,
}

/// In-memory turn history shared with the web API (FR-748).
#[derive(Clone, Default)]
pub struct ChangeLog(Arc<Mutex<VecDeque<TurnChanges>>>);

impl ChangeLog {
    /// Appends a turn, keeping only the newest [`MAX_TURNS`] records.
    pub fn push(&self, turn: TurnChanges) {
        let mut log = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        log.push_back(turn);
        while log.len() > MAX_TURNS {
            log.pop_front();
        }
    }

    /// The turns newest-first, optionally filtered to one graph node.
    pub fn list(&self, node: Option<&str>) -> Vec<TurnChanges> {
        let log = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        log.iter()
            .rev()
            .filter(|turn| node.is_none_or(|node| turn.node_id.as_deref() == Some(node)))
            .cloned()
            .collect()
    }
}

/// The last [`MAX_TAIL_LINES`] lines of `text`, each cut to
/// [`MAX_TAIL_CHARS`] characters on a char boundary (NFR-752).
pub fn output_tail(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(MAX_TAIL_LINES);
    lines[start..]
        .iter()
        .map(|line| line.chars().take(MAX_TAIL_CHARS).collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(id: usize, node: Option<&str>) -> TurnChanges {
        TurnChanges {
            id: id.to_string(),
            agent: "codex".to_owned(),
            graph_id: None,
            node_id: node.map(str::to_owned),
            started_ms: id as u64,
            finished_ms: id as u64 + 1,
            files: Vec::new(),
            truncated: false,
            output_tail: Vec::new(),
            links: Vec::new(),
        }
    }

    // TC-751: scan_diff_reports_created_modified_deleted (FR-748)
    #[test]
    fn scan_diff_reports_created_modified_deleted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keep.md"), "keep").unwrap();
        std::fs::write(dir.path().join("gone.md"), "gone").unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();

        let before = scan_workspace(dir.path(), 1000);

        std::fs::write(dir.path().join("new.md"), "brand new file").unwrap();
        std::fs::write(dir.path().join("keep.md"), "keep but longer now").unwrap();
        std::fs::remove_file(dir.path().join("gone.md")).unwrap();
        std::fs::create_dir_all(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target/ignored.md"), "ignored").unwrap();

        let after = scan_workspace(dir.path(), 1000);
        let (changes, truncated) = diff_scans(&before, &after, 200);

        let by_path: HashMap<&str, ChangeKind> = changes
            .iter()
            .map(|change| (change.path.as_str(), change.change))
            .collect();
        assert!(!truncated, "{changes:?}");
        assert_eq!(by_path.len(), 3, "{changes:?}");
        assert_eq!(by_path.get("new.md"), Some(&ChangeKind::Created));
        assert_eq!(by_path.get("keep.md"), Some(&ChangeKind::Modified));
        assert_eq!(by_path.get("gone.md"), Some(&ChangeKind::Deleted));
        assert!(!by_path.contains_key("target/ignored.md"));
    }

    #[cfg(unix)]
    #[test]
    fn scan_detects_same_size_rewrite_when_mtime_unchanged() {
        use filetime::{FileTime, set_file_mtime};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("same.md");
        std::fs::write(&path, "0123456789").unwrap();
        let before = scan_workspace(dir.path(), 100);

        let prior_mtime = FileTime::from_last_modification_time(&std::fs::metadata(&path).unwrap());
        std::fs::write(&path, "9876543210").unwrap();
        set_file_mtime(&path, prior_mtime).unwrap();

        let after = scan_workspace(dir.path(), 100);
        let (changes, truncated) = diff_scans(&before, &after, 10);
        assert!(!truncated);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "same.md");
        assert_eq!(changes[0].change, ChangeKind::Modified);
    }

    #[test]
    fn diff_scans_truncates_at_max() {
        let mut before = WorkspaceScan::default();
        let mut after = WorkspaceScan::default();
        for index in 0..10 {
            after.files.insert(format!("f{index}.md"), (1, 1, 0));
        }
        before.files.insert("f0.md".to_owned(), (1, 1, 0));
        let (changes, truncated) = diff_scans(&before, &after, 3);
        assert!(truncated);
        assert_eq!(changes.len(), 3);
    }

    #[test]
    fn change_log_keeps_newest_and_filters_by_node() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ChangeLog>();

        let log = ChangeLog::default();
        for index in 0..60usize {
            let node = if index % 2 == 0 { Some("a") } else { Some("b") };
            log.push(turn(index, node));
        }

        let all = log.list(None);
        assert_eq!(all.len(), MAX_TURNS);
        assert_eq!(all.first().unwrap().id, "59");
        assert_eq!(all.last().unwrap().id, "10");

        let node_a = log.list(Some("a"));
        assert_eq!(node_a.len(), 25);
        assert!(
            node_a
                .iter()
                .all(|turn| turn.node_id.as_deref() == Some("a"))
        );
        assert_eq!(node_a.first().unwrap().id, "58");
        assert_eq!(node_a.last().unwrap().id, "10");

        assert!(log.list(Some("missing")).is_empty());
    }

    #[test]
    fn output_tail_bounds_lines_and_width() {
        let wide = "x".repeat(600);
        let mut text = String::new();
        for index in 0..50 {
            text.push_str(&format!("line {index}\n"));
        }
        text.push_str(&wide);
        let tail = output_tail(&text);
        assert_eq!(tail.len(), MAX_TAIL_LINES);
        assert_eq!(tail.first().unwrap(), "line 11");
        assert_eq!(tail.last().unwrap().chars().count(), MAX_TAIL_CHARS);
        assert!(tail.last().unwrap().chars().all(|c| c == 'x'));
    }
}
