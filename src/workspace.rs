use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// Maximum number of entries persisted in `recent-workspaces.json` (FR-872).
pub const MAX_RECENT_WORKSPACES: usize = 10;

/// Credential directories relative to `$HOME` that are never valid workspaces (FR-873).
pub const CREDENTIAL_DIRS_FROM_HOME: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".config/gh",
    ".docker",
    ".kube",
    ".password-store",
    ".local/share/keyrings",
    ".cursor",
    ".codex",
    ".gemini",
    ".local/share/opencode",
];

/// Reason why a candidate workspace directory was rejected (FR-873).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceDirError {
    DoesNotExist,
    NotADirectory,
    NotWritable,
    DeniedCredentialDir,
    DeniedStateDir,
    DeniedConfigDir,
    DeniedFilesystemRoot,
    InvalidPath,
}

impl std::fmt::Display for WorkspaceDirError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::DoesNotExist => "does not exist",
            Self::NotADirectory => "not a directory",
            Self::NotWritable => "not writable",
            Self::DeniedCredentialDir => "denied: credential directory",
            Self::DeniedStateDir => "denied: DmuxAI state directory",
            Self::DeniedConfigDir => "denied: DmuxAI config directory",
            Self::DeniedFilesystemRoot => "denied: filesystem root",
            Self::InvalidPath => "invalid path",
        };
        f.write_str(text)
    }
}

impl std::error::Error for WorkspaceDirError {}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()))
        .map(PathBuf::from)
}

fn is_root_path(path: &Path) -> bool {
    path == Path::new("/") || path.parent().is_none()
}

fn is_same_or_inside(canonical: &Path, target: &Path) -> bool {
    if target.as_os_str().is_empty() || is_root_path(target) {
        return false;
    }
    if canonical.starts_with(target) {
        return true;
    }
    if let Ok(canon_target) = fs::canonicalize(target)
        && !is_root_path(&canon_target)
        && canonical.starts_with(&canon_target)
    {
        return true;
    }
    false
}

/// Checks whether a canonicalized directory path falls under any denied root (FR-873).
pub fn check_workspace_denied(canonical: &Path) -> Result<(), WorkspaceDirError> {
    if is_root_path(canonical) {
        return Err(WorkspaceDirError::DeniedFilesystemRoot);
    }
    if let Ok(state_dir) = crate::state_path::state_base()
        && is_same_or_inside(canonical, &state_dir)
    {
        return Err(WorkspaceDirError::DeniedStateDir);
    }
    if let Some(cfg_file) = crate::settings::default_user_config_path()
        && let Some(cfg_dir) = cfg_file.parent()
        && is_same_or_inside(canonical, cfg_dir)
    {
        return Err(WorkspaceDirError::DeniedConfigDir);
    }
    if let Some(home) = home_dir() {
        for rel in CREDENTIAL_DIRS_FROM_HOME {
            let cred = home.join(rel);
            if is_same_or_inside(canonical, &cred) {
                return Err(WorkspaceDirError::DeniedCredentialDir);
            }
        }
    }
    Ok(())
}

/// Validates a candidate workspace directory (FR-873).
///
/// Canonicalizes the path (after `~` expansion), ensures it is an existing
/// readable and writable directory (without creating directories or mutating
/// permissions), and enforces the deny list (`/` root, DmuxAI state dir,
/// DmuxAI user config dir, and credential directories).
pub fn validate_workspace_dir(path: &Path) -> Result<PathBuf, WorkspaceDirError> {
    validate_workspace_dir_inner(path, true)
}

/// Like [`validate_workspace_dir`] but never creates a probe file: only the
/// mode bits and `read_dir` are checked. Used where validation must stay
/// read-only (listing recent paths on every `GET /api/workspace`).
pub fn validate_workspace_dir_no_probe(path: &Path) -> Result<PathBuf, WorkspaceDirError> {
    validate_workspace_dir_inner(path, false)
}

