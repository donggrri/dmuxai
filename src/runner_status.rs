//! Read-only, side-effect-free runner status probes and cache (SPEC-runner-status).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::custom_agent::CustomAgentSpec;
use crate::settings::DmuxSettings;

/// Default TTL for cached probe rows (FR-3305).
pub const STATUS_CACHE_TTL: Duration = Duration::from_secs(60);

/// Minimum interval between forced refreshes for the same runner binary (FR-3305).
pub const STATUS_REFRESH_MIN_INTERVAL: Duration = Duration::from_secs(3);

/// Default timeout per probe subprocess (FR-3300..FR-3302, NFR-3311).
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

const MAX_SANITIZED_LEN: usize = 200;
const MAX_VERSION_LEN: usize = 64;

/// Authentication status reported for a runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    Ok,
    NotLoggedIn,
    Unknown,
    Error,
}

/// One row in `dmuxai runners status --json` and `GET /api/runners/status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunnerStatusRow {
    pub id: String,
    pub label: String,
    pub probe: bool,
    pub enabled: bool,
    pub installed: Option<bool>,
    pub path: Option<String>,
    pub version: Option<String>,
    pub auth: AuthState,
    pub auth_method: Option<String>,
    pub account: Option<String>,
    pub hint: Option<String>,
    pub error: Option<String>,
    pub checked_at: String,
    pub duration_ms: u64,
    pub cached: bool,
}

/// Parsed result of `cursor-agent status --format json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorAuth {
    LoggedIn { account: Option<String> },
    NotLoggedIn,
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeKind {
    Cursor,
    Agy,
}

/// Extension point mapping canonical runner ids to their status probe (Section 1).
pub fn probe_kind(id: &str) -> Option<ProbeKind> {
    match crate::runner_id::canonical_runner_id(id).as_str() {
        crate::runner_id::CURSOR_RUNNER_ID => Some(ProbeKind::Cursor),
        "agy" => Some(ProbeKind::Agy),
        _ => None,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorStatusPayload {
    is_authenticated: Option<bool>,
    user_info: Option<serde_json::Value>,
}

/// Parses `cursor-agent status --format json`, reading ONLY `isAuthenticated`
/// and `userInfo.email` and discarding every other field (FR-3300).
pub fn parse_cursor_status_json(raw: &str) -> CursorAuth {
    let Ok(payload) = serde_json::from_str::<CursorStatusPayload>(raw) else {
        return CursorAuth::Error("unparseable cursor-agent status output".to_owned());
    };
    match payload.is_authenticated {
        Some(true) => {
            let account = payload
                .user_info
                .as_ref()
                .and_then(|u| u.get("email"))
                .and_then(|e| e.as_str())
                .and_then(mask_email);
            CursorAuth::LoggedIn { account }
        }
        Some(false) => CursorAuth::NotLoggedIn,
        None => CursorAuth::Error("missing isAuthenticated in status output".to_owned()),
    }
}

/// Classifies `agy models` exit status and output into an [`AuthState`] (FR-3302).
pub fn classify_agy_models(success: bool, output: &str) -> AuthState {
    if success {
        AuthState::Ok
    } else if output.to_ascii_lowercase().contains("sign in") {
        AuthState::NotLoggedIn
    } else {
        AuthState::Error
    }
}

/// Masks an email address as `<first-char>***@<domain>`, returning `None` if not an email (FR-3304).
pub fn mask_email(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let (local, domain) = trimmed.split_once('@')?;
    if local.is_empty()
        || domain.is_empty()
        || domain.contains('@')
        || !domain.contains('.')
        || domain.starts_with('.')
        || domain.ends_with('.')
        || domain.starts_with('-')
        || domain.ends_with('-')
    {
        return None;
    }
    let first = local.chars().next()?;
    Some(format!("{first}***@{domain}"))
}

fn is_email_local_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-')
}

fn is_email_domain_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-')
}

/// Strips email-shaped substrings, collapses whitespace, and caps length at 200 chars (FR-3304).
pub fn sanitize_text(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut keep = vec![true; chars.len()];

    for (idx, &ch) in chars.iter().enumerate() {
        if ch != '@' {
            continue;
        }
        let mut start = idx;
        while start > 0 && is_email_local_char(chars[start - 1]) {
            start -= 1;
        }
        if start == idx {
            continue;
        }
        let mut end = idx + 1;
        while end < chars.len() && is_email_domain_char(chars[end]) {
            end += 1;
        }
        while end > idx + 1 && matches!(chars[end - 1], '.' | '-') {
            end -= 1;
        }
        if end <= idx + 1 {
            continue;
        }
        let domain = &chars[idx + 1..end];
        if domain.first() == Some(&'.') || domain.first() == Some(&'-') {
            continue;
        }
        let Some(last_dot) = domain.iter().rposition(|&c| c == '.') else {
            continue;
        };
        if last_dot + 1 >= domain.len()
            || !domain[last_dot + 1..]
                .iter()
                .any(char::is_ascii_alphanumeric)
        {
            continue;
        }
        for flag in &mut keep[start..end] {
            *flag = false;
        }
    }

    let mut stripped = String::with_capacity(raw.len());
    for (i, &ch) in chars.iter().enumerate() {
        if keep[i] {
            stripped.push(if ch.is_control() { ' ' } else { ch });
        } else if i == 0 || keep[i - 1] {
            stripped.push(' ');
        }
    }

    let collapsed = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(MAX_SANITIZED_LEN).collect()
}

