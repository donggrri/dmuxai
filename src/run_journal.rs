//! Run journal storage, metadata, masking, and retention (SPEC-run-journal FR-2560..FR-2566).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::state_path::{repo_key, valid_pending_plan_hash};

/// Run ID length: exactly 16 lowercase hex characters (`[0-9a-f]{16}`).
pub const RUN_ID_LEN: usize = 16;

/// Validates that a run ID contains only 16 lowercase hex chars (no path traversal).
pub fn valid_run_id(id: &str) -> bool {
    valid_pending_plan_hash(id)
}

/// Generates a new random 16-character lowercase hex run ID.
pub fn generate_run_id() -> String {
    let raw = uuid::Uuid::new_v4().simple().to_string();
    raw[..RUN_ID_LEN].to_ascii_lowercase()
}

/// Maximum allowed manifest file size on read (128 KiB).
pub const MAX_MANIFEST_READ_BYTES: u64 = 128 * 1024;

/// Maximum diagnostics tail buffer size on write (64 KiB).
pub const MAX_DIAGNOSTICS_WRITE_BYTES: usize = 64 * 1024;

/// Execution origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOrigin {
    CliHeadless,
    WebDashboard,
    Tui,
}

/// Run terminal or active status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Active,
    #[serde(alias = "running")]
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, RunStatus::Active | RunStatus::Running)
    }

    pub fn is_terminal(&self) -> bool {
        !self.is_active()
    }
}

impl std::str::FromStr for RunStatus {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(RunStatus::Active),
            "running" => Ok(RunStatus::Running),
            "completed" => Ok(RunStatus::Completed),
            "failed" => Ok(RunStatus::Failed),
            "cancelled" => Ok(RunStatus::Cancelled),
            _ => Err("unknown run status"),
        }
    }
}

/// Links to conversation and graph structures.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLinks {
    pub conversation_id: Option<String>,
    pub turn_id: Option<String>,
    pub graph_id: Option<String>,
    pub node_id: Option<String>,
    pub attempt_index: Option<u32>,
}

/// Diagnostic references in manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunDiagnosticsRef {
    pub stderr_bytes: u64,
    pub tail_sample: Option<String>,
}

/// Run manifest stored on disk as `manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunManifest {
    pub schema_version: u32,
    pub run_id: String,
    pub origin: String,
    pub status: String,
    pub created_at_ms: u64,
    pub completed_at_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub requested_runner: Option<String>,
    pub resolved_runner: Option<String>,
    pub requested_model: Option<String>,
    pub resolved_model: Option<String>,
    pub dmuxai_version: Option<String>,
    pub build_commit: Option<String>,
    #[serde(default)]
    pub dmuxai_exit_code: Option<i32>,
    pub child_exit_code: Option<i32>,
    pub child_signal: Option<i32>,
    pub error_kind: Option<String>,
    pub error_summary: Option<String>,
    pub links: RunLinks,
    pub diagnostics: RunDiagnosticsRef,
    pub end_summary: Option<serde_json::Value>,
}

/// Typed journal storage error.
#[derive(Debug)]
pub enum JournalError {
    Io(io::Error),
    SymlinkDetected(PathBuf),
    InvalidId(String),
    IdMismatch { expected: String, found: String },
    UnknownSchemaVersion(u32),
    SizeExceeded { size: u64, limit: u64 },
    CorruptManifest(String),
}

impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::SymlinkDetected(p) => write!(f, "symlink rejected at {}", p.display()),
            Self::InvalidId(id) => write!(f, "invalid run id '{id}'"),
            Self::IdMismatch { expected, found } => {
                write!(f, "run id mismatch: expected '{expected}', found '{found}'")
            }
            Self::UnknownSchemaVersion(v) => write!(f, "unknown schema version {v}"),
            Self::SizeExceeded { size, limit } => {
                write!(f, "file size {size} exceeds limit of {limit} bytes")
            }
            Self::CorruptManifest(e) => write!(f, "corrupt manifest: {e}"),
        }
    }
}

impl std::error::Error for JournalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for JournalError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// Retention and storage configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalConfig {
    pub max_age_ms: u64,
    pub max_total_bytes: u64,
}

