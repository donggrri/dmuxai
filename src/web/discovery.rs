//! Dashboard discovery file (SPEC-agent-api FR-773, SPEC-web-local-access FR-2957).

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Content of `<workspace>/.dmuxai/web.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovery {
    pub base_url: String,
    pub port: u16,
    pub pid: u32,
    #[serde(default)]
    pub token_required: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub token_required_explicit: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl Discovery {
    pub fn legacy_with_token(base_url: String, port: u16, pid: u32, token: String) -> Self {
        Self {
            base_url,
            port,
            pid,
            token_required: true,
            token_required_explicit: false,
            token: Some(token),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserWorkspaceRef {
    pub id: String,
    pub path: String,
}

/// User-level `<state base>/web.json` (FR-2957).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserDiscovery {
    pub base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub windows_url: Option<String>,
    pub port: u16,
    pub pid: u32,
    pub uid: u32,
    pub mode: String,
    pub token_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default)]
    pub workspaces: Vec<UserWorkspaceRef>,
    pub started_at_ms: u64,
}

/// `<root>/.dmuxai/web.json`
pub fn discovery_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".dmuxai").join("web.json")
}

pub fn user_discovery_path() -> io::Result<PathBuf> {
    Ok(crate::state_path::state_base()?.join("web.json"))
}

/// Write the discovery file (pretty JSON) with owner-only permissions on Unix.
pub fn write_discovery(workspace_root: &Path, d: &Discovery) -> io::Result<PathBuf> {
    let dir = workspace_root.join(crate::workspace::WORKSPACE_STATE_DIR);
    crate::workspace::ensure_state_dir(&dir)?;
    let path = dir.join("web.json");
    write_owner_only_json(&path, &dir, d)
}

fn ensure_owned_state_dir_permissions(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;
        if !dir.exists() {
            return Ok(());
        }
        let meta = std::fs::metadata(dir)?;
        let uid = unsafe { libc::geteuid() };
        if meta.uid() == uid {
            let mut perms = meta.permissions();
            let mode = perms.mode() & 0o777;
            if mode != 0o700 {
                perms.set_mode((perms.mode() & !0o777) | 0o700);
                std::fs::set_permissions(dir, perms)?;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}

pub fn write_user_discovery(d: &UserDiscovery) -> io::Result<PathBuf> {
    let base = crate::state_path::state_base()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&base)?;
        ensure_owned_state_dir_permissions(&base)?;
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(&base)?;
    }
    let path = base.join("web.json");
    write_owner_only_json(&path, &base, d)
}

fn write_owner_only_json<T: Serialize>(path: &Path, dir: &Path, value: &T) -> io::Result<PathBuf> {
    let body = serde_json::to_string_pretty(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let temp_path = dir.join(format!("web.json.{}.tmp", std::process::id()));
        let _ = std::fs::remove_file(&temp_path);
        let mut temp = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp_path)?;
        temp.write_all(body.as_bytes())?;
        temp.flush()?;
        drop(temp);
        std::fs::rename(&temp_path, path)?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, body.as_bytes())?;
    }
    Ok(path.to_path_buf())
}

/// Refresh `workspaces` in the user-level file when this process owns it (FR-2957).
pub fn sync_user_discovery_workspaces(workspaces: &[UserWorkspaceRef]) -> io::Result<()> {
    let path = user_discovery_path()?;
    let Some(mut existing) = read_user_discovery_raw(&path) else {
        return Ok(());
    };
    if existing.pid != std::process::id() {
        return Ok(());
    }
    existing.workspaces = workspaces.to_vec();
    write_user_discovery(&existing)?;
    Ok(())
}

/// Remove the discovery file if present; ignore a missing file.
pub fn remove_discovery(workspace_root: &Path) {
    let _ = std::fs::remove_file(discovery_path(workspace_root));
}

pub fn remove_user_discovery_owned() {
    if let Ok(path) = user_discovery_path()
        && let Some(existing) = read_user_discovery_raw(&path)
        && existing.pid == std::process::id()
    {
        let _ = std::fs::remove_file(&path);
    }
}

/// Read and parse the discovery file, or `None` when missing or malformed.
pub fn read_discovery(workspace_root: &Path) -> Option<Discovery> {
    let data = std::fs::read_to_string(discovery_path(workspace_root)).ok()?;
    let mut d: Discovery = serde_json::from_str(&data).ok()?;
    if d.token.is_some() {
        d.token_required = true;
    }
    Some(d)
}

pub fn read_user_discovery() -> Option<UserDiscovery> {
    let path = user_discovery_path().ok()?;
    let d = read_user_discovery_raw(&path)?;
    if !process_alive(d.pid) {
        return None;
    }
    Some(d)
}

fn read_user_discovery_raw(path: &Path) -> Option<UserDiscovery> {
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

pub fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        if unsafe { libc::kill(pid as i32, 0) } == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Returns false when an existing user discovery owned by a live `web` process should block overwrite.
pub fn user_discovery_may_overwrite(mode: &str) -> bool {
    let Ok(path) = user_discovery_path() else {
        return true;
    };
    let Some(existing) = read_user_discovery_raw(&path) else {
        return true;
    };
    if !process_alive(existing.pid) {
        return true;
    }
    if existing.mode == "serve" && mode == "web" {
        return true;
    }
    if existing.mode == "web" && mode != "web" {
        return false;
    }
    if existing.mode == "web" && mode == "web" && existing.pid != std::process::id() {
        return false;
    }
    existing.pid == std::process::id()
}

/// Validate a `DMUXAI_WEB_TOKEN` value: `None`/empty means "generate a random
/// token"; otherwise the token must be 32..=128 characters of
/// `[A-Za-z0-9_-]`. An invalid value is an error (the caller warns and falls
/// back to a random token, FR-773).
pub fn token_from_env(value: Option<&str>) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    let valid = (32..=128).contains(&value.len())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if valid {
        Ok(Some(value.to_owned()))
    } else {
        Err(
            "DMUXAI_WEB_TOKEN must be 32-128 characters of A-Z a-z 0-9 _ -; using a random token"
                .to_owned(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // TC-2502 / FR-2501, FR-2503: creating the state dir writes .dmuxai/.gitignore
    #[test]
    fn tc_2502_discovery_creates_state_gitignore() {
        let workspace = tempfile::tempdir().unwrap();
        let discovery = Discovery::legacy_with_token(
            "http://127.0.0.1".to_owned(),
            7420,
            std::process::id(),
            "abcd1234".repeat(5),
        );
        write_discovery(workspace.path(), &discovery).unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.path().join(".dmuxai").join(".gitignore")).unwrap(),
            "*\n"
        );
        assert!(!workspace.path().join(".gitignore").exists());
    }

    #[test]
    fn tc_774_discovery_file_and_token_env() {
        let workspace = tempfile::tempdir().unwrap();
        let discovery = Discovery::legacy_with_token(
            "http://127.0.0.1".to_owned(),
            7420,
            std::process::id(),
            "abcd1234".repeat(5),
        );

        let path = write_discovery(workspace.path(), &discovery).unwrap();
        assert_eq!(path, discovery_path(workspace.path()));
        assert_eq!(read_discovery(workspace.path()).as_ref(), Some(&discovery));

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("\"token\""), "pretty JSON carries the token");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "discovery file is owner-only");
        }

        remove_discovery(workspace.path());
        assert!(!path.exists());
        assert!(token_from_env(None).unwrap().is_none());
    }

    // TC-2957 / FR-2957
    #[test]
    fn tc_2957_user_level_discovery_written_0600_not_clobbered_removed_and_stale_ignored() {
        let _guard = crate::state_path::test_env_lock();
        let prev = std::env::var_os("DMUXAI_STATE_DIR");
        let state_dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("DMUXAI_STATE_DIR", state_dir.path()) };

        let user = UserDiscovery {
            base_url: "http://127.0.0.1:7420/".to_owned(),
            windows_url: None,
            port: 7420,
            pid: std::process::id(),
            uid: crate::web::access::effective_server_uid(),
            mode: "web".to_owned(),
            token_required: false,
            token: None,
            workspaces: vec![UserWorkspaceRef {
                id: "ws1".to_owned(),
                path: "/tmp/ws".to_owned(),
            }],
            started_at_ms: 1,
        };
        let path = write_user_discovery(&user).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            let base = crate::state_path::state_base().unwrap();
            assert_eq!(
                std::fs::metadata(&base).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        assert_eq!(read_user_discovery().as_ref(), Some(&user));

        assert!(!user_discovery_may_overwrite("serve"));

        #[cfg(unix)]
        {
            let other_pid = unsafe { libc::getppid() as u32 };
            if other_pid > 1 && process_alive(other_pid) && other_pid != std::process::id() {
                let foreign = UserDiscovery {
                    base_url: "http://127.0.0.1:7421/".to_owned(),
                    windows_url: None,
                    port: 7421,
                    pid: other_pid,
                    uid: user.uid,
                    mode: "web".to_owned(),
                    token_required: false,
                    token: None,
                    workspaces: vec![],
                    started_at_ms: 2,
                };
                write_user_discovery(&foreign).unwrap();
                assert!(!user_discovery_may_overwrite("web"));
                write_user_discovery(&user).unwrap();
            }
        }

        let serve_live = UserDiscovery {
            mode: "serve".to_owned(),
            pid: std::process::id(),
            ..user.clone()
        };
        write_user_discovery(&serve_live).unwrap();
        assert!(user_discovery_may_overwrite("web"));

        let dead_pid = 4_194_304_u32;
        assert!(!process_alive(dead_pid));
        let stale = UserDiscovery {
            pid: dead_pid,
            mode: "web".to_owned(),
            ..user.clone()
        };
        write_user_discovery(&stale).unwrap();
        assert!(user_discovery_may_overwrite("web"));

        write_user_discovery(&user).unwrap();
        sync_user_discovery_workspaces(&[UserWorkspaceRef {
            id: "ws2".to_owned(),
            path: "/tmp/ws2".to_owned(),
        }])
        .unwrap();
        let updated = read_user_discovery().unwrap();
        assert_eq!(updated.workspaces[0].id, "ws2");

        remove_user_discovery_owned();
        assert!(!path.exists());

        match prev {
            Some(v) => unsafe { std::env::set_var("DMUXAI_STATE_DIR", v) },
            None => unsafe { std::env::remove_var("DMUXAI_STATE_DIR") },
        }
    }
}
