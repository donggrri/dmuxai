use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::Hasher,
    io::{self, Read},
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

/// How long `git status` may run when sampling the workspace for read-only checks.
const GIT_WORKTREE_FINGERPRINT_TIMEOUT: Duration = Duration::from_secs(5);

/// Chunk size used to stream worktree file content through the fingerprint
/// hash (FR-2540). Memory per file is bounded to this buffer regardless of
/// file size; the whole file is still hashed.
const GIT_FINGERPRINT_HASH_CHUNK_BYTES: usize = 64 * 1024;

/// Maximum number of porcelain entries hashed into the read-only fingerprint;
/// beyond this only the status text is sampled (FR-2540).
const GIT_FINGERPRINT_MAX_ENTRIES: usize = 4096;

/// Maximum number of entries persisted in `recent-workspaces.json` (FR-872).
pub const MAX_RECENT_WORKSPACES: usize = 10;

/// Workspace-local directory for DmuxAI-generated state.
pub const WORKSPACE_STATE_DIR: &str = ".dmuxai";

/// Self-ignoring content for `<workspace>/.dmuxai/.gitignore` (FR-2501).
pub const WORKSPACE_STATE_GITIGNORE: &str = "*\n";

/// Ensures `dir` (normally `<workspace>/.dmuxai`) exists and contains a
/// self-ignoring `.gitignore`. An existing ignore file is never modified.
pub fn ensure_state_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let ignore = dir.join(".gitignore");
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&ignore)
    {
        Ok(mut file) => {
            use std::io::Write;
            file.write_all(WORKSPACE_STATE_GITIGNORE.as_bytes())?;
            file.flush()?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    Ok(())
}

/// Ensures the workspace state directory that owns `path` exists and is
/// self-ignoring, then creates `path` itself (FR-2501..FR-2503).
pub fn ensure_state_path(path: &Path) -> io::Result<()> {
    let state_dir = path
        .ancestors()
        .find(|candidate| candidate.file_name() == Some(std::ffi::OsStr::new(WORKSPACE_STATE_DIR)))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "path is not inside the workspace state directory",
            )
        })?;
    ensure_state_dir(state_dir)?;
    fs::create_dir_all(path)?;
    Ok(())
}

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