impl Default for JournalConfig {
    fn default() -> Self {
        Self {
            max_age_ms: 90 * 86_400 * 1000,      // 90 days
            max_total_bytes: 1024 * 1024 * 1024, // 1 GiB
        }
    }
}

/// Pruning report.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PruneReport {
    pub deleted_runs: usize,
    pub reclaimed_bytes: u64,
}

// Pre-compiled regex patterns for secret masking
static ANSI_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]|\x1b\([a-zA-Z]|\x1b\][^\x07\x1b]*(\x07|\x1b\\)").unwrap()
});

static BEARER_AUTH_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Matches Bearer <token> (even short tokens) and Authorization: <scheme> <token>
    // e.g. "Authorization: Bearer sk-12345" or "Authorization: Basic dXNlcjpwYXNz" or "Bearer abc"
    Regex::new(r"(?i)\bBearer\s+[^\s,;'\x22]+").unwrap()
});

static AUTH_HEADER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bAuthorization:\s*(?:[A-Za-z0-9_-]+\s+)?[^\s,;'\x22\r\n]+").unwrap()
});

static JSON_SECRET_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Matches JSON quoted key / value assignments e.g. "api_key": "my secret" or "secret": "abc"
    Regex::new(r#"(?i)"([a-z0-9_]*(?:api_key|token|secret|password|access_token)[a-z0-9_]*)"\s*:\s*"([^"\\]*(?:\\.[^"\\]*)*)""#).unwrap()
});

static ASSIGNMENT_QUOTED_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Matches KEY="value with spaces" or KEY='value with spaces'
    Regex::new(r#"(?i)\b([a-z0-9_]*(?:api_key|token|secret|password|access_token)[a-z0-9_]*)\s*([:=])\s*(?:"([^"]*)"|'([^']*)')"#).unwrap()
});

static ASSIGNMENT_UNQUOTED_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Matches KEY=value (unquoted)
    Regex::new(r#"(?i)\b([a-z0-9_]*(?:api_key|token|secret|password|access_token)[a-z0-9_]*)\s*([:=])\s*([^\s,;'"\r\n]+)"#).unwrap()
});

static URL_QUERY_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Matches ?token=... or &token=... or ?api_key=...
    Regex::new(r"(?i)([?&](?:api_key|token|auth|access_token|key|sig|signature)=)([^&\s#]+)")
        .unwrap()
});

static COMMON_TOKEN_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Matches sk-..., ghp_...
    Regex::new(r"\b(sk-[A-Za-z0-9_-]{10,}|ghp_[A-Za-z0-9]{10,})\b").unwrap()
});

/// Strips ANSI escape sequences and non-standard control characters (retaining \n, \r, \t).
pub fn strip_ansi_and_control(input: &str) -> String {
    let stripped = ANSI_RE.replace_all(input, "");
    let mut out = String::with_capacity(stripped.len());
    for ch in stripped.chars() {
        if ch == '\n' || ch == '\r' || ch == '\t' || !ch.is_control() {
            out.push(ch);
        }
    }
    out
}

