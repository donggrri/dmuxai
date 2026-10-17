//! Workspace confinement (SPEC-docs-preview FR-740).

use std::path::{Component, Path, PathBuf};

use super::DocsError;

/// A canonical workspace root. Construct once per server/app.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

/// A file path proven to live inside the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPath {
    /// Canonical absolute path (symlinks resolved).
    pub absolute: PathBuf,
    /// `/`-separated path relative to the root, as requested (not the symlink
    /// target), e.g. `docs/a.md`.
    pub relative: String,
}

impl Workspace {
    pub fn new(root: &Path) -> std::io::Result<Self> {
        Ok(Self {
            root: dunce_canonicalize(root)?,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Validates the syntax of a workspace path without touching the disk:
    /// relative, no `..`, no NUL, no drive/UNC prefix, no backslash, non-empty.
    /// Returns the normalized `/`-joined form (with `.` components removed).
    pub fn normalize_relative(requested: &str) -> Result<String, DocsError> {
        if requested.is_empty()
            || requested.contains('\0')
            || requested.contains('\\')
            || requested.starts_with('/')
            || requested.contains(':')
        {
            return Err(DocsError::OutsideWorkspace);
        }
        let mut parts: Vec<&str> = Vec::new();
        for component in Path::new(requested).components() {
            match component {
                Component::Normal(part) => match part.to_str() {
                    Some(part) => parts.push(part),
                    None => return Err(DocsError::OutsideWorkspace),
                },
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(DocsError::OutsideWorkspace);
                }
            }
        }
        if parts.is_empty() {
            return Err(DocsError::OutsideWorkspace);
        }
        Ok(parts.join("/"))
    }

    /// Resolves an existing workspace path. Symlinks are followed and the
    /// canonical target must still be inside the root.
    pub fn resolve(&self, requested: &str) -> Result<ResolvedPath, DocsError> {
        let relative = Self::normalize_relative(requested)?;
        let joined = self.root.join(&relative);
        let absolute = match dunce_canonicalize(&joined) {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // A dangling symlink is reported as outside rather than missing
                // when its link itself exists.
                if std::fs::symlink_metadata(&joined).is_ok() {
                    return Err(DocsError::OutsideWorkspace);
                }
                return Err(DocsError::NotFound);
            }
            Err(error) => return Err(DocsError::Io(error.to_string())),
        };
        if !absolute.starts_with(&self.root) {
            return Err(DocsError::OutsideWorkspace);
        }
        Ok(ResolvedPath { absolute, relative })
    }

    /// Maps an absolute path (e.g. from a `file://` URI) to a workspace path,
    /// if it exists and is inside the root after canonicalization.
    pub fn relative_of(&self, absolute: &Path) -> Option<String> {
        let canonical = dunce_canonicalize(absolute).ok()?;
        let rel = canonical.strip_prefix(&self.root).ok()?;
        let parts: Vec<String> = rel
            .components()
            .map(|c| match c {
                Component::Normal(p) => p.to_str().map(str::to_owned),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        if parts.is_empty() {
            return None;
        }
        Some(parts.join("/"))
    }
}

/// `fs::canonicalize` without the Windows `\\?\` verbatim prefix when the
/// path can be expressed without it, so `starts_with` comparisons agree.
fn dunce_canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    let canonical = std::fs::canonicalize(path)?;
    #[cfg(windows)]
    {
        let text = canonical.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\")
            && !rest.starts_with("UNC\\")
        {
            return Ok(PathBuf::from(rest));
        }
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    // TC-740: rejects_traversal_and_absolute_paths (FR-740)
    #[test]
    fn rejects_traversal_and_absolute_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("ok.md"), "# ok").unwrap();
        std::fs::write(dir.path().join("docs/a.md"), "a").unwrap();
        let ws = Workspace::new(dir.path()).unwrap();

        for bad in [
            "../x",
            "/etc/passwd",
            "a/../../x",
            "docs/../ok.md",
            "C:\\x",
            "C:/x",
            "\\\\server\\share",
            "a\0b",
            "",
            ".",
        ] {
            assert_eq!(
                ws.resolve(bad),
                Err(DocsError::OutsideWorkspace),
                "{bad:?} must be rejected"
            );
        }
        let ok = ws.resolve("ok.md").unwrap();
        assert_eq!(ok.relative, "ok.md");
        assert!(ok.absolute.starts_with(ws.root()));
        assert_eq!(ws.resolve("./docs/a.md").unwrap().relative, "docs/a.md");
        assert_eq!(ws.resolve("missing.md"), Err(DocsError::NotFound));
        let outside = tempfile::tempdir().unwrap();
        assert_eq!(ws.relative_of(outside.path()), None);
        assert_eq!(
            ws.relative_of(&dir.path().join("docs/a.md")).as_deref(),
            Some("docs/a.md")
        );
    }

    // TC-741: rejects_symlink_escape (FR-740)
    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.md"), "secret").unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/ok.md"), "ok").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.md"), dir.path().join("link"))
            .unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("outdir")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("sub/ok.md"), dir.path().join("inner")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("gone"), dir.path().join("dangling")).unwrap();
        let ws = Workspace::new(dir.path()).unwrap();

        assert_eq!(ws.resolve("link"), Err(DocsError::OutsideWorkspace));
        assert_eq!(
            ws.resolve("outdir/secret.md"),
            Err(DocsError::OutsideWorkspace)
        );
        assert_eq!(ws.resolve("dangling"), Err(DocsError::OutsideWorkspace));
        let inner = ws.resolve("inner").unwrap();
        assert_eq!(inner.relative, "inner");
        assert!(inner.absolute.ends_with("sub/ok.md"));
    }
}
