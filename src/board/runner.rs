//! Decision board child runner for `board ask` and `board run`
//! (SPEC-decision-board FR-3132, FR-3133).

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::{
    BoardError, BoardItem, BoardStore, Lane, ListFilter, SayRequest, TEXT_MAX,
    ensure_pending_plan_from_copy, prune_old_plan_copies, validate_item_id_early, validate_message,
};
use crate::process::{apply_child_cwd, apply_child_env_policy};
use crate::run_journal::mask_sensitive_tokens;
use crate::settings::BoardSettings;

pub const BOARD_CHILD_TIMEOUT_SECS: u64 = 900;
const STALE_LOCK_SECS: u64 = 960;
const THREAD_CONTEXT_LIMIT: usize = 20;

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
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "pid={}", std::process::id());
                    return Ok(Self { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let is_stale = fs::metadata(&path)
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > Duration::from_secs(STALE_LOCK_SECS));
                    if is_stale && attempt == 0 {
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
    let Some(val) = parsed else {
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
        .filter(|s| !s.is_empty())
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
        .or_else(|| val.get("error"))
        .or_else(|| val.get("text"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    (run_id, plan_hash, text, summary)
}

pub fn spawn_child_dmuxai(
    workspace: &Path,
    args: &[String],
    stdin_prompt: &str,
    timeout: Duration,
) -> Result<ChildOutcome, BoardError> {
    let bin = resolve_child_bin()?;
    let mut cmd = Command::new(&bin);
    cmd.args(args);
    apply_child_cwd(&mut cmd, workspace);
    apply_child_env_policy(&mut cmd);
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
    let stdout_handle = thread::spawn(move || {
        let mut buf = String::new();
        if let Some(mut out) = stdout_pipe {
            let _ = out.read_to_string(&mut buf);
        }
        buf
    });
    let stderr_handle = thread::spawn(move || {
        let mut buf = String::new();
        if let Some(mut err) = stderr_pipe {
            let _ = err.read_to_string(&mut buf);
        }
        buf
    });

    let deadline = Instant::now() + timeout;
    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().unwrap_or(1),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break 124;
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                let _ = child.kill();
                return Err(BoardError::Internal(e.to_string()));
            }
        }
    };

    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();
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

fn build_ask_prompt(item: &BoardItem, question: &str) -> String {
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
    let _guard = guard;
    let ws_str = workspace.to_string_lossy().into_owned();
    let timeout = child_timeout();

    if !params.revise {
        let board_cfg = BoardSettings::load_user_only();
        let (cfg_agent, cfg_model) = board_cfg.planner_agent_and_model();
        let agent = params.agent.or(cfg_agent);
        let model = params.model.or(cfg_model);
        let mut args = vec![
            "-C".to_owned(),
            ws_str,
            "run".to_owned(),
            "--read-only".to_owned(),
            "--json".to_owned(),
        ];
        if let Some(a) = agent {
            args.push("--agent".to_owned());
            args.push(a);
        }
        if let Some(m) = model {
            args.push("--model".to_owned());
            args.push(m);
        }
        args.push("-".to_owned());

        let prompt = build_ask_prompt(&item, &params.text);
        let outcome = spawn_child_dmuxai(&workspace, &args, &prompt, timeout)?;
        let raw_answer = outcome
            .text
            .or(outcome.summary)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("planner ask failed (exit {})", outcome.exit_code));
        let masked = mask_sensitive_tokens(&raw_answer);
        let answer: String = masked.chars().take(TEXT_MAX).collect();
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
    let outcome = spawn_child_dmuxai(&workspace, &args, &prompt, timeout)?;
    if outcome.exit_code == 7
        && let Some(new_hash) = outcome.plan_hash
    {
        let updated = store.attach_plan(&params.id, &new_hash, &params.state_root, "planner")?;
        let rev = updated.plan.as_ref().map(|p| p.revision).unwrap_or(0);
        let steps = updated.plan.as_ref().map(|p| p.steps.len()).unwrap_or(0);
        let msg = format!("Revised plan to revision {rev} ({new_hash}, {steps} steps).");
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
}

pub fn run_item_prepare(store: &BoardStore, params: RunParams) -> Result<PreparedRun, BoardError> {
    validate_item_id_early(&params.id)?;
    if params.by == "planner" {
        return Err(BoardError::Forbidden("planner cannot run items".to_owned()));
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
            if decision.plan_hash.as_deref() != Some(plan.hash.as_str()) {
                return Err(BoardError::Conflict(
                    "decision plan hash does not match the current plan".to_owned(),
                ));
            }
            if params.by.starts_with("bot:") && decision.by != "owner" {
                return Err(BoardError::Forbidden(
                    "bots may only run items approved by owner or in the auto lane".to_owned(),
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
    let in_progress_item =
        store.move_lane(&params.id, Lane::InProgress, &params.by, Some(item.version))?;

    if let Ok(all_items) = store.list(&ListFilter {
        include_all: true,
        ..Default::default()
    }) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        prune_old_plan_copies(store.base(), &all_items, now);
    }

    Ok(PreparedRun {
        item: in_progress_item,
        workspace,
        plan_hash,
        prompt: pending.prompt,
        guard,
        by: params.by,
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
    } = prepared;
    let _guard = guard;
    let args = vec![
        "-C".to_owned(),
        workspace.to_string_lossy().into_owned(),
        "run".to_owned(),
        "--graph".to_owned(),
        "--approve-plan-id".to_owned(),
        plan_hash,
        "--json".to_owned(),
        "-".to_owned(),
    ];
    let outcome = spawn_child_dmuxai(&workspace, &args, &prompt, child_timeout())?;
    store.record_run_result(
        &item.id,
        outcome.run_id.as_deref(),
        outcome.exit_code,
        outcome.summary.as_deref(),
        &by,
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
        let fake_bin = write_fake_runner(
            dir.path(),
            "fake-dmuxai.sh",
            &format!(
                r#"#!/bin/sh
echo "DMUXAI_WEB_TOKEN=$DMUXAI_WEB_TOKEN" > "{}"
cat > /dev/null
case "$*" in
  *"--read-only"*)
    printf '{{"ok":true,"exit_code":0,"text":"Answer with Bearer sk-secret-999"}}\n'
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
        assert_eq!(
            fs::read_to_string(&token_leak_check).unwrap().trim(),
            "DMUXAI_WEB_TOKEN="
        );

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
        let (_, hash) =
            edit_plan_from_json(&workspace, &state, "ship it", &sample_plan_json(), None).unwrap();
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

        // Remove graph-pending/<hash>.json to verify FR-3130 restore during run.
        let pending_path = pending_plan_path(&state, &hash);
        fs::remove_file(&pending_path).unwrap();

        let fake_bin = write_fake_runner(
            dir.path(),
            "fake-run.sh",
            r#"#!/bin/sh
cat > /dev/null
printf '{"ok":true,"exit_code":0,"run_id":"run-abc123","text":"done"}\n'
exit 0
"#,
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

        let finished = run_item_execute(&store, prepared).unwrap();
        unsafe { std::env::remove_var("DMUXAI_BOARD_CHILD_BIN") };
        assert_eq!(finished.lane, Lane::Done);
        assert_eq!(finished.run_ids, vec!["run-abc123".to_owned()]);
        assert_eq!(finished.thread.last().unwrap().from, "run");
    }
}