/// Formats a [`SystemTime`] as UTC RFC 3339 (`YYYY-MM-DDTHH:MM:SSZ`).
pub fn format_utc_rfc3339(time: SystemTime) -> String {
    let secs = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hour = rem / 3_600;
    let min = (rem % 3_600) / 60;
    let sec = rem % 60;

    // Civil-from-days conversion (proleptic Gregorian calendar).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y + i64::from(m <= 2);

    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}

type CacheKey = (String, String);

fn status_cache() -> &'static Mutex<HashMap<CacheKey, (Instant, RunnerStatusRow)>> {
    static CACHE: OnceLock<Mutex<HashMap<CacheKey, (Instant, RunnerStatusRow)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_lookup(
    key: &CacheKey,
    refresh: bool,
    now: Instant,
    enabled: bool,
    label: &str,
) -> Option<RunnerStatusRow> {
    let guard = status_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (checked_at, row) = guard.get(key)?;
    let age = now.saturating_duration_since(*checked_at);
    let hit = if refresh {
        age < STATUS_REFRESH_MIN_INTERVAL
    } else {
        age < STATUS_CACHE_TTL
    };
    if !hit {
        return None;
    }
    let mut cached_row = row.clone();
    cached_row.enabled = enabled;
    cached_row.label = label.to_owned();
    cached_row.cached = true;
    Some(cached_row)
}

fn cache_store(key: CacheKey, now: Instant, row: RunnerStatusRow) {
    let mut guard = status_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.insert(key, (now, row));
}

#[cfg(test)]
pub(crate) fn clear_status_cache() {
    let mut guard = status_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.clear();
}

fn resolve_executable(program: PathBuf) -> Option<PathBuf> {
    let raw = program.to_string_lossy();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = Path::new(trimmed);
    let found = if candidate.is_absolute() || trimmed.chars().any(std::path::is_separator) {
        crate::settings::is_executable_file(candidate).then(|| candidate.to_path_buf())
    } else {
        crate::settings::find_on_path(trimmed)
    }?;
    if found.is_absolute() {
        Some(found)
    } else {
        Some(
            std::env::current_dir()
                .map(|cwd| cwd.join(&found))
                .unwrap_or(found),
        )
    }
}

fn has_cursor_api_key_env() -> bool {
    ["CURSOR_API_KEY", "CURSOR_AUTH_TOKEN"]
        .iter()
        .any(|var| std::env::var_os(var).is_some_and(|v| !v.to_string_lossy().trim().is_empty()))
}

enum ProbeExecError {
    TimedOut,
    SpawnFailed,
}

async fn run_probe_command(
    program: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<std::process::Output, ProbeExecError> {
    let mut cmd = crate::process::headless_command(program.to_path_buf());
    cmd.args(args);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(_)) => Err(ProbeExecError::SpawnFailed),
        Err(_) => Err(ProbeExecError::TimedOut),
    }
}

fn parse_version_output(stdout: &str) -> Option<String> {
    let line = stdout.lines().map(str::trim).find(|l| !l.is_empty())?;
    let sanitized = sanitize_text(line);
    if sanitized.is_empty() {
        return None;
    }
    Some(sanitized.chars().take(MAX_VERSION_LEN).collect())
}

async fn probe_version(
    program: &Path,
    timeout: Duration,
) -> Result<Option<String>, ProbeExecError> {
    match run_probe_command(program, &["--version"], timeout).await {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            Ok(parse_version_output(&stdout))
        }
        Ok(_) => Ok(None),
        Err(ProbeExecError::TimedOut) => Err(ProbeExecError::TimedOut),
        Err(ProbeExecError::SpawnFailed) => Ok(None),
    }
}

fn error_row(
    id: String,
    label: String,
    enabled: bool,
    path: String,
    version: Option<String>,
    message: String,
    started: Instant,
) -> RunnerStatusRow {
    RunnerStatusRow {
        id,
        label,
        probe: true,
        enabled,
        installed: Some(true),
        path: Some(path),
        version,
        auth: AuthState::Error,
        auth_method: None,
        account: None,
        hint: None,
        error: Some(sanitize_text(&message)),
        checked_at: format_utc_rfc3339(SystemTime::now()),
        duration_ms: started.elapsed().as_millis() as u64,
        cached: false,
    }
}