/// Strips `--config PATH` / `--config=PATH` (before `--`) from `args`.
/// Returns `(last_config_arg, remaining_args)`.
pub fn extract_config_flag<I, S>(args: I) -> Result<(Option<String>, Vec<String>), String>
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
            "--config" => {
                index += 1;
                match raw.get(index) {
                    Some(val) => chosen = Some(val.clone()),
                    None => return Err(String::new()),
                }
            }
            s if s.starts_with("--config=") => {
                chosen = Some(s["--config=".len()..].to_owned());
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
    let (_, stripped) =
        extract_config_flag(stripped.clone()).unwrap_or_else(|_| (None, stripped.clone()));
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
        .env(HEADLESS_DASHBOARD_ENV, "1")
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

/// Set by [`spawn_dashboard`] for a terminal-less re-exec (FR-1470); kept only
/// so child binaries older than FR-1600 still see it. Since FR-1600 `--web` is
/// always headless, so the variable no longer gates behaviour.
pub const HEADLESS_DASHBOARD_ENV: &str = "DMUXAI_HEADLESS_DASHBOARD";

/// Worktree-state fingerprint of the workspace, excluding `.dmuxai/` (FR-2540).
///
/// The fingerprint combines `git status --porcelain=v2` entries with the staged
/// blob hash (`hI`) and a worktree content summary for every listed path, so
/// changing the content of an already-dirty or already-untracked file is
/// detected even though the file's status line would stay identical to a
/// filename/status-only comparison.
///
/// Returns `None` when the directory is not inside a git repository, git is
/// missing, or the sampling does not finish in time.
pub fn git_worktree_fingerprint_sync(workspace: &Path) -> Option<String> {
    let status = run_git_quiet(
        workspace,
        &[
            "status",
            "--porcelain=v2",
            "--untracked-files=all",
            "--no-renames",
            "-z",
            "--",
            ".",
            ":(exclude).dmuxai",
        ],
    )?;
    // Porcelain paths are relative to the repository root, which may be above
    // the workspace when the workspace is a subdirectory.
    let repo_root = run_git_quiet(workspace, &["rev-parse", "--show-toplevel"])?;
    let repo_root = PathBuf::from(String::from_utf8_lossy(&repo_root).trim().to_owned());

    let mut fingerprint = String::new();
    let mut entries = 0usize;
    for raw in status.split(|byte| *byte == 0) {
        if raw.is_empty() {
            continue;
        }
        entries += 1;
        if entries > GIT_FINGERPRINT_MAX_ENTRIES {
            fingerprint.push_str("truncated\n");
            break;
        }
        fingerprint.push_str(&fingerprint_entry(
            &String::from_utf8_lossy(raw),
            &repo_root,
        ));
        fingerprint.push('\n');
    }
    Some(fingerprint)
}

fn run_git_quiet(workspace: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if output.status.success() {
        Some(output.stdout)
    } else {
        None
    }
}

/// Canonical fingerprint contribution of one porcelain entry (FR-2540): status
/// letters, modes, staged blob hash, path, and a worktree content summary.
fn fingerprint_entry(token: &str, repo_root: &Path) -> String {
    let kind = token.split(' ').next().unwrap_or("");
    match kind {
        "1" => {
            let mut fields = token.splitn(9, ' ');
            let _ = fields.next();
            let x_y = fields.next().unwrap_or("");
            let sub = fields.next().unwrap_or("");
            let m_h = fields.next().unwrap_or("");
            let m_i = fields.next().unwrap_or("");
            let m_w = fields.next().unwrap_or("");
            let head_hash = fields.next().unwrap_or("");
            let index_hash = fields.next().unwrap_or("");
            let path = fields.next().unwrap_or("");
            format!(
                "{x_y} {sub} {m_h} {m_i} {m_w} {head_hash} {index_hash} {path} {}",
                worktree_content_summary(&repo_root.join(path)),
            )
        }
        "u" => {
            let mut fields = token.splitn(11, ' ');
            let _ = fields.next();
            let x_y = fields.next().unwrap_or("");
            let sub = fields.next().unwrap_or("");
            let m1 = fields.next().unwrap_or("");
            let m2 = fields.next().unwrap_or("");
            let m3 = fields.next().unwrap_or("");
            let m_w = fields.next().unwrap_or("");
            let h1 = fields.next().unwrap_or("");
            let h2 = fields.next().unwrap_or("");
            let h3 = fields.next().unwrap_or("");
            let path = fields.next().unwrap_or("");
            format!(
                "{x_y} {sub} {m1} {m2} {m3} {m_w} {h1} {h2} {h3} {path} {}",
                worktree_content_summary(&repo_root.join(path)),
            )
        }
        "?" => {
            let path = token.strip_prefix("? ").unwrap_or("");
            format!(
                "? {path} {}",
                worktree_content_summary(&repo_root.join(path))
            )
        }
        _ => token.to_owned(),
    }
}

/// Content summary of a worktree file used by the read-only fingerprint
/// (FR-2540): the file length plus a full-file streaming hash, read in
/// bounded [`GIT_FINGERPRINT_HASH_CHUNK_BYTES`] chunks, so an edit at any
/// offset of an already-dirty or untracked file is detected even when its
/// length and porcelain line stay identical. Unreadable paths and non-regular
/// files (FIFOs, sockets, directories — opening one can block) get a stable
/// marker so deletions and permission failures remain deterministic across
/// samples.
fn worktree_content_summary(path: &Path) -> String {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return "unreadable".to_owned(),
    };
    if !meta.is_file() {
        return "unreadable".to_owned();
    }
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return "unreadable".to_owned(),
    };
    let len = meta.len();
    let mut hasher = DefaultHasher::new();
    let mut buf = vec![0u8; GIT_FINGERPRINT_HASH_CHUNK_BYTES];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.write(&buf[..n]),
            Err(_) => return "i/o-error".to_owned(),
        }
    }
    format!("len={len}hash={:016x}", hasher.finish())
}

/// Async wrapper around [`git_worktree_fingerprint_sync`] with a bounded wait.
pub async fn git_worktree_fingerprint(workspace: &Path) -> Option<String> {
    let workspace = workspace.to_path_buf();
    tokio::time::timeout(
        GIT_WORKTREE_FINGERPRINT_TIMEOUT,
        tokio::task::spawn_blocking(move || git_worktree_fingerprint_sync(&workspace)),
    )
    .await
    .ok()?
    .ok()?
}

