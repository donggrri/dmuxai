//! Decision board event notifications (SPEC-decision-board FR-3135, NFR-3130).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    let _ = std::thread::Builder::new()
        .name("board-notify".to_owned())
        .spawn(move || {
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
    use crate::board::{BoardPlan, Kind, Lane};
    use tempfile::tempdir;

    #[test]
    fn tc_3135_board_notify_command_payload_masked_and_non_blocking() {
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        let board_base = state.join("board");
        ensure_dir(&board_base).unwrap();
        let out_file = dir.path().join("hook-out.json");
        let script = dir.path().join("hook.sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\ncat > \"{}\"\n", out_file.display()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
        }

        let store = BoardStore::open(board_base.clone()).unwrap();
        let mut item = store
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
        item.plan = Some(BoardPlan {
            hash: "0123456789abcdef".to_owned(),
            prompt: "SENSITIVE PROMPT BODY".to_owned(),
            tasks: vec![],
            steps: vec![],
            revision: 1,
            history: vec![],
            previous_steps: None,
        });

        let settings = BoardSettings {
            notify_command: Some(vec![script.to_string_lossy().into_owned()]),
            ..Default::default()
        };
        dispatch_board_event_with_settings(
            &board_base,
            &item,
            "needs_decision",
            "bot:test",
            &settings,
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !out_file.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let raw = std::fs::read_to_string(&out_file).expect("hook output should exist");
        let json: serde_json::Value = serde_json::from_str(raw.trim()).unwrap();
        assert_eq!(json["schema"], BOARD_EVENT_SCHEMA);
        assert_eq!(json["event"], "needs_decision");
        assert_eq!(json["item_id"], item.id);
        assert_eq!(json["lane"], "needs_decision");
        assert_eq!(json["kind"], "debug");
        assert_eq!(json["plan_hash"], "0123456789abcdef");
        assert!(!raw.contains("sk-secret-token-12345"));
        assert!(!raw.contains("SENSITIVE PROMPT BODY"));
    }

    #[test]
    fn tc_3136_no_notifier_without_board_config() {
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        let board_base = state.join("board");
        ensure_dir(&board_base).unwrap();
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
        let before = BOARD_NOTIFY_THREAD_SPAWNS.load(Ordering::Relaxed);
        dispatch_board_event_with_settings(
            &board_base,
            &item,
            "needs_decision",
            "bot:test",
            &BoardSettings::default(),
        );
        let after = BOARD_NOTIFY_THREAD_SPAWNS.load(Ordering::Relaxed);
        assert_eq!(before, after, "no notifier thread should spawn");
    }
}