async fn probe_cursor(
    id: String,
    label: String,
    enabled: bool,
    program: PathBuf,
    timeout: Duration,
) -> RunnerStatusRow {
    let started = Instant::now();
    let path_str = program.to_string_lossy().into_owned();

    let version = match probe_version(&program, timeout).await {
        Ok(v) => v,
        Err(ProbeExecError::TimedOut) => {
            return error_row(
                id,
                label,
                enabled,
                path_str,
                None,
                "timed out".to_owned(),
                started,
            );
        }
        Err(ProbeExecError::SpawnFailed) => None,
    };

    let status_out =
        match run_probe_command(&program, &["status", "--format", "json"], timeout).await {
            Ok(out) => out,
            Err(ProbeExecError::TimedOut) => {
                return error_row(
                    id,
                    label,
                    enabled,
                    path_str,
                    version,
                    "timed out".to_owned(),
                    started,
                );
            }
            Err(ProbeExecError::SpawnFailed) => {
                return error_row(
                    id,
                    label,
                    enabled,
                    path_str,
                    version,
                    "failed to run cursor-agent status".to_owned(),
                    started,
                );
            }
        };

    if !status_out.status.success() {
        let code = status_out.status.code().unwrap_or(-1);
        return error_row(
            id,
            label,
            enabled,
            path_str,
            version,
            format!("cursor-agent status failed (exit {code})"),
            started,
        );
    }

    let stdout = String::from_utf8_lossy(&status_out.stdout);
    match parse_cursor_status_json(&stdout) {
        CursorAuth::LoggedIn { account } => RunnerStatusRow {
            id,
            label,
            probe: true,
            enabled,
            installed: Some(true),
            path: Some(path_str),
            version,
            auth: AuthState::Ok,
            auth_method: Some("login".to_owned()),
            account,
            hint: None,
            error: None,
            checked_at: format_utc_rfc3339(SystemTime::now()),
            duration_ms: started.elapsed().as_millis() as u64,
            cached: false,
        },
        CursorAuth::Error(msg) => error_row(id, label, enabled, path_str, version, msg, started),
        CursorAuth::NotLoggedIn => {
            if !has_cursor_api_key_env() {
                return RunnerStatusRow {
                    id,
                    label,
                    probe: true,
                    enabled,
                    installed: Some(true),
                    path: Some(path_str),
                    version,
                    auth: AuthState::NotLoggedIn,
                    auth_method: None,
                    account: None,
                    hint: Some("Run `cursor-agent login`".to_owned()),
                    error: None,
                    checked_at: format_utc_rfc3339(SystemTime::now()),
                    duration_ms: started.elapsed().as_millis() as u64,
                    cached: false,
                };
            }

            match run_probe_command(&program, &["models"], timeout).await {
                Ok(out) if out.status.success() => RunnerStatusRow {
                    id,
                    label,
                    probe: true,
                    enabled,
                    installed: Some(true),
                    path: Some(path_str),
                    version,
                    auth: AuthState::Ok,
                    auth_method: Some("api_key_env".to_owned()),
                    account: None,
                    hint: None,
                    error: None,
                    checked_at: format_utc_rfc3339(SystemTime::now()),
                    duration_ms: started.elapsed().as_millis() as u64,
                    cached: false,
                },
                Ok(_) | Err(ProbeExecError::SpawnFailed) => RunnerStatusRow {
                    id,
                    label,
                    probe: true,
                    enabled,
                    installed: Some(true),
                    path: Some(path_str),
                    version,
                    auth: AuthState::NotLoggedIn,
                    auth_method: None,
                    account: None,
                    hint: Some("Run `cursor-agent login` or check CURSOR_API_KEY".to_owned()),
                    error: None,
                    checked_at: format_utc_rfc3339(SystemTime::now()),
                    duration_ms: started.elapsed().as_millis() as u64,
                    cached: false,
                },
                Err(ProbeExecError::TimedOut) => error_row(
                    id,
                    label,
                    enabled,
                    path_str,
                    version,
                    "timed out".to_owned(),
                    started,
                ),
            }
        }
    }
}

