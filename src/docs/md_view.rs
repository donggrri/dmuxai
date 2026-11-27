//! Confinement, size-capped reading, and HTML rendering for `/api/md/view`
//! (SPEC-board-ux-042 FR-3414, FR-3415, NFR-3412).

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

/// NFR-3412 / FR-3415: maximum Markdown file size served by `/api/md/view` (1 MiB).
pub const MAX_MD_VIEW_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MdViewError {
    /// 400: empty path, NUL byte, or non-UTF-8 file content.
    BadInput(&'static str),
    /// 403: outside allowed roots, hidden path under home, not `.md`, or directory.
    Forbidden(&'static str),
    /// 404: missing file inside allowed roots.
    NotFound,
    /// 413: file exceeds 1 MiB.
    TooLarge,
    /// 500: unexpected I/O failure.
    Io,
}

impl MdViewError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::BadInput(msg) | Self::Forbidden(msg) => msg,
            Self::NotFound => "file not found",
            Self::TooLarge => "file exceeds 1 MiB limit",
            Self::Io => "failed to read file",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MdViewResponse {
    pub path: String,
    pub html: String,
    pub bytes: usize,
}

fn valid_home(home: Option<&Path>) -> Option<PathBuf> {
    let h = home?;
    if h.as_os_str().is_empty() || h == Path::new("/") {
        return None;
    }
    let canon = std::fs::canonicalize(h).ok()?;
    if canon == Path::new("/") {
        return None;
    }
    Some(canon)
}

fn is_under_home_visible(canonical: &Path, canonical_home: &Path) -> bool {
    let Ok(rel) = canonical.strip_prefix(canonical_home) else {
        return false;
    };
    let mut has_component = false;
    for component in rel.components() {
        match component {
            Component::Normal(part) => {
                has_component = true;
                let Some(s) = part.to_str() else {
                    return false;
                };
                if s.starts_with('.') {
                    return false;
                }
            }
            _ => return false,
        }
    }
    has_component
}

fn lexical_escapes_root(raw: &str) -> bool {
    let mut depth: isize = 0;
    for component in Path::new(raw).components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            Component::RootDir | Component::Prefix(_) => return true,
        }
    }
    false
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(p) => out.push(p),
        }
    }
    out
}

/// Resolves and validates a Markdown file path for FR-3414 / FR-3415.
pub fn resolve_md_view_path(
    raw: &str,
    workspace_root: &Path,
    home: Option<&Path>,
) -> Result<PathBuf, MdViewError> {
    if raw.contains('\0') {
        return Err(MdViewError::BadInput("invalid path"));
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(MdViewError::BadInput("empty path"));
    }

    let canonical_ws = std::fs::canonicalize(workspace_root).map_err(|_| MdViewError::Io)?;
    let canonical_home = valid_home(home);

    let is_tilde = trimmed == "~" || trimmed.starts_with("~/");
    let candidate = if is_tilde {
        let Some(ref h) = canonical_home else {
            return Err(MdViewError::Forbidden("home directory unavailable"));
        };
        if trimmed == "~" {
            h.clone()
        } else {
            h.join(&trimmed[2..])
        }
    } else if Path::new(trimmed).is_absolute() {
        PathBuf::from(trimmed)
    } else {
        canonical_ws.join(trimmed)
    };

    let canonical = match std::fs::canonicalize(&candidate) {
        Ok(path) => path,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if std::fs::symlink_metadata(&candidate).is_ok() {
                return Err(MdViewError::Forbidden("path outside allowed roots"));
            }
            if is_tilde {
                if lexical_escapes_root(trimmed.strip_prefix("~/").unwrap_or("")) {
                    return Err(MdViewError::Forbidden("path outside allowed roots"));
                }
            } else if Path::new(trimmed).is_absolute() {
                let norm = lexical_normalize(&candidate);
                let in_ws = norm.starts_with(&canonical_ws) || norm.starts_with(workspace_root);
                let in_home = canonical_home
                    .as_deref()
                    .is_some_and(|h| is_under_home_visible(&norm, h));
                if !in_ws && !in_home {
                    return Err(MdViewError::Forbidden("path outside allowed roots"));
                }
            } else if lexical_escapes_root(trimmed) {
                return Err(MdViewError::Forbidden("path outside allowed roots"));
            }
            return Err(MdViewError::NotFound);
        }
        Err(_) => return Err(MdViewError::Io),
    };

    let in_ws = canonical.starts_with(&canonical_ws);
    let in_home = canonical_home
        .as_deref()
        .is_some_and(|h| is_under_home_visible(&canonical, h));
    if !in_ws && !in_home {
        return Err(MdViewError::Forbidden("path outside allowed roots"));
    }

    let metadata = std::fs::metadata(&canonical).map_err(|_| MdViewError::Io)?;
    if !metadata.is_file() {
        return Err(MdViewError::Forbidden("not a regular file"));
    }

    let is_md = canonical
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
    if !is_md {
        return Err(MdViewError::Forbidden("only .md files can be viewed"));
    }

    if metadata.len() > MAX_MD_VIEW_BYTES {
        return Err(MdViewError::TooLarge);
    }

    Ok(canonical)
}

