//! Workspace document list (FR-741). OWNER: deepseek.
//!
//! Contract:
//! - `pub struct DocEntry { pub path: String, pub kind: DocKind, pub size: u64, pub mtime_ms: u64,
//!    pub editable: bool }` (Serialize)
//! - `pub struct DocList { pub entries: Vec<DocEntry>, pub truncated: bool }` (Serialize)
//! - `pub fn list_documents(ws: &Workspace, limit: usize) -> DocList` — skip IGNORED_DIRS and
//!   symlinked directories, sorted by path, `editable` = kind.editable() && size <= MAX_TEXT_BYTES.
//! - Walk stops after `20 * limit` regular files (NFR-752); `truncated` is set when capped.

use std::path::Path;

use serde::Serialize;

use super::confine::Workspace;
use super::{DocKind, IGNORED_DIRS, MAX_TEXT_BYTES, mtime_ms};

/// Stop collecting after this many files relative to `limit` (NFR-752).
const WALK_CAP_MULTIPLIER: usize = 20;

/// One listable workspace file (FR-741).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DocEntry {
    pub path: String,
    pub kind: DocKind,
    pub size: u64,
    pub mtime_ms: u64,
    pub editable: bool,
}

/// The document list plus the NFR-752 truncation flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DocList {
    pub entries: Vec<DocEntry>,
    pub truncated: bool,
}

/// Lists every regular file under `ws` (FR-741), sorted by workspace path and
/// capped at `limit` entries. Ignored directories and symlinked directories are
/// never descended into.
pub fn list_documents(ws: &Workspace, limit: usize) -> DocList {
    let mut entries = Vec::new();
    let walk_cap = limit.saturating_mul(WALK_CAP_MULTIPLIER);
    let mut truncated = false;
    let root = ws.root();
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
                if IGNORED_DIRS.contains(&name.as_str()) {
                    continue;
                }
                stack.push((entry.path(), relative));
                continue;
            }
            if file_type.is_symlink() {
                let Ok(target) = std::fs::canonicalize(entry.path()) else {
                    continue;
                };
                if !target.starts_with(root) {
                    continue;
                }
                match std::fs::metadata(entry.path()) {
                    Ok(meta) if meta.is_dir() => continue,
                    Ok(meta) if meta.is_file() => {
                        if entries.len() >= walk_cap {
                            truncated = true;
                            break 'walk;
                        }
                        entries.push(make_entry(&relative, &entry.path(), &meta));
                    }
                    _ => continue,
                }
                continue;
            }
            if file_type.is_file()
                && let Ok(meta) = entry.metadata()
            {
                if entries.len() >= walk_cap {
                    truncated = true;
                    break 'walk;
                }
                entries.push(make_entry(&relative, &entry.path(), &meta));
            }
        }
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    if entries.len() > limit {
        truncated = true;
        entries.truncate(limit);
    }
    DocList { entries, truncated }
}

fn make_entry(relative: &str, path: &Path, meta: &std::fs::Metadata) -> DocEntry {
    let kind = DocKind::for_path(path);
    let size = meta.len();
    DocEntry {
        path: relative.to_owned(),
        kind,
        size,
        mtime_ms: mtime_ms(meta),
        editable: kind.editable() && size <= MAX_TEXT_BYTES,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // TC-742: list_skips_ignored_dirs_and_truncates (FR-741)
    #[test]
    fn list_skips_ignored_dirs_and_truncates() {
        let dir = tempfile::tempdir().unwrap();
        for ignored in IGNORED_DIRS {
            std::fs::create_dir_all(dir.path().join(ignored).join("nested")).unwrap();
            std::fs::write(dir.path().join(ignored).join("nested/x.md"), "# x").unwrap();
        }
        std::fs::create_dir_all(dir.path().join("docs/sub")).unwrap();
        std::fs::write(dir.path().join("docs/a.md"), "# a").unwrap();
        std::fs::write(dir.path().join("docs/sub/b.rs"), "fn b() {}").unwrap();
        std::fs::write(dir.path().join("top.txt"), "top").unwrap();

        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("secret.md"), "secret").unwrap();
            std::os::unix::fs::symlink(outside.path(), dir.path().join("linkdir")).unwrap();
            std::fs::write(outside.path().join("outside.md"), "outside").unwrap();
            std::os::unix::fs::symlink(
                outside.path().join("outside.md"),
                dir.path().join("link.md"),
            )
            .unwrap();
        }

        let ws = Workspace::new(dir.path()).unwrap();
        let list = list_documents(&ws, 100);

        let paths: Vec<&str> = list.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(!list.truncated);
        assert_eq!(paths, vec!["docs/a.md", "docs/sub/b.rs", "top.txt"]);
        for ignored in IGNORED_DIRS {
            assert!(
                paths.iter().all(|p| !p.starts_with(&format!("{ignored}/"))),
                "ignored dir {ignored} must not appear in {paths:?}"
            );
        }
        assert!(!paths.contains(&"linkdir/secret.md"));
        #[cfg(unix)]
        assert!(!paths.contains(&"link.md"));

        let markdown = list.entries.iter().find(|e| e.path == "docs/a.md").unwrap();
        assert_eq!(markdown.kind, DocKind::Markdown);
        assert!(markdown.editable);
        assert!(markdown.size > 0);
        let source = list
            .entries
            .iter()
            .find(|e| e.path == "docs/sub/b.rs")
            .unwrap();
        assert_eq!(source.kind, DocKind::Text);
        assert!(source.editable);
        assert!(source.mtime_ms > 0);

        let capped = list_documents(&ws, 2);
        assert!(capped.truncated);
        assert_eq!(capped.entries.len(), 2);
        assert_eq!(capped.entries[0].path, "docs/a.md");
    }
}