async fn probe_agy(
    id: String,
    label: String,
    enabled: bool,
    program: PathBuf,
    timeout: Duration,
) -> RunnerStatusRow {
    let started = Instant::now();
    let path_str = program.to_string_lossy().into_owned();

    let version = match probe_version(&program, timeout).await {
        Ok(v) => v,
        Err(ProbeExecError::TimedOut) => {
            return error_row(
                id,
                label,
                enabled,
                path_str,
                None,
                "timed out".to_owned(),
                started,
            );
        }
        Err(ProbeExecError::SpawnFailed) => None,
    };

    let models_out = match run_probe_command(&program, &["models"], timeout).await {
        Ok(out) => out,
        Err(ProbeExecError::TimedOut) => {
            return error_row(
                id,
                label,
                enabled,
                path_str,
                version,
                "timed out".to_owned(),
                started,
            );
        }
        Err(ProbeExecError::SpawnFailed) => {
            return error_row(
                id,
                label,
                enabled,
                path_str,
                version,
                "failed to run agy models".to_owned(),
                started,
            );
        }
    };

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&models_out.stdout),
        String::from_utf8_lossy(&models_out.stderr)
    );
    match classify_agy_models(models_out.status.success(), &combined) {
        AuthState::Ok => RunnerStatusRow {
            id,
            label,
            probe: true,
            enabled,
            installed: Some(true),
            path: Some(path_str),
            version,
            auth: AuthState::Ok,
            auth_method: Some("login".to_owned()),
            account: None,
            hint: None,
            error: None,
            checked_at: format_utc_rfc3339(SystemTime::now()),
            duration_ms: started.elapsed().as_millis() as u64,
            cached: false,
        },
        AuthState::NotLoggedIn => RunnerStatusRow {
            id,
            label,
            probe: true,
            enabled,
            installed: Some(true),
            path: Some(path_str),
            version,
            auth: AuthState::NotLoggedIn,
            auth_method: None,
            account: None,
            hint: Some("Run `agy` without arguments and sign in".to_owned()),
            error: None,
            checked_at: format_utc_rfc3339(SystemTime::now()),
            duration_ms: started.elapsed().as_millis() as u64,
            cached: false,
        },
        AuthState::Error | AuthState::Unknown => {
            let code = models_out.status.code().unwrap_or(-1);
            error_row(
                id,
                label,
                enabled,
                path_str,
                version,
                format!("agy models failed (exit {code})"),
                started,
            )
        }
    }
}

fn not_installed_hint(kind: ProbeKind) -> &'static str {
    match kind {
        ProbeKind::Cursor => "cursor-agent not found on PATH; install the Cursor CLI",
        ProbeKind::Agy => "agy not found on PATH; install the Antigravity CLI",
    }
}

async fn collect_one_runner(
    id: String,
    label: String,
    enabled: bool,
    spec: Option<CustomAgentSpec>,
    refresh: bool,
    timeout: Duration,
    now: Instant,
) -> RunnerStatusRow {
    let Some(kind) = probe_kind(&id) else {
        let installed = crate::agent_api::runner_installed(&id, spec.as_ref());
        return RunnerStatusRow {
            id,
            label,
            probe: false,
            enabled,
            installed,
            path: None,
            version: None,
            auth: AuthState::Unknown,
            auth_method: None,
            account: None,
            hint: Some("no status probe for this runner yet".to_owned()),
            error: None,
            checked_at: format_utc_rfc3339(SystemTime::now()),
            duration_ms: 0,
            cached: false,
        };
    };

    let raw_program = match kind {
        ProbeKind::Cursor => crate::agent_api::resolve_cursor_program_path(),
        ProbeKind::Agy => crate::agent_api::resolve_agy_program_path(),
    };
    let resolved = resolve_executable(raw_program);
    let path_key = resolved
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cache_key = (id.clone(), path_key);

    if let Some(cached_row) = cache_lookup(&cache_key, refresh, now, enabled, &label) {
        return cached_row;
    }

    let row = match resolved {
        None => RunnerStatusRow {
            id,
            label,
            probe: true,
            enabled,
            installed: Some(false),
            path: None,
            version: None,
            auth: AuthState::Unknown,
            auth_method: None,
            account: None,
            hint: Some(not_installed_hint(kind).to_owned()),
            error: None,
            checked_at: format_utc_rfc3339(SystemTime::now()),
            duration_ms: 0,
            cached: false,
        },
        Some(program) => match kind {
            ProbeKind::Cursor => probe_cursor(id, label, enabled, program, timeout).await,
            ProbeKind::Agy => probe_agy(id, label, enabled, program, timeout).await,
        },
    };

    cache_store(cache_key, now, row.clone());
    row
}

/// Collects status rows for all registered runners (or one target runner) concurrently (FR-3305).
pub async fn collect_status(
    settings: &DmuxSettings,
    agent: Option<&str>,
    refresh: bool,
) -> Vec<RunnerStatusRow> {
    collect_status_with(
        settings,
        agent,
        refresh,
        DEFAULT_PROBE_TIMEOUT,
        Instant::now(),
    )
    .await
}

