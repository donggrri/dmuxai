//! Decision board event notifications (SPEC-decision-board FR-3135, NFR-3130).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::{BoardItem, EventDetail};
use crate::events::format_ts;
use crate::notify::{
    DEFAULT_SECRET_HEADER, DEFAULT_TIMEOUT_SECS, append_log, deliver_command_json,
    deliver_webhook_json,
};
use crate::run_journal::mask_sensitive_tokens;
use crate::settings::BoardSettings;

pub const BOARD_EVENT_SCHEMA: &str = "dmuxai.board.event.v1";

pub(crate) static BOARD_NOTIFY_THREAD_SPAWNS: AtomicU64 = AtomicU64::new(0);

fn in_flight_gate() -> &'static (Mutex<usize>, Condvar) {
    static GATE: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new();
    GATE.get_or_init(|| (Mutex::new(0), Condvar::new()))
}

struct InFlightGuard;

impl InFlightGuard {
    fn enter() -> Self {
        let (lock, _) = in_flight_gate();
        if let Ok(mut count) = lock.lock() {
            *count += 1;
        }
        Self
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        let (lock, cvar) = in_flight_gate();
        if let Ok(mut count) = lock.lock() {
            *count = count.saturating_sub(1);
            if *count == 0 {
                cvar.notify_all();
            }
        }
    }
}

