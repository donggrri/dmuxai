//! Decision board child runner for `board ask` and `board run`
//! (SPEC-decision-board FR-3132, FR-3133).

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::{
    BoardError, BoardItem, BoardStore, Lane, ListFilter, SayRequest, TEXT_MAX, Verdict,
    ensure_pending_plan_from_copy, normalize_plan_prompt, prune_old_plan_copies,
    validate_item_id_early, validate_message,
};
use crate::process::{apply_child_cwd, apply_child_env_policy};
use crate::run_journal::{mask_sensitive_tokens, valid_run_id};
use crate::settings::BoardSettings;
use crate::state_path::valid_pending_plan_hash;

pub const BOARD_CHILD_TIMEOUT_SECS: u64 = 900;
const STALE_LOCK_SECS: u64 = 960;
const THREAD_CONTEXT_LIMIT: usize = 20;

/// Environment variable carrying the one-time launch token to the detached
/// `board run --exec-prepared` child (SPEC-board-approval-hardening FR-3800).
pub const LAUNCH_TOKEN_ENV: &str = "DMUXAI_BOARD_LAUNCH_TOKEN";

/// Contents of `<board base>/runs/<id>.lock` written by [`run_item_prepare`]
/// (FR-3800). Only the SHA-256 of the launch token is stored; it is cleared
/// once the token is used (FR-3801).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RunLock {
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_sha256: Option<String>,
    #[serde(default)]
    pub plan_hash: String,
    #[serde(default)]
    pub actor: String,
    #[serde(default)]
    pub created_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepared_lane: Option<Lane>,
}

fn run_lock_path(board_base: &Path, id: &str) -> PathBuf {
    board_base.join("runs").join(format!("{id}.lock"))
}

/// Reads the run lock of item `id`; `None` when missing or not a JSON record.
pub fn read_run_lock(board_base: &Path, id: &str) -> Option<RunLock> {
    let text = fs::read_to_string(run_lock_path(board_base, id)).ok()?;
    serde_json::from_str(text.trim()).ok()
}

fn write_run_lock(board_base: &Path, id: &str, lock: &RunLock) -> Result<(), BoardError> {
    crate::board::store::ensure_dir(&board_base.join("runs"))?;
    let bytes = serde_json::to_vec(lock).map_err(|e| BoardError::Internal(e.to_string()))?;
    crate::board::store::atomic_write(&run_lock_path(board_base, id), &bytes)
}

/// 64 lowercase hex characters. UUID v4 draws from the OS CSPRNG (`getrandom`),
/// so two of them carry 244 random bits.
fn generate_launch_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn launch_token_sha256(token: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let digest = Sha256::digest(token.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest.iter() {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Reads the launch token from [`LAUNCH_TOKEN_ENV`] and removes it from this
/// process's environment so no later child can inherit it (FR-3800).
pub fn take_launch_token_from_env() -> Option<String> {
    let token = std::env::var(LAUNCH_TOKEN_ENV).ok();
    if token.is_some() {
        // SAFETY: called once on the CLI thread before any agent child is spawned.
        unsafe { std::env::remove_var(LAUNCH_TOKEN_ENV) };
    }
    token.filter(|t| !t.trim().is_empty())
}

fn lock_pid(contents: &str) -> Option<u32> {
    let trimmed = contents.trim();
    if let Some(pid) = trimmed.strip_prefix("pid=") {
        return pid.parse::<u32>().ok();
    }
    serde_json::from_str::<Value>(trimmed)
        .ok()?
        .get("pid")?
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
}

fn lock_is_stale(path: &Path) -> bool {
    #[cfg(unix)]
    if let Ok(contents) = fs::read_to_string(path)
        && let Some(pid) = lock_pid(&contents)
        && pid > 0
    {
        // SAFETY: kill(pid, 0) checks process existence without sending a signal.
        let res = unsafe { libc::kill(pid as i32, 0) };
        if res == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return true;
        }
    }
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age > Duration::from_secs(STALE_LOCK_SECS))
}

/// RAII lock file under `<board_base>/<subdir>/<id>.lock` preventing concurrent
/// asks or runs on the same board item.
pub struct BoardLockGuard {
    path: PathBuf,
}

impl BoardLockGuard {
    pub fn acquire(
        board_base: &Path,
        subdir: &str,
        id: &str,
        conflict_msg: &str,
    ) -> Result<Self, BoardError> {
        let dir = board_base.join(subdir);
        crate::board::store::ensure_dir(&dir)?;
        let path = dir.join(format!("{id}.lock"));
        for attempt in 0..2 {
            let mut opts = OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            match opts.open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "pid={}", std::process::id());
                    return Ok(Self { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if lock_is_stale(&path) && attempt == 0 {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    return Err(BoardError::Conflict(conflict_msg.to_owned()));
                }
                Err(e) => return Err(BoardError::Internal(e.to_string())),
            }
        }
        Err(BoardError::Conflict(conflict_msg.to_owned()))
    }

    pub fn is_active(board_base: &Path, subdir: &str, id: &str) -> bool {
        let path = board_base.join(subdir).join(format!("{id}.lock"));
        if !path.exists() {
            return false;
        }
        if lock_is_stale(&path) {
            let _ = fs::remove_file(&path);
            return false;
        }
        true
    }

    /// Takes over the run lock of a prepared run inside the detached
    /// `--exec-prepared` child: the presented launch token must hash to the
    /// stored value for this plan hash. The token is consumed and the lock
    /// now names this process (FR-3801, FR-3808).
    pub fn take_over(
        store: &BoardStore,
        id: &str,
        plan_hash: &str,
        launch_token: Option<&str>,
    ) -> Result<(Self, RunLock), BoardError> {
        let refused = || {
            BoardError::Forbidden(
                "board run --exec-prepared needs the launch token of a prepared run; start runs with `board run`"
                    .to_owned(),
            )
        };
        let token = launch_token
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or_else(refused)?;
        let presented = launch_token_sha256(token);
        store.with_store_lock(|| {
            let mut lock = read_run_lock(store.base(), id).ok_or_else(refused)?;
            let expected = lock.token_sha256.as_deref().ok_or_else(refused)?;
            if lock.plan_hash != plan_hash
                || !constant_time_eq(presented.as_bytes(), expected.as_bytes())
            {
                return Err(refused());
            }
            lock.token_sha256 = None;
            lock.pid = std::process::id();
            write_run_lock(store.base(), id, &lock)?;
            Ok((
                Self {
                    path: run_lock_path(store.base(), id),
                },
                lock,
            ))
        })
    }

    pub fn disarm(self) {
        std::mem::forget(self);
    }
}

impl Drop for BoardLockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Debug, Clone)]
pub struct ChildOutcome {
    pub exit_code: i32,
    pub run_id: Option<String>,
    pub plan_hash: Option<String>,
    pub text: Option<String>,
    pub summary: Option<String>,
}

fn resolve_child_bin() -> Result<PathBuf, BoardError> {
    if let Some(override_bin) = std::env::var_os("DMUXAI_BOARD_CHILD_BIN")
        && !override_bin.is_empty()
    {
        return Ok(PathBuf::from(override_bin));
    }
    std::env::current_exe().map_err(|e| BoardError::Internal(e.to_string()))
}

fn resolve_self_bin() -> Result<PathBuf, BoardError> {
    if let Some(override_bin) = std::env::var_os("DMUXAI_BOARD_SELF_BIN")
        && !override_bin.is_empty()
    {
        return Ok(PathBuf::from(override_bin));
    }
    std::env::current_exe().map_err(|e| BoardError::Internal(e.to_string()))
}

fn apply_state_dir_env(cmd: &mut Command, state_root: &Path) {
    let parent_has_state_dir = std::env::var_os("DMUXAI_STATE_DIR").is_some_and(|v| !v.is_empty());
    let differs_from_default = crate::state_path::state_base().ok().as_deref() != Some(state_root);
    if parent_has_state_dir || differs_from_default {
        cmd.env("DMUXAI_STATE_DIR", state_root);
    }
}