fn validate_workspace_dir_inner(
    path: &Path,
    write_probe: bool,
) -> Result<PathBuf, WorkspaceDirError> {
    let raw_os = path.as_os_str();
    if raw_os.is_empty() || raw_os.len() > 4096 {
        return Err(WorkspaceDirError::InvalidPath);
    }
    let raw = raw_os.to_string_lossy();
    if raw.as_bytes().contains(&0) || raw.len() > 4096 {
        return Err(WorkspaceDirError::InvalidPath);
    }
    let expanded = crate::settings::expand_home(&raw);
    if expanded.as_os_str().is_empty() || expanded.as_os_str().len() > 4096 {
        return Err(WorkspaceDirError::InvalidPath);
    }

    let metadata = match fs::metadata(&expanded) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(WorkspaceDirError::DoesNotExist);
        }
        Err(err) if err.kind() == io::ErrorKind::NotADirectory => {
            return Err(WorkspaceDirError::NotADirectory);
        }
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            return Err(WorkspaceDirError::NotWritable);
        }
        Err(_) => return Err(WorkspaceDirError::InvalidPath),
    };
    if !metadata.is_dir() {
        return Err(WorkspaceDirError::NotADirectory);
    }

    let canonical = fs::canonicalize(&expanded).map_err(|err| match err.kind() {
        io::ErrorKind::NotFound => WorkspaceDirError::DoesNotExist,
        io::ErrorKind::NotADirectory => WorkspaceDirError::NotADirectory,
        io::ErrorKind::PermissionDenied => WorkspaceDirError::NotWritable,
        _ => WorkspaceDirError::InvalidPath,
    })?;

    check_workspace_denied(&canonical)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o222 == 0 {
            return Err(WorkspaceDirError::NotWritable);
        }
    }

    // Must be readable and writable without creating directories.
    fs::read_dir(&canonical).map_err(|_| WorkspaceDirError::NotWritable)?;

    if !write_probe {
        return Ok(canonical);
    }

    static PROBE_SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = PROBE_SEQ.fetch_add(1, Ordering::Relaxed);
    let probe = canonical.join(format!(".dmuxai-write-probe-{}-{seq}", std::process::id()));
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(file) => {
            drop(file);
            let _ = fs::remove_file(&probe);
        }
        Err(_) => return Err(WorkspaceDirError::NotWritable),
    }

    Ok(canonical)
}

/// Resolves the workspace root with precedence (FR-870):
/// 1. CLI flag (`-C` / `--workspace-dir`)
/// 2. `DMUXAI_WORKSPACE` environment variable
/// 3. User-config `[workspace] dir` (`~` expanded)
/// 4. Process current working directory
pub fn resolve_workspace_root_with(flag: Option<&Path>) -> io::Result<PathBuf> {
    if let Some(flag_path) = flag {
        return validate_workspace_dir(flag_path)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()));
    }
    if let Ok(path) = std::env::var("DMUXAI_WORKSPACE") {
        let path = PathBuf::from(path);
        if path.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "DMUXAI_WORKSPACE must not be empty",
            ));
        }
        return validate_workspace_dir(&path)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()));
    }
    let user_ws = crate::settings::load_user_workspace_settings();
    if let Some(dir) = user_ws
        .dir
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let expanded = crate::settings::expand_home(dir);
        return validate_workspace_dir(&expanded)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()));
    }
    std::env::current_dir()
}

/// Resolves the workspace root from `DMUXAI_WORKSPACE`, user config `[workspace] dir`, or the process cwd.
pub fn resolve_workspace_root() -> io::Result<PathBuf> {
    resolve_workspace_root_with(None)
}