/// Wait up to `max_wait` (capped at `min(DEFAULT_TIMEOUT_SECS, 10)`s) for
/// in-flight CLI board notification threads to finish before process exit.
pub fn wait_for_pending(max_wait: Duration) {
    let cap = Duration::from_secs(DEFAULT_TIMEOUT_SECS.min(10));
    let bounded = max_wait.min(cap);
    let deadline = Instant::now() + bounded;
    let (lock, cvar) = in_flight_gate();
    let Ok(mut count) = lock.lock() else {
        return;
    };
    while *count > 0 {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        let Ok((guard, _)) = cvar.wait_timeout(count, remaining) else {
            break;
        };
        count = guard;
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BoardNotifyEvent {
    pub schema: &'static str,
    pub event: String,
    pub item_id: String,
    pub title: String,
    pub lane: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,
    pub by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_run: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_revision: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_paths: Option<Vec<String>>,
    pub at_ms: u64,
}

impl BoardNotifyEvent {
    pub fn from_item(item: &BoardItem, event: &str, by: &str, at_ms: u64) -> Self {
        let bot = item
            .source
            .bot
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(mask_sensitive_tokens);
        let verdict = if event == "decision" {
            item.decision
                .as_ref()
                .map(|d| d.verdict.as_str().to_owned())
        } else {
            None
        };
        Self {
            schema: BOARD_EVENT_SCHEMA,
            event: event.to_owned(),
            item_id: item.id.clone(),
            title: mask_sensitive_tokens(&item.title),
            lane: item.lane.as_str().to_owned(),
            kind: item.kind.as_str().to_owned(),
            bot,
            workspace_path: item.workspace.path.as_deref().map(mask_sensitive_tokens),
            by: mask_sensitive_tokens(by),
            plan_hash: item.plan.as_ref().map(|p| p.hash.clone()),
            verdict,
            auto_run: None,
            plan_revision: None,
            review_id: None,
            spec_paths: None,
            at_ms,
        }
    }

    /// Payload for coordinator observability events (FR-3850..FR-3852).
    pub fn from_item_with_detail(
        item: &BoardItem,
        event: &str,
        by: &str,
        at_ms: u64,
        detail: &EventDetail,
    ) -> Self {
        let mut payload = Self::from_item(item, event, by, at_ms);
        if let Some(hash) = &detail.plan_hash {
            payload.plan_hash = Some(hash.clone());
        }
        payload.plan_revision = detail.plan_revision;
        payload.review_id = detail.review_id.as_deref().map(mask_sensitive_tokens);
        if !detail.spec_paths.is_empty() {
            payload.spec_paths = Some(
                detail
                    .spec_paths
                    .iter()
                    .map(|p| mask_sensitive_tokens(p))
                    .collect(),
            );
        }
        payload
    }

    pub fn from_item_with_auto_run(
        item: &BoardItem,
        event: &str,
        by: &str,
        at_ms: u64,
        auto_run: bool,
    ) -> Self {
        let mut payload = Self::from_item(item, event, by, at_ms);
        if event == "decision" && auto_run {
            payload.auto_run = Some(true);
        }
        payload
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Dispatch a board notification on a background thread when `[board]` has a
/// notify target configured and allows `event`. Never blocks or fails the write;
/// spawns no thread when no notify target is configured (NFR-3130).
pub fn dispatch_board_event(board_base: &Path, item: &BoardItem, event: &str, by: &str) {
    let settings = BoardSettings::load_user_only();
    dispatch_board_event_with_settings(board_base, item, event, by, &settings);
}

pub fn dispatch_board_event_with_settings(
    board_base: &Path,
    item: &BoardItem,
    event: &str,
    by: &str,
    settings: &BoardSettings,
) {
    if !settings.has_notify_target() || !settings.allows_event(event) {
        return;
    }
    let at_ms = now_ms();
    let payload = BoardNotifyEvent::from_item(item, event, by, at_ms);
    spawn_notify_delivery(board_base, item, at_ms, &payload, settings);
}

pub fn dispatch_board_event_detail(
    board_base: &Path,
    item: &BoardItem,
    event: &str,
    by: &str,
    detail: &EventDetail,
) {
    let settings = BoardSettings::load_user_only();
    dispatch_board_event_detail_with_settings(board_base, item, event, by, detail, &settings);
}

pub fn dispatch_board_event_detail_with_settings(
    board_base: &Path,
    item: &BoardItem,
    event: &str,
    by: &str,
    detail: &EventDetail,
    settings: &BoardSettings,
) {
    if !settings.has_notify_target() || !settings.allows_event(event) {
        return;
    }
    let at_ms = now_ms();
    let payload = BoardNotifyEvent::from_item_with_detail(item, event, by, at_ms, detail);
    spawn_notify_delivery(board_base, item, at_ms, &payload, settings);
}

/// Hook for the spec review gate (SPEC-coordinator-observability FR-3852):
/// posts a `spec_review` thread message and raises `spec_review_requested` on
/// the `in_progress` board item named by [`super::BOARD_ITEM_ID_ENV`] in this
/// process, else on the item whose `run_ids` holds `run_id`. Absolute `spec_paths` under
/// `workspace` are made relative, others outside it are dropped, and at most
/// [`SPEC_PATHS_MAX`] are kept. Never fails the caller: errors go to
/// `<state base>/board/notify.log`, and a missing board or no matching item
/// leaves the board untouched.
pub fn notify_spec_review_requested(
    workspace: &Path,
    run_id: &str,
    review_id: &str,
    spec_paths: &[String],
) {
    let Ok(state) = crate::state_path::state_base() else {
        return;
    };
    notify_spec_review_requested_in(
        &state.join("board"),
        workspace,
        None,
        run_id,
        review_id,
        spec_paths,
    );
}

/// [`notify_spec_review_requested`] on the board at `board_base`, preferring
/// `item_id` (the review record's `board_item_id`) over
/// [`super::BOARD_ITEM_ID_ENV`] before falling back to `run_ids`.
pub fn notify_spec_review_requested_in(
    board_base: &Path,
    workspace: &Path,
    item_id: Option<&str>,
    run_id: &str,
    review_id: &str,
    spec_paths: &[String],
) {
    if !board_base.join("items").is_dir() {
        return;
    }
    let paths = workspace_relative_spec_paths(workspace, spec_paths);
    let env_item_id = std::env::var(super::BOARD_ITEM_ID_ENV).ok();
    let item_id = item_id
        .or(env_item_id.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let board_base = board_base.to_path_buf();
    let result = super::BoardStore::open(board_base.clone())
        .and_then(|store| store.record_spec_review_request(item_id, run_id, review_id, &paths));
    if let Err(error) = result {
        append_log(
            &board_base.join("notify.log"),
            &format!("{} spec_review_requested {error}", format_ts(now_ms())),
        );
    }
}

pub const SPEC_PATHS_MAX: usize = 20;

fn workspace_relative_spec_paths(workspace: &Path, spec_paths: &[String]) -> Vec<String> {
    let root = std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let mut out: Vec<String> = Vec::new();
    for raw in spec_paths {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let path = Path::new(trimmed);
        let relative = if path.is_absolute() {
            match path
                .strip_prefix(&root)
                .or_else(|_| path.strip_prefix(workspace))
            {
                Ok(rel) => rel.to_path_buf(),
                Err(_) => continue,
            }
        } else {
            path.to_path_buf()
        };
        let mut clean = PathBuf::new();
        let mut escapes = false;
        for component in relative.components() {
            match component {
                std::path::Component::Normal(part) => clean.push(part),
                std::path::Component::CurDir => {}
                _ => escapes = true,
            }
        }
        if escapes || clean.as_os_str().is_empty() {
            continue;
        }
        let text = clean.to_string_lossy().replace('\\', "/");
        if !out.contains(&text) {
            out.push(text);
        }
        if out.len() >= SPEC_PATHS_MAX {
            break;
        }
    }
    out
}

/// Dispatch a `decision` notification, optionally setting `auto_run: true` when
/// owner approval started a detached run (SPEC-board-approve-autorun FR-3231).
pub fn dispatch_decision_event(board_base: &Path, item: &BoardItem, by: &str, auto_run: bool) {
    let settings = BoardSettings::load_user_only();
    dispatch_decision_event_with_settings(board_base, item, by, auto_run, &settings);
}

pub fn dispatch_decision_event_with_settings(
    board_base: &Path,
    item: &BoardItem,
    by: &str,
    auto_run: bool,
    settings: &BoardSettings,
) {
    if !settings.has_notify_target() || !settings.allows_event("decision") {
        return;
    }
    let at_ms = now_ms();
    let payload = BoardNotifyEvent::from_item_with_auto_run(item, "decision", by, at_ms, auto_run);
    spawn_notify_delivery(board_base, item, at_ms, &payload, settings);
}

fn spawn_notify_delivery(
    board_base: &Path,
    item: &BoardItem,
    at_ms: u64,
    payload: &BoardNotifyEvent,
    settings: &BoardSettings,
) {
    let body = match serde_json::to_string(payload) {
        Ok(json) => json,
        Err(_) => return,
    };
    let ts = format_ts(at_ms);
    let log_path = board_base.join("notify.log");
    let webhook_url = settings.notify_webhook_url.clone();
    let secret = settings
        .notify_webhook_secret_env
        .as_deref()
        .and_then(|name| std::env::var(name).ok())
        .filter(|value| !value.is_empty());
    let command = settings.notify_command.clone();
    let cwd = item
        .workspace
        .path
        .as_deref()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| board_base.to_path_buf());

    BOARD_NOTIFY_THREAD_SPAWNS.fetch_add(1, Ordering::Relaxed);
    let guard = InFlightGuard::enter();
    let _ = std::thread::Builder::new()
        .name("board-notify".to_owned())
        .spawn(move || {
            let _guard = guard;
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                append_log(&log_path, &format!("{ts} runtime error"));
                return;
            };
            rt.block_on(async move {
                let timeout = Duration::from_secs(DEFAULT_TIMEOUT_SECS);
                if let Some(url) = &webhook_url {
                    let _ = rustls::crypto::ring::default_provider().install_default();
                    match reqwest::Client::builder()
                        .redirect(reqwest::redirect::Policy::none())
                        .timeout(timeout)
                        .build()
                    {
                        Ok(client) => {
                            deliver_webhook_json(
                                &client,
                                url,
                                DEFAULT_SECRET_HEADER,
                                secret.as_deref(),
                                body.clone(),
                                &ts,
                                timeout,
                                &log_path,
                            )
                            .await;
                        }
                        Err(error) => {
                            append_log(&log_path, &format!("{ts} webhook {error}"));
                        }
                    }
                }
                if let Some(command) = &command {
                    deliver_command_json(command, &cwd, body, &ts, timeout, &log_path).await;
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::store::{AddItemRequest, BoardStore, ensure_dir};
    use crate::board::{Kind, Lane};
    use tempfile::tempdir;

    #[test]
    fn tc_3135_board_notify_command_payload_masked_and_non_blocking() {
        let _env_guard = crate::state_path::test_env_lock();
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        let board_base = state.join("board");
        let xdg_cfg = dir.path().join("xdg_cfg");
        std::fs::create_dir_all(xdg_cfg.join("dmuxai")).unwrap();
        ensure_dir(&board_base).unwrap();

        // Start a local webhook server to verify secret header delivery and failure logging.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx_req, rx_req) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            while let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                let _ = tx_req.send(req);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
            }
        });

        let out_file = dir.path().join("hook-out.jsonl");
        let script = dir.path().join("hook.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nsleep 0.2\nbody=$(cat)\nprintf '%s\\n' \"$body\" >> \"{}\"\n",
                out_file.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
        }

        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let prev_secret = std::env::var_os("DMUXAI_TEST_BOARD_SECRET");
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &xdg_cfg);
            std::env::set_var("DMUXAI_TEST_BOARD_SECRET", "top-secret-webhook-hdr");
        }
        std::fs::write(
            xdg_cfg.join("dmuxai").join("dmuxai.toml"),
            format!(
                "[board]\nnotify_command = [\"{}\"]\nnotify_webhook_url = \"http://{}/hook?token=sk-query-secret-123\"\nnotify_webhook_secret_env = \"DMUXAI_TEST_BOARD_SECRET\"\n",
                script.display(),
                addr
            ),
        )
        .unwrap();

        let store = BoardStore::open(board_base.clone()).unwrap();
        let start = Instant::now();
        let item = store
            .add(AddItemRequest {
                title: "Fix Bearer sk-secret-token-12345".to_owned(),
                summary: "summary".to_owned(),
                kind: Kind::Debug,
                grade: None,
                lane: Some(Lane::NeedsDecision),
                source_bot: Some("bot".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(dir.path().to_path_buf()),
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:test".to_owned(),
                state_root: state,
            })
            .unwrap();
        assert!(
            start.elapsed() < Duration::from_millis(150),
            "dispatch_board_event must not block on slow hook"
        );

        wait_for_pending(Duration::from_secs(5));
        unsafe {
            match prev_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match prev_secret {
                Some(v) => std::env::set_var("DMUXAI_TEST_BOARD_SECRET", v),
                None => std::env::remove_var("DMUXAI_TEST_BOARD_SECRET"),
            }
        }

        let raw_all = std::fs::read_to_string(&out_file).expect("hook output should exist");
        let (raw, json) = raw_all
            .lines()
            .filter_map(|line| {
                serde_json::from_str::<serde_json::Value>(line.trim())
                    .ok()
                    .map(|val| (line.to_owned(), val))
            })
            .find(|(_, val)| val["item_id"] == item.id)
            .expect("hook output for item should exist");
        assert_eq!(json["schema"], BOARD_EVENT_SCHEMA);
        assert_eq!(json["event"], "needs_decision");
        assert_eq!(json["item_id"], item.id);
        assert_eq!(json["lane"], "needs_decision");
        assert_eq!(json["kind"], "debug");
        assert!(!raw.contains("sk-secret-token-12345"));

        let mut matched_webhook = None;
        while let Ok(req) = rx_req.recv_timeout(Duration::from_secs(2)) {
            if req.contains(&item.id) {
                matched_webhook = Some(req);
                break;
            }
        }
        let webhook_req = matched_webhook.expect("webhook should be called for item");
        assert!(
            webhook_req
                .to_ascii_lowercase()
                .contains("x-dmuxai-secret: top-secret-webhook-hdr"),
            "missing secret header: {webhook_req}"
        );

        // Verify append_log writes 0600 and masks secrets without creating .gitignore in board/.
        let log_path = board_base.join("notify.log");
        append_log(&log_path, "webhook error Bearer sk-log-secret-999");
        assert!(!board_base.join(".gitignore").exists());
        let log_text = std::fs::read_to_string(&log_path).unwrap();
        assert!(!log_text.contains("sk-log-secret-999"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&log_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn tc_3136_no_notifier_without_board_config() {
        let _env_guard = crate::state_path::test_env_lock();
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        let board_base = state.join("board");
        let empty_xdg = dir.path().join("empty_xdg");
        std::fs::create_dir_all(&empty_xdg).unwrap();
        ensure_dir(&board_base).unwrap();

        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &empty_xdg) };
        let before = BOARD_NOTIFY_THREAD_SPAWNS.load(Ordering::Relaxed);
        let store = BoardStore::open(board_base.clone()).unwrap();
        let item = store
            .add(AddItemRequest {
                title: "no notify".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: Some(Lane::NeedsDecision),
                source_bot: Some("bot".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: None,
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:test".to_owned(),
                state_root: state,
            })
            .unwrap();
        store
            .decide(
                &item.id,
                crate::board::DecideRequest {
                    verdict: crate::board::Verdict::Approve,
                    text: None,
                    option: None,
                    plan_hash: None,
                    by: "owner".to_owned(),
                },
            )
            .unwrap();
        let after = BOARD_NOTIFY_THREAD_SPAWNS.load(Ordering::Relaxed);
        unsafe {
            match prev_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
        assert_eq!(before, after, "no notifier thread should spawn");
    }

    // TC-3231 / FR-3231: decision notify payload carries masked bot, verdict, and optional auto_run.
    #[test]
    fn tc_3231_decision_notify_payload_has_bot_verdict_and_auto_run() {
        let _env_guard = crate::state_path::test_env_lock();
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        let board_base = state.join("board");
        let xdg_cfg = dir.path().join("xdg_cfg");
        std::fs::create_dir_all(xdg_cfg.join("dmuxai")).unwrap();
        ensure_dir(&board_base).unwrap();

        let out_file = dir.path().join("decision-notify.jsonl");
        let script = dir.path().join("hook.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nbody=$(cat)\nprintf '%s\\n' \"$body\" >> \"{}\"\n",
                out_file.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
        }
        std::fs::write(
            xdg_cfg.join("dmuxai").join("dmuxai.toml"),
            format!("[board]\nnotify_command = [\"{}\"]\n", script.display()),
        )
        .unwrap();

        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &xdg_cfg) };

        let store = BoardStore::open(board_base.clone()).unwrap();
        let item = store
            .add(AddItemRequest {
                title: "Wake bot".to_owned(),
                summary: "secret summary sk-summary-hidden-123".to_owned(),
                kind: Kind::Feature,
                grade: None,
                lane: Some(Lane::NeedsDecision),
                source_bot: Some("coordinator sk-bot-secret-999".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(dir.path().to_path_buf()),
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:coordinator".to_owned(),
                state_root: state,
            })
            .unwrap();
        store
            .say(
                &item.id,
                crate::board::SayRequest {
                    text: "private thread body".to_owned(),
                    from: "bot:coordinator".to_owned(),
                },
            )
            .unwrap();

        let decided = store
            .decide_without_notify(
                &item.id,
                crate::board::DecideRequest {
                    verdict: crate::board::Verdict::Approve,
                    text: None,
                    option: None,
                    plan_hash: None,
                    by: "owner".to_owned(),
                },
            )
            .unwrap();
        dispatch_decision_event(&board_base, &decided, "owner", true);

        // Also verify request_changes without auto_run omits auto_run and keeps verdict.
        let req_changes = store
            .decide_without_notify(
                &item.id,
                crate::board::DecideRequest {
                    verdict: crate::board::Verdict::RequestChanges,
                    text: Some("revise step 1".to_owned()),
                    option: None,
                    plan_hash: None,
                    by: "owner".to_owned(),
                },
            )
            .unwrap();
        dispatch_decision_event(&board_base, &req_changes, "owner", false);

        wait_for_pending(Duration::from_secs(5));
        unsafe {
            match prev_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }

        let raw = std::fs::read_to_string(&out_file).expect("notify output should exist");
        let events: Vec<serde_json::Value> = raw
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l.trim()).ok())
            .filter(|v| v["item_id"] == item.id)
            .collect();
        let item_serialized = serde_json::to_string(&events).unwrap();
        assert!(!item_serialized.contains("sk-bot-secret-999"));
        assert!(!item_serialized.contains("sk-summary-hidden-123"));
        assert!(!item_serialized.contains("private thread body"));

        let needs_ev = events
            .iter()
            .find(|v| v["event"] == "needs_decision")
            .expect("needs_decision event");
        assert!(needs_ev["bot"].as_str().unwrap().contains("coordinator"));
        assert!(needs_ev.get("verdict").is_none());
        assert!(needs_ev.get("auto_run").is_none());

        let approve_ev = events
            .iter()
            .find(|v| v["event"] == "decision" && v["verdict"] == "approve")
            .expect("approve decision event");
        assert!(approve_ev["bot"].as_str().unwrap().contains("coordinator"));
        assert_eq!(approve_ev["auto_run"], true);

        let changes_ev = events
            .iter()
            .find(|v| v["event"] == "decision" && v["verdict"] == "request_changes")
            .expect("request_changes decision event");
        assert_eq!(changes_ev["verdict"], "request_changes");
        assert!(changes_ev.get("auto_run").is_none());
    }

    // TC-3852 / FR-3852: the spec review hook posts a `spec_review` thread
    // message and a `spec_review_requested` board + notify event with up to
    // 20 workspace-relative paths; an unknown run leaves the board unchanged.
    #[test]
    fn tc_3852_spec_review_requested_hook() {
        let _env_guard = crate::state_path::test_env_lock();
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        let board_base = state.join("board");
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let xdg_cfg = dir.path().join("xdg_cfg");
        std::fs::create_dir_all(xdg_cfg.join("dmuxai")).unwrap();
        ensure_dir(&board_base).unwrap();

        let out_file = dir.path().join("spec-review-notify.jsonl");
        let script = dir.path().join("hook.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nbody=$(cat)\nprintf '%s\\n' \"$body\" >> \"{}\"\n",
                out_file.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
        }
        std::fs::write(
            xdg_cfg.join("dmuxai").join("dmuxai.toml"),
            format!("[board]\nnotify_command = [\"{}\"]\n", script.display()),
        )
        .unwrap();
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let prev_state = std::env::var_os("DMUXAI_STATE_DIR");
        let prev_item = std::env::var_os(crate::board::BOARD_ITEM_ID_ENV);
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &xdg_cfg);
            std::env::set_var("DMUXAI_STATE_DIR", &state);
            std::env::remove_var(crate::board::BOARD_ITEM_ID_ENV);
        }

        let run_id = "0123456789abcdef";
        let store = BoardStore::open(board_base.clone()).unwrap();
        let item = store
            .add(AddItemRequest {
                title: "spec review".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: Some(Lane::InProgress),
                source_bot: None,
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
        let item = store
            .update(
                &item.id,
                crate::board::UpdateItemRequest {
                    title: None,
                    summary: None,
                    recommendation: None,
                    options: None,
                    grade: None,
                    kind: None,
                    run_id: Some(run_id.to_owned()),
                    if_version: None,
                    by: "bot:test".to_owned(),
                },
            )
            .unwrap();

        let mut paths = vec![
            workspace
                .join("docs/specs/SPEC-a.md")
                .to_string_lossy()
                .into_owned(),
            "docs/specs/SPEC-b.md".to_owned(),
            "../outside.md".to_owned(),
            "/etc/passwd".to_owned(),
            "docs/specs/SPEC-b.md".to_owned(),
            "docs/x/../y.md".to_owned(),
        ];
        paths.extend((0..30).map(|i| format!("docs/specs/SPEC-n{i}.md")));
        notify_spec_review_requested(&workspace, run_id, "rev-1", &paths);

        let updated = store.get(&item.id).unwrap();
        assert_eq!(updated.version, item.version + 1);
        let message = updated.thread.last().expect("thread message");
        assert_eq!(message.from, "spec_review");
        assert!(message.text.contains("rev-1"), "{}", message.text);
        assert!(message.text.contains(run_id), "{}", message.text);

        let events: Vec<_> = store
            .list_events(0)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "spec_review_requested")
            .collect();
        assert_eq!(events.len(), 1, "{events:?}");
        let event = &events[0];
        assert_eq!(event.item_id, item.id);
        assert_eq!(event.version, updated.version);
        assert_eq!(event.by, "spec_review");
        assert_eq!(event.detail.review_id.as_deref(), Some("rev-1"));
        let spec_paths = &event.detail.spec_paths;
        assert_eq!(spec_paths.len(), SPEC_PATHS_MAX);
        assert_eq!(spec_paths[0], "docs/specs/SPEC-a.md");
        assert_eq!(spec_paths[1], "docs/specs/SPEC-b.md");
        assert_eq!(spec_paths[2], "docs/specs/SPEC-n0.md");
        assert!(
            spec_paths
                .iter()
                .all(|p| !p.contains("..") && !p.starts_with('/')),
            "{spec_paths:?}"
        );

        // Unknown or invalid run ids leave the board untouched.
        let events_before = std::fs::read(board_base.join("events.jsonl")).unwrap();
        let item_before = std::fs::read(store.item_path(&item.id).unwrap()).unwrap();
        notify_spec_review_requested(&workspace, "fedcba9876543210", "rev-2", &paths);
        notify_spec_review_requested(&workspace, "not-a-run", "rev-3", &paths);
        assert_eq!(
            std::fs::read(board_base.join("events.jsonl")).unwrap(),
            events_before
        );
        assert_eq!(
            std::fs::read(store.item_path(&item.id).unwrap()).unwrap(),
            item_before
        );
        let log = std::fs::read_to_string(board_base.join("notify.log")).unwrap_or_default();
        assert!(
            log.contains("spec_review_requested"),
            "invalid run id logged: {log}"
        );

        wait_for_pending(Duration::from_secs(5));
        unsafe {
            match prev_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match prev_state {
                Some(v) => std::env::set_var("DMUXAI_STATE_DIR", v),
                None => std::env::remove_var("DMUXAI_STATE_DIR"),
            }
            if let Some(v) = prev_item {
                std::env::set_var(crate::board::BOARD_ITEM_ID_ENV, v);
            }
        }

        let raw = std::fs::read_to_string(&out_file).expect("notify output should exist");
        let payloads: Vec<serde_json::Value> = raw
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l.trim()).ok())
            .filter(|v| v["event"] == "spec_review_requested")
            .collect();
        assert_eq!(payloads.len(), 1, "{raw}");
        assert_eq!(payloads[0]["item_id"], item.id);
        assert_eq!(payloads[0]["by"], "spec_review");
        assert_eq!(payloads[0]["review_id"], "rev-1");
        assert_eq!(
            payloads[0]["spec_paths"].as_array().unwrap().len(),
            SPEC_PATHS_MAX
        );
        assert_eq!(payloads[0]["spec_paths"][0], "docs/specs/SPEC-a.md");
    }

    // TC-3852 / FR-3852: inside a board child run the hook uses the item from
    // `DMUXAI_BOARD_ITEM_ID` while it is `in_progress`, even before the run id
    // is on the item; an invalid id or an item in another lane falls back to
    // `run_ids`.
    #[test]
    fn tc_3852_spec_review_hook_uses_board_item_env() {
        let _env_guard = crate::state_path::test_env_lock();
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        let board_base = state.join("board");
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let empty_xdg = dir.path().join("empty_xdg");
        std::fs::create_dir_all(&empty_xdg).unwrap();
        ensure_dir(&board_base).unwrap();
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let prev_state = std::env::var_os("DMUXAI_STATE_DIR");
        let prev_item = std::env::var_os(crate::board::BOARD_ITEM_ID_ENV);
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &empty_xdg);
            std::env::set_var("DMUXAI_STATE_DIR", &state);
        }

        let run_id = "0123456789abcdef";
        let store = BoardStore::open(board_base.clone()).unwrap();
        let add = |title: &str, lane: Lane| {
            store
                .add(AddItemRequest {
                    title: title.to_owned(),
                    summary: String::new(),
                    kind: Kind::Feature,
                    grade: None,
                    lane: Some(lane),
                    source_bot: None,
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
                .unwrap()
        };
        let paused = add("paused run", Lane::InProgress);
        let parked = add("not running", Lane::NeedsDecision);
        let linked = add("linked run", Lane::InProgress);
        store
            .update(
                &linked.id,
                crate::board::UpdateItemRequest {
                    title: None,
                    summary: None,
                    recommendation: None,
                    options: None,
                    grade: None,
                    kind: None,
                    run_id: Some(run_id.to_owned()),
                    if_version: None,
                    by: "bot:test".to_owned(),
                },
            )
            .unwrap();
        let paths = vec!["docs/specs/SPEC-a.md".to_owned()];
        let call = |env_item: &str, review_id: &str| {
            unsafe { std::env::set_var(crate::board::BOARD_ITEM_ID_ENV, env_item) };
            notify_spec_review_requested(&workspace, run_id, review_id, &paths);
        };
        let reviews_on = |id: &str| -> Vec<String> {
            store
                .list_events(0)
                .unwrap()
                .into_iter()
                .filter(|e| e.kind == "spec_review_requested" && e.item_id == id)
                .filter_map(|e| e.detail.review_id)
                .collect()
        };

        // The env item wins over the item that holds the run id.
        call(&paused.id, "rev-env");
        // Not in_progress, invalid or empty → fall back to run_ids.
        call(&parked.id, "rev-parked");
        call("../not-an-id", "rev-invalid");
        call("", "rev-empty");

        unsafe {
            match prev_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match prev_state {
                Some(v) => std::env::set_var("DMUXAI_STATE_DIR", v),
                None => std::env::remove_var("DMUXAI_STATE_DIR"),
            }
            match prev_item {
                Some(v) => std::env::set_var(crate::board::BOARD_ITEM_ID_ENV, v),
                None => std::env::remove_var(crate::board::BOARD_ITEM_ID_ENV),
            }
        }

        assert_eq!(reviews_on(&paused.id), vec!["rev-env"]);
        let paused_after = store.get(&paused.id).unwrap();
        let message = paused_after.thread.last().expect("thread message");
        assert_eq!(message.from, "spec_review");
        assert!(message.text.contains("rev-env"), "{}", message.text);
        assert!(reviews_on(&parked.id).is_empty());
        assert_eq!(store.get(&parked.id).unwrap().version, parked.version);
        assert_eq!(
            reviews_on(&linked.id),
            vec!["rev-parked", "rev-invalid", "rev-empty"]
        );
    }
}