/// Masks sensitive credentials in text before writing to disk.
pub fn mask_sensitive_tokens(input: &str) -> String {
    let clean = strip_ansi_and_control(input);

    // 1. JSON quoted secret fields: "api_key": "secret" -> "api_key": "[REDACTED]"
    let step1 = JSON_SECRET_RE.replace_all(&clean, |caps: &regex::Captures| {
        format!(r#""{}": "[REDACTED]""#, &caps[1])
    });

    // 2. Quoted assignments with spaces: API_KEY="my secret key" -> API_KEY="[REDACTED]"
    let step2 = ASSIGNMENT_QUOTED_RE.replace_all(&step1, |caps: &regex::Captures| {
        let key = &caps[1];
        let sep = &caps[2];
        format!(r#"{key}{sep}"[REDACTED]""#)
    });

    // 3. Unquoted assignments: API_KEY=secret -> API_KEY=[REDACTED]
    let step3 = ASSIGNMENT_UNQUOTED_RE.replace_all(&step2, |caps: &regex::Captures| {
        let key = &caps[1];
        let sep = &caps[2];
        format!("{key}{sep}[REDACTED]")
    });

    // 4. Authorization headers
    let step4 = AUTH_HEADER_RE.replace_all(&step3, "Authorization: [REDACTED]");

    // 5. Bearer tokens
    let step5 = BEARER_AUTH_RE.replace_all(&step4, "Bearer [REDACTED]");

    // 6. URL query tokens: ?token=xyz -> ?token=[REDACTED]
    let step6 = URL_QUERY_RE.replace_all(&step5, "${1}[REDACTED]");

    // 7. Common token prefixes (e.g. sk-...)
    let step7 = COMMON_TOKEN_PREFIX_RE.replace_all(&step6, "[REDACTED]");

    step7.into_owned()
}

/// Checks if a JSON object key indicates sensitive content.
fn is_sensitive_key_name(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower.contains("token")
        || lower.contains("password")
        || lower.contains("secret")
        || lower.contains("api_key")
        || lower.contains("apikey")
        || lower.contains("access_token")
        || lower.contains("auth")
}

/// Recursively masks all string values in a serde_json::Value,
/// redacting sensitive field values and sanitizing secret tokens.
pub fn mask_json_value(val: &mut serde_json::Value) {
    match val {
        serde_json::Value::String(s) => {
            *s = mask_sensitive_tokens(s);
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                mask_json_value(item);
            }
        }
        serde_json::Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                if let Some(item) = map.get_mut(&key) {
                    if is_sensitive_key_name(&key) {
                        match item {
                            serde_json::Value::String(_) => {
                                *item = serde_json::Value::String("[REDACTED]".to_string());
                            }
                            serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                                mask_json_value(item);
                            }
                            _ => {
                                *item = serde_json::Value::String("[REDACTED]".to_string());
                            }
                        }
                    } else {
                        mask_json_value(item);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Truncates a UTF-8 string to at most `max_bytes` keeping valid char boundaries.
/// Returns the tail of the string (last <= max_bytes).
pub fn truncate_tail_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut start = s.len() - max_bytes;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// Verifies that neither the given path nor any ancestor is a symlink.
pub fn verify_no_symlinks(path: &Path) -> Result<(), JournalError> {
    let mut current = Some(path);
    while let Some(p) = current {
        if p.as_os_str().is_empty() {
            break;
        }
        match fs::symlink_metadata(p) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(JournalError::SymlinkDetected(p.to_path_buf()));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // Not found is fine for components that don't exist yet
            }
            Err(e) => return Err(JournalError::Io(e)),
        }
        current = p.parent();
    }
    Ok(())
}

/// Checks if a direct path is a symlink.
pub fn is_symlink(path: &Path) -> Result<bool, io::Error> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(meta.file_type().is_symlink()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Helper to secure permissions of an existing directory without following symlinks.
#[cfg(unix)]
pub(crate) fn secure_dir_permissions_0700(path: &Path) -> Result<(), io::Error> {
    if is_symlink(path)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to chmod symlink at {}", path.display()),
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)?;
    let meta = file.metadata()?;
    let mode = meta.permissions().mode() & 0o777;
    if mode != 0o700 {
        file.set_permissions(fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn secure_dir_permissions_0700(_path: &Path) -> Result<(), io::Error> {
    Ok(())
}

/// Helper to fsync a directory after file rename or modification without following symlinks.
#[cfg(unix)]
pub(crate) fn sync_dir(path: &Path) -> Result<(), io::Error> {
    if is_symlink(path)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to sync symlink directory at {}", path.display()),
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)?;
    file.sync_all()
}

#[cfg(not(unix))]
pub(crate) fn sync_dir(_path: &Path) -> Result<(), io::Error> {
    Ok(())
}

/// Resolves the per-repo data directory for dmuxai artifacts:
/// `$XDG_DATA_HOME/dmuxai/repos/<repo-key>` or `~/.local/share/dmuxai/repos/<repo-key>`.
pub fn repo_data_dir(workspace_root: &Path) -> io::Result<PathBuf> {
    let base = if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        if !data_home.is_empty() {
            PathBuf::from(data_home).join("dmuxai")
        } else {
            fallback_data_base()?
        }
    } else {
        fallback_data_base()?
    };
    let key = repo_key(workspace_root)?;
    Ok(base.join("repos").join(key))
}

/// Resolves the XDG data home base directory for dmuxai runs:
/// `$XDG_DATA_HOME/dmuxai/repos/<repo-key>/runs` or `~/.local/share/dmuxai/repos/<repo-key>/runs`.
pub fn repo_runs_dir(workspace_root: &Path) -> io::Result<PathBuf> {
    Ok(repo_data_dir(workspace_root)?.join("runs"))
}

fn fallback_data_base() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "neither XDG_DATA_HOME nor HOME is configured",
        )
    })?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("dmuxai"))
}

