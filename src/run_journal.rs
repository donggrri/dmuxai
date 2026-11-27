//! Run journal storage, metadata, masking, and retention (SPEC-run-journal FR-2560..FR-2566).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

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

/// Maximum progress copy file size on write (4 MiB).
pub const MAX_PROGRESS_COPY_BYTES: u64 = 4 * 1024 * 1024;

/// Maximum bounded progress read buffer size (1 MiB).
pub const MAX_PROGRESS_READ_BYTES: u64 = 1024 * 1024;

/// Execution origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOrigin {
    CliHeadless,
    WebDashboard,
    Tui,
    Mcp,
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
    AwaitingApproval,
    Approved,
}

impl RunStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, RunStatus::Active | RunStatus::Running)
    }

    pub fn is_terminal(&self) -> bool {
        !self.is_active()
    }

    /// Canonical manifest status string (inverse of `FromStr`).
    pub fn as_status_str(&self) -> &'static str {
        match self {
            RunStatus::Active => "active",
            RunStatus::Running => "running",
            RunStatus::Completed => "completed",
            RunStatus::Failed => "failed",
            RunStatus::Cancelled => "cancelled",
            RunStatus::AwaitingApproval => "awaiting_approval",
            RunStatus::Approved => "approved",
        }
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
            "awaiting_approval" => Ok(RunStatus::AwaitingApproval),
            "approved" => Ok(RunStatus::Approved),
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
    pub pid: Option<u32>,
    #[serde(default)]
    pub dmuxai_exit_code: Option<i32>,
    pub child_exit_code: Option<i32>,
    pub child_signal: Option<i32>,
    pub error_kind: Option<String>,
    pub error_summary: Option<String>,
    pub links: RunLinks,
    pub diagnostics: RunDiagnosticsRef,
    pub end_summary: Option<serde_json::Value>,
    #[serde(default)]
    pub workspace_root: Option<String>,
}

/// Normalizes legacy `"cursor"` `"agent"` / `"runner"` fields inside one JSONL
/// progress line on read (FR-3208) without touching the file on disk.
pub fn normalize_progress_line(line: &str) -> String {
    let Ok(mut val) = serde_json::from_str::<serde_json::Value>(line) else {
        return line.to_owned();
    };
    let Some(obj) = val.as_object_mut() else {
        return line.to_owned();
    };
    let mut changed = false;
    for key in ["agent", "runner"] {
        if let Some(serde_json::Value::String(raw)) = obj.get(key) {
            let canon = crate::runner_id::canonical_runner_id(raw);
            if canon != *raw {
                obj.insert(key.to_owned(), serde_json::Value::String(canon));
                changed = true;
            }
        }
    }
    if changed {
        serde_json::to_string(&val).unwrap_or_else(|_| line.to_owned())
    } else {
        line.to_owned()
    }
}

