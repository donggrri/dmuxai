use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Resolves the workspace root from `DMUXAI_WORKSPACE` or the process cwd.
pub fn resolve_workspace_root() -> io::Result<PathBuf> {
    if let Ok(path) = std::env::var("DMUXAI_WORKSPACE") {
        let path = PathBuf::from(path);
        if path.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "DMUXAI_WORKSPACE must not be empty",
            ));
        }
        return Ok(path);
    }
    std::env::current_dir()
}

/// Canonicalizes a workspace path for durable storage keys and provider `--workspace`.
pub fn normalize_workspace_path(path: &Path) -> io::Result<String> {
    let canonical: PathBuf = match fs::canonicalize(path) {
        Ok(path) => path,
        Err(_) if path.is_absolute() => path.to_path_buf(),
        Err(_) => std::env::current_dir()?.join(path),
    };
    Ok(canonical.to_string_lossy().into_owned())
}

/// Warn when a Linux workspace lives on a Windows drvfs mount (`/mnt/<drive>/...`).
pub fn wsl_mnt_warning(canonical_workspace: &str) -> Option<String> {
    let path = canonical_workspace.trim_end_matches('/');
    if !path.starts_with("/mnt/") {
        return None;
    }
    let rest = path.strip_prefix("/mnt/")?;
    let drive = rest.split('/').next()?;
    if drive.len() == 1 && drive.chars().all(|ch| ch.is_ascii_alphabetic()) {
        Some(format!(
            "Workspace is on a Windows mount ({canonical_workspace}). File permissions and git worktrees are unreliable here; prefer a path under your Linux home (for example ~/projects/...)."
        ))
    } else {
        None
    }
}

/// Creates the directory when missing and ensures the current user can write to it.
pub fn ensure_directory_writable(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    let metadata = fs::metadata(path)?;
    if metadata.is_dir() {
        let probe = path.join(".dmuxai-write-probe");
        fs::write(&probe, b"")?;
        let _ = fs::remove_file(probe);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = metadata.permissions();
        let mode = permissions.mode();
        if mode & 0o200 == 0 {
            permissions.set_mode(mode | 0o200);
            fs::set_permissions(path, permissions)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn detects_wsl_mnt_paths() {
        assert!(wsl_mnt_warning("/mnt/c/Users/foo").is_some());
        assert!(wsl_mnt_warning("/home/user/proj").is_none());
    }

    #[test]
    fn normalize_workspace_uses_canonical_path() {
        let dir = tempdir().unwrap();
        let normalized = normalize_workspace_path(dir.path()).unwrap();
        assert!(normalized.ends_with(dir.path().file_name().unwrap().to_string_lossy().as_ref()));
    }

    #[test]
    fn ensure_directory_writable_creates_and_probes() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("nested").join("workspace");
        ensure_directory_writable(&nested).unwrap();
        assert!(nested.is_dir());
    }
}