/// Open a file for writing with O_NOFOLLOW and 0600 permissions.
#[cfg(unix)]
pub(crate) fn open_file_write_nofollow(path: &Path, create_new: bool) -> Result<File, io::Error> {
    let mut opts = OpenOptions::new();
    opts.write(true);
    if create_new {
        opts.create_new(true);
    } else {
        opts.create(true).truncate(true);
    }
    opts.mode(0o600);
    opts.custom_flags(libc::O_NOFOLLOW);
    opts.open(path)
}

#[cfg(not(unix))]
pub(crate) fn open_file_write_nofollow(path: &Path, create_new: bool) -> Result<File, io::Error> {
    let mut opts = OpenOptions::new();
    opts.write(true);
    if create_new {
        opts.create_new(true);
    } else {
        opts.create(true).truncate(true);
    }
    opts.open(path)
}

/// Open a file for reading with O_NOFOLLOW and O_NONBLOCK (on Unix) to avoid blocking FIFO opens.
#[cfg(unix)]
fn open_file_read_nofollow(path: &Path) -> Result<File, io::Error> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    opts.open(path)
}

#[cfg(not(unix))]
fn open_file_read_nofollow(path: &Path) -> Result<File, io::Error> {
    File::open(path)
}

/// Journal storage engine managing runs for a repository.
#[derive(Debug, Clone)]
pub struct JournalStore {
    root: PathBuf,
}

impl JournalStore {
    /// Creates a new JournalStore with the given root runs directory.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Root path of the store.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Saves a run manifest atomically, masking secrets and enforcing directory permissions.
    /// This is the raw low-level unpruned save API.
    pub fn save(&self, manifest: &RunManifest) -> Result<(), JournalError> {
        self.save_internal(manifest)?;
        Ok(())
    }

    /// Saves a run manifest atomically and optionally prunes old terminal runs according to policy.
    /// Returns the prune report when retention pruning was executed, or propagates any prune errors.
    pub fn save_with_retention(
        &self,
        manifest: &RunManifest,
        retention_config: Option<&JournalConfig>,
        now_ms: u64,
    ) -> Result<Option<PruneReport>, JournalError> {
        self.save_internal(manifest)?;

        if let Some(cfg) = retention_config {
            let report = self.prune(cfg, now_ms)?;
            Ok(Some(report))
        } else {
            Ok(None)
        }
    }

    fn save_internal(&self, manifest: &RunManifest) -> Result<(), JournalError> {
        let run_id = &manifest.run_id;
        if !valid_run_id(run_id) {
            return Err(JournalError::InvalidId(run_id.clone()));
        }

        // Verify root and path ancestors have no symlinks
        verify_no_symlinks(&self.root)?;

        if self.root.exists() && !self.root.is_dir() {
            return Err(JournalError::Io(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("store root {} is not a directory", self.root.display()),
            )));
        }

        // Ensure root exists with 0700 permissions
        #[cfg(unix)]
        {
            if !self.root.exists() {
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(&self.root)?;
            }
            secure_dir_permissions_0700(&self.root)?;
        }
        #[cfg(not(unix))]
        {
            if !self.root.exists() {
                fs::create_dir_all(&self.root)?;
            }
        }

        let run_dir = self.root.join(run_id);
        if is_symlink(&run_dir)? {
            return Err(JournalError::SymlinkDetected(run_dir));
        }

        #[cfg(unix)]
        {
            if !run_dir.exists() {
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(&run_dir)?;
            }
            secure_dir_permissions_0700(&run_dir)?;
        }
        #[cfg(not(unix))]
        {
            if !run_dir.exists() {
                fs::create_dir_all(&run_dir)?;
            }
        }