/// Variant of [`collect_status`] allowing tests to inject probe timeout and current [`Instant`].
pub async fn collect_status_with(
    settings: &DmuxSettings,
    agent: Option<&str>,
    refresh: bool,
    timeout: Duration,
    now: Instant,
) -> Vec<RunnerStatusRow> {
    let target_canon = agent.map(crate::runner_id::canonical_runner_id);
    let selected: Vec<(String, String)> = crate::agent_api::runner_ids(settings)
        .into_iter()
        .filter(|(id, _)| {
            target_canon
                .as_ref()
                .is_none_or(|t| crate::runner_id::canonical_runner_id(id) == *t)
        })
        .collect();

    if selected.is_empty() {
        return Vec::new();
    }

    let mut join_set = tokio::task::JoinSet::new();
    for (idx, (id, label)) in selected.iter().enumerate() {
        let id = id.clone();
        let label = label.clone();
        let enabled = settings.runner_enabled(&id);
        let spec = settings
            .custom_agents
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(&id))
            .cloned();
        join_set.spawn(async move {
            let row = collect_one_runner(id, label, enabled, spec, refresh, timeout, now).await;
            (idx, row)
        });
    }

    let mut ordered: Vec<Option<RunnerStatusRow>> = vec![None; selected.len()];
    while let Some(joined) = join_set.join_next().await {
        if let Ok((idx, row)) = joined {
            ordered[idx] = Some(row);
        }
    }
    ordered.into_iter().flatten().collect()
}

/// Formats status rows into the `{ "ok": true, "runners": [...] }` JSON envelope (FR-3306).
pub fn status_json(rows: &[RunnerStatusRow]) -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "runners": rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EnvVarRestore {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvVarRestore {
        fn capture(keys: &[&'static str]) -> Self {
            Self {
                saved: keys.iter().map(|&k| (k, std::env::var_os(k))).collect(),
            }
        }
    }

    impl Drop for EnvVarRestore {
        fn drop(&mut self) {
            for (k, v) in self.saved.drain(..) {
                // SAFETY: callers hold crate::state_path::test_env_lock().
                unsafe {
                    match v {
                        Some(val) => std::env::set_var(k, val),
                        None => std::env::remove_var(k),
                    }
                }
            }
        }
    }

    fn write_executable_script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    // TC-3300 / FR-3300
    #[test]
    fn tc_3300_cursor_status_parse() {
        let authed = r#"{
            "isAuthenticated": true,
            "userInfo": {
                "email": "dev@example.com",
                "name": "Secret Person",
                "userId": "usr_12345"
            },
            "teamId": "team_secret",
            "extraField": "ignored"
        }"#;
        assert_eq!(
            parse_cursor_status_json(authed),
            CursorAuth::LoggedIn {
                account: Some("d***@example.com".to_owned()),
            }
        );

        let authed_bad_email =
            r#"{"isAuthenticated": true, "userInfo": {"email": "not-an-email"}}"#;
        assert_eq!(
            parse_cursor_status_json(authed_bad_email),
            CursorAuth::LoggedIn { account: None }
        );

        let unauthed = r#"{"isAuthenticated": false, "teamId": "team_secret"}"#;
        assert_eq!(parse_cursor_status_json(unauthed), CursorAuth::NotLoggedIn);

        assert!(matches!(
            parse_cursor_status_json("not json at all"),
            CursorAuth::Error(_)
        ));
        assert!(matches!(
            parse_cursor_status_json(r#"{"userInfo":{"email":"a@b.com"}}"#),
            CursorAuth::Error(_)
        ));
    }

    // TC-3301 / FR-3304
    #[test]
    fn tc_3301_account_masking_and_sanitize() {
        assert_eq!(mask_email("alice@d.com"), Some("a***@d.com".to_owned()));
        assert_eq!(mask_email("a@d.com"), Some("a***@d.com".to_owned()));
        assert_eq!(mask_email("not-an-email"), None);
        assert_eq!(mask_email("@d.com"), None);
        assert_eq!(mask_email("alice@"), None);
        assert_eq!(mask_email("alice@nodot"), None);
        assert_eq!(mask_email("alice @d.com"), None);

        let sanitized =
            sanitize_text("  failed for   alice@example.com\nand\tbob+tag@corp.co.uk!  ");
        assert!(!sanitized.contains("alice@example.com"));
        assert!(!sanitized.contains("bob+tag@corp.co.uk"));
        assert!(!sanitized.contains('\n'));
        assert!(!sanitized.contains('\t'));
        assert_eq!(sanitized, "failed for and !");

        let long = "x".repeat(500);
        let truncated = sanitize_text(&long);
        assert_eq!(truncated.chars().count(), 200);
    }

    // TC-3302 / FR-3302
    #[test]
    fn tc_3302_agy_models_classification() {
        assert_eq!(
            classify_agy_models(true, "gemini-4-hydrogen-high\tGemini 4 Hydrogen (High)\n"),
            AuthState::Ok
        );
        assert_eq!(
            classify_agy_models(false, "Please sign in first using `agy`"),
            AuthState::NotLoggedIn
        );
        assert_eq!(
            classify_agy_models(false, "SIGN IN required"),
            AuthState::NotLoggedIn
        );
        assert_eq!(
            classify_agy_models(false, "network timeout connecting to backend"),
            AuthState::Error
        );
    }

    // TC-3303 / FR-3300, FR-3301, FR-3302, FR-3303, NFR-3311
    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // env lock must serialize the whole probe test
    async fn tc_3303_probe_with_fake_binaries() {
        let _guard = crate::state_path::test_env_lock();
        let _restore = EnvVarRestore::capture(&[
            "DMUXAI_CURSOR_BIN",
            "DMUXAI_AGY_BIN",
            "CURSOR_API_KEY",
            "CURSOR_AUTH_TOKEN",
        ]);
        clear_status_cache();

        let dir = tempfile::tempdir().unwrap();
        let settings = DmuxSettings::default();
        let t0 = Instant::now();

        // 1. Logged-in cursor & logged-in agy (plus unprobed codex)
        let cursor_log_1 = dir.path().join("cursor1.log");
        let agy_log_1 = dir.path().join("agy1.log");
        let fake_cursor_1 = write_executable_script(
            dir.path(),
            "cursor-ok",
            &format!(
                r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "{log}"
case "$1" in
  --version)
    echo "2026.11.22-ok"
    ;;
  status)
    echo '{{"isAuthenticated":true,"userInfo":{{"email":"dev@example.com","name":"Full Name"}},"teamId":"secret-team"}}'
    ;;
  *)
    echo "unexpected args: $*" >&2
    exit 9
    ;;
esac
"#,
                log = cursor_log_1.display()
            ),
        );
        let fake_agy_1 = write_executable_script(
            dir.path(),
            "agy-ok",
            &format!(
                r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "{log}"
case "$1" in
  --version)
    echo "1.5.18"
    ;;
  models)
    echo "gemini-4-hydrogen-high"
    ;;
  *)
    echo "unexpected args: $*" >&2
    exit 9
    ;;
