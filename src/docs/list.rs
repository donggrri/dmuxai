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

use serde::{Deserialize, Serialize};

use super::confine::Workspace;
use super::{DocKind, DocsError, IGNORED_DIRS, MAX_TEXT_BYTES, mtime_ms};

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

/// One listable workspace tree entry (SPEC-docs-tree FR-830).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntry {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub entry_type: String,
    pub kind: DocKind,
    pub size: u64,
    pub mtime_ms: u64,
    pub editable: bool,
    pub has_children: bool,
}

/// The tree listing result for one directory (FR-830).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeList {
    pub dir: String,
    pub entries: Vec<TreeEntry>,
    pub truncated: bool,
}

fn file_entry_props(path: &Path, meta: &std::fs::Metadata) -> (DocKind, u64, u64, bool) {
    let kind = DocKind::for_path(path);
    let size = meta.len();
    let editable = kind.editable() && size <= MAX_TEXT_BYTES;
    let mtime = mtime_ms(meta);
    (kind, size, mtime, editable)
}

fn make_entry(relative: &str, path: &Path, meta: &std::fs::Metadata) -> DocEntry {
    let (kind, size, mtime_ms, editable) = file_entry_props(path, meta);
    DocEntry {
        path: relative.to_owned(),
        kind,
        size,
        mtime_ms,
        editable,
    }
}

fn dir_has_children(ws: &Workspace, dir_path: &Path) -> bool {
    let Ok(reader) = std::fs::read_dir(dir_path) else {
        return false;
    };
    for entry in reader.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if IGNORED_DIRS.contains(&name.as_str()) {
            continue;
        }
        if file_type.is_dir() {
            return true;
        }
        if file_type.is_file() {
            return true;
        }
        if file_type.is_symlink() {
            let Ok(target) = std::fs::canonicalize(entry.path()) else {
                continue;
            };
            if !target.starts_with(ws.root()) {
                continue;
            }
            if let Ok(meta) = std::fs::metadata(entry.path())
                && meta.is_file()
            {
                return true;
            }
        }
    }
    false
}