fn format_display_path(
    raw: &str,
    canonical: &Path,
    canonical_ws: &Path,
    canonical_home: Option<&Path>,
) -> String {
    if let Ok(rel) = canonical.strip_prefix(canonical_ws) {
        let joined = rel
            .components()
            .filter_map(|c| match c {
                Component::Normal(p) => p.to_str(),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/");
        if !joined.is_empty() {
            return joined;
        }
    }
    if let Some(home) = canonical_home
        && let Ok(rel) = canonical.strip_prefix(home)
    {
        let joined = rel
            .components()
            .filter_map(|c| match c {
                Component::Normal(p) => p.to_str(),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/");
        if !joined.is_empty() {
            return format!("~/{joined}");
        }
    }
    raw.trim().to_owned()
}

/// Resolves, reads (capped at 1 MiB), and renders a Markdown file for `/api/md/view`.
pub fn view_markdown(
    raw: &str,
    workspace_root: &Path,
    home: Option<&Path>,
) -> Result<MdViewResponse, MdViewError> {
    let canonical = resolve_md_view_path(raw, workspace_root, home)?;
    let canonical_ws = std::fs::canonicalize(workspace_root).map_err(|_| MdViewError::Io)?;
    let canonical_home = valid_home(home);

    let file = std::fs::File::open(&canonical).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            MdViewError::NotFound
        } else {
            MdViewError::Io
        }
    })?;
    let mut bytes = Vec::new();
    file.take(MAX_MD_VIEW_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| MdViewError::Io)?;
    if bytes.len() as u64 > MAX_MD_VIEW_BYTES {
        return Err(MdViewError::TooLarge);
    }

    let text =
        String::from_utf8(bytes).map_err(|_| MdViewError::BadInput("file is not valid utf-8"))?;
    let byte_len = text.len();
    let html = crate::docs::markdown::render_markdown(&text);
    let path = format_display_path(raw, &canonical, &canonical_ws, canonical_home.as_deref());

    Ok(MdViewResponse {
        path,
        html,
        bytes: byte_len,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // TC-3415 (FR-3415, NFR-3412)
    #[test]
    fn tc_3415_md_view_confines_paths_home_workspace_symlink_ext_size() {
        let ws = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();

        std::fs::create_dir_all(ws.path().join("docs")).unwrap();
        std::fs::write(ws.path().join("docs/guide.md"), "# Guide").unwrap();
        std::fs::write(ws.path().join("README.MD"), "# Upper").unwrap();
        std::fs::write(ws.path().join("notes.txt"), "plain").unwrap();
        std::fs::create_dir_all(ws.path().join("folder.md")).unwrap();

        std::fs::create_dir_all(home.path().join("notes")).unwrap();
        std::fs::write(home.path().join("notes/X.md"), "# Note").unwrap();
        std::fs::create_dir_all(home.path().join(".secret")).unwrap();
        std::fs::write(home.path().join(".secret/X.md"), "secret").unwrap();
        std::fs::write(home.path().join(".hidden.md"), "secret").unwrap();

        std::fs::write(outside.path().join("outside.md"), "secret").unwrap();

        let big = vec![b'a'; (MAX_MD_VIEW_BYTES + 1) as usize];
        std::fs::write(ws.path().join("big.md"), &big).unwrap();

        // 1. Workspace file ok
        let resolved = resolve_md_view_path("docs/guide.md", ws.path(), Some(home.path())).unwrap();
        assert!(resolved.ends_with("docs/guide.md"));

        // Case-insensitive .MD ok
        let upper = resolve_md_view_path("README.MD", ws.path(), Some(home.path())).unwrap();
        assert!(upper.ends_with("README.MD"));

        // 2. ~/notes/X.md ok
        let home_ok = resolve_md_view_path("~/notes/X.md", ws.path(), Some(home.path())).unwrap();
        assert!(home_ok.ends_with("notes/X.md"));

        // 3. ~/.secret/X.md and ~/.hidden.md refused
        assert!(matches!(
            resolve_md_view_path("~/.secret/X.md", ws.path(), Some(home.path())),
            Err(MdViewError::Forbidden(_))
        ));
        assert!(matches!(
            resolve_md_view_path("~/.hidden.md", ws.path(), Some(home.path())),
            Err(MdViewError::Forbidden(_))
        ));

        // 4. ../ escape refused (both existing outside file and non-existent)
        assert!(matches!(
            resolve_md_view_path("../outside.md", ws.path(), Some(home.path())),
            Err(MdViewError::Forbidden(_))
        ));
        assert!(matches!(
            resolve_md_view_path("~/../outside.md", ws.path(), Some(home.path())),
            Err(MdViewError::Forbidden(_))
        ));

        // 5. Symlink inside workspace pointing outside both roots refused
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                outside.path().join("outside.md"),
                ws.path().join("escape.md"),
            )
            .unwrap();
            assert!(matches!(
                resolve_md_view_path("escape.md", ws.path(), Some(home.path())),
                Err(MdViewError::Forbidden(_))
            ));
        }

        // 6. .txt refused
        assert!(matches!(
            resolve_md_view_path("notes.txt", ws.path(), Some(home.path())),
            Err(MdViewError::Forbidden(_))
        ));

        // 7. Directory refused (including dir named folder.md and docs and ~)
        assert!(matches!(
            resolve_md_view_path("docs", ws.path(), Some(home.path())),
            Err(MdViewError::Forbidden(_))
        ));
        assert!(matches!(
            resolve_md_view_path("folder.md", ws.path(), Some(home.path())),
            Err(MdViewError::Forbidden(_))
        ));
        assert!(matches!(
            resolve_md_view_path("~", ws.path(), Some(home.path())),
            Err(MdViewError::Forbidden(_))
        ));

        // 8. > 1 MiB -> TooLarge
        assert_eq!(
            resolve_md_view_path("big.md", ws.path(), Some(home.path())),
            Err(MdViewError::TooLarge)
        );

        // 9. Home "/" (and None / empty) refused
        assert!(matches!(
            resolve_md_view_path("~/notes/X.md", ws.path(), Some(Path::new("/"))),
            Err(MdViewError::Forbidden(_))
        ));
        assert!(matches!(
            resolve_md_view_path("~/notes/X.md", ws.path(), Some(Path::new(""))),
            Err(MdViewError::Forbidden(_))
        ));
        assert!(matches!(
            resolve_md_view_path("~/notes/X.md", ws.path(), None),
            Err(MdViewError::Forbidden(_))
        ));

        // 10. NUL byte & empty -> BadInput; missing -> NotFound
        assert!(matches!(
            resolve_md_view_path("docs/\0guide.md", ws.path(), Some(home.path())),
            Err(MdViewError::BadInput(_))
        ));
        assert!(matches!(
            resolve_md_view_path("", ws.path(), Some(home.path())),
            Err(MdViewError::BadInput(_))
        ));
        assert_eq!(
            resolve_md_view_path("docs/missing.md", ws.path(), Some(home.path())),
            Err(MdViewError::NotFound)
        );

        // 11. view_markdown formats home path as ~/... and workspace path as relative
        let home_resp = view_markdown(
            home.path().join("notes/X.md").to_str().unwrap(),
            ws.path(),
            Some(home.path()),
        )
        .unwrap();
        assert_eq!(home_resp.path, "~/notes/X.md");
        assert!(home_resp.html.contains("Note"));
        assert_eq!(home_resp.bytes, 6);
    }
}