esac
"#,
                log = agy_log_1.display()
            ),
        );

        // SAFETY: serialized by test_env_lock().
        unsafe {
            std::env::set_var("DMUXAI_CURSOR_BIN", &fake_cursor_1);
            std::env::set_var("DMUXAI_AGY_BIN", &fake_agy_1);
            std::env::remove_var("CURSOR_API_KEY");
            std::env::remove_var("CURSOR_AUTH_TOKEN");
        }

        let rows = collect_status_with(&settings, None, false, Duration::from_secs(2), t0).await;
        let json = status_json(&rows);
        assert_eq!(json["ok"], true);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, crate::runner_id::CURSOR_RUNNER_ID);
        assert!(rows[0].probe);
        assert_eq!(rows[0].installed, Some(true));
        assert_eq!(rows[0].version.as_deref(), Some("2026.11.22-ok"));
        assert_eq!(rows[0].auth, AuthState::Ok);
        assert_eq!(rows[0].auth_method.as_deref(), Some("login"));
        assert_eq!(rows[0].account.as_deref(), Some("d***@example.com"));
        let serialized = serde_json::to_string(&rows[0]).unwrap();
        assert!(!serialized.contains("Full Name"));
        assert!(!serialized.contains("secret-team"));
        assert!(!serialized.contains("dev@example.com"));

        assert_eq!(rows[1].id, "agy");
        assert!(rows[1].probe);
        assert_eq!(rows[1].installed, Some(true));
        assert_eq!(rows[1].version.as_deref(), Some("1.5.18"));
        assert_eq!(rows[1].auth, AuthState::Ok);
        assert_eq!(rows[1].auth_method.as_deref(), Some("login"));
        assert_eq!(rows[1].account, None);

        assert_eq!(rows[2].id, "codex");
        assert!(!rows[2].probe);
        assert_eq!(rows[2].auth, AuthState::Unknown);
        assert_eq!(
            rows[2].hint.as_deref(),
            Some("no status probe for this runner yet")
        );

        let c_argv = std::fs::read_to_string(&cursor_log_1).unwrap();
        assert_eq!(
            c_argv.lines().collect::<Vec<_>>(),
            vec!["--version", "status --format json"]
        );
        let a_argv = std::fs::read_to_string(&agy_log_1).unwrap();
        assert_eq!(
            a_argv.lines().collect::<Vec<_>>(),
            vec!["--version", "models"]
        );

        // 2. Cursor logged out without API key (must NOT run `models`) and AGY sign-in prompt
        let cursor_log_2 = dir.path().join("cursor2.log");
        let fake_cursor_out = write_executable_script(
            dir.path(),
            "cursor-out",
            &format!(
                r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "{log}"
case "$1" in
  --version) echo "2026.11.22" ;;
  status) echo '{{"isAuthenticated":false}}' ;;
  models) echo "should not run"; exit 0 ;;