        // Verify run_dir is not a symlink after creation/check
        if is_symlink(&run_dir)? {
            return Err(JournalError::SymlinkDetected(run_dir));
        }

        let manifest_path = run_dir.join("manifest.json");
        if is_symlink(&manifest_path)? {
            return Err(JournalError::SymlinkDetected(manifest_path));
        }

        // Prepare masked manifest JSON
        let mut value = serde_json::to_value(manifest)
            .map_err(|e| JournalError::CorruptManifest(e.to_string()))?;
        mask_json_value(&mut value);

        // Normalize status alias if necessary, keeping active authoritative
        if let Some(st) = value.get_mut("status").and_then(|v| v.as_str())
            && st == "running"
        {
            value["status"] = serde_json::Value::String("active".to_string());
        }

        let payload = serde_json::to_vec_pretty(&value)
            .map_err(|e| JournalError::CorruptManifest(e.to_string()))?;

        // Bug 3: Enforce serialized payload cap (128 KiB) before write
        if payload.len() as u64 > MAX_MANIFEST_READ_BYTES {
            return Err(JournalError::SizeExceeded {
                size: payload.len() as u64,
                limit: MAX_MANIFEST_READ_BYTES,
            });
        }

        // Write atomically using random tmp file in run_dir
        let tmp_name = format!(".manifest-{}.tmp", uuid::Uuid::new_v4().simple());
        let tmp_path = run_dir.join(tmp_name);

        let write_res = (|| -> io::Result<()> {
            let mut tmp_file = open_file_write_nofollow(&tmp_path, true)?;
            tmp_file.write_all(&payload)?;
            tmp_file.sync_all()?;
            Ok(())
        })();

        if let Err(e) = write_res {
            let _ = fs::remove_file(&tmp_path);
            return Err(JournalError::Io(e));
        }

        // Rename tmp to manifest.json
        if let Err(e) = fs::rename(&tmp_path, &manifest_path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(JournalError::Io(e));
        }

        // Sync parent directory after successful rename for durable atomic contract
        sync_dir(&run_dir)?;