/// Strips `-C DIR`, `--workspace-dir DIR`, `--workspace-dir=DIR` (before `--`)
/// from `args` (arguments after `argv[0]`). Returns `(last_workspace_arg, remaining_args)`.
pub fn extract_workspace_dir_flag<I, S>(args: I) -> Result<(Option<String>, Vec<String>), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let raw: Vec<String> = args.into_iter().map(|s| s.as_ref().to_owned()).collect();
    let mut chosen: Option<String> = None;
    let mut rest: Vec<String> = Vec::with_capacity(raw.len());
    let mut index = 0;
    let mut past_double_dash = false;
    while index < raw.len() {
        let arg = &raw[index];
        if past_double_dash {
            rest.push(arg.clone());
            index += 1;
            continue;
        }
        match arg.as_str() {
            "--" => {
                past_double_dash = true;
                rest.push(arg.clone());
            }
            "-C" | "--workspace-dir" => {
                index += 1;
                match raw.get(index) {
                    Some(val) => chosen = Some(val.clone()),
                    None => return Err(String::new()),
                }
            }
            s if s.starts_with("--workspace-dir=") => {
                chosen = Some(s["--workspace-dir=".len()..].to_owned());
            }
            s if s.starts_with("-C=") => {
                chosen = Some(s["-C=".len()..].to_owned());
            }
            _ => rest.push(arg.clone()),
        }
        index += 1;
    }
    Ok((chosen, rest))
}

/// Builds the argument list (excluding `argv[0]`) for re-executing `dmuxai` on
/// a workspace switch (FR-874): strips any prior `-C` / `--workspace-dir`
/// flags, prepends `-C <canonical_workspace>`, and replaces `--port 0` with the
/// actual bound port when provided.
pub fn build_reexec_args(
    orig_args: &[String],
    canonical_workspace: &Path,
    bound_port: Option<u16>,
) -> Vec<String> {
    let (_, stripped) = extract_workspace_dir_flag(orig_args).unwrap_or_else(|_| {
        (
            None,
            orig_args
                .iter()
                .filter(|a| *a != "-C" && *a != "--workspace-dir")
                .cloned()
                .collect(),
        )
    });
    let mut out = vec![
        "-C".to_owned(),
        canonical_workspace.to_string_lossy().into_owned(),
    ];
    let mut saw_port = false;
    let mut i = 0;
    while i < stripped.len() {
        if stripped[i] == "--port"
            && let Some(next) = stripped.get(i + 1)
        {
            saw_port = true;
            out.push("--port".to_owned());
            if let Some(port) = bound_port {
                out.push(port.to_string());
            } else {
                out.push(next.clone());
            }
            i += 2;
            continue;
        }
        out.push(stripped[i].clone());
        i += 1;
    }
    if !saw_port && let Some(port) = bound_port {
        out.push("--port".to_owned());
        out.push(port.to_string());
    }
    out
}

/// Returns the canonicalized browse roots (FR-872): `$HOME` followed by any
/// valid, non-denied directories from user-config `[workspace] browse_roots`.
pub fn resolve_browse_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = home_dir()
        && let Ok(canon) = fs::canonicalize(&home)
        && check_workspace_denied(&canon).is_ok()
    {
        roots.push(canon);
    }
    let user_ws = crate::settings::load_user_workspace_settings();
    for raw in &user_ws.browse_roots {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let expanded = crate::settings::expand_home(trimmed);
        if let Ok(canon) = fs::canonicalize(&expanded)
            && canon.is_dir()
            && check_workspace_denied(&canon).is_ok()
            && !roots.contains(&canon)
        {
            roots.push(canon);
        }
    }
    roots
}

fn recent_workspaces_path() -> io::Result<PathBuf> {
    Ok(crate::state_path::state_base()?.join("recent-workspaces.json"))
}