fn child_timeout() -> Duration {
    let secs = std::env::var("DMUXAI_BOARD_CHILD_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(BOARD_CHILD_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

fn parse_child_json(
    stdout: &str,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return (None, None, None, None);
    }
    let parsed = serde_json::from_str::<Value>(trimmed).ok().or_else(|| {
        trimmed
            .lines()
            .rev()
            .find_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
    });
    let Some(val) = parsed.filter(Value::is_object) else {
        return (
            None,
            None,
            Some(trimmed.to_owned()),
            Some(trimmed.to_owned()),
        );
    };
    let run_id = val
        .get("run_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty() && valid_run_id(s))
        .map(str::to_owned);
    let plan_hash = val
        .get("plan_hash")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let text = val
        .get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let summary = val
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            val.get("error")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
            val.get("text")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .map(str::to_owned);
    (run_id, plan_hash, text, summary)
}

fn kill_child_process_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pgid = child.id() as i32;
        if pgid > 0 {
            // SAFETY: child was spawned with process_group(0), so pgid == child.id().
            unsafe {
                libc::killpg(pgid, libc::SIGTERM);
            }
            let grace_deadline = Instant::now() + Duration::from_millis(200);
            while Instant::now() < grace_deadline {
                if let Ok(Some(_)) = child.try_wait() {
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

pub fn spawn_child_dmuxai(
    workspace: &Path,
    args: &[String],
    stdin_prompt: &str,
    timeout: Duration,
    state_root: &Path,
) -> Result<ChildOutcome, BoardError> {
    let bin = resolve_child_bin()?;
    let mut cmd = Command::new(&bin);
    cmd.args(args);
    apply_child_cwd(&mut cmd, workspace);
    apply_child_env_policy(&mut cmd);
    apply_state_dir_env(&mut cmd, state_root);
    let run_id = std::env::var(crate::term::policy::RUN_ID_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(crate::run_journal::generate_run_id);
    cmd.env(crate::term::policy::RUN_ID_ENV, run_id);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| BoardError::Internal(format!("failed to spawn child dmuxai: {e}")))?;

    if let Some(mut stdin) = child.stdin.take() {
        let prompt_bytes = stdin_prompt.as_bytes().to_vec();
        thread::spawn(move || {
            let _ = stdin.write_all(&prompt_bytes);
            let _ = stdin.flush();
        });
    }

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let (stdout_tx, stdout_rx) = mpsc::channel();
    let (stderr_tx, stderr_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = String::new();
        if let Some(mut out) = stdout_pipe {
            let _ = out.read_to_string(&mut buf);
        }
        let _ = stdout_tx.send(buf);
    });
    thread::spawn(move || {
        let mut buf = String::new();
        if let Some(mut err) = stderr_pipe {
            let _ = err.read_to_string(&mut buf);
        }
        let _ = stderr_tx.send(buf);
    });

    let deadline = Instant::now() + timeout;
    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().unwrap_or(1),
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_child_process_group(&mut child);
                    break 124;
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                kill_child_process_group(&mut child);
                return Err(BoardError::Internal(e.to_string()));
            }
        }
    };

    let pipe_wait = if exit_code == 124 {
        Duration::from_millis(200)
    } else {
        Duration::from_secs(2)
    };
    let stdout = stdout_rx.recv_timeout(pipe_wait).unwrap_or_default();
    let stderr = stderr_rx.recv_timeout(pipe_wait).unwrap_or_default();
    let (run_id, plan_hash, text, mut summary) = parse_child_json(&stdout);
    if summary.is_none() && !stderr.trim().is_empty() {
        summary = Some(stderr.trim().to_owned());
    }
    Ok(ChildOutcome {
        exit_code,
        run_id,
        plan_hash,
        text,
        summary,
    })
}

fn require_item_workspace(item: &BoardItem) -> Result<PathBuf, BoardError> {
    let Some(ws) = item.workspace.path.as_deref() else {
        return Err(BoardError::Usage("board item has no workspace".to_owned()));
    };
    let path = PathBuf::from(ws);
    if !path.is_dir() {
        return Err(BoardError::Usage(format!(
            "board item workspace '{}' does not exist",
            path.display()
        )));
    }
    Ok(path)
}

fn format_recent_thread(item: &BoardItem) -> String {
    let total = item.thread.len();
    let start = total.saturating_sub(THREAD_CONTEXT_LIMIT);
    let mut out = String::new();
    for msg in &item.thread[start..] {
        out.push_str(&format!("- [{}]: {}\n", msg.from, msg.text));
    }
    out
}

pub fn build_ask_prompt(item: &BoardItem, question: &str) -> String {
    let mut prompt = format!("Title: {}\n", item.title);
    if !item.summary.trim().is_empty() {
        prompt.push_str(&format!("Summary: {}\n", item.summary));
    }
    if let Some(plan) = &item.plan {
        prompt.push_str(&format!(
            "\nCurrent plan (revision {}, hash {}):\n",
            plan.revision, plan.hash
        ));
        for step in &plan.steps {
            prompt.push_str(&format!(
                "- {} [{}] ({}): {}\n",
                step.id, step.role, step.runner, step.job
            ));
        }
    }
    let thread_ctx = format_recent_thread(item);
    if !thread_ctx.is_empty() {
        prompt.push_str("\nRecent thread:\n");
        prompt.push_str(&thread_ctx);
    }
    prompt.push_str(&format!("\nQuestion:\n{question}\n"));
    prompt
}

fn build_revise_prompt(original_prompt: &str, item: &BoardItem, feedback: &str) -> String {
    let mut prompt = original_prompt.to_owned();
    prompt.push_str("\n\nOwner feedback:\n");
    let thread_ctx = format_recent_thread(item);
    if !thread_ctx.is_empty() {
        prompt.push_str(&thread_ctx);
        prompt.push('\n');
    }
    prompt.push_str(feedback);
    prompt.push('\n');
    prompt
}

pub struct AskParams {
    pub id: String,
    pub text: String,
    pub revise: bool,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub by: String,
    pub state_root: PathBuf,
}

pub struct PreparedAsk {
    pub item: BoardItem,
    pub workspace: PathBuf,
    pub guard: BoardLockGuard,
    pub params: AskParams,
}

pub fn ask_item_prepare(store: &BoardStore, params: AskParams) -> Result<PreparedAsk, BoardError> {
    validate_item_id_early(&params.id)?;
    validate_message(&params.text)?;
    let existing = store.get(&params.id)?;
    let workspace = require_item_workspace(&existing)?;
    if params.revise && existing.plan.is_none() {
        return Err(BoardError::Usage(
            "cannot --revise an item without a plan".to_owned(),
        ));
    }
    if params.revise
        && (existing.lane == Lane::InProgress
            || BoardLockGuard::is_active(store.base(), "runs", &params.id))
    {
        return Err(BoardError::Conflict(
            "cannot --revise while a run is in progress for this item".to_owned(),
        ));
    }
    let guard = BoardLockGuard::acquire(
        store.base(),
        "asks",
        &params.id,
        "an ask is already running for this item",
    )?;
    let item = store.say(
        &params.id,
        SayRequest {
            text: params.text.clone(),
            from: params.by.clone(),
        },
    )?;
    Ok(PreparedAsk {
        item,
        workspace,
        guard,
        params,
    })
}

pub fn ask_run_args(workspace: &Path, agent: Option<&str>, model: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "-C".to_owned(),
        workspace.to_string_lossy().into_owned(),
        "run".to_owned(),
        "--read-only".to_owned(),
        "--json".to_owned(),
    ];
    if let Some(a) = agent {
        args.push("--agent".to_owned());
        args.push(a.to_owned());
    }
    if let Some(m) = model {
        args.push("--model".to_owned());
        args.push(m.to_owned());
    }
    args.push("-".to_owned());
    args
}

pub fn ask_item_execute(
    store: &BoardStore,
    prepared: PreparedAsk,
) -> Result<BoardItem, BoardError> {
    let PreparedAsk {
        item,
        workspace,
        guard,
        params,
    } = prepared;
    let ws_str = workspace.to_string_lossy().into_owned();
    let timeout = child_timeout();

    if !params.revise {
        let board_cfg = BoardSettings::load_user_only();
        let (cfg_agent, cfg_model) = board_cfg.planner_agent_and_model();
        let agent = params.agent.or(cfg_agent);
        let model = params.model.or(cfg_model);
        let args = ask_run_args(&workspace, agent.as_deref(), model.as_deref());

        let prompt = build_ask_prompt(&item, &params.text);
        let outcome = spawn_child_dmuxai(&workspace, &args, &prompt, timeout, &params.state_root)?;
        let raw_answer = if outcome.exit_code == 0 {
            outcome
                .text
                .or(outcome.summary)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "planner ask returned empty response".to_owned())
        } else {
            let mut msg = format!("planner ask failed (exit {})", outcome.exit_code);
            if let Some(detail) = outcome
                .summary
                .or(outcome.text)
                .filter(|s| !s.trim().is_empty())
            {
                let clipped: String = detail.chars().take(2000).collect();
                msg.push_str(": ");
                msg.push_str(&clipped);
            }
            msg
        };
        let masked = mask_sensitive_tokens(&raw_answer);
        let answer: String = masked.chars().take(TEXT_MAX).collect();
        // FR-3132: release the ask lock before posting the answer so an immediate run is not refused
        drop(guard);
        return store.say(
            &params.id,
            SayRequest {
                text: answer,
                from: "planner".to_owned(),
            },
        );
    }

    let plan = item
        .plan
        .as_ref()
        .ok_or_else(|| BoardError::Usage("cannot --revise an item without a plan".to_owned()))?;
    let pending = ensure_pending_plan_from_copy(&params.state_root, store.base(), &plan.hash)?;
    let prompt = build_revise_prompt(&pending.prompt, &item, &params.text);
    let args = vec![
        "-C".to_owned(),
        ws_str,
        "run".to_owned(),
        "--graph".to_owned(),
        "--json".to_owned(),
        "-".to_owned(),
    ];
    let outcome = spawn_child_dmuxai(&workspace, &args, &prompt, timeout, &params.state_root)?;
    if outcome.exit_code == 7
        && let Some(new_hash) = outcome.plan_hash
    {
        let updated = store.attach_plan(&params.id, &new_hash, &params.state_root, "planner")?;
        let rev = updated.plan.as_ref().map(|p| p.revision).unwrap_or(0);
        let steps = updated.plan.as_ref().map(|p| p.steps.len()).unwrap_or(0);
        let msg = format!("Revised plan to revision {rev} ({new_hash}, {steps} steps).");
        // FR-3132: release the ask lock before posting the answer so an immediate run is not refused
        drop(guard);
        return store.say(
            &params.id,
            SayRequest {
                text: msg,
                from: "planner".to_owned(),
            },
        );
    }

    let mut msg = format!(
        "Plan revision failed (exit {}); no revision attached.",
        outcome.exit_code
    );
    if let Some(summary) = outcome.summary.as_deref().filter(|s| !s.trim().is_empty()) {
        let masked = mask_sensitive_tokens(summary);
        let clipped: String = masked.chars().take(2000).collect();
        msg.push(' ');
        msg.push_str(&clipped);
    }
    // FR-3132: release the ask lock before posting the answer so an immediate run is not refused
    drop(guard);
    store.say(
        &params.id,
        SayRequest {
            text: msg,
            from: "planner".to_owned(),
        },
    )
}

pub fn ask_item(store: &BoardStore, params: AskParams) -> Result<BoardItem, BoardError> {
    let prepared = ask_item_prepare(store, params)?;
    ask_item_execute(store, prepared)
}

pub struct RunParams {
    pub id: String,
    pub expected_plan_hash: Option<String>,
    pub by: String,
    pub state_root: PathBuf,
}