/// Lists the immediate children of `dir` inside `ws` (SPEC-docs-tree FR-830, FR-831).
pub fn list_children(ws: &Workspace, dir: &str, limit: usize) -> Result<TreeList, DocsError> {
    let (target_dir, rel_dir) = if dir.is_empty() || dir == "." {
        (ws.root().to_path_buf(), String::new())
    } else {
        let norm = Workspace::normalize_relative(dir)?;
        for part in norm.split('/') {
            if IGNORED_DIRS.contains(&part) {
                return Err(DocsError::NotFound);
            }
        }
        let resolved = ws.resolve(&norm)?;
        if let Ok(rel) = resolved.absolute.strip_prefix(ws.root()) {
            for component in rel.components() {
                if let std::path::Component::Normal(c) = component
                    && let Some(s) = c.to_str()
                    && IGNORED_DIRS.contains(&s)
                {
                    return Err(DocsError::NotFound);
                }
            }
        }
        let meta = std::fs::metadata(&resolved.absolute)?;
        if !meta.is_dir() {
            return Err(DocsError::NotAFile);
        }
        (resolved.absolute, resolved.relative)
    };

    let reader = std::fs::read_dir(&target_dir)?;
    let mut dir_entries = Vec::new();
    let mut file_entries = Vec::new();

    for entry in reader.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let path = if rel_dir.is_empty() {
            name.clone()
        } else {
            format!("{rel_dir}/{name}")
        };

        if file_type.is_dir() {
            if IGNORED_DIRS.contains(&name.as_str()) {
                continue;
            }
            let mtime = entry.metadata().ok().map(|m| mtime_ms(&m)).unwrap_or(0);
            dir_entries.push(TreeEntry {
                name,
                path,
                entry_type: "dir".to_string(),
                kind: DocKind::Dir,
                size: 0,
                mtime_ms: mtime,
                editable: false,
                has_children: false,
            });
            continue;
        }

        if file_type.is_symlink() {
            let Ok(target) = std::fs::canonicalize(entry.path()) else {
                continue;
            };
            if !target.starts_with(ws.root()) {
                continue;
            }
            let Ok(meta) = std::fs::metadata(entry.path()) else {
                continue;
            };
            if meta.is_dir() {
                // Symlinked directories are omitted
                continue;
            }
            if meta.is_file() {
                let (kind, size, mtime, editable) = file_entry_props(&entry.path(), &meta);
                file_entries.push(TreeEntry {
                    name,
                    path,
                    entry_type: "file".to_string(),
                    kind,
                    size,
                    mtime_ms: mtime,
                    editable,
                    has_children: false,
                });
            }
            continue;
        }

        if file_type.is_file()
            && let Ok(meta) = entry.metadata()
        {
            let (kind, size, mtime, editable) = file_entry_props(&entry.path(), &meta);
            file_entries.push(TreeEntry {
                name,
                path,
                entry_type: "file".to_string(),
                kind,
                size,
                mtime_ms: mtime,
                editable,
                has_children: false,
            });
        }
    }

    dir_entries.sort_by(|a, b| a.name.cmp(&b.name));
    file_entries.sort_by(|a, b| a.name.cmp(&b.name));

    let total = dir_entries.len() + file_entries.len();
    let truncated = total > limit;

    let mut entries = dir_entries;
    entries.extend(file_entries);
    if entries.len() > limit {
        entries.truncate(limit);
    }

    for entry in entries.iter_mut() {
        if entry.entry_type == "dir" {
            let child_path = if entry.path.is_empty() {
                ws.root().to_path_buf()
            } else {
                ws.root().join(&entry.path)
            };
            entry.has_children = dir_has_children(ws, &child_path);
        }
    }

    Ok(TreeList {
        dir: rel_dir,
        entries,
        truncated,
    })
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

    // TC-830: tree_children_lists_one_level (SPEC-docs-tree FR-830, FR-831)
    #[test]
    fn tree_children_lists_one_level() {
        let dir = tempfile::tempdir().unwrap();
        for ignored in IGNORED_DIRS {
            std::fs::create_dir_all(dir.path().join(ignored).join("nested")).unwrap();
            std::fs::write(dir.path().join(ignored).join("nested/x.md"), "# x").unwrap();
        }
        std::fs::create_dir_all(dir.path().join("docs/sub")).unwrap();
        std::fs::create_dir_all(dir.path().join("empty_dir")).unwrap();
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
                dir.path().join("link_out.md"),
            )
            .unwrap();
            std::os::unix::fs::symlink(dir.path().join("docs/a.md"), dir.path().join("link_in.md"))
                .unwrap();
        }

        let ws = Workspace::new(dir.path()).unwrap();

        // 1. Root listing
        let root_tree = list_children(&ws, "", 100).expect("list root children");
        assert!(!root_tree.truncated);
        assert_eq!(root_tree.dir, "");

        // Dirs first, sorted by name
        let dir_entries: Vec<&TreeEntry> = root_tree
            .entries
            .iter()
            .filter(|e| e.entry_type == "dir")
            .collect();
        let file_entries: Vec<&TreeEntry> = root_tree
            .entries
            .iter()
            .filter(|e| e.entry_type == "file")
            .collect();

        assert_eq!(
            root_tree.entries.len(),
            dir_entries.len() + file_entries.len()
        );
        // Verify directories appear before files in the entries vector
        for (i, d) in dir_entries.iter().enumerate() {
            assert_eq!(&root_tree.entries[i], *d);
        }

        let dir_names: Vec<&str> = dir_entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(dir_names, vec!["docs", "empty_dir"]);

        let docs_entry = dir_entries.iter().find(|e| e.name == "docs").unwrap();
        assert_eq!(docs_entry.kind, DocKind::Dir);
        assert_eq!(docs_entry.kind, "dir");
        assert_eq!(docs_entry.size, 0);
        assert!(!docs_entry.editable);
        assert!(docs_entry.has_children);

        let empty_entry = dir_entries.iter().find(|e| e.name == "empty_dir").unwrap();
        assert!(!empty_entry.has_children);

        // Files: top.txt (and link_in.md on unix)
        let file_names: Vec<&str> = file_entries.iter().map(|e| e.name.as_str()).collect();
        assert!(file_names.contains(&"top.txt"));
        #[cfg(unix)]
        {
            assert!(file_names.contains(&"link_in.md"));
            assert!(!file_names.contains(&"link_out.md"));
            assert!(!dir_names.contains(&"linkdir"));
        }
        for ignored in IGNORED_DIRS {
            assert!(!dir_names.contains(ignored));
        }

        // 2. Subdir listing: "docs"
        let docs_tree = list_children(&ws, "docs", 100).expect("list docs children");
        assert!(!docs_tree.truncated);
        assert_eq!(docs_tree.dir, "docs");
        assert_eq!(docs_tree.entries.len(), 2);
        assert_eq!(docs_tree.entries[0].name, "sub");
        assert_eq!(docs_tree.entries[0].path, "docs/sub");
        assert_eq!(docs_tree.entries[0].entry_type, "dir");
        assert!(docs_tree.entries[0].has_children);
        assert_eq!(docs_tree.entries[1].name, "a.md");
        assert_eq!(docs_tree.entries[1].path, "docs/a.md");
        assert_eq!(docs_tree.entries[1].entry_type, "file");
        assert_eq!(docs_tree.entries[1].kind, DocKind::Markdown);
        assert!(docs_tree.entries[1].editable);
        assert!(!docs_tree.entries[1].has_children);

        // 3. Sub-subdir listing: "docs/sub"
        let sub_tree = list_children(&ws, "docs/sub", 100).expect("list sub children");
        assert_eq!(sub_tree.entries.len(), 1);
        assert_eq!(sub_tree.entries[0].name, "b.rs");
        assert_eq!(sub_tree.entries[0].path, "docs/sub/b.rs");
        assert_eq!(sub_tree.entries[0].kind, DocKind::Text);

        // 4. Truncation when limit is smaller than total entries
        let capped = list_children(&ws, "", 2).expect("capped children");
        assert!(capped.truncated);
        assert_eq!(capped.entries.len(), 2);

        // 5. Confinement & error checks
        assert_eq!(
            list_children(&ws, "../outside", 100),
            Err(DocsError::OutsideWorkspace)
        );
        assert_eq!(
            list_children(&ws, "/etc", 100),
            Err(DocsError::OutsideWorkspace)
        );
        assert_eq!(list_children(&ws, "target", 100), Err(DocsError::NotFound));
        assert_eq!(list_children(&ws, "missing", 100), Err(DocsError::NotFound));
        assert_eq!(list_children(&ws, "top.txt", 100), Err(DocsError::NotAFile));
    }
}