esac
"#,
                log = cursor_log_2.display()
            ),
        );
        let fake_agy_out = write_executable_script(
            dir.path(),
            "agy-out",
            r#"#!/usr/bin/env bash
case "$1" in
  --version) echo "1.5.18" ;;
  models) echo "Please sign in first (user@secret.org)" >&2; exit 1 ;;
esac
"#,
        );
        unsafe {
            std::env::set_var("DMUXAI_CURSOR_BIN", &fake_cursor_out);
            std::env::set_var("DMUXAI_AGY_BIN", &fake_agy_out);
        }
        let rows_out =
            collect_status_with(&settings, None, false, Duration::from_secs(2), t0).await;
        assert_eq!(rows_out[0].auth, AuthState::NotLoggedIn);
        assert_eq!(rows_out[0].auth_method, None);
        assert_eq!(
            rows_out[0].hint.as_deref(),
            Some("Run `cursor-agent login`")
        );
        let c2_argv = std::fs::read_to_string(&cursor_log_2).unwrap();
        assert_eq!(
            c2_argv.lines().collect::<Vec<_>>(),
            vec!["--version", "status --format json"]
        );

        assert_eq!(rows_out[1].auth, AuthState::NotLoggedIn);
        assert_eq!(
            rows_out[1].hint.as_deref(),
            Some("Run `agy` without arguments and sign in")
        );
        let agy_out_json = serde_json::to_string(&rows_out[1]).unwrap();
        assert!(!agy_out_json.contains("user@secret.org"));

        // 3. Cursor logged out + API key env + `models` succeeds (then fails)
        let cursor_log_3 = dir.path().join("cursor3.log");
        let fake_cursor_key_ok = write_executable_script(
            dir.path(),
            "cursor-key-ok",
            &format!(
                r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "{log}"
case "$1" in
  --version) echo "2026.11.22" ;;
  status) echo '{{"isAuthenticated":false}}' ;;
  models) echo "composer-2.5"; exit 0 ;;
esac
"#,
                log = cursor_log_3.display()
            ),
        );
        let secret = "super-secret-cursor-key-xyz987";
        unsafe {
            std::env::set_var("DMUXAI_CURSOR_BIN", &fake_cursor_key_ok);
            std::env::set_var("CURSOR_API_KEY", secret);
        }
        let rows_key_ok =
            collect_status_with(&settings, Some("cursor"), false, Duration::from_secs(2), t0).await;
        assert_eq!(rows_key_ok.len(), 1);
        assert_eq!(rows_key_ok[0].auth, AuthState::Ok);
        assert_eq!(rows_key_ok[0].auth_method.as_deref(), Some("api_key_env"));
        assert_eq!(rows_key_ok[0].account, None);
        assert!(
            !serde_json::to_string(&rows_key_ok[0])
                .unwrap()
                .contains(secret)
        );
        let c3_argv = std::fs::read_to_string(&cursor_log_3).unwrap();
        assert_eq!(
            c3_argv.lines().collect::<Vec<_>>(),
            vec!["--version", "status --format json", "models"]
        );

        // API key set, `models` fails -> not_logged_in with hint naming cursor-agent login & CURSOR_API_KEY
        let fake_cursor_key_fail = write_executable_script(
            dir.path(),
            "cursor-key-fail",
            r#"#!/usr/bin/env bash
case "$1" in
  --version) echo "2026.11.22" ;;
  status) echo '{"isAuthenticated":false}' ;;
  models) echo "invalid api key" >&2; exit 1 ;;