pub struct PreparedRun {
    pub item: BoardItem,
    pub workspace: PathBuf,
    pub plan_hash: String,
    pub prompt: String,
    pub guard: BoardLockGuard,
    pub by: String,
    pub state_root: PathBuf,
    launch_token: String,
}

impl PreparedRun {
    /// The raw one-time launch token whose SHA-256 is in the run lock. It is
    /// only ever handed to the detached child through [`LAUNCH_TOKEN_ENV`].
    pub fn launch_token(&self) -> &str {
        &self.launch_token
    }
}

pub fn run_item_prepare(store: &BoardStore, params: RunParams) -> Result<PreparedRun, BoardError> {
    validate_item_id_early(&params.id)?;
    if params.by == "planner" {
        return Err(BoardError::Forbidden("planner cannot run items".to_owned()));
    }
    if BoardLockGuard::is_active(store.base(), "asks", &params.id) {
        return Err(BoardError::Conflict(
            "an ask is currently in progress for this item".to_owned(),
        ));
    }
    let guard = BoardLockGuard::acquire(
        store.base(),
        "runs",
        &params.id,
        "a run is already in progress for this item",
    )?;
    let item = store.get(&params.id)?;
    if item.lane == Lane::InProgress {
        return Err(BoardError::Conflict(
            "item is already in_progress".to_owned(),
        ));
    }
    let plan = item
        .plan
        .as_ref()
        .ok_or_else(|| BoardError::Conflict("item has no plan".to_owned()))?;
    if let Some(expected) = params.expected_plan_hash.as_deref()
        && expected != plan.hash
    {
        return Err(BoardError::Conflict(
            "plan hash does not match the current plan".to_owned(),
        ));
    }
    match item.lane {
        Lane::Auto => {}
        Lane::Approved => {
            let decision = item
                .decision
                .as_ref()
                .ok_or_else(|| BoardError::Conflict("approved item has no decision".to_owned()))?;
            if decision.verdict != Verdict::Approve {
                return Err(BoardError::Conflict(
                    "approved item decision verdict must be approve".to_owned(),
                ));
            }
            if decision.by != "owner" {
                return Err(BoardError::Forbidden(
                    "bots may only run items approved by owner or in the auto lane".to_owned(),
                ));
            }
            if decision.plan_hash.as_deref() != Some(plan.hash.as_str()) {
                return Err(BoardError::Conflict(
                    "decision plan hash does not match the current plan".to_owned(),
                ));
            }
        }
        _ => {
            return Err(BoardError::Conflict(format!(
                "item in lane '{}' cannot be run (must be approved or auto)",
                item.lane.as_str()
            )));
        }
    }
    let workspace = require_item_workspace(&item)?;
    let pending = ensure_pending_plan_from_copy(&params.state_root, store.base(), &plan.hash)?;
    let plan_hash = plan.hash.clone();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let launch_token = generate_launch_token();
    write_run_lock(
        store.base(),
        &params.id,
        &RunLock {
            pid: std::process::id(),
            token_sha256: Some(launch_token_sha256(&launch_token)),
            plan_hash: plan_hash.clone(),
            actor: params.by.clone(),
            created_ms: now,
            prepared_lane: Some(item.lane),
        },
    )?;
    let in_progress_item =
        store.move_lane(&params.id, Lane::InProgress, &params.by, Some(item.version))?;

    if let Ok(all_items) = store.list(&ListFilter {
        include_all: true,
        ..Default::default()
    }) {
        prune_old_plan_copies(store.base(), &all_items, now);
    }

    Ok(PreparedRun {
        item: in_progress_item,
        workspace,
        plan_hash,
        prompt: pending.prompt,
        guard,
        by: params.by,
        state_root: params.state_root,
        launch_token,
    })
}

pub fn run_item_execute(
    store: &BoardStore,
    prepared: PreparedRun,
) -> Result<BoardItem, BoardError> {
    let PreparedRun {
        item,
        workspace,
        plan_hash,
        prompt,
        guard,
        by,
        state_root,
        launch_token: _,
    } = prepared;
    let _guard = guard;
    let normalized_prompt = normalize_plan_prompt(&prompt);
    let args = vec![
        "-C".to_owned(),
        workspace.to_string_lossy().into_owned(),
        "run".to_owned(),
        "--graph".to_owned(),
        "--approve-plan-id".to_owned(),
        plan_hash.clone(),
        "--json".to_owned(),
        "-".to_owned(),
    ];
    let outcome = match spawn_child_dmuxai(
        &workspace,
        &args,
        &normalized_prompt,
        child_timeout(),
        &state_root,
    ) {
        Ok(outcome) => outcome,
        Err(err) => {
            let _ =
                store.record_run_result(&item.id, None, 1, Some(&err.to_string()), &by, &plan_hash);
            return Err(err);
        }
    };
    store.record_run_result(
        &item.id,
        outcome.run_id.as_deref(),
        outcome.exit_code,
        outcome.summary.as_deref(),
        &by,
        &plan_hash,
    )
}

/// Point the run lock at the detached child while its token is still unused,
/// so a stale-lock check sees a dead child as stale (FR-3808).
fn record_launched_pid(store: &BoardStore, id: &str, pid: u32) {
    let _ = store.with_store_lock(|| {
        if let Some(mut lock) = read_run_lock(store.base(), id)
            && lock.token_sha256.is_some()
        {
            lock.pid = pid;
            write_run_lock(store.base(), id, &lock)?;
        }
        Ok(())
    });
}

/// Wait for the detached child on a background thread so a long-lived parent
/// (MCP server, dashboard) never keeps a zombie (FR-3808).
fn reap_detached_child(mut child: std::process::Child) {
    let _ = thread::Builder::new()
        .name("board-run-reaper".to_owned())
        .spawn(move || {
            let _ = child.wait();
        });
}