/// Loads up to [`MAX_RECENT_WORKSPACES`] validated recent workspace paths (most recent first) (FR-872).
pub fn load_recent_workspaces() -> Vec<String> {
    let Ok(path) = recent_workspaces_path() else {
        return Vec::new();
    };
    let Ok(contents) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(raw_list) = serde_json::from_str::<Vec<String>>(&contents) else {
        return Vec::new();
    };
    let mut valid = Vec::new();
    for entry in raw_list {
        if let Ok(canon) = validate_workspace_dir_no_probe(Path::new(&entry)) {
            let s = canon.to_string_lossy().into_owned();
            if !valid.contains(&s) {
                valid.push(s);
                if valid.len() >= MAX_RECENT_WORKSPACES {
                    break;
                }
            }
        }
    }
    valid
}

/// Records `path` at the front of `recent-workspaces.json` (max 10, canonical strings) (FR-872, FR-874).
pub fn record_recent_workspace(path: &Path) -> io::Result<Vec<String>> {
    let canon = validate_workspace_dir(path)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    let canon_str = canon.to_string_lossy().into_owned();
    let file_path = recent_workspaces_path()?;
    let mut list: Vec<String> = fs::read_to_string(&file_path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default();
    list.retain(|item| item != &canon_str);
    list.insert(0, canon_str);
    list.truncate(MAX_RECENT_WORKSPACES);
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&list).map_err(io::Error::other)?;
    fs::write(&file_path, json)?;
    Ok(list)
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

/// `true` when `pid` is still a live process. Linux checks `/proc`.
pub fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        false
    }
}

/// Arguments for a second dashboard that does not replace this process.
pub fn open_dashboard_args(workspace: &Path) -> Vec<String> {
    vec![
        "--web".to_owned(),
        "-C".to_owned(),
        workspace.display().to_string(),
        "--port".to_owned(),
        "0".to_owned(),
    ]
}

/// Browser URL for a discovery record (`base_url` has no token).
pub fn discovery_dashboard_url(discovery: &crate::web::discovery::Discovery) -> String {
    format!(
        "{}/?token={}",
        discovery.base_url.trim_end_matches('/'),
        discovery.token
    )
}

/// Spawn `exe --web -C workspace --port 0` in its own process group.
/// The child does not inherit `DMUXAI_WEB_TOKEN` or `DMUXAI_OPEN_CONVERSATION`.
pub fn spawn_dashboard(exe: &Path, workspace: &Path) -> io::Result<std::process::Child> {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(open_dashboard_args(workspace))
        .env_remove("DMUXAI_WEB_TOKEN")
        .env_remove("DMUXAI_OPEN_CONVERSATION")
        .env("DMUXAI_WORKSPACE", workspace)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn()
}