fn normalize_manifest_runners(manifest: &mut RunManifest) {
    if let Some(runner) = manifest.requested_runner.as_ref() {
        manifest.requested_runner = Some(crate::runner_id::canonical_runner_id(runner));
    }
    if let Some(runner) = manifest.resolved_runner.as_ref() {
        manifest.resolved_runner = Some(crate::runner_id::canonical_runner_id(runner));
    }
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

/// Conservative literal substrings derived from every masking regex (case-insensitive).
/// If none appear in the input, compiled regexes are not touched.
const MASK_LITERAL_TRIGGERS: &[&str] = &[
    "api_key",
    "access_token",
    "token",
    "secret",
    "password",
    "passwd",
    "auth",
    "bearer",
    "authorization",
    "key",
    "sig",
    "signature",
    "sk-",
    "ghp_",
    "gho_",
    "github_pat",
    "xox",
    "credential",
    "cookie",
    "session",
];

fn has_disallowed_control(input: &str) -> bool {
    input
        .chars()
        .any(|ch| ch.is_control() && ch != '\n' && ch != '\r' && ch != '\t')
}

fn contains_ascii_ci(haystack: &str, needle: &str) -> bool {
    let n = needle.len();
    if n == 0 || haystack.len() < n {
        return false;
    }
    let nb = needle.as_bytes();
    haystack
        .as_bytes()
        .windows(n)
        .any(|w| w.eq_ignore_ascii_case(nb))
}

fn may_need_masking(s: &str) -> bool {
    MASK_LITERAL_TRIGGERS
        .iter()
        .any(|needle| contains_ascii_ci(s, needle))
}

/// Strips ANSI escape sequences and non-standard control characters (retaining \n, \r, \t).
pub fn strip_ansi_and_control(input: &str) -> String {
    let needs_ansi = input.as_bytes().contains(&0x1b);
    if !needs_ansi && !has_disallowed_control(input) {
        return input.to_string();
    }
    let stripped = if needs_ansi {
        ANSI_RE.replace_all(input, "").into_owned()
    } else {
        input.to_owned()
    };
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
    if !input.as_bytes().contains(&0x1b)
        && !has_disallowed_control(input)
        && !may_need_masking(input)
    {
        return input.to_string();
    }
    let clean = strip_ansi_and_control(input);
    if !may_need_masking(&clean) {
        return clean;
    }

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
fn secure_dir_permissions_0700(path: &Path) -> Result<(), io::Error> {
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
fn secure_dir_permissions_0700(_path: &Path) -> Result<(), io::Error> {
    Ok(())
}

/// Helper to fsync a directory after file rename or modification without following symlinks.
#[cfg(unix)]
fn sync_dir(path: &Path) -> Result<(), io::Error> {
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
fn sync_dir(_path: &Path) -> Result<(), io::Error> {
    Ok(())
}

/// Resolves the data base directory for dmuxai runs:
/// `$DMUXAI_DATA_DIR` when set, otherwise `$XDG_DATA_HOME/dmuxai` or
/// `~/.local/share/dmuxai`.
pub fn data_base() -> io::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("DMUXAI_DATA_DIR")
        && !dir.is_empty()
    {
        return Ok(PathBuf::from(dir));
    }
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME")
        && !data_home.is_empty()
    {
        return Ok(PathBuf::from(data_home).join("dmuxai"));
    }
    fallback_data_base()
}

/// Resolves the XDG data home base directory for dmuxai runs:
/// `<data_base>/repos/<repo-key>/runs`.
pub fn repo_runs_dir(workspace_root: &Path) -> io::Result<PathBuf> {
    let base = data_base()?;
    let key = repo_key(workspace_root)?;
    Ok(base.join("repos").join(key).join("runs"))
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
fn open_file_write_nofollow(path: &Path, create_new: bool) -> Result<File, io::Error> {
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
fn open_file_write_nofollow(path: &Path, create_new: bool) -> Result<File, io::Error> {
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

/// Single-handle append writer for `<root>/<id>/progress.jsonl`.
/// Created once per run so each progress line is a single write without
/// per-line ancestor symlink walks or `sync_data`.
#[derive(Debug)]
pub struct ProgressCopy {
    file: File,
    written_bytes: u64,
}

impl ProgressCopy {
    /// Appends a masked progress line, silently skipping once the file would
    /// exceed `MAX_PROGRESS_COPY_BYTES`.
    pub fn append_line(&mut self, line: &str) -> Result<(), JournalError> {
        let masked = mask_sensitive_tokens(line);
        let mut entry = masked;
        entry.push('\n');
        let entry_bytes = entry.as_bytes();
        let next_len = self.written_bytes.saturating_add(entry_bytes.len() as u64);
        if next_len > MAX_PROGRESS_COPY_BYTES {
            return Ok(());
        }
        self.file.write_all(entry_bytes)?;
        self.written_bytes = next_len;
        Ok(())
    }
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
        self.save_internal(manifest, true)?;
        Ok(())
    }

    /// Saves a run manifest with the same validation, permissions, nofollow and
    /// atomic temp-file + rename contract as [`JournalStore::save`], but skips
    /// the durability syncs (`sync_all` on the temp file and the parent
    /// directory fsync). Used by headless run paths where per-run latency
    /// matters more than crash-exact durability (FR-2810).
    pub fn save_relaxed(&self, manifest: &RunManifest) -> Result<(), JournalError> {
        self.save_internal(manifest, false)?;
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
        self.save_internal(manifest, true)?;

        if let Some(cfg) = retention_config {
            let report = self.prune(cfg, now_ms)?;
            Ok(Some(report))
        } else {
            Ok(None)
        }
    }

    fn save_internal(&self, manifest: &RunManifest, durable: bool) -> Result<(), JournalError> {
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
            if durable {
                tmp_file.sync_all()?;
            }
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
        if durable {
            sync_dir(&run_dir)?;
        }

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

        let mut manifest = manifest;
        normalize_manifest_runners(&mut manifest);
        Ok(manifest)
    }

    /// FR-2816: supersedes earlier runs that were left `awaiting_approval` with
    /// `end_summary.plan_hash == plan_hash` once an approved plan with that hash
    /// actually starts executing under run `approved_by`. Each matching
    /// manifest becomes `approved` (terminal) with
    /// `end_summary.approved_by_run_id = approved_by` and keeps its
    /// `plan_hash`. The scan is bounded to the `max_scan` most recent run
    /// directories (valid run ids only, symlinks skipped) and never touches the
    /// current run (`approved_by`). Returns the number of superseded manifests.
    /// Load failures on individual runs are skipped; the first save error is
    /// returned so the caller can warn without failing the run.
    pub fn supersede_awaiting(
        &self,
        plan_hash: &str,
        approved_by: &str,
        max_scan: usize,
    ) -> Result<usize, JournalError> {
        if !valid_pending_plan_hash(plan_hash) {
            return Err(JournalError::InvalidId(plan_hash.to_string()));
        }
        if !valid_run_id(approved_by) {
            return Err(JournalError::InvalidId(approved_by.to_string()));
        }
        verify_no_symlinks(&self.root)?;
        if !self.root.is_dir() {
            return Ok(0);
        }

        let entries = fs::read_dir(&self.root)?;
        let mut candidates: Vec<(String, SystemTime)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if is_symlink(&path).unwrap_or(true) {
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
            let modified = fs::symlink_metadata(&path)
                .and_then(|meta| meta.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            candidates.push((name.to_owned(), modified));
        }

        // Newest first, deterministic tie-break on the id, bounded scan.
        candidates.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
        candidates.truncate(max_scan);

        let mut superseded = 0;
        for (name, _) in candidates.into_iter() {
            if name.as_str() == approved_by {
                continue;
            }
            let Ok(mut manifest) = self.load(&name) else {
                // Corrupt / oversized / foreign manifest: preserve like prune.
                continue;
            };
            if manifest.status != "awaiting_approval" {
                continue;
            }
            let hash_matches = manifest
                .end_summary
                .as_ref()
                .and_then(|summary| summary.get("plan_hash"))
                .and_then(|value| value.as_str())
                .is_some_and(|hash| hash == plan_hash);
            if !hash_matches {
                continue;
            }
            manifest.status = RunStatus::Approved.as_status_str().to_owned();
            let now_ms = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(manifest.created_at_ms);
            if manifest.completed_at_ms.is_none() {
                manifest.completed_at_ms = Some(now_ms);
            }
            if manifest.duration_ms.is_none() {
                manifest.duration_ms = Some(now_ms.saturating_sub(manifest.created_at_ms));
            }
            if let Some(summary) = manifest.end_summary.as_mut()
                && let Some(object) = summary.as_object_mut()
            {
                object.insert(
                    "approved_by_run_id".to_owned(),
                    serde_json::Value::String(approved_by.to_owned()),
                );
            }
            self.save_relaxed(&manifest)?;
            superseded += 1;
        }
        Ok(superseded)
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

    /// Opens `<root>/<id>/progress.jsonl` for appending masked progress lines
    /// without per-line `sync_data`. Validates the id, refuses symlinks,
    /// creates the file with 0600 on unix, and tracks the current size against
    /// `MAX_PROGRESS_COPY_BYTES`.
    pub fn open_progress_copy(&self, id: &str) -> Result<ProgressCopy, JournalError> {
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

        let progress_path = run_dir.join("progress.jsonl");
        if is_symlink(&progress_path)? {
            return Err(JournalError::SymlinkDetected(progress_path));
        }

        let mut opts = OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            opts.mode(0o600);
            opts.custom_flags(libc::O_NOFOLLOW);
        }

        let file = opts.open(&progress_path)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(JournalError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "progress path {} is not a regular file",
                    progress_path.display()
                ),
            )));
        }

        Ok(ProgressCopy {
            file,
            written_bytes: meta.len(),
        })
    }

    /// Appends a masked progress line to `<root>/<id>/progress.jsonl`.
    /// Validates the id, refuses symlinks, creates the file with 0600 on unix,
    /// and silently skips (Ok) once the file would exceed MAX_PROGRESS_COPY_BYTES.
    pub fn append_progress(&self, id: &str, line: &str) -> Result<(), JournalError> {
        self.open_progress_copy(id)?.append_line(line)
    }

    /// Loads the last `max_lines` from `<root>/<id>/progress.jsonl`,
    /// bounded to reading at most the last 1 MiB.
    pub fn load_progress_tail(
        &self,
        id: &str,
        max_lines: usize,
    ) -> Result<Vec<String>, JournalError> {
        if !valid_run_id(id) {
            return Err(JournalError::InvalidId(id.to_string()));
        }

        verify_no_symlinks(&self.root)?;

        let run_dir = self.root.join(id);
        if is_symlink(&run_dir)? {
            return Err(JournalError::SymlinkDetected(run_dir));
        }

        let progress_path = run_dir.join("progress.jsonl");
        if !progress_path.exists() {
            return Ok(Vec::new());
        }
        if is_symlink(&progress_path)? {
            return Err(JournalError::SymlinkDetected(progress_path));
        }

        let mut file = open_file_read_nofollow(&progress_path)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(JournalError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "progress path {} is not a regular file",
                    progress_path.display()
                ),
            )));
        }

        let len = meta.len();
        let bytes_to_read = std::cmp::min(len, MAX_PROGRESS_READ_BYTES);
        let seek_pos = len.saturating_sub(bytes_to_read);
        if seek_pos > 0 {
            file.seek(SeekFrom::Start(seek_pos))?;
        }

        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(bytes_to_read)
            .read_to_end(&mut bytes)?;

        let text = String::from_utf8_lossy(&bytes);
        let mut lines: Vec<String> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(normalize_progress_line)
            .collect();

        // If we sought into the middle of the file (seek_pos > 0), the first line might be partial; drop it
        if seek_pos > 0 && !lines.is_empty() {
            lines.remove(0);
        }

        if lines.len() > max_lines {
            let start = lines.len() - max_lines;
            lines = lines[start..].to_vec();
        }

        Ok(lines)
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