esac
"#,
        );
        unsafe {
            std::env::set_var("DMUXAI_CURSOR_BIN", &fake_cursor_key_fail);
        }
        let rows_key_fail = collect_status_with(
            &settings,
            Some("cursor-agent"),
            false,
            Duration::from_secs(2),
            t0,
        )
        .await;
        assert_eq!(rows_key_fail[0].auth, AuthState::NotLoggedIn);
        assert_eq!(rows_key_fail[0].auth_method, None);
        let hint = rows_key_fail[0].hint.as_deref().unwrap();
        assert!(hint.contains("cursor-agent login"), "hint: {hint}");
        assert!(hint.contains("CURSOR_API_KEY"), "hint: {hint}");
        assert!(
            !serde_json::to_string(&rows_key_fail[0])
                .unwrap()
                .contains(secret)
        );

        // 4. Missing binary -> installed: false, path: None, auth: unknown, install hint
        unsafe {
            std::env::set_var("DMUXAI_CURSOR_BIN", dir.path().join("missing-cursor"));
            std::env::set_var("DMUXAI_AGY_BIN", dir.path().join("missing-agy"));
            std::env::remove_var("CURSOR_API_KEY");
        }
        let rows_missing =
            collect_status_with(&settings, None, false, Duration::from_secs(2), t0).await;
        assert_eq!(rows_missing[0].installed, Some(false));
        assert_eq!(rows_missing[0].path, None);
        assert_eq!(rows_missing[0].auth, AuthState::Unknown);
        assert!(rows_missing[0].hint.as_deref().unwrap().contains("install"));
        assert_eq!(rows_missing[1].installed, Some(false));
        assert_eq!(rows_missing[1].path, None);
        assert_eq!(rows_missing[1].auth, AuthState::Unknown);
        assert!(rows_missing[1].hint.as_deref().unwrap().contains("install"));

        // 5. Hanging script -> error "timed out" quickly under injected short timeout
        let fake_cursor_hang = write_executable_script(
            dir.path(),
            "cursor-hang",
            r#"#!/usr/bin/env bash
sleep 5
"#,
        );
        unsafe {
            std::env::set_var("DMUXAI_CURSOR_BIN", &fake_cursor_hang);
        }
        let rows_hang = collect_status_with(
            &settings,
            Some("cursor-agent"),
            false,
            Duration::from_millis(120),
            t0,
        )
        .await;
        assert_eq!(rows_hang[0].auth, AuthState::Error);
        assert!(
            rows_hang[0]
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("timed out")
        );
    }

    // TC-3304 / FR-3305
    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // env lock must serialize the whole probe test
    async fn tc_3304_cache_ttl_and_refresh_limit() {
        let _guard = crate::state_path::test_env_lock();
        let _restore =
            EnvVarRestore::capture(&["DMUXAI_CURSOR_BIN", "CURSOR_API_KEY", "CURSOR_AUTH_TOKEN"]);
        clear_status_cache();

        let dir = tempfile::tempdir().unwrap();
        let count_file = dir.path().join("invocations.log");
        let fake_cursor = write_executable_script(
            dir.path(),
            "cursor-count",
            &format!(
                r#"#!/usr/bin/env bash
if [ "$1" = "status" ]; then
  echo "call" >> "{count}"
  echo '{{"isAuthenticated":true,"userInfo":{{"email":"c@example.com"}}}}'
elif [ "$1" = "--version" ]; then
  echo "2026.11.22"
fi
"#,
                count = count_file.display()
            ),
        );
        // SAFETY: serialized by test_env_lock().
        unsafe {
            std::env::set_var("DMUXAI_CURSOR_BIN", &fake_cursor);
            std::env::remove_var("CURSOR_API_KEY");
            std::env::remove_var("CURSOR_AUTH_TOKEN");
        }

        let mut settings = DmuxSettings::default();
        let t0 = Instant::now();

        // 1st probe: runs binary, cached = false
        let r1 = collect_status_with(
            &settings,
            Some("cursor-agent"),
            false,
            Duration::from_secs(2),
            t0,
        )
        .await;
        assert!(!r1[0].cached);
        assert!(r1[0].enabled);
        let calls = || {
            std::fs::read_to_string(&count_file)
                .map(|s| s.lines().count())
                .unwrap_or(0)
        };
        assert_eq!(calls(), 1);

        // 2nd probe within TTL (t0 + 1s): cached = true, no spawn; reflects updated enabled setting
        settings
            .runner_enabled
            .insert(crate::runner_id::CURSOR_RUNNER_ID.to_owned(), false);
        let r2 = collect_status_with(
            &settings,
            Some("cursor-agent"),
            false,
            Duration::from_secs(2),
            t0 + Duration::from_secs(1),
        )
        .await;
        assert!(r2[0].cached);
        assert!(!r2[0].enabled);
        assert_eq!(calls(), 1);

        // Refresh within 3s limit (t0 + 2s): still cached, no spawn
        let r3 = collect_status_with(
            &settings,
            Some("cursor-agent"),
            true,
            Duration::from_secs(2),
            t0 + Duration::from_secs(2),
        )
        .await;
        assert!(r3[0].cached);
        assert_eq!(calls(), 1);

        // Refresh after 3s limit (t0 + 4s): bypasses TTL and re-probes
        let t_refresh = t0 + Duration::from_secs(4);
        let r4 = collect_status_with(
            &settings,
            Some("cursor-agent"),
            true,
            Duration::from_secs(2),
            t_refresh,
        )
        .await;
        assert!(!r4[0].cached);
        assert!(!r4[0].enabled);
        assert_eq!(calls(), 2);

        // Non-refresh after 60s TTL (t_refresh + 61s): re-probes
        let r5 = collect_status_with(
            &settings,
            Some("cursor-agent"),
            false,
            Duration::from_secs(2),
            t_refresh + Duration::from_secs(61),
        )
        .await;
        assert!(!r5[0].cached);
        assert_eq!(calls(), 3);
    }
}