/// If `workspace` already has a live dashboard, return its URL.
pub fn live_dashboard_url(workspace: &Path) -> Option<String> {
    let discovery = crate::web::discovery::read_discovery(workspace)?;
    if process_alive(discovery.pid) {
        Some(discovery_dashboard_url(&discovery))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {

    // TC-873 (FR-872): listing recents (every GET /api/workspace) must not
    // write probe files into the listed directories.
    #[test]
    fn load_recent_workspaces_does_not_write_probes() {
        let _guard = crate::state_path::test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let state_dir = root.join("state");
        let recent = root.join("recent-project");
        fs::create_dir_all(&state_dir).unwrap();
        fs::create_dir_all(&recent).unwrap();
        fs::write(
            state_dir.join("recent-workspaces.json"),
            serde_json::to_string(&vec![recent.to_string_lossy().into_owned()]).unwrap(),
        )
        .unwrap();
        let prev_state = std::env::var_os("DMUXAI_STATE_DIR");
        // SAFETY: serialized by test_env_lock().
        unsafe { std::env::set_var("DMUXAI_STATE_DIR", &state_dir) };
        let before = fs::metadata(&recent).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let listed = load_recent_workspaces();
        let after = fs::metadata(&recent).unwrap().modified().unwrap();
        // SAFETY: serialized by test_env_lock().
        unsafe {
            match prev_state {
                Some(v) => std::env::set_var("DMUXAI_STATE_DIR", v),
                None => std::env::remove_var("DMUXAI_STATE_DIR"),
            }
        }
        assert_eq!(listed, vec![recent.to_string_lossy().into_owned()]);
        assert_eq!(
            before, after,
            "listing recents must not touch the directory"
        );
    }

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

    // TC-870 (FR-870, FR-873): precedence order and directory validation.
    #[test]
    fn workspace_dir_precedence_and_validation() {
        let _guard = crate::state_path::test_env_lock();
        let root = tempdir().unwrap();
        let home = root.path().join("home");
        let state_dir = root.path().join("state");
        let xdg_cfg = root.path().join("xdg_config");
        let dmux_cfg_dir = xdg_cfg.join("dmuxai");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&state_dir).unwrap();
        fs::create_dir_all(&dmux_cfg_dir).unwrap();

        let prev_home = std::env::var_os("HOME");
        let prev_state = std::env::var_os("DMUXAI_STATE_DIR");
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let prev_ws = std::env::var_os("DMUXAI_WORKSPACE");

        // SAFETY: serialized by test_env_lock().
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("DMUXAI_STATE_DIR", &state_dir);
            std::env::set_var("XDG_CONFIG_HOME", &xdg_cfg);
            std::env::remove_var("DMUXAI_WORKSPACE");
        }

        let dir_flag = root.path().join("from_flag");
        let dir_env = root.path().join("from_env");
        let dir_cfg = home.join("from_cfg");
        fs::create_dir_all(&dir_flag).unwrap();
        fs::create_dir_all(&dir_env).unwrap();
        fs::create_dir_all(&dir_cfg).unwrap();

        // 1. Default falls back to cwd when nothing is set.
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(resolve_workspace_root_with(None).unwrap(), cwd);

        // 2. User config `[workspace] dir` (`~` expanded) beats cwd.
        let user_toml = dmux_cfg_dir.join("dmuxai.toml");
        fs::write(&user_toml, "[workspace]\ndir = \"~/from_cfg\"\n").unwrap();
        assert_eq!(
            resolve_workspace_root_with(None).unwrap(),
            dir_cfg.canonicalize().unwrap()
        );

        // Repo-level `dmuxai.toml` `[workspace]` is ignored.
        let repo_ws = root.path().join("repo_ws");
        let ignored_target = root.path().join("ignored_target");
        fs::create_dir_all(&repo_ws).unwrap();
        fs::create_dir_all(&ignored_target).unwrap();
        fs::write(
            repo_ws.join("dmuxai.toml"),
            format!("[workspace]\ndir = \"{}\"\n", ignored_target.display()),
        )
        .unwrap();
        let loaded = crate::settings::DmuxSettings::load_with_user_config(&repo_ws, None);
        assert_eq!(loaded.workspace.dir, None);

        // 3. DMUXAI_WORKSPACE beats user config.
        unsafe { std::env::set_var("DMUXAI_WORKSPACE", &dir_env) };
        assert_eq!(
            resolve_workspace_root_with(None).unwrap(),
            dir_env.canonicalize().unwrap()
        );

        // 4. Flag beats DMUXAI_WORKSPACE and user config.
        assert_eq!(
            resolve_workspace_root_with(Some(&dir_flag)).unwrap(),
            dir_flag.canonicalize().unwrap()
        );

        // Symlinks and `..` canonicalize.
        let dotdot = dir_flag.join("..").join("from_flag");
        assert_eq!(
            validate_workspace_dir(&dotdot).unwrap(),
            dir_flag.canonicalize().unwrap()
        );
        #[cfg(unix)]
        {
            let link = root.path().join("flag_link");
            std::os::unix::fs::symlink(&dir_flag, &link).unwrap();
            assert_eq!(
                validate_workspace_dir(&link).unwrap(),
                dir_flag.canonicalize().unwrap()
            );
        }

        // Missing path -> `does not exist` (and does NOT create the directory).
        let missing = root.path().join("missing_dir");
        assert_eq!(
            validate_workspace_dir(&missing).unwrap_err().to_string(),
            "does not exist"
        );
        assert!(!missing.exists());

        // File -> `not a directory`.
        let file_path = root.path().join("plain_file.txt");
        fs::write(&file_path, b"hi").unwrap();
        assert_eq!(
            validate_workspace_dir(&file_path).unwrap_err().to_string(),
            "not a directory"
        );

        // Read-only directory -> `not writable`.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let ro_dir = root.path().join("readonly_dir");
            fs::create_dir_all(&ro_dir).unwrap();
            fs::set_permissions(&ro_dir, fs::Permissions::from_mode(0o555)).unwrap();
            assert_eq!(
                validate_workspace_dir(&ro_dir).unwrap_err().to_string(),
                "not writable"
            );
            fs::set_permissions(&ro_dir, fs::Permissions::from_mode(0o755)).unwrap();
        }

        // `/` -> `denied: filesystem root`.
        assert_eq!(
            validate_workspace_dir(Path::new("/"))
                .unwrap_err()
                .to_string(),
            "denied: filesystem root"
        );

        // State dir and child inside it -> `denied: DmuxAI state directory`.
        let state_child = state_dir.join("repos").join("x");
        fs::create_dir_all(&state_child).unwrap();
        assert_eq!(
            validate_workspace_dir(&state_dir).unwrap_err().to_string(),
            "denied: DmuxAI state directory"
        );
        assert_eq!(
            validate_workspace_dir(&state_child)
                .unwrap_err()
                .to_string(),
            "denied: DmuxAI state directory"
        );

        // User config dir and child inside it -> `denied: DmuxAI config directory`.
        let cfg_child = dmux_cfg_dir.join("sub");
        fs::create_dir_all(&cfg_child).unwrap();
        assert_eq!(
            validate_workspace_dir(&dmux_cfg_dir)
                .unwrap_err()
                .to_string(),
            "denied: DmuxAI config directory"
        );
        assert_eq!(
            validate_workspace_dir(&cfg_child).unwrap_err().to_string(),
            "denied: DmuxAI config directory"
        );

        // Credential dirs and child inside `~/.ssh` -> `denied: credential directory`.
        for rel in CREDENTIAL_DIRS_FROM_HOME {
            let cred = home.join(rel);
            fs::create_dir_all(&cred).unwrap();
            assert_eq!(
                validate_workspace_dir(&cred).unwrap_err().to_string(),
                "denied: credential directory",
                "failed for credential dir {rel}"
            );
        }
        let ssh_sub = home.join(".ssh").join("keys");
        fs::create_dir_all(&ssh_sub).unwrap();
        assert_eq!(
            validate_workspace_dir(&ssh_sub).unwrap_err().to_string(),
            "denied: credential directory"
        );

        // NUL or > 4096 bytes -> `invalid path`.
        assert_eq!(
            validate_workspace_dir(Path::new("bad\0path"))
                .unwrap_err()
                .to_string(),
            "invalid path"
        );
        let huge = "a".repeat(4097);
        assert_eq!(
            validate_workspace_dir(Path::new(&huge))
                .unwrap_err()
                .to_string(),
            "invalid path"
        );

        // Recent workspaces persistence & filtering of invalidated entries.
        record_recent_workspace(&dir_flag).unwrap();
        record_recent_workspace(&dir_env).unwrap();
        let recent = load_recent_workspaces();
        assert_eq!(
            recent,
            vec![
                dir_env
                    .canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                dir_flag
                    .canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            ]
        );

        // Restore env.
        unsafe {
            match prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match prev_state {
                Some(v) => std::env::set_var("DMUXAI_STATE_DIR", v),
                None => std::env::remove_var("DMUXAI_STATE_DIR"),
            }
            match prev_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match prev_ws {
                Some(v) => std::env::set_var("DMUXAI_WORKSPACE", v),
                None => std::env::remove_var("DMUXAI_WORKSPACE"),
            }
        }
    }
}