/// Spawn a detached OS subprocess (`setsid` + null stdio) running the hidden
/// `board run <id> --exec-prepared --plan-hash H --by ACTOR` command, and
/// return immediately with the `in_progress` item (FR-3133). The launch token
/// travels only in [`LAUNCH_TOKEN_ENV`]; `--by` is informational (FR-3800,
/// FR-3801).
pub fn launch_detached_board_run(
    store: &BoardStore,
    prepared: PreparedRun,
) -> Result<BoardItem, BoardError> {
    let bin = match resolve_self_bin() {
        Ok(bin) => bin,
        Err(err) => {
            let _ = store.record_run_result(
                &prepared.item.id,
                None,
                1,
                Some(&err.to_string()),
                &prepared.by,
                &prepared.plan_hash,
            );
            return Err(err);
        }
    };
    let mut cmd = Command::new(&bin);
    cmd.arg("board")
        .arg("run")
        .arg(&prepared.item.id)
        .arg("--exec-prepared")
        .arg("--plan-hash")
        .arg(&prepared.plan_hash)
        .arg("--by")
        .arg(&prepared.by)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    apply_state_dir_env(&mut cmd, &prepared.state_root);
    let run_id = std::env::var(crate::term::policy::RUN_ID_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(crate::run_journal::generate_run_id);
    cmd.env(crate::term::policy::RUN_ID_ENV, run_id);
    cmd.env(LAUNCH_TOKEN_ENV, &prepared.launch_token);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid() is async-signal-safe and detaches into a new session.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    match cmd.spawn() {
        Ok(child) => {
            record_launched_pid(store, &prepared.item.id, child.id());
            reap_detached_child(child);
            prepared.guard.disarm();
            Ok(prepared.item)
        }
        Err(e) => {
            let msg = format!("failed to spawn detached board run: {e}");
            let _ = store.record_run_result(
                &prepared.item.id,
                None,
                1,
                Some(&msg),
                &prepared.by,
                &prepared.plan_hash,
            );
            Err(BoardError::Internal(msg))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AutoRunOutcome {
    pub started: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl AutoRunOutcome {
    pub fn not_started() -> Self {
        Self {
            started: false,
            reason: None,
        }
    }
}

/// Start a detached `board run` immediately after an owner `approve` decision
/// when the item has a plan, `[board] auto_run_on_approve` is enabled, and
/// launching is allowed (SPEC-board-approve-autorun FR-3232..FR-3235, NFR-3230).
/// Never writes a `decision` or `checked` record and never moves into
/// `approved` or `rejected`.
pub fn auto_run_after_owner_approve(
    store: &BoardStore,
    item: &BoardItem,
    state_root: &Path,
    launch_allowed: bool,
) -> AutoRunOutcome {
    let Some(decision) = item.decision.as_ref() else {
        return AutoRunOutcome::not_started();
    };
    if decision.verdict != Verdict::Approve || decision.by != "owner" {
        return AutoRunOutcome::not_started();
    }
    if item.plan.is_none() {
        return AutoRunOutcome::not_started();
    }
    let settings = BoardSettings::load_user_only();
    if !settings.auto_run_on_approve {
        return AutoRunOutcome {
            started: false,
            reason: Some("auto_run_on_approve is false".to_owned()),
        };
    }
    if !launch_allowed {
        let reason = "view-only dashboard".to_owned();
        let _ = store.append_run_message(&item.id, &format!("auto-run skipped: {reason}"));
        return AutoRunOutcome {
            started: false,
            reason: Some(reason),
        };
    }
    let prepared = match run_item_prepare(
        store,
        RunParams {
            id: item.id.clone(),
            expected_plan_hash: decision.plan_hash.clone(),
            by: "owner".to_owned(),
            state_root: state_root.to_path_buf(),
        },
    ) {
        Ok(prepared) => prepared,
        Err(err) => {
            let reason = mask_sensitive_tokens(&err.to_string());
            let _ = store.append_run_message(&item.id, &format!("auto-run skipped: {reason}"));
            return AutoRunOutcome {
                started: false,
                reason: Some(reason),
            };
        }
    };
    match launch_detached_board_run(store, prepared) {
        Ok(_) => AutoRunOutcome {
            started: true,
            reason: None,
        },
        Err(err) => {
            let reason = mask_sensitive_tokens(&err.to_string());
            let _ = store.append_run_message(&item.id, &format!("auto-run skipped: {reason}"));
            AutoRunOutcome {
                started: false,
                reason: Some(reason),
            }
        }
    }
}

/// FR-3802: a prepared run executes only for an owner approval of exactly this
/// plan, or for an item that was in the `auto` lane when it was prepared.
fn ensure_prepared_run_allowed(
    item: &BoardItem,
    lock: &RunLock,
    plan_hash: &str,
) -> Result<(), BoardError> {
    if lock.prepared_lane == Some(Lane::Auto) {
        return Ok(());
    }
    let approved_by_owner = item
        .decision
        .as_ref()
        .is_some_and(|d| d.verdict == Verdict::Approve && d.by == "owner");
    if !approved_by_owner {
        return Err(BoardError::Forbidden(
            "prepared runs need an owner approval or an auto lane item".to_owned(),
        ));
    }
    if item.decision.as_ref().and_then(|d| d.plan_hash.as_deref()) != Some(plan_hash) {
        return Err(BoardError::Conflict(
            "decision plan hash does not match the prepared plan".to_owned(),
        ));
    }
    Ok(())
}

/// Execute a previously prepared `in_progress` board run inside the detached
/// `--exec-prepared` subprocess and record its result (FR-3133). The caller
/// presents the launch token; the actor comes from the run lock (FR-3801).
pub fn run_item_execute_prepared(
    store: &BoardStore,
    id: &str,
    plan_hash: &str,
    launch_token: Option<&str>,
    state_root: &Path,
) -> Result<BoardItem, BoardError> {
    validate_item_id_early(id)?;
    if !valid_pending_plan_hash(plan_hash) {
        return Err(BoardError::Usage(
            "plan hash must be 16 lowercase hex characters".to_owned(),
        ));
    }
    let (guard, lock) = BoardLockGuard::take_over(store, id, plan_hash, launch_token)?;
    let by = lock.actor.as_str();
    if by == "planner" || (by != "owner" && !by.starts_with("bot:")) {
        return Err(BoardError::Forbidden(
            "prepared run has no valid actor".to_owned(),
        ));
    }
    let item = store.get(id)?;
    if item.lane != Lane::InProgress {
        return Err(BoardError::Conflict(format!(
            "item in lane '{}' is not in_progress",
            item.lane.as_str()
        )));
    }
    let current_hash = item.plan.as_ref().map(|p| p.hash.as_str());
    if current_hash != Some(plan_hash) {
        let msg = "plan hash does not match the current plan";
        let _ = store.record_run_result(id, None, 1, Some(msg), by, plan_hash);
        return Err(BoardError::Conflict(msg.to_owned()));
    }
    ensure_prepared_run_allowed(&item, &lock, plan_hash)?;
    let workspace = match require_item_workspace(&item) {
        Ok(ws) => ws,
        Err(err) => {
            let _ = store.record_run_result(id, None, 1, Some(&err.to_string()), by, plan_hash);
            return Err(err);
        }
    };
    let pending = match ensure_pending_plan_from_copy(state_root, store.base(), plan_hash) {
        Ok(p) => p,
        Err(err) => {
            let _ = store.record_run_result(id, None, 1, Some(&err.to_string()), by, plan_hash);
            return Err(err);
        }
    };
    run_item_execute(
        store,
        PreparedRun {
            item,
            workspace,
            plan_hash: plan_hash.to_owned(),
            prompt: pending.prompt,
            guard,
            by: by.to_owned(),
            state_root: state_root.to_path_buf(),
            launch_token: String::new(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::plan::{edit_plan_from_json, pending_plan_path};
    use crate::board::store::{AddItemRequest, BoardStore, ensure_dir};
    use crate::board::{DecideRequest, Kind, Verdict};
    use tempfile::tempdir;

    fn write_fake_runner(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    fn sample_plan_json() -> String {
        r#"{"tasks":[{"id":"task-1","title":"do it","done_check":"done","size":"small","depends_on":[]}],"steps":[{"id":"step-1","role":"implement","job":"edit","runner":"cursor","depends_on":[],"task":"task-1"}]}"#.to_owned()
    }

    #[test]
    fn tc_3132_board_ask_answer_and_revise_with_fake_runner() {
        let _guard = crate::state_path::test_env_lock();
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        let board_base = state.join("board");
        let workspace = dir.path().join("ws");
        fs::create_dir_all(&workspace).unwrap();
        ensure_dir(&board_base).unwrap();

        let store = BoardStore::open(board_base.clone()).unwrap();
        let item = store
            .add(AddItemRequest {
                title: "Add feature".to_owned(),
                summary: "Need details".to_owned(),
                kind: Kind::Feature,
                grade: None,
                lane: None,
                source_bot: Some("bot".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(workspace.clone()),
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:test".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        let (_, hash1) =
            edit_plan_from_json(&workspace, &state, "orig prompt", &sample_plan_json(), None)
                .unwrap();
        let item = store
            .attach_plan(&item.id, &hash1, &state, "bot:test")
            .unwrap();
        assert_eq!(item.plan.as_ref().unwrap().revision, 1);

        // Pre-create a second pending plan so revise can return its hash on exit 7.
        let (_, hash2) = edit_plan_from_json(
            &workspace,
            &state,
            "orig prompt",
            &sample_plan_json().replace("\"edit\"", "\"edit-v2\""),
            None,
        )
        .unwrap();

        let token_leak_check = dir.path().join("env_check.txt");
        let argv_file = dir.path().join("ask_argv.txt");
        let cwd_file = dir.path().join("ask_cwd.txt");
        let stdin_file = dir.path().join("ask_stdin.txt");
        let fake_bin = write_fake_runner(
            dir.path(),
            "fake-dmuxai.sh",
            &format!(
                r#"#!/bin/sh
echo "DMUXAI_WEB_TOKEN=$DMUXAI_WEB_TOKEN" > "{}"
echo "DMUXAI_STATE_DIR=$DMUXAI_STATE_DIR" >> "{}"
printf '%s\n' "$@" > "{}"
pwd > "{}"
cat > "{}"
case "$*" in
  *"--read-only"*)
    printf '{{"ok":true,"exit_code":0,"error":null,"text":"Answer with Bearer sk-secret-999"}}\n'
    exit 0
    ;;
  *"--graph"*)
    printf '{{"ok":false,"exit_code":7,"plan_hash":"{}"}}\n'
    exit 7
    ;;
esac
exit 1
"#,
                token_leak_check.display(),
                token_leak_check.display(),
                argv_file.display(),
                cwd_file.display(),
                stdin_file.display(),
                hash2
            ),
        );

        unsafe {
            std::env::set_var("DMUXAI_BOARD_CHILD_BIN", &fake_bin);
            std::env::set_var("DMUXAI_WEB_TOKEN", "top-secret-token");
        }
        let answered = ask_item(
            &store,
            AskParams {
                id: item.id.clone(),
                text: "Why step 1?".to_owned(),
                revise: false,
                agent: None,
                model: None,
                by: "owner".to_owned(),
                state_root: state.clone(),
            },
        )
        .unwrap();
        assert_eq!(answered.thread.len(), 2);
        assert_eq!(answered.thread[0].from, "owner");
        assert_eq!(answered.thread[1].from, "planner");
        assert!(!answered.thread[1].text.contains("sk-secret-999"));
        assert!(answered.thread[1].text.contains("Answer with"));
        let env_txt = fs::read_to_string(&token_leak_check).unwrap();
        assert!(
            env_txt.contains("DMUXAI_WEB_TOKEN=\n") || env_txt.starts_with("DMUXAI_WEB_TOKEN=\n")
        );
        assert!(env_txt.contains(&format!("DMUXAI_STATE_DIR={}", state.display())));
        let argv_txt = fs::read_to_string(&argv_file).unwrap();
        assert!(argv_txt.contains("--read-only"));
        assert!(argv_txt.contains("--json"));
        let canonical_ws = fs::canonicalize(&workspace).unwrap();
        assert_eq!(
            fs::canonicalize(fs::read_to_string(&cwd_file).unwrap().trim()).unwrap(),
            canonical_ws
        );
        let stdin_txt = fs::read_to_string(&stdin_file).unwrap();
        assert!(stdin_txt.contains("Why step 1?"));

        // Non-zero exit on non-revise ask must report failure even when stderr/text is present.
        let fail_bin = write_fake_runner(
            dir.path(),
            "fake-ask-fail.sh",
            "#!/bin/sh\ncat > /dev/null\necho 'config warning' >&2\nprintf '{\"ok\":false,\"exit_code\":2,\"error\":null,\"text\":\"partial\"}\\n'\nexit 2\n",
        );
        unsafe { std::env::set_var("DMUXAI_BOARD_CHILD_BIN", &fail_bin) };
        let failed_ask = ask_item(
            &store,
            AskParams {
                id: item.id.clone(),
                text: "Will it fail?".to_owned(),
                revise: false,
                agent: None,
                model: None,
                by: "owner".to_owned(),
                state_root: state.clone(),
            },
        )
        .unwrap();
        assert!(
            failed_ask
                .thread
                .last()
                .unwrap()
                .text
                .starts_with("planner ask failed (exit 2)"),
            "got: {}",
            failed_ask.thread.last().unwrap().text
        );

        // 124 timeout kills process group (including child sleep) and does not hang.
        let timeout_bin = write_fake_runner(
            dir.path(),
            "fake-ask-timeout.sh",
            "#!/bin/sh\ncat > /dev/null\nsleep 10 &\nwait\n",
        );
        unsafe {
            std::env::set_var("DMUXAI_BOARD_CHILD_BIN", &timeout_bin);
            std::env::set_var("DMUXAI_BOARD_CHILD_TIMEOUT_SECS", "1");
        }
        let timed_out = ask_item(
            &store,
            AskParams {
                id: item.id.clone(),
                text: "Slow ask".to_owned(),
                revise: false,
                agent: None,
                model: None,
                by: "owner".to_owned(),
                state_root: state.clone(),
            },
        )
        .unwrap();
        unsafe { std::env::remove_var("DMUXAI_BOARD_CHILD_TIMEOUT_SECS") };
        assert!(
            timed_out
                .thread
                .last()
                .unwrap()
                .text
                .contains("planner ask failed (exit 124)"),
            "got: {}",
            timed_out.thread.last().unwrap().text
        );

        // --revise is blocked while a live runs/<id>.lock exists, but proceeds if lock PID is dead.
        {
            let _run_guard =
                BoardLockGuard::acquire(&board_base, "runs", &item.id, "busy").unwrap();
            let err = ask_item_prepare(
                &store,
                AskParams {
                    id: item.id.clone(),
                    text: "Revise mid-run".to_owned(),
                    revise: true,
                    agent: None,
                    model: None,
                    by: "owner".to_owned(),
                    state_root: state.clone(),
                },
            )
            .err()
            .expect("revise should be blocked while run lock is active");
            assert!(matches!(err, BoardError::Conflict(_)));
        }
        let stale_dir = board_base.join("runs");
        ensure_dir(&stale_dir).unwrap();
        fs::write(stale_dir.join(format!("{}.lock", item.id)), "pid=999999\n").unwrap();

        unsafe { std::env::set_var("DMUXAI_BOARD_CHILD_BIN", &fake_bin) };
        let revised = ask_item(
            &store,
            AskParams {
                id: item.id.clone(),
                text: "Update step 1".to_owned(),
                revise: true,
                agent: None,
                model: None,
                by: "owner".to_owned(),
                state_root: state.clone(),
            },
        )
        .unwrap();
        unsafe {
            std::env::remove_var("DMUXAI_BOARD_CHILD_BIN");
            std::env::remove_var("DMUXAI_WEB_TOKEN");
        }
        assert_eq!(revised.plan.as_ref().unwrap().hash, hash2);
        assert_eq!(revised.plan.as_ref().unwrap().revision, 2);
        assert_eq!(revised.thread.last().unwrap().from, "planner");
        assert!(revised.thread.last().unwrap().text.contains(&hash2));
        let revise_stdin = fs::read_to_string(&stdin_file).unwrap();
        assert!(revise_stdin.contains("orig prompt"));
        assert!(revise_stdin.contains("Owner feedback:"));

        // parse_child_json handles error:null and rejects non-hex run_id.
        let (rid, _, _, sum) =
            parse_child_json(r#"{"ok":true,"run_id":"not-hex","error":null,"text":"ok"}"#);
        assert_eq!(rid, None);
        assert_eq!(sum.as_deref(), Some("ok"));
    }

    #[test]
    fn tc_3133_board_run_lane_checks_records_run_and_moves_lane() {
        let _guard = crate::state_path::test_env_lock();
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        let board_base = state.join("board");
        let workspace = dir.path().join("ws");
        fs::create_dir_all(&workspace).unwrap();
        ensure_dir(&board_base).unwrap();

        let store = BoardStore::open(board_base.clone()).unwrap();
        let item = store
            .add(AddItemRequest {
                title: "Run feature".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: Some(Lane::NeedsDecision),
                source_bot: Some("bot".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(workspace.clone()),
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:test".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        // Trailing newline prompt is normalized when stored and fed to child stdin.
        let (pending_plan, hash) =
            edit_plan_from_json(&workspace, &state, "ship it\r\n", &sample_plan_json(), None)
                .unwrap();
        assert_eq!(pending_plan.prompt, "ship it");
        store
            .attach_plan(&item.id, &hash, &state, "bot:test")
            .unwrap();

        // Cannot run from needs_decision.
        let conflict = run_item_prepare(
            &store,
            RunParams {
                id: item.id.clone(),
                expected_plan_hash: None,
                by: "owner".to_owned(),
                state_root: state.clone(),
            },
        );
        assert!(matches!(conflict, Err(BoardError::Conflict(_))));

        // Answer verdict in approved lane is rejected.
        store
            .decide(
                &item.id,
                DecideRequest {
                    verdict: Verdict::Answer,
                    text: Some("ok".to_owned()),
                    option: None,
                    plan_hash: Some(hash.clone()),
                    by: "owner".to_owned(),
                },
            )
            .unwrap();

        // Approve as owner.
        store
            .decide(
                &item.id,
                DecideRequest {
                    verdict: Verdict::Approve,
                    text: None,
                    option: None,
                    plan_hash: Some(hash.clone()),
                    by: "owner".to_owned(),
                },
            )
            .unwrap();

        // Stale plan hash -> Conflict.
        let stale = run_item_prepare(
            &store,
            RunParams {
                id: item.id.clone(),
                expected_plan_hash: Some("0000000000000000".to_owned()),
                by: "bot:coordinator".to_owned(),
                state_root: state.clone(),
            },
        );
        assert!(matches!(stale, Err(BoardError::Conflict(_))));

        // Planner actor cannot run items.
        let planner_err = run_item_prepare(
            &store,
            RunParams {
                id: item.id.clone(),
                expected_plan_hash: Some(hash.clone()),
                by: "planner".to_owned(),
                state_root: state.clone(),
            },
        );
        assert!(matches!(planner_err, Err(BoardError::Forbidden(_))));

        // Remove graph-pending/<hash>.json to verify FR-3130 restore during run.
        let pending_path = pending_plan_path(&state, &hash);
        fs::remove_file(&pending_path).unwrap();

        let argv_file = dir.path().join("run_argv.txt");
        let cwd_file = dir.path().join("run_cwd.txt");
        let stdin_file = dir.path().join("run_stdin.txt");
        let fake_bin = write_fake_runner(
            dir.path(),
            "fake-run.sh",
            &format!(
                r#"#!/bin/sh
printf '%s\n' "$@" > "{}"
pwd > "{}"
cat > "{}"
printf '{{"ok":true,"exit_code":0,"run_id":"0123456789abcdef","text":"done"}}\n'
exit 0
"#,
                argv_file.display(),
                cwd_file.display(),
                stdin_file.display()
            ),
        );
        unsafe { std::env::set_var("DMUXAI_BOARD_CHILD_BIN", &fake_bin) };

        let prepared = run_item_prepare(
            &store,
            RunParams {
                id: item.id.clone(),
                expected_plan_hash: Some(hash.clone()),
                by: "bot:coordinator".to_owned(),
                state_root: state.clone(),
            },
        )
        .unwrap();
        assert_eq!(prepared.item.lane, Lane::InProgress);
        assert!(pending_path.exists());

        // Concurrent prepare while locked -> 409 Conflict.
        let concurrent = run_item_prepare(
            &store,
            RunParams {
                id: item.id.clone(),
                expected_plan_hash: Some(hash.clone()),
                by: "bot:coordinator".to_owned(),
                state_root: state.clone(),
            },
        );
        assert!(matches!(concurrent, Err(BoardError::Conflict(_))));

        let finished = run_item_execute(&store, prepared).unwrap();
        assert_eq!(finished.lane, Lane::Done);
        assert_eq!(finished.run_ids, vec!["0123456789abcdef".to_owned()]);
        assert_eq!(finished.thread.last().unwrap().from, "run");
        let argv_txt = fs::read_to_string(&argv_file).unwrap();
        assert!(argv_txt.contains("--approve-plan-id"));
        assert!(argv_txt.contains(&hash));
        assert_eq!(fs::read_to_string(&stdin_file).unwrap(), "ship it");
        assert_eq!(
            fs::canonicalize(fs::read_to_string(&cwd_file).unwrap().trim()).unwrap(),
            fs::canonicalize(&workspace).unwrap()
        );

        // Auto lane item can be run by a bot without decision, and detached launch + exec_prepared works.
        let auto_item = store
            .add(AddItemRequest {
                title: "Auto debug".to_owned(),
                summary: String::new(),
                kind: Kind::Debug,
                grade: None,
                lane: None,
                source_bot: Some("bot".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(workspace.clone()),
                recommendation: None,
                options: vec![],
                plan_hash: Some(hash.clone()),
                key: None,
                by: "bot:test".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        assert_eq!(auto_item.lane, Lane::Auto);

        let self_argv = dir.path().join("self_argv.txt");
        let fake_self = write_fake_runner(
            dir.path(),
            "fake-self.sh",
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\nexit 0\n",
                self_argv.display()
            ),
        );
        unsafe { std::env::set_var("DMUXAI_BOARD_SELF_BIN", &fake_self) };
        let auto_prep = run_item_prepare(
            &store,
            RunParams {
                id: auto_item.id.clone(),
                expected_plan_hash: Some(hash.clone()),
                by: "bot:coordinator".to_owned(),
                state_root: state.clone(),
            },
        )
        .unwrap();
        let auto_token = auto_prep.launch_token().to_owned();
        let detached_item = launch_detached_board_run(&store, auto_prep).unwrap();
        assert_eq!(detached_item.lane, Lane::InProgress);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !self_argv.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        let self_args = fs::read_to_string(&self_argv).unwrap();
        assert!(self_args.contains("--exec-prepared"));
        assert!(self_args.contains(&hash));

        let exec_done =
            run_item_execute_prepared(&store, &auto_item.id, &hash, Some(&auto_token), &state)
                .unwrap();
        assert_eq!(exec_done.lane, Lane::Done);

        // Spawn failure moves item to unknown instead of leaving it wedged in_progress.
        let fail_item = store
            .add(AddItemRequest {
                title: "Fail spawn".to_owned(),
                summary: String::new(),
                kind: Kind::Debug,
                grade: None,
                lane: None,
                source_bot: Some("bot".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(workspace),
                recommendation: None,
                options: vec![],
                plan_hash: Some(hash.clone()),
                key: None,
                by: "bot:test".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        let fail_prep = run_item_prepare(
            &store,
            RunParams {
                id: fail_item.id.clone(),
                expected_plan_hash: None,
                by: "bot:coordinator".to_owned(),
                state_root: state,
            },
        )
        .unwrap();
        unsafe {
            std::env::set_var("DMUXAI_BOARD_CHILD_BIN", "/nonexistent/dmuxai-missing-bin");
        }
        let spawn_err = run_item_execute(&store, fail_prep);
        assert!(spawn_err.is_err());
        let after_fail = store.get(&fail_item.id).unwrap();
        assert_eq!(after_fail.lane, Lane::Unknown);
        unsafe {
            std::env::remove_var("DMUXAI_BOARD_CHILD_BIN");
            std::env::remove_var("DMUXAI_BOARD_SELF_BIN");
        }
    }

    #[test]
    fn tc_3138_spawn_child_exports_run_id_and_run_respects_ask_lock() {
        let _env_guard = crate::state_path::test_env_lock();
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        let board = state.join("board");
        let workspace = dir.path().join("ws");
        ensure_dir(&workspace).unwrap();
        let store = BoardStore::open(board.clone()).unwrap();

        let env_out = dir.path().join("child-run-id.txt");
        let fake_bin = write_fake_runner(
            dir.path(),
            "capture-run-id.sh",
            &format!(
                "#!/bin/sh\ncat > /dev/null\nprintf '%s' \"$DMUXAI_RUN_ID\" > \"{}\"\nprintf '{{\"ok\":true,\"exit_code\":0,\"run_id\":\"0123456789abcdef\",\"text\":\"done\"}}\\n'\nexit 0\n",
                env_out.display()
            ),
        );
        unsafe {
            std::env::remove_var(crate::term::policy::RUN_ID_ENV);
            std::env::set_var("DMUXAI_BOARD_CHILD_BIN", &fake_bin);
        }
        let outcome =
            spawn_child_dmuxai(&workspace, &[], "hello", Duration::from_secs(5), &state).unwrap();
        unsafe { std::env::remove_var("DMUXAI_BOARD_CHILD_BIN") };
        assert_eq!(outcome.exit_code, 0);
        let exported = fs::read_to_string(&env_out).unwrap();
        assert!(
            !exported.trim().is_empty(),
            "expected DMUXAI_RUN_ID in child"
        );

        let plan_json = r#"{"tasks":[{"id":"task-1","title":"t","done_check":"d","size":"small","depends_on":[]}],"steps":[{"id":"s1","role":"implement","job":"j","runner":"cursor","depends_on":[],"task":"task-1"}]}"#;
        let (_, hash) = edit_plan_from_json(&workspace, &state, "ship", plan_json, None).unwrap();
        let item = store
            .add(AddItemRequest {
                title: "Ask lock test".to_owned(),
                summary: String::new(),
                kind: Kind::Debug,
                grade: None,
                lane: None,
                source_bot: Some("bot".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(workspace),
                recommendation: None,
                options: vec![],
                plan_hash: Some(hash),
                key: None,
                by: "bot:test".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        let _ask_lock = BoardLockGuard::acquire(&board, "asks", &item.id, "asking").unwrap();
        let err = match run_item_prepare(
            &store,
            RunParams {
                id: item.id,
                expected_plan_hash: None,
                by: "bot:coordinator".to_owned(),
                state_root: state,
            },
        ) {
            Ok(_) => panic!("expected Conflict when asks lock is active"),
            Err(err) => err,
        };
        assert_eq!(err.exit_code(), crate::agent_api::exit::CONFLICT);
    }

    // TC-3233 / FR-3233: auto-run is only triggered from owner approve paths, never by
    // non-approve verdicts, non-owner decisions, no-plan items, say/ask/plan/move/update/rules/MCP.
    #[test]
    fn tc_3233_auto_run_only_from_owner_approve_paths() {
        let _env_guard = crate::state_path::test_env_lock();
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        let board = state.join("board");
        let ws = dir.path().join("ws");
        let xdg_cfg = dir.path().join("xdg_cfg");
        fs::create_dir_all(&ws).unwrap();
        fs::create_dir_all(&xdg_cfg).unwrap();
        ensure_dir(&board).unwrap();
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let prev_state = std::env::var_os("DMUXAI_STATE_DIR");
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &xdg_cfg);
            std::env::set_var("DMUXAI_STATE_DIR", &state);
        }

        let store = BoardStore::open(board.clone()).unwrap();
        let (_, hash) =
            edit_plan_from_json(&ws, &state, "ship", &sample_plan_json(), None).unwrap();

        // 1. Item without a plan approved by owner -> not started, no side effects.
        let no_plan = store
            .add(AddItemRequest {
                title: "no plan".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: Some(Lane::NeedsDecision),
                source_bot: Some("coord".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(ws.clone()),
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:coord".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        let decided_no_plan = store
            .decide_without_notify(
                &no_plan.id,
                DecideRequest {
                    verdict: Verdict::Approve,
                    text: None,
                    option: None,
                    plan_hash: None,
                    by: "owner".to_owned(),
                },
            )
            .unwrap();
        let outcome = auto_run_after_owner_approve(&store, &decided_no_plan, &state, true);
        assert_eq!(outcome, AutoRunOutcome::not_started());
        assert_eq!(store.get(&no_plan.id).unwrap().lane, Lane::Approved);
        assert!(!BoardLockGuard::is_active(&board, "runs", &no_plan.id));

        // 2. Non-approve verdicts (reject, request_changes, answer) -> not started, no side effects.
        let item = store
            .add(AddItemRequest {
                title: "feature".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: Some(Lane::NeedsDecision),
                source_bot: Some("coord".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(ws.clone()),
                recommendation: None,
                options: vec!["optA".to_owned()],
                plan_hash: Some(hash.clone()),
                key: None,
                by: "bot:coord".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        for req in [
            DecideRequest {
                verdict: Verdict::RequestChanges,
                text: Some("fix step".to_owned()),
                option: None,
                plan_hash: Some(hash.clone()),
                by: "owner".to_owned(),
            },
            DecideRequest {
                verdict: Verdict::Answer,
                text: None,
                option: Some(0),
                plan_hash: Some(hash.clone()),
                by: "owner".to_owned(),
            },
            DecideRequest {
                verdict: Verdict::Reject,
                text: None,
                option: None,
                plan_hash: Some(hash.clone()),
                by: "owner".to_owned(),
            },
        ] {
            let decided = store.decide_without_notify(&item.id, req).unwrap();
            let before_thread = decided.thread.len();
            let outcome = auto_run_after_owner_approve(&store, &decided, &state, true);
            assert_eq!(outcome, AutoRunOutcome::not_started());
            assert_eq!(store.get(&item.id).unwrap().thread.len(), before_thread);
            assert!(!BoardLockGuard::is_active(&board, "runs", &item.id));
        }

        // 3. Non-owner decision struct -> refused without side effects.
        let mut forged = store.get(&item.id).unwrap();
        forged.lane = Lane::Approved;
        forged.decision = Some(crate::board::Decision {
            verdict: Verdict::Approve,
            text: None,
            option: None,
            plan_hash: Some(hash.clone()),
            by: "bot:coord".to_owned(),
            at_ms: 1,
        });
        assert_eq!(
            auto_run_after_owner_approve(&store, &forged, &state, true),
            AutoRunOutcome::not_started()
        );
        assert!(!BoardLockGuard::is_active(&board, "runs", &item.id));

        // 4. say, ask, plan, move, update, rules, and MCP paths leave lane/locks untouched
        //    and never write a decision or checked record.
        store
            .move_lane(&item.id, Lane::NeedsDecision, "owner", None)
            .unwrap();
        let approved = store
            .decide_without_notify(
                &item.id,
                DecideRequest {
                    verdict: Verdict::Approve,
                    text: None,
                    option: None,
                    plan_hash: Some(hash.clone()),
                    by: "owner".to_owned(),
                },
            )
            .unwrap();
        assert_eq!(approved.lane, Lane::Approved);

        store
            .say(
                &item.id,
                SayRequest {
                    text: "note".to_owned(),
                    from: "owner".to_owned(),
                },
            )
            .unwrap();
        assert_eq!(store.get(&item.id).unwrap().lane, Lane::Approved);
        assert!(!BoardLockGuard::is_active(&board, "runs", &item.id));

        let fake_ask = write_fake_runner(
            dir.path(),
            "fake-ask.sh",
            "#!/bin/sh\ncat > /dev/null\nprintf '{\"ok\":true,\"exit_code\":0,\"text\":\"answer\"}\\n'\nexit 0\n",
        );
        unsafe { std::env::set_var("DMUXAI_BOARD_CHILD_BIN", &fake_ask) };
        ask_item(
            &store,
            AskParams {
                id: item.id.clone(),
                text: "question".to_owned(),
                revise: false,
                agent: None,
                model: None,
                by: "owner".to_owned(),
                state_root: state.clone(),
            },
        )
        .unwrap();
        unsafe { std::env::remove_var("DMUXAI_BOARD_CHILD_BIN") };
        assert_eq!(store.get(&item.id).unwrap().lane, Lane::Approved);
        assert!(!BoardLockGuard::is_active(&board, "runs", &item.id));

        // MCP paths (add, say, update, move, plan) and unknown board_decide never auto-run.
        let mcp_decide = crate::board::mcp_invoke(
            &ws,
            "board_decide",
            &serde_json::json!({ "id": item.id, "verdict": "approve" }),
        );
        assert_eq!(mcp_decide["ok"], false);
        let mcp_say = crate::board::mcp_invoke(
            &ws,
            "board_say",
            &serde_json::json!({ "id": item.id, "text": "from mcp" }),
        );
        assert_eq!(mcp_say["ok"], true);
        assert_eq!(store.get(&item.id).unwrap().lane, Lane::Approved);
        assert!(!BoardLockGuard::is_active(&board, "runs", &item.id));

        // Attaching a new plan clears decision and moves back to needs_decision, never auto-runs.
        let (_, hash2) =
            edit_plan_from_json(&ws, &state, "ship v2", &sample_plan_json(), None).unwrap();
        let mcp_plan = crate::board::mcp_invoke(
            &ws,
            "board_plan",
            &serde_json::json!({ "id": item.id, "hash": hash2 }),
        );
        assert_eq!(mcp_plan["ok"], true);
        let after_plan = store.get(&item.id).unwrap();
        assert_eq!(after_plan.lane, Lane::NeedsDecision);
        assert!(after_plan.decision.is_none());
        assert!(after_plan.checked.is_none());
        assert!(!BoardLockGuard::is_active(&board, "runs", &item.id));

        unsafe {
            match prev_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match prev_state {
                Some(v) => std::env::set_var("DMUXAI_STATE_DIR", v),
                None => std::env::remove_var("DMUXAI_STATE_DIR"),
            }
        }
    }

    // TC-3234 / FR-3234: auto-run keeps every FR-3133 check; on plan change, active ask/run lock,
    // missing workspace, or wrong lane it skips with a masked `run` thread message and never starts.
    #[test]
    fn tc_3234_auto_run_skipped_when_plan_changed_or_busy() {
        let _env_guard = crate::state_path::test_env_lock();
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        let board = state.join("board");
        let ws = dir.path().join("ws");
        let xdg_cfg = dir.path().join("xdg_cfg");
        fs::create_dir_all(&ws).unwrap();
        fs::create_dir_all(&xdg_cfg).unwrap();
        ensure_dir(&board).unwrap();

        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &xdg_cfg) };

        let store = BoardStore::open(board.clone()).unwrap();
        let (_, hash1) =
            edit_plan_from_json(&ws, &state, "ship 1", &sample_plan_json(), None).unwrap();
        let (_, hash2) =
            edit_plan_from_json(&ws, &state, "ship 2", &sample_plan_json(), None).unwrap();

        let item = store
            .add(AddItemRequest {
                title: "skip cases".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: Some(Lane::NeedsDecision),
                source_bot: Some("coord".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(ws.clone()),
                recommendation: None,
                options: vec![],
                plan_hash: Some(hash1.clone()),
                key: None,
                by: "bot:coord".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        let approved = store
            .decide_without_notify(
                &item.id,
                DecideRequest {
                    verdict: Verdict::Approve,
                    text: None,
                    option: None,
                    plan_hash: Some(hash1),
                    by: "owner".to_owned(),
                },
            )
            .unwrap();

        // 1. Active ask lock -> skipped with `run` message, lane remains approved.
        {
            let _ask_lock = BoardLockGuard::acquire(&board, "asks", &item.id, "asking").unwrap();
            let outcome = auto_run_after_owner_approve(&store, &approved, &state, true);
            assert!(!outcome.started);
            let reason = outcome.reason.expect("expected skip reason");
            assert!(reason.contains("ask"), "reason: {reason}");
            let cur = store.get(&item.id).unwrap();
            assert_eq!(cur.lane, Lane::Approved);
            let last = cur.thread.last().unwrap();
            assert_eq!(last.from, "run");
            assert!(last.text.starts_with("auto-run skipped: "));
        }

        // 2. Active run lock -> skipped with `run` message, lane remains approved.
        {
            let _run_lock = BoardLockGuard::acquire(&board, "runs", &item.id, "running").unwrap();
            let outcome = auto_run_after_owner_approve(&store, &approved, &state, true);
            assert!(!outcome.started);
            let reason = outcome.reason.expect("expected skip reason");
            assert!(reason.contains("run"), "reason: {reason}");
            let cur = store.get(&item.id).unwrap();
            assert_eq!(cur.lane, Lane::Approved);
            assert!(
                cur.thread
                    .last()
                    .unwrap()
                    .text
                    .starts_with("auto-run skipped: ")
            );
        }

        // 3. Plan changed after decision -> skipped with `run` message.
        store
            .attach_plan(&item.id, &hash2, &state, "bot:coord")
            .unwrap();
        let outcome = auto_run_after_owner_approve(&store, &approved, &state, true);
        assert!(!outcome.started);
        assert!(outcome.reason.is_some());
        let cur = store.get(&item.id).unwrap();
        assert_ne!(cur.lane, Lane::InProgress);
        assert!(
            cur.thread
                .last()
                .unwrap()
                .text
                .starts_with("auto-run skipped: ")
        );
        assert!(cur.checked.is_none());

        // 4. Item without workspace -> skipped and secrets in path/reason are masked.
        let no_ws_item = store
            .add(AddItemRequest {
                title: "no ws".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: Some(Lane::NeedsDecision),
                source_bot: None,
                source_channel: None,
                source_ref: None,
                workspace: None,
                recommendation: None,
                options: vec![],
                plan_hash: Some(hash2.clone()),
                key: None,
                by: "bot:coord".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        let no_ws_approved = store
            .decide_without_notify(
                &no_ws_item.id,
                DecideRequest {
                    verdict: Verdict::Approve,
                    text: None,
                    option: None,
                    plan_hash: Some(hash2),
                    by: "owner".to_owned(),
                },
            )
            .unwrap();
        let outcome = auto_run_after_owner_approve(&store, &no_ws_approved, &state, true);
        assert!(!outcome.started);
        let reason = outcome.reason.expect("expected skip reason");
        assert!(reason.contains("workspace"), "reason: {reason}");
        let cur_no_ws = store.get(&no_ws_item.id).unwrap();
        assert_eq!(cur_no_ws.lane, Lane::Approved);
        assert!(
            cur_no_ws
                .thread
                .last()
                .unwrap()
                .text
                .starts_with("auto-run skipped: ")
        );

        // Masking check on `append_run_message`: sensitive tokens in reason are masked.
        let masked = store
            .append_run_message(
                &no_ws_item.id,
                "auto-run skipped: bad token sk-secret-token-12345",
            )
            .unwrap();
        assert!(
            !masked
                .thread
                .last()
                .unwrap()
                .text
                .contains("sk-secret-token-12345")
        );

        unsafe {
            match prev_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
    }

    struct Hardening {
        dir: tempfile::TempDir,
        state: PathBuf,
        board: PathBuf,
        ws: PathBuf,
        store: BoardStore,
        hash: String,
    }

    fn hardening() -> Hardening {
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        let board = state.join("board");
        let ws = dir.path().join("ws");
        ensure_dir(&state).unwrap();
        fs::create_dir_all(&ws).unwrap();
        let store = BoardStore::open(board.clone()).unwrap();
        let (_, hash) =
            edit_plan_from_json(&ws, &state, "ship", &sample_plan_json(), None).unwrap();
        Hardening {
            dir,
            state,
            board,
            ws,
            store,
            hash,
        }
    }

    /// An item with the shared plan, brought into `lane` the way users do it.
    fn hardening_item(h: &Hardening, lane: Lane) -> BoardItem {
        let item = h
            .store
            .add(AddItemRequest {
                title: format!("hardening {}", lane.as_str()),
                summary: String::new(),
                kind: if lane == Lane::Auto {
                    Kind::Debug
                } else {
                    Kind::Feature
                },
                grade: None,
                lane: (lane != Lane::Auto).then_some(Lane::NeedsDecision),
                source_bot: Some("coord".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(h.ws.clone()),
                recommendation: None,
                options: vec![],
                plan_hash: Some(h.hash.clone()),
                key: None,
                by: "bot:coord".to_owned(),
                state_root: h.state.clone(),
            })
            .unwrap();
        let verdict = match lane {
            Lane::Approved => Verdict::Approve,
            Lane::Rejected => Verdict::Reject,
            _ => return item,
        };
        h.store
            .decide_without_notify(
                &item.id,
                DecideRequest {
                    verdict,
                    text: None,
                    option: None,
                    plan_hash: (verdict == Verdict::Approve).then(|| h.hash.clone()),
                    by: "owner".to_owned(),
                },
            )
            .unwrap()
    }

    fn fake_ok_child(dir: &Path) -> PathBuf {
        write_fake_runner(
            dir,
            "fake-ok-run.sh",
            "#!/bin/sh\ncat > /dev/null\nprintf '{\"ok\":true,\"exit_code\":0,\"run_id\":\"0123456789abcdef\",\"text\":\"done\"}\\n'\nexit 0\n",
        )
    }

    fn prepare(h: &Hardening, item: &BoardItem) -> PreparedRun {
        run_item_prepare(
            &h.store,
            RunParams {
                id: item.id.clone(),
                expected_plan_hash: Some(h.hash.clone()),
                by: "bot:coordinator".to_owned(),
                state_root: h.state.clone(),
            },
        )
        .unwrap()
    }

    fn wait_for(deadline: Duration, mut ready: impl FnMut() -> bool) -> bool {
        let until = Instant::now() + deadline;
        while Instant::now() < until {
            if ready() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        ready()
    }

    fn files_containing(root: &Path, needle: &str) -> Vec<PathBuf> {
        let mut hits = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if fs::read(&path)
                    .map(|b| String::from_utf8_lossy(&b).contains(needle))
                    .unwrap_or(false)
                {
                    hits.push(path);
                }
            }
        }
        hits
    }

    // TC-3800 / FR-3800: prepare writes a 0600 run lock with the SHA-256 of a
    // 128-bit+ token; the raw token reaches the detached child only through
    // `DMUXAI_BOARD_LAUNCH_TOKEN`, never argv, the board files or the item.
    #[cfg(unix)]
    #[test]
    fn tc_3800_prepare_writes_token_hash_not_argv() {
        use std::os::unix::fs::PermissionsExt;
        let _env = crate::state_path::test_env_lock();
        let h = hardening();
        let item = hardening_item(&h, Lane::Approved);
        let prepared = prepare(&h, &item);
        let token = prepared.launch_token().to_owned();
        assert!(token.len() >= 32, "token too short: {}", token.len());
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(token, prepare_token_of_another_item(&h));

        let lock_path = h.board.join("runs").join(format!("{}.lock", item.id));
        let lock_mode = fs::metadata(&lock_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(lock_mode, 0o600);
        let dir_mode = fs::metadata(h.board.join("runs"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);
        let lock = read_run_lock(&h.board, &item.id).unwrap();
        assert_eq!(
            lock.token_sha256.as_deref(),
            Some(launch_token_sha256(&token).as_str())
        );
        assert_eq!(lock.plan_hash, h.hash);
        assert_eq!(lock.actor, "bot:coordinator");
        assert_eq!(lock.pid, std::process::id());
        assert!(lock.created_ms > 0);
        assert_eq!(lock.prepared_lane, Some(Lane::Approved));

        let argv_file = h.dir.path().join("self_argv.txt");
        let env_file = h.dir.path().join("self_env.txt");
        let fake_self = write_fake_runner(
            h.dir.path(),
            "fake-self-token.sh",
            &format!(
                "#!/bin/sh\nprintf '%s' \"$DMUXAI_BOARD_LAUNCH_TOKEN\" > \"{env}.tmp\"\nprintf '%s\\n' \"$@\" > \"{argv}\"\nmv \"{env}.tmp\" \"{env}\"\nexit 0\n",
                env = env_file.display(),
                argv = argv_file.display(),
            ),
        );
        unsafe { std::env::set_var("DMUXAI_BOARD_SELF_BIN", &fake_self) };
        let launched = launch_detached_board_run(&h.store, prepared);
        unsafe { std::env::remove_var("DMUXAI_BOARD_SELF_BIN") };
        let launched = launched.unwrap();
        assert_eq!(launched.lane, Lane::InProgress);
        assert!(wait_for(Duration::from_secs(3), || env_file.exists()));
        assert_eq!(fs::read_to_string(&env_file).unwrap(), token);
        let argv = fs::read_to_string(&argv_file).unwrap();
        assert!(argv.contains("--exec-prepared"));
        assert!(!argv.contains(&token), "token leaked into argv: {argv}");
        assert!(!serde_json::to_string(&launched).unwrap().contains(&token));
        assert!(
            files_containing(&h.state, &token).is_empty(),
            "token persisted under state"
        );
    }

    fn prepare_token_of_another_item(h: &Hardening) -> String {
        let other = hardening_item(h, Lane::Auto);
        let prepared = prepare(h, &other);
        prepared.launch_token().to_owned()
    }

    // TC-3801 / FR-3801: exec-prepared needs the lock's token for that item and
    // plan hash; missing, wrong and reused tokens are refused (exit 5) without
    // touching the item, and the actor comes from the lock.
    #[test]
    fn tc_3801_exec_prepared_requires_launch_token() {
        let _env = crate::state_path::test_env_lock();
        let h = hardening();
        let item = hardening_item(&h, Lane::Approved);
        let prepared = prepare(&h, &item);
        let token = prepared.launch_token().to_owned();
        prepared.guard.disarm();
        let before = h.store.get(&item.id).unwrap();
        assert_eq!(before.lane, Lane::InProgress);

        let fake_bin = fake_ok_child(h.dir.path());
        unsafe { std::env::set_var("DMUXAI_BOARD_CHILD_BIN", &fake_bin) };
        let wrong_hash = "0000000000000000";
        let attempts: [(Option<&str>, &str); 4] = [
            (None, h.hash.as_str()),
            (Some(""), h.hash.as_str()),
            (Some("00ff00ff00ff00ff00ff00ff00ff00ff"), h.hash.as_str()),
            (Some(token.as_str()), wrong_hash),
        ];
        for (presented, hash) in attempts {
            let err = run_item_execute_prepared(&h.store, &item.id, hash, presented, &h.state)
                .unwrap_err();
            assert_eq!(
                err.exit_code(),
                crate::agent_api::exit::FORBIDDEN,
                "{presented:?}"
            );
            assert_eq!(h.store.get(&item.id).unwrap(), before);
        }

        let done =
            run_item_execute_prepared(&h.store, &item.id, &h.hash, Some(&token), &h.state).unwrap();
        assert_eq!(done.lane, Lane::Done);
        let events = fs::read_to_string(h.board.join("events.jsonl")).unwrap();
        let last: Value = serde_json::from_str(events.lines().last().unwrap()).unwrap();
        assert_eq!(last["by"], "bot:coordinator");

        let after = h.store.get(&item.id).unwrap();
        let reuse = run_item_execute_prepared(&h.store, &item.id, &h.hash, Some(&token), &h.state)
            .unwrap_err();
        assert_eq!(reuse.exit_code(), crate::agent_api::exit::FORBIDDEN);
        assert_eq!(h.store.get(&item.id).unwrap(), after);
        unsafe { std::env::remove_var("DMUXAI_BOARD_CHILD_BIN") };
    }

    /// Force `item` into `in_progress` with a run lock holding a known token, as
    /// if it had been prepared from `prepared_lane`.
    fn force_prepared(h: &Hardening, item: &BoardItem, prepared_lane: Lane) -> String {
        h.store
            .move_lane(&item.id, Lane::InProgress, "owner", None)
            .unwrap();
        let token = generate_launch_token();
        write_run_lock(
            &h.board,
            &item.id,
            &RunLock {
                pid: std::process::id(),
                token_sha256: Some(launch_token_sha256(&token)),
                plan_hash: h.hash.clone(),
                actor: "bot:coordinator".to_owned(),
                created_ms: 1,
                prepared_lane: Some(prepared_lane),
            },
        )
        .unwrap();
        token
    }

    // TC-3802 / FR-3802: even with a valid token, exec-prepared runs only an item
    // approved by the owner for that plan hash, or one prepared from `auto`.
    #[test]
    fn tc_3802_exec_prepared_checks_decision() {
        let _env = crate::state_path::test_env_lock();
        let h = hardening();
        let fake_bin = fake_ok_child(h.dir.path());
        unsafe { std::env::set_var("DMUXAI_BOARD_CHILD_BIN", &fake_bin) };

        let pending = hardening_item(&h, Lane::NeedsDecision);
        let rejected = hardening_item(&h, Lane::Rejected);
        let stale = hardening_item(&h, Lane::Approved);
        let approved = hardening_item(&h, Lane::Approved);
        let auto = hardening_item(&h, Lane::Auto);

        let item_file = h.board.join("items").join(format!("{}.json", stale.id));
        let mut raw: Value = serde_json::from_slice(&fs::read(&item_file).unwrap()).unwrap();
        raw["decision"]["plan_hash"] = Value::String("0000000000000000".to_owned());
        fs::write(&item_file, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();

        let cases = [
            (
                &pending,
                Lane::NeedsDecision,
                Some(crate::agent_api::exit::FORBIDDEN),
            ),
            (
                &rejected,
                Lane::Rejected,
                Some(crate::agent_api::exit::FORBIDDEN),
            ),
            (
                &stale,
                Lane::Approved,
                Some(crate::agent_api::exit::CONFLICT),
            ),
            (&approved, Lane::Approved, None),
            (&auto, Lane::Auto, None),
        ];
        for (item, prepared_lane, refused) in cases {
            let token = force_prepared(&h, item, prepared_lane);
            let result =
                run_item_execute_prepared(&h.store, &item.id, &h.hash, Some(&token), &h.state);
            let current = h.store.get(&item.id).unwrap();
            match refused {
                Some(code) => {
                    let err = result.unwrap_err();
                    assert_eq!(err.exit_code(), code, "{}: {err}", item.title);
                    assert_eq!(current.lane, Lane::InProgress, "{}", item.title);
                    assert!(current.run_ids.is_empty(), "{}", item.title);
                }
                None => {
                    assert_eq!(result.unwrap().lane, Lane::Done, "{}", item.title);
                }
            }
        }
        unsafe { std::env::remove_var("DMUXAI_BOARD_CHILD_BIN") };
    }

    // TC-3808 / FR-3808: the parent reaps the detached child (no zombie), and the
    // run lock names the child so a dead child leaves a stale, inactive lock.
    #[cfg(target_os = "linux")]
    #[test]
    fn tc_3808_detached_child_reaped() {
        let _env = crate::state_path::test_env_lock();
        let h = hardening();
        let item = hardening_item(&h, Lane::Auto);
        let prepared = prepare(&h, &item);
        let pid_file = h.dir.path().join("child.pid");
        let fake_self = write_fake_runner(
            h.dir.path(),
            "fake-self-pid.sh",
            &format!(
                "#!/bin/sh\necho $$ > \"{p}.tmp\"\nmv \"{p}.tmp\" \"{p}\"\nexit 0\n",
                p = pid_file.display()
            ),
        );
        unsafe { std::env::set_var("DMUXAI_BOARD_SELF_BIN", &fake_self) };
        let launched = launch_detached_board_run(&h.store, prepared);
        unsafe { std::env::remove_var("DMUXAI_BOARD_SELF_BIN") };
        launched.unwrap();

        assert!(wait_for(Duration::from_secs(3), || pid_file.exists()));
        let child_pid: u32 = fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(read_run_lock(&h.board, &item.id).unwrap().pid, child_pid);

        // A zombie keeps its /proc entry until reaped; a reaped child has none.
        let reaped = || !Path::new(&format!("/proc/{child_pid}")).exists();
        assert!(
            wait_for(Duration::from_secs(3), reaped),
            "detached child {child_pid} left as a zombie"
        );
        assert!(wait_for(Duration::from_secs(3), || {
            !BoardLockGuard::is_active(&h.board, "runs", &item.id)
        }));
    }
}