        Ok(())
    }

    /// Loads a run manifest from disk with 128 KiB size bound and symlink rejection.
    pub fn load(&self, id: &str) -> Result<RunManifest, JournalError> {
        if !valid_run_id(id) {
            return Err(JournalError::InvalidId(id.to_string()));
        }

        verify_no_symlinks(&self.root)?;

        let run_dir = self.root.join(id);
        if is_symlink(&run_dir)? {
            return Err(JournalError::SymlinkDetected(run_dir));
        }

        let manifest_path = run_dir.join("manifest.json");
        if is_symlink(&manifest_path)? {
            return Err(JournalError::SymlinkDetected(manifest_path));
        }

        let mut file = open_file_read_nofollow(&manifest_path)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(JournalError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "manifest path {} is not a regular file",
                    manifest_path.display()
                ),
            )));
        }
        if meta.len() > MAX_MANIFEST_READ_BYTES {
            return Err(JournalError::SizeExceeded {
                size: meta.len(),
                limit: MAX_MANIFEST_READ_BYTES,
            });
        }

        let mut buf =
            Vec::with_capacity(std::cmp::min(meta.len(), MAX_MANIFEST_READ_BYTES) as usize);
        Read::by_ref(&mut file)
            .take(MAX_MANIFEST_READ_BYTES + 1)
            .read_to_end(&mut buf)?;

        if buf.len() as u64 > MAX_MANIFEST_READ_BYTES {
            return Err(JournalError::SizeExceeded {
                size: buf.len() as u64,
                limit: MAX_MANIFEST_READ_BYTES,
            });
        }

        let manifest: RunManifest = serde_json::from_slice(&buf)
            .map_err(|e| JournalError::CorruptManifest(e.to_string()))?;

        if manifest.schema_version != 1 {
            return Err(JournalError::UnknownSchemaVersion(manifest.schema_version));
        }

        // Validate status enum
        if manifest.status.parse::<RunStatus>().is_err() {
            return Err(JournalError::CorruptManifest(format!(
                "invalid run status '{}'",
                manifest.status
            )));
        }

        if manifest.run_id != id {
            return Err(JournalError::IdMismatch {
                expected: id.to_string(),
                found: manifest.run_id,
            });
        }

        Ok(manifest)
    }

    /// Replaces whole diagnostic tail snapshot bounded to 64 KiB with pre-masking.
    pub fn write_diagnostics(&self, id: &str, tail: &str) -> Result<(), JournalError> {
        if !valid_run_id(id) {
            return Err(JournalError::InvalidId(id.to_string()));
        }

        verify_no_symlinks(&self.root)?;

        let run_dir = self.root.join(id);
        if is_symlink(&run_dir)? {
            return Err(JournalError::SymlinkDetected(run_dir));
        }

        #[cfg(unix)]
        {
            if !run_dir.exists() {
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(&run_dir)?;
            }
        }
        #[cfg(not(unix))]
        {
            if !run_dir.exists() {
                fs::create_dir_all(&run_dir)?;
            }
        }

        let diag_path = run_dir.join("diagnostics.txt");
        if is_symlink(&diag_path)? {
            return Err(JournalError::SymlinkDetected(diag_path));
        }

        // Mask secrets BEFORE truncation
        let masked = mask_sensitive_tokens(tail);

        // Bounded tail snapshot <= 64 KiB
        let bounded_tail = truncate_tail_utf8(&masked, MAX_DIAGNOSTICS_WRITE_BYTES);

        let tmp_name = format!(".diagnostics-{}.tmp", uuid::Uuid::new_v4().simple());
        let tmp_path = run_dir.join(tmp_name);

        let write_res = (|| -> io::Result<()> {
            let mut tmp_file = open_file_write_nofollow(&tmp_path, true)?;
            tmp_file.write_all(bounded_tail.as_bytes())?;
            tmp_file.sync_all()?;
            Ok(())
        })();

        if let Err(e) = write_res {
            let _ = fs::remove_file(&tmp_path);
            return Err(JournalError::Io(e));
        }

        if let Err(e) = fs::rename(&tmp_path, &diag_path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(JournalError::Io(e));
        }

        sync_dir(&run_dir)?;

        Ok(())
    }

    /// Loads diagnostic logs up to 64 KiB with UTF-8 safety and symlink rejection.
    pub fn load_diagnostics(&self, id: &str) -> Result<String, JournalError> {
        if !valid_run_id(id) {
            return Err(JournalError::InvalidId(id.to_string()));
        }

        verify_no_symlinks(&self.root)?;

        let run_dir = self.root.join(id);
        if is_symlink(&run_dir)? {
            return Err(JournalError::SymlinkDetected(run_dir));
        }

        let diag_path = run_dir.join("diagnostics.txt");
        if is_symlink(&diag_path)? {
            return Err(JournalError::SymlinkDetected(diag_path));
        }

        let mut file = open_file_read_nofollow(&diag_path)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(JournalError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "diagnostics path {} is not a regular file",
                    diag_path.display()
                ),
            )));
        }
        let len = meta.len();

        let mut bytes = Vec::new();
        if len > MAX_DIAGNOSTICS_WRITE_BYTES as u64 {
            let seek_pos = len - MAX_DIAGNOSTICS_WRITE_BYTES as u64;
            file.seek(SeekFrom::Start(seek_pos))?;
        }
        Read::by_ref(&mut file)
            .take((MAX_DIAGNOSTICS_WRITE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;

        if bytes.len() > MAX_DIAGNOSTICS_WRITE_BYTES {
            // Keep tail bounded
            let start = bytes.len() - MAX_DIAGNOSTICS_WRITE_BYTES;
            bytes = bytes[start..].to_vec();
        }

        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Prunes terminal runs based on age and size limits.
    /// Preserves active runs, corrupt runs, unknown schema runs, and symlinks.
    pub fn prune(&self, config: &JournalConfig, now_ms: u64) -> Result<PruneReport, JournalError> {
        verify_no_symlinks(&self.root)?;

        if !self.root.exists() {
            return Ok(PruneReport::default());
        }
        if !self.root.is_dir() {
            return Err(JournalError::Io(io::Error::new(
                io::ErrorKind::NotADirectory,
                "store root is not a directory",
            )));
        }

        struct TerminalCandidate {
            dir: PathBuf,
            completed_at_ms: u64,
            size_bytes: u64,
        }

        let mut candidates = Vec::new();
        let mut total_storage_bytes: u64 = 0;

        let entries = match fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) => return Err(JournalError::Io(e)),
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if is_symlink(&path).unwrap_or(true) {
                // Never inspect or follow symlinks
                continue;
            }
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !valid_run_id(name) {
                continue;
            }

            let dir_size = measure_dir_size(&path);
            total_storage_bytes = total_storage_bytes.saturating_add(dir_size);

            let manifest = match self.load(name) {
                Ok(m) => m,
                Err(_) => {
                    // Corrupt manifest, size exceeded, schema != 1, ID mismatch, or invalid status: preserve
                    continue;
                }
            };

            let status_res = manifest.status.parse::<RunStatus>();
            let is_terminal = match status_res {
                Ok(s) => s.is_terminal(),
                Err(_) => false,
            };

            if !is_terminal {
                // Active / running: strictly preserve
                continue;
            }

            let completed_at = manifest.completed_at_ms.unwrap_or(manifest.created_at_ms);

            candidates.push(TerminalCandidate {
                dir: path,
                completed_at_ms: completed_at,
                size_bytes: dir_size,
            });
        }

        // Sort terminal candidates oldest first (by completed_at_ms)
        candidates.sort_by_key(|c| c.completed_at_ms);

        let mut deleted_runs = 0;
        let mut reclaimed_bytes: u64 = 0;
        let mut remaining = Vec::new();

        // 1. Age-based pruning
        for cand in candidates {
            let age_ms = now_ms.saturating_sub(cand.completed_at_ms);
            if age_ms > config.max_age_ms {
                if safe_delete_run_dir(&cand.dir).is_ok() {
                    deleted_runs += 1;
                    reclaimed_bytes = reclaimed_bytes.saturating_add(cand.size_bytes);
                    total_storage_bytes = total_storage_bytes.saturating_sub(cand.size_bytes);
                }
            } else {
                remaining.push(cand);
            }
        }

        // 2. Size-based pruning (oldest first among remaining)
        for cand in remaining {
            if total_storage_bytes > config.max_total_bytes
                && safe_delete_run_dir(&cand.dir).is_ok()
            {
                deleted_runs += 1;
                reclaimed_bytes = reclaimed_bytes.saturating_add(cand.size_bytes);
                total_storage_bytes = total_storage_bytes.saturating_sub(cand.size_bytes);
            }
        }

        Ok(PruneReport {
            deleted_runs,
            reclaimed_bytes,
        })
    }
}

