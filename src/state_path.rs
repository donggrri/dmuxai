use std::{io, path::Path, path::PathBuf};

use crate::workspace::normalize_workspace_path;

/// FNV-1a 64-bit hash of the UTF-8 bytes of `value`.
///
/// A fixed, deterministic algorithm is required (FR-453): Rust's debug
/// `DefaultHasher` deliberately changes between releases, which would move
/// the per-repo database directory on every toolchain update.
pub fn fnv1a64(value: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// State base directory for DmuxAI-owned state (FR-451): `$DMUXAI_STATE_DIR`
/// when set, otherwise `$XDG_STATE_HOME/dmuxai`, `~/.local/state/dmuxai`, or
/// (Windows) `%LOCALAPPDATA%\DmuxAI`.
pub fn state_base() -> io::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("DMUXAI_STATE_DIR")
        && !dir.is_empty()
    {
        return Ok(PathBuf::from(dir));
    }
    #[cfg(windows)]
    {
        if let Some(base) = std::env::var_os("LOCALAPPDATA") {
            if !base.is_empty() {
                return Ok(PathBuf::from(base).join("DmuxAI"));
            }
        }
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "neither DMUXAI_STATE_DIR nor LOCALAPPDATA is configured",
        ));
    }
    #[cfg(not(windows))]
    {
        if let Some(base) = std::env::var_os("XDG_STATE_HOME")
            && !base.is_empty()
        {
            return Ok(PathBuf::from(base).join("dmuxai"));
        }
        let home = std::env::var_os("HOME").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "neither DMUXAI_STATE_DIR nor XDG_STATE_HOME nor HOME is configured",
            )
        })?;
        Ok(PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("dmuxai"))
    }
}

/// Stable key for a workspace (FR-450, FR-453):
/// `<sanitized basename>-<16 hex FNV-1a 64 of the canonical workspace root>`.
pub fn repo_key(workspace_root: &Path) -> io::Result<String> {
    let canonical = normalize_workspace_path(workspace_root)?;
    let basename = Path::new(&canonical)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let sanitized: String = basename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let sanitized = if sanitized.is_empty() {
        "workspace".to_owned()
    } else {
        sanitized
    };
    Ok(format!("{sanitized}-{:016x}", fnv1a64(&canonical)))
}

/// `conversations.sqlite3` inside a per-workspace directory under the state
/// base, so each worktree gets an isolated database (FR-450).
pub fn repo_db_path(workspace_root: &Path) -> io::Result<PathBuf> {
    Ok(state_base()?
        .join("repos")
        .join(repo_key(workspace_root)?)
        .join("conversations.sqlite3"))
}

/// Pre-repo-key single database shared by every workspace.
pub fn legacy_db_path() -> io::Result<PathBuf> {
    Ok(state_base()?.join("conversations.sqlite3"))
}

