//! Decision board event notifications (SPEC-decision-board FR-3135, NFR-3130).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::BoardItem;
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
    pub workspace_path: Option<String>,
    pub by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_hash: Option<String>,
    pub at_ms: u64,
}

impl BoardNotifyEvent {
    pub fn from_item(item: &BoardItem, event: &str, by: &str, at_ms: u64) -> Self {
        Self {
            schema: BOARD_EVENT_SCHEMA,
            event: event.to_owned(),
            item_id: item.id.clone(),
            title: mask_sensitive_tokens(&item.title),
            lane: item.lane.as_str().to_owned(),
            kind: item.kind.as_str().to_owned(),
            workspace_path: item.workspace.path.as_deref().map(mask_sensitive_tokens),
            by: mask_sensitive_tokens(by),
            plan_hash: item.plan.as_ref().map(|p| p.hash.clone()),
            at_ms,
        }
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
    let body = match serde_json::to_string(&payload) {
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
}