#[cfg(test)]
mod tests {
    use super::{RunDiagnosticsRef, RunLinks, RunManifest, RunStatus, mask_sensitive_tokens};

    // TC-2816 / FR-2816: `approved` is a terminal status; FromStr, serde and
    // the manifest string all agree.
    #[test]
    fn tc_2816_approved_status_roundtrip_and_terminal() {
        assert_eq!(
            "approved".parse::<RunStatus>().expect("parse approved"),
            RunStatus::Approved
        );
        assert_eq!(
            serde_json::to_value(RunStatus::Approved).expect("serialize approved"),
            serde_json::Value::String("approved".to_owned())
        );
        assert_eq!(
            serde_json::from_str::<RunStatus>("\"approved\"").expect("deserialize approved"),
            RunStatus::Approved
        );
        assert_eq!(RunStatus::Approved.as_status_str(), "approved");
        assert!(RunStatus::Approved.is_terminal());
        assert!(!RunStatus::Approved.is_active());
    }

    // TC-2802 / FR-2802, FR-2810: prefilter must not skip real secrets; benign progress lines pass through.
    #[test]
    fn tc_2810_mask_prefilter_keeps_masking() {
        let cases = [
            (r#"API_KEY="my secret key""#, true),
            (r#""api_key": "json_secret_pass""#, true),
            ("Authorization: Bearer sk-1234567890abcdef", true),
            ("https://example.com/api?token=secrettoken999&other=1", true),
            ("found sk-1234567890abcdefghij", true),
            ("leaked ghp_abcdefghijklmnopqrst", true),
        ];
        for (input, should_redact) in cases {
            let masked = mask_sensitive_tokens(input);
            assert!(
                masked.contains("[REDACTED]") == should_redact,
                "input {:?} -> {:?}",
                input,
                masked
            );
            if should_redact {
                assert!(
                    !masked.contains("json_secret_pass")
                        && !masked.contains("secrettoken999")
                        && !masked.contains("sk-1234567890abcdefghij")
                        && !masked.contains("ghp_abcdefghijklmnopqrst"),
                    "raw secret leaked in {:?}",
                    masked
                );
            }
        }

        let plain = r#"{"type":"progress","kind":"tool","summary":"read src/main.rs"}"#;
        assert_eq!(mask_sensitive_tokens(plain), plain);
    }

    // TC-3210 / FR-3208
    #[test]
    fn tc_3210_legacy_cursor_journal_runner_normalizes() {
        let dir = tempfile::tempdir().unwrap();
        let store = super::JournalStore::new(dir.path().to_path_buf());
        let legacy_id = "0123456789abcdef";
        let legacy_dir = dir.path().join(legacy_id);
        std::fs::create_dir_all(&legacy_dir).unwrap();
        let legacy_manifest = serde_json::json!({
            "schema_version": 1,
            "run_id": legacy_id,
            "origin": "cli_headless",
            "status": "completed",
            "created_at_ms": 1,
            "completed_at_ms": 2,
            "duration_ms": 1,
            "requested_runner": "cursor",
            "resolved_runner": "cursor",
            "links": {},
            "diagnostics": { "stderr_bytes": 0 }
        })
        .to_string();
        let manifest_path = legacy_dir.join("manifest.json");
        std::fs::write(&manifest_path, &legacy_manifest).unwrap();
        let legacy_progress = r#"{"type":"progress","ts_ms":1,"agent":"cursor","kind":"started","summary":"started"}"#;
        let progress_path = legacy_dir.join("progress.jsonl");
        std::fs::write(&progress_path, format!("{legacy_progress}\n")).unwrap();

        let loaded = store.load(legacy_id).expect("load legacy manifest");
        assert_eq!(
            loaded.requested_runner.as_deref(),
            Some(crate::runner_id::CURSOR_RUNNER_ID)
        );
        assert_eq!(
            loaded.resolved_runner.as_deref(),
            Some(crate::runner_id::CURSOR_RUNNER_ID)
        );
        let tail = store
            .load_progress_tail(legacy_id, 10)
            .expect("load legacy progress");
        assert_eq!(tail.len(), 1);
        let progress_val: serde_json::Value = serde_json::from_str(&tail[0]).unwrap();
        assert_eq!(progress_val["agent"], crate::runner_id::CURSOR_RUNNER_ID);

        // Files on disk are not rewritten.
        assert_eq!(
            std::fs::read_to_string(&manifest_path).unwrap(),
            legacy_manifest
        );
        assert_eq!(
            std::fs::read_to_string(&progress_path).unwrap().trim(),
            legacy_progress
        );

        let fresh_id = "fedcba9876543210";
        let fresh = RunManifest {
            schema_version: 1,
            run_id: fresh_id.to_owned(),
            origin: "cli_headless".to_owned(),
            status: "completed".to_owned(),
            created_at_ms: 1,
            completed_at_ms: Some(2),
            duration_ms: Some(1),
            requested_runner: Some(crate::runner_id::CURSOR_RUNNER_ID.to_owned()),
            resolved_runner: Some(crate::runner_id::CURSOR_RUNNER_ID.to_owned()),
            requested_model: None,
            resolved_model: None,
            dmuxai_version: None,
            build_commit: None,
            pid: None,
            dmuxai_exit_code: None,
            child_exit_code: None,
            child_signal: None,
            error_kind: None,
            error_summary: None,
            links: RunLinks::default(),
            diagnostics: RunDiagnosticsRef::default(),
            end_summary: None,
            workspace_root: None,
        };
        store.save_relaxed(&fresh).unwrap();
        let saved_raw = std::fs::read_to_string(dir.path().join(fresh_id).join("manifest.json"))
            .expect("saved manifest");
        let saved_json: serde_json::Value = serde_json::from_str(&saved_raw).unwrap();
        assert_eq!(
            saved_json["requested_runner"],
            crate::runner_id::CURSOR_RUNNER_ID
        );
        assert_eq!(
            saved_json["resolved_runner"],
            crate::runner_id::CURSOR_RUNNER_ID
        );
    }
}