/// Safely measures directory size without following symlinks.
fn measure_dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(meta) = fs::symlink_metadata(&path)
                && !meta.file_type().is_symlink()
                && meta.is_file()
            {
                total += meta.len();
            }
        }
    }
    total
}

/// Safely deletes a run directory without following symlinks.
/// If `dir` is a symlink, returns an error.
/// If any entry inside `dir` is a symlink or unknown/nested directory,
/// returns an error to avoid touching external/foreign entries or partial deletion.
fn safe_delete_run_dir(dir: &Path) -> io::Result<()> {
    if is_symlink(dir)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "refusing to delete run directory that is a symlink: {}",
                dir.display()
            ),
        ));
    }

    let entries = fs::read_dir(dir)?;
    let mut to_delete = Vec::new();

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "symlink child detected inside run directory: {}",
                    path.display()
                ),
            ));
        }
        if meta.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "unexpected subdirectory detected inside run directory: {}",
                    path.display()
                ),
            ));
        }
        if meta.is_file() {
            let fname = entry.file_name();
            let name_lossy = fname.to_string_lossy();
            if name_lossy == "manifest.json"
                || name_lossy == "diagnostics.txt"
                || name_lossy.starts_with(".manifest-")
                || name_lossy.starts_with(".diagnostics-")
            {
                to_delete.push(path);
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "unknown file entry detected inside run directory: {}",
                        path.display()
                    ),
                ));
            }
        }
    }

    // Delete validated files
    for path in to_delete {
        fs::remove_file(&path)?;
    }

    fs::remove_dir(dir)
}