/// Returns the per-workspace database path and, on the very first open, a
/// one-line notice when the legacy global database was copied into it
/// (FR-452). The legacy file is always left in place.
pub fn prepare_repo_db(workspace_root: &Path) -> io::Result<(PathBuf, Option<String>)> {
    let db_path = repo_db_path(workspace_root)?;
    if db_path.exists() {
        return Ok((db_path, None));
    }
    let legacy = legacy_db_path()?;
    if !legacy.is_file() {
        return Ok((db_path, None));
    }
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(&legacy, &db_path)?;
    let notice = format!(
        "Migrated the global conversation database to {} (the original file at {} was kept).",
        db_path.display(),
        legacy.display()
    );
    Ok((db_path, Some(notice)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{Mutex, MutexGuard},
        time::Duration,
    };

    // Serializes tests that touch process-wide environment variables.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn lock_env() -> MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn set_env(key: &str, value: impl AsRef<std::ffi::OsStr>) {
        // SAFETY: callers hold ENV_LOCK, so no other test accesses the
        // environment concurrently while the variable is in flux.
        unsafe { std::env::set_var(key, value) };
    }

    fn remove_env(key: &str) {
        // SAFETY: same exclusion as `set_env`.
        unsafe { std::env::remove_var(key) };
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dmuxai-state-path-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // FR-453 / TC-453: FNV-1a 64 must match published fixed test vectors so
    // the repo key is stable across runs and builds.
    #[test]
    fn repo_key_is_stable_fnv1a() {
        assert_eq!(fnv1a64(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64("a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64("foobar"), 0x85944171f73967e8);

        let dir = temp_dir("fnv");
        std::fs::create_dir_all(dir.join("my repo")).unwrap();
        let first = repo_key(&dir.join("my repo").canonicalize().unwrap()).unwrap();
        let second = repo_key(&dir.join("my repo").canonicalize().unwrap()).unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("my_repo-"));
        let expected_hash = fnv1a64(
            normalize_workspace_path(&dir.join("my repo"))
                .unwrap()
                .as_str(),
        );
        assert_eq!(
            first.split('-').next_back().unwrap(),
            format!("{expected_hash:016x}")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    // FR-450 / TC-450: different canonical workspace roots map to different
    // database paths.
    #[test]
    fn different_workspaces_get_different_db_paths() {
        let dir = temp_dir("diff");
        let left = temp_dir("diff-a");
        let right = temp_dir("diff-b");
        let _guard = lock_env();
        set_env("DMUXAI_STATE_DIR", &dir);

        let left_path = repo_db_path(&left).unwrap();
        let right_path = repo_db_path(&right).unwrap();
        assert_ne!(left_path, right_path);
        assert!(left_path.ends_with("conversations.sqlite3"));
        remove_env("DMUXAI_STATE_DIR");
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(left);
        let _ = std::fs::remove_dir_all(right);
    }

    // FR-451 / TC-451: DMUXAI_STATE_DIR overrides the state base.
    #[test]
    fn state_dir_env_overrides_base() {
        let _guard = lock_env();
        set_env("DMUXAI_STATE_DIR", "/custom/dmuxai-state");
        let base = state_base().unwrap();
        remove_env("DMUXAI_STATE_DIR");
        assert_eq!(base, PathBuf::from("/custom/dmuxai-state"));
    }

    // FR-452 / TC-452: first open copies a legacy DB into the per-repo
    // location and leaves the original untouched; subsequent opens neither
    // copy again nor report a notice.
    #[test]
    fn legacy_db_is_copied_once_and_left_in_place() {
        let _guard = lock_env();
        let dir = temp_dir("legacy");
        let workspace = temp_dir("legacy-ws");
        std::fs::create_dir_all(workspace.join("proj")).unwrap();
        let workspace = workspace.join("proj");
        set_env("DMUXAI_STATE_DIR", &dir);

        let legacy = dir.join("conversations.sqlite3");
        std::fs::write(&legacy, b"legacy-bytes").unwrap();
        std::thread::sleep(Duration::from_millis(5));

        let (path, notice) = prepare_repo_db(&workspace).unwrap();
        assert!(notice.is_some());
        assert!(path.is_file());
        assert_eq!(std::fs::read(&path).unwrap(), b"legacy-bytes");
        assert!(legacy.is_file());
        assert_eq!(std::fs::read(&legacy).unwrap(), b"legacy-bytes");

        // A second open must not copy again or re-notify.
        std::fs::write(&legacy, b"changed").unwrap();
        let (path_again, notice_again) = prepare_repo_db(&workspace).unwrap();
        assert!(notice_again.is_none());
        assert_eq!(path_again, path);
        assert_eq!(std::fs::read(&path_again).unwrap(), b"legacy-bytes");

        remove_env("DMUXAI_STATE_DIR");
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(workspace);
    }

    // FR-454 / TC-454: the UnsupportedSchema message names the database
    // path, says a newer dmuxai wrote it, and suggests DMUXAI_STATE_DIR.
    #[test]
    fn unsupported_schema_message_names_path() {
        let error = crate::conversation::ConversationError::UnsupportedSchema {
            version: 3,
            db_path: PathBuf::from("/state/dmuxai/repos/x-abc/conversations.sqlite3"),
        };
        let message = error.to_string();
        assert!(message.contains("/state/dmuxai/repos/x-abc/conversations.sqlite3"));
        assert!(message.contains("newer dmuxai"));
        assert!(message.contains("DMUXAI_STATE_DIR"));
    }
}