/// Returns true when the worktree fingerprint changed since `before` (FR-2540).
pub async fn read_only_workspace_write_detected(before: Option<&str>, workspace: &Path) -> bool {
    let Some(before) = before else {
        return false;
    };
    git_worktree_fingerprint(workspace)
        .await
        .is_some_and(|after| after != before)
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

    // TC-1470 (FR-1470): "Open in new window" asks for a headless dashboard.
    #[test]
    fn tc_1470_spawn_dashboard_sets_headless_env() {
        assert_eq!(super::HEADLESS_DASHBOARD_ENV, "DMUXAI_HEADLESS_DASHBOARD");
        let args = super::open_dashboard_args(std::path::Path::new("/w"));
        assert_eq!(args, ["--web", "-C", "/w", "--port", "0"]);
    }

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

    // TC-2501 / FR-2501, FR-2502: first creation writes `*`, repeated calls
    // are idempotent, an existing ignore file is preserved, and the workspace
    // root `.gitignore` is never touched.
    #[test]
    fn tc_2501_state_dir_gitignore_is_created_idempotently() {
        let dir = tempdir().unwrap();
        let root_ignore = dir.path().join(".gitignore");
        fs::write(&root_ignore, "target\n").unwrap();
        let state = dir.path().join(WORKSPACE_STATE_DIR);

        ensure_state_path(&state.join("attachments")).unwrap();
        assert_eq!(
            fs::read_to_string(state.join(".gitignore")).unwrap(),
            WORKSPACE_STATE_GITIGNORE
        );
        assert_eq!(fs::read_to_string(&root_ignore).unwrap(), "target\n");

        fs::write(state.join("attachments").join("keep.txt"), "keep").unwrap();
        ensure_state_path(&state.join("attachments")).unwrap();
        assert_eq!(
            fs::read_to_string(state.join(".gitignore")).unwrap(),
            WORKSPACE_STATE_GITIGNORE
        );
        assert_eq!(
            fs::read_to_string(state.join("attachments").join("keep.txt")).unwrap(),
            "keep"
        );

        fs::write(state.join(".gitignore"), "custom\n").unwrap();
        ensure_state_dir(&state).unwrap();
        assert_eq!(
            fs::read_to_string(state.join(".gitignore")).unwrap(),
            "custom\n",
            "an existing user ignore file must not be overwritten or appended"
        );

        let missing = tempdir().unwrap();
        fs::create_dir_all(missing.path().join(WORKSPACE_STATE_DIR)).unwrap();
        ensure_state_dir(&missing.path().join(WORKSPACE_STATE_DIR)).unwrap();
        assert_eq!(
            fs::read_to_string(missing.path().join(WORKSPACE_STATE_DIR).join(".gitignore"))
                .unwrap(),
            WORKSPACE_STATE_GITIGNORE,
            "an existing state directory without an ignore file is filled in"
        );
        assert!(!missing.path().join(".gitignore").exists());
    }

    // TC-870 (FR-870, FR-873): precedence order and directory validation.
    #[test]
    fn tc_870_workspace_dir_precedence_and_validation() {
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

    fn git_init_commit_repo(dir: &Path) -> impl Fn(&[&str]) + '_ {
        let run = move |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .status()
                .expect("git");
            assert!(status.success(), "git {:?} failed", args);
        };
        run(&["init", "-q"]);
        std::fs::write(dir.join("README.md"), "# test\n").unwrap();
        run(&["add", "README.md"]);
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "init",
        ]);
        run
    }

    // TC-2540 / FR-2540: the fingerprint ignores `.dmuxai/` and detects plain
    // path changes as well as content edits to files that are already dirty or
    // untracked, where porcelain status alone would not change.
    #[test]
    fn tc_2540_git_worktree_fingerprint_excludes_dmuxai_and_sees_content_edits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let git = git_init_commit_repo(root);
        let before = super::git_worktree_fingerprint_sync(root).unwrap();
        assert!(before.is_empty(), "clean repo: {before:?}");

        std::fs::create_dir_all(root.join(".dmuxai/graphs")).unwrap();
        std::fs::write(root.join(".dmuxai/graphs/run.json"), "{}").unwrap();
        assert_eq!(
            super::git_worktree_fingerprint_sync(root).as_deref(),
            Some(before.as_str()),
            ".dmuxai changes must not affect the fingerprint"
        );

        // Content edits to an already-dirty tracked file change the fingerprint
        // even though the porcelain status line is `"M README.md"` both times.
        std::fs::write(root.join("README.md"), "dirty edit one\n").unwrap();
        let dirty_one = super::git_worktree_fingerprint_sync(root).unwrap();
        assert_ne!(
            before, dirty_one,
            "worktree edit must change the fingerprint"
        );
        std::fs::write(root.join("README.md"), "dirty edit two\n").unwrap();
        let dirty_two = super::git_worktree_fingerprint_sync(root).unwrap();
        assert_ne!(
            dirty_one, dirty_two,
            "a second edit to the same already-dirty file must change the fingerprint"
        );

        // Staging different content changes the staged blob hash.
        git(&["add", "README.md"]);
        let staged = super::git_worktree_fingerprint_sync(root).unwrap();
        assert_ne!(
            dirty_two, staged,
            "staging new content must change the fingerprint"
        );

        // Untracked file: creation and later content edits are detected.
        std::fs::write(root.join("SQA_WRITE.txt"), "one\n").unwrap();
        let untracked_one = super::git_worktree_fingerprint_sync(root).unwrap();
        assert_ne!(staged, untracked_one);
        std::fs::write(root.join("SQA_WRITE.txt"), "two\n").unwrap();
        let untracked_two = super::git_worktree_fingerprint_sync(root).unwrap();
        assert_ne!(
            untracked_one, untracked_two,
            "editing an already-untracked file must change the fingerprint"
        );
    }

    // TC-2543 (FR-2540): content detection covers the entire file, not just a
    // prefix. Editing a single byte beyond the first MiB with unchanged length
    // and an identical porcelain status line must change the fingerprint for
    // an already-dirty tracked file and for an already-untracked file.
    #[test]
    fn tc_2543_fingerprint_detects_tail_edit_beyond_first_mib() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let git = git_init_commit_repo(root);

        let mut body = vec![b'a'; 1 << 20];
        body.extend_from_slice(b"tailA\n");
        std::fs::write(root.join("big.txt"), &body).unwrap();
        git(&["add", "big.txt"]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "big",
        ]);

        // Make the tracked file already-dirty with a head-region edit.
        body[0] = b'b';
        std::fs::write(root.join("big.txt"), &body).unwrap();
        let dirty_one = super::git_worktree_fingerprint_sync(root).unwrap();
        let status_one = porcelain_line(root, "big.txt");

        // Tail-only edit: same length, byte at offset 1 MiB, porcelain status
        // unchanged — this is exactly what a first-MiB prefix hash would miss.
        body[1 << 20] = b'B';
        std::fs::write(root.join("big.txt"), &body).unwrap();
        let dirty_two = super::git_worktree_fingerprint_sync(root).unwrap();
        let status_two = porcelain_line(root, "big.txt");
        assert_eq!(
            status_one, status_two,
            "porcelain status must stay identical"
        );
        assert_ne!(
            dirty_one, dirty_two,
            "a tail edit beyond the first MiB must change the fingerprint"
        );

        // Already-untracked file larger than 1 MiB; tail-only edit with
        // unchanged length and unchanged porcelain status.
        let mut untracked = vec![b'x'; 1 << 20];
        untracked.extend_from_slice(b"tailU\n");
        std::fs::write(root.join("UNTRACKED_BIG.txt"), &untracked).unwrap();
        let untracked_one = super::git_worktree_fingerprint_sync(root).unwrap();
        let untracked_status_one = porcelain_line(root, "UNTRACKED_BIG.txt");
        untracked[1 << 20] = b'Y';
        std::fs::write(root.join("UNTRACKED_BIG.txt"), &untracked).unwrap();
        let untracked_two = super::git_worktree_fingerprint_sync(root).unwrap();
        let untracked_status_two = porcelain_line(root, "UNTRACKED_BIG.txt");
        assert_eq!(
            untracked_status_one, untracked_status_two,
            "untracked porcelain status must stay identical"
        );
        assert_ne!(
            untracked_one, untracked_two,
            "a tail edit to an already-untracked file must change the fingerprint"
        );
    }

    /// The single porcelain v1 line git reports for `path`, as raw status
    /// evidence that the tracked tail-edit case leaves the line identical.
    fn porcelain_line(root: &Path, path: &str) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["status", "--porcelain", "--untracked-files=all", "--", path])
            .output()
            .expect("git status");
        assert!(out.status.success(), "git status {path} failed");
        String::from_utf8_lossy(&out.stdout).trim_end().to_owned()
    }

    // FR-2540 robustness (whole-file streaming): opening a non-regular file
    // (a FIFO here) must neither hang the sampling thread on a blocking
    // open(2) nor break fingerprint determinism across samples. Note: git
    // status itself does not list FIFOs, so the helper is also asserted
    // directly.
    #[cfg(unix)]
    #[test]
    fn fingerprint_skips_untracked_fifo_without_hanging() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _git = git_init_commit_repo(root);
        let fifo = root.join("pipe.fifo");
        let ok = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .current_dir(root)
            .status()
            .expect("mkfifo")
            .success();
        assert!(ok, "mkfifo failed");

        let started = std::time::Instant::now();
        let summary = super::worktree_content_summary(&fifo);
        assert_eq!(
            summary, "unreadable",
            "a non-regular file gets the stable unreadable marker {summary:?}"
        );
        let second_summary = super::worktree_content_summary(&fifo);
        assert_eq!(
            summary, second_summary,
            "FIFO handling must be deterministic across samples"
        );

        let first = super::git_worktree_fingerprint_sync(root).unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "fingerprinting must not block on a FIFO open: {first:?}"
        );
        let second = super::git_worktree_fingerprint_sync(root).unwrap();
        assert_eq!(first, second);
    }
}
