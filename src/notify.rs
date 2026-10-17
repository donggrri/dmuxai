//! State-change delivery: webhook POST and command hook
//! (SPEC-agent-api FR-770, FR-771, FR-772).
//!
//! `Notifier::spawn` builds the delivery configuration once and runs a single
//! background task that consumes a bounded queue, so `notify` never blocks the
//! caller (UI thread, runner or event log writer). Failures are appended to
//! `<workspace>/.dmuxai/notify.log`; webhook secrets and URL query strings
//! never reach the log.

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use crate::events::{EventRecord, format_ts};

/// Bounded queue between `notify()` and the background delivery task (FR-770).
const QUEUE_CAPACITY: usize = 256;
/// Default per-delivery timeout when `timeout_secs` is not configured.
const DEFAULT_TIMEOUT_SECS: u64 = 5;
/// Header used for the webhook secret unless `secret_header` overrides it.
const DEFAULT_SECRET_HEADER: &str = "X-Dmuxai-Secret";
/// Default stall threshold surfaced via [`NotifySettings::stall_after`].
const DEFAULT_STALL_AFTER_SECS: u64 = 600;

/// Valid `to_state` names for the `[notify] events` filter (FR-772).
const VALID_EVENT_STATES: &[&str] = &[
    "pending",
    "started",
    "running",
    "completed",
    "error",
    "cancelled",
    "stalled",
    "ready",
    "waiting_gate",
    "failed",
    "skipped",
    "active",
    "open",
];

/// `[notify]` table in dmuxai.toml (FR-770..FR-772).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct NotifySettings {
    pub webhook_url: Option<String>,
    pub webhook_secret_env: Option<String>,
    pub secret_header: Option<String>,
    pub timeout_secs: Option<u64>,
    pub command: Option<Vec<String>>,
    pub events: Option<Vec<String>>,
    pub stall_after_secs: Option<u64>,
}

impl NotifySettings {
    /// Busy-runner stall threshold; `0` disables the detector (OQ-762).
    pub fn stall_after(&self) -> Duration {
        Duration::from_secs(self.stall_after_secs.unwrap_or(DEFAULT_STALL_AFTER_SECS))
    }

    /// Config problems that should be surfaced as startup warnings.
    pub fn validate(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Some(url) = &self.webhook_url
            && !url.starts_with("http://")
            && !url.starts_with("https://")
        {
            warnings.push("webhook_url must start with http:// or https://".to_owned());
        }
        if self
            .command
            .as_ref()
            .is_some_and(|command| command.is_empty())
        {
            warnings.push("command must not be empty".to_owned());
        }
        if let Some(events) = &self.events {
            for name in events {
                if !VALID_EVENT_STATES.contains(&name.as_str()) {
                    warnings.push(format!(
                        "events entry '{name}' is not a known state (valid: {})",
                        VALID_EVENT_STATES.join(", ")
                    ));
                }
            }
        }
        if self.timeout_secs == Some(0) {
            warnings.push("timeout_secs must be greater than 0".to_owned());
        }
        warnings
    }
}

/// Event delivery handle. Cloning shares the same background task.
#[derive(Clone)]
pub struct Notifier {
    tx: mpsc::Sender<EventRecord>,
    /// `events` filter: only these `to_state` values are delivered (FR-772).
    allowed_states: Option<Vec<String>>,
    /// Events dropped because the delivery queue was full (logged by the worker).
    queue_full_drops: Arc<AtomicU64>,
}

impl Notifier {
    /// Build a notifier from `[notify]` settings and start its background
    /// delivery task. Returns `None` when nothing is configured (neither
    /// `webhook_url` nor `command`) or when no tokio runtime is running.
    pub fn spawn(settings: &NotifySettings, workspace_root: &Path) -> Option<Self> {
        Self::spawn_with(settings, workspace_root, |name| std::env::var(name).ok())
    }

    fn spawn_with<F>(
        settings: &NotifySettings,
        workspace_root: &Path,
        secret_lookup: F,
    ) -> Option<Self>
    where
        F: Fn(&str) -> Option<String>,
    {
        if settings.webhook_url.is_none() && settings.command.is_none() {
            return None;
        }
        // Delivery is async; without a runtime there is nothing to attach to.
        if tokio::runtime::Handle::try_current().is_err() {
            return None;
        }

        // reqwest is built without a bundled TLS provider; install the ring
        // one. `install_default` fails when a provider is already installed,
        // which is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();

        // `timeout_secs = 0` is a config error (validate warns); clamp it so a
        // zero duration cannot turn into an instant or unlimited timeout.
        let timeout =
            Duration::from_secs(settings.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS).max(1));
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .build()
            .ok()?;

        // Secrets never live in dmuxai.toml (OQ-761): read the env var named
        // by `webhook_secret_env` once, and only deliver it when non-empty.
        let secret = settings
            .webhook_secret_env
            .as_deref()
            .and_then(secret_lookup)
            .filter(|value| !value.is_empty());
        let secret_header = settings
            .secret_header
            .clone()
            .unwrap_or_else(|| DEFAULT_SECRET_HEADER.to_owned());

        let (tx, mut rx) = mpsc::channel::<EventRecord>(QUEUE_CAPACITY);
        let log_path = workspace_root.join(".dmuxai").join("notify.log");
        let task_log_path = log_path.clone();
        let webhook_url = settings.webhook_url.clone();
        let command = settings.command.clone();
        let workspace_root = workspace_root.to_path_buf();
        let queue_full_drops = Arc::new(AtomicU64::new(0));
        let task_drops = Arc::clone(&queue_full_drops);

        tokio::spawn(async move {
            // One event at a time, in arrival order (FR-770: sequential).
            while let Some(record) = rx.recv().await {
                flush_queue_full_drops(&task_drops, &task_log_path);
                if let Some(url) = &webhook_url {
                    deliver_webhook(
                        &client,
                        url,
                        &secret_header,
                        secret.as_deref(),
                        &record,
                        timeout,
                        &task_log_path,
                    )
                    .await;
                }
                if let Some(command) = &command {
                    deliver_command(command, &workspace_root, &record, timeout, &task_log_path)
                        .await;
                }
            }
            flush_queue_full_drops(&task_drops, &task_log_path);
        });

        Some(Self {
            tx,
            allowed_states: settings.events.clone(),
            queue_full_drops,
        })
    }

    /// Queue one event for background delivery. Never blocks: when the queue
    /// is full the event is dropped and the drop is logged (FR-770).
    pub fn notify(&self, record: &EventRecord) {
        // FR-772: with `events` configured, only listed `to_state` values are
        // delivered (the event log itself keeps everything).
        if let Some(allowed) = &self.allowed_states
            && !allowed.contains(&record.to_state)
        {
            return;
        }
        match self.tx.try_send(record.clone()) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.queue_full_drops.fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }
}

fn flush_queue_full_drops(drops: &AtomicU64, log_path: &Path) {
    let count = drops.swap(0, Ordering::Relaxed);
    if count == 0 {
        return;
    }
    let ts = format_ts(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default(),
    );
    append_log(
        log_path,
        &format!("{ts} dropped {count} events (queue full)"),
    );
}

async fn deliver_webhook(
    client: &reqwest::Client,
    url: &str,
    secret_header: &str,
    secret: Option<&str>,
    record: &EventRecord,
    timeout: Duration,
    log_path: &Path,
) {
    let body = match serde_json::to_string(record) {
        Ok(body) => body,
        Err(error) => {
            append_log(log_path, &format!("{} webhook {error}", record.ts));
            return;
        }
    };
    let mut request = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body);
    if let Some(secret) = secret {
        request = request.header(secret_header, secret);
    }
    // The client already carries the timeout; belt-and-braces in case a
    // future builder change drops it.
    let request = request.timeout(timeout);
    match request.send().await {
        Ok(response) => {
            // Non-2xx (including 3xx: redirects are never followed) is a
            // delivery failure, logged without the secret or the URL.
            if !response.status().is_success() {
                append_log(
                    log_path,
                    &format!("{} webhook {}", record.ts, response.status()),
                );
            }
        }
        Err(error) => {
            append_log(
                log_path,
                &format!("{} webhook {}", record.ts, describe_error(&error)),
            );
        }
    }
}

/// Classify a reqwest error without echoing the URL (which can carry a query
/// string) or any header value.
fn describe_error(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect error"
    } else if error.is_request() {
        "request error"
    } else {
        "delivery error"
    }
}

async fn deliver_command(
    command: &[String],
    workspace_root: &Path,
    record: &EventRecord,
    timeout: Duration,
    log_path: &Path,
) {
    let Some(program) = command.first() else {
        return;
    };
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(&command[1..])
        .current_dir(workspace_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            append_log(log_path, &format!("{} command {error}", record.ts));
            return;
        }
    };

    let mut stdin = child.stdin.take();
    let payload = match serde_json::to_string(record) {
        Ok(payload) => payload,
        Err(error) => {
            append_log(log_path, &format!("{} command {error}", record.ts));
            return;
        }
    };
    let waited = tokio::time::timeout(timeout, async move {
        if let Some(pipe) = stdin.as_mut() {
            // FR-771: the event JSON plus a trailing newline on stdin.
            let _ = pipe.write_all(payload.as_bytes()).await;
            let _ = pipe.write_all(b"\n").await;
            let _ = pipe.flush().await;
        }
        drop(stdin); // close the pipe so the hook sees EOF
        child.wait().await
    })
    .await;
    match waited {
        Ok(Ok(status)) if status.success() => {}
        Ok(Ok(status)) => append_log(log_path, &format!("{} command failed: {status}", record.ts)),
        Ok(Err(error)) => append_log(log_path, &format!("{} command {error}", record.ts)),
        Err(_) => {
            // Dropping the child (kill_on_drop) kills the hook process.
            append_log(log_path, &format!("{} command timeout", record.ts));
        }
    }
}

/// Append one line to `<workspace>/.dmuxai/notify.log`, creating the
/// directory if needed. Failures are swallowed: logging never panics.
fn append_log(log_path: &Path, line: &str) {
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        use std::io::Write;
        let _ = writeln!(file, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::net::TcpListener;

    struct MockRequest {
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl MockRequest {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
        }
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    fn parse_head(buf: &[u8], header_end: usize) -> (String, String, Vec<(String, String)>) {
        let head = String::from_utf8_lossy(&buf[..header_end]);
        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap_or_default();
        let mut headers = Vec::new();
        for line in lines {
            if let Some((name, value)) = line.split_once(':') {
                headers.push((name.trim().to_owned(), value.trim().to_owned()));
            }
        }
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_owned();
        let path = parts.next().unwrap_or_default().to_owned();
        (method, path, headers)
    }

    /// Serve exactly one HTTP request: read head + content-length body, reply
    /// with `response`, and hand the parsed request back to the test.
    async fn serve_one(listener: TcpListener, response: Vec<u8>) -> std::io::Result<MockRequest> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut stream, _) = listener.accept().await?;
        let mut buf: Vec<u8> = Vec::new();
        let header_end = loop {
            if let Some(pos) = find(&buf, b"\r\n\r\n") {
                break pos;
            }
            let mut chunk = [0u8; 4096];
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                break usize::MAX; // peer hung up before the head was complete
            }
            buf.extend_from_slice(&chunk[..read]);
        };
        let (method, path, headers) = if header_end == usize::MAX {
            (String::new(), String::new(), Vec::new())
        } else {
            parse_head(&buf, header_end)
        };
        let content_length = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.parse::<usize>().ok())
            .unwrap_or(0);
        let body = if header_end == usize::MAX {
            Vec::new()
        } else {
            let body_start = header_end + 4;
            while buf.len() < body_start + content_length {
                let mut chunk = [0u8; 4096];
                let read = stream.read(&mut chunk).await?;
                if read == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..read]);
            }
            buf[body_start..].to_vec()
        };
        let _ = stream.write_all(&response).await;
        let _ = stream.shutdown().await;
        Ok(MockRequest {
            method,
            path,
            headers,
            body,
        })
    }

    fn sample_record(to_state: &str) -> EventRecord {
        EventRecord {
            v: 1,
            seq: 7,
            ts: "2026-10-17T11:22:33.456Z".to_owned(),
            ts_ms: 1_792_236_153_456,
            workspace: "/repo".to_owned(),
            kind: "agent".to_owned(),
            task: Some("task-1".to_owned()),
            todo: None,
            node: None,
            runner: Some("agy".to_owned()),
            model: Some("gemini-3.8-flash-high".to_owned()),
            from_state: Some("running".to_owned()),
            to_state: to_state.to_owned(),
            message: None,
        }
    }

    async fn wait_for_log_contains(log_path: &Path, needle: &str) -> bool {
        for _ in 0..100 {
            if let Ok(log) = std::fs::read_to_string(log_path)
                && log.contains(needle)
            {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    #[tokio::test]
    async fn webhook_posts_json_with_secret_no_redirect() {
        // TC-772 / FR-770, FR-772, NFR-761: local mock listener, no network.
        let workspace = tempfile::tempdir().unwrap();
        let log_path = workspace.path().join(".dmuxai").join("notify.log");

        let secret_env = "DMUXAI_TEST_WEBHOOK_SECRET";

        let delivered = sample_record("completed");
        let filtered = sample_record("running");

        // FR-770 happy path: mock answers 200 after reading the full body.
        let ok_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ok_addr = ok_listener.local_addr().unwrap();
        let ok_server = tokio::spawn(serve_one(
            ok_listener,
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec(),
        ));

        let settings = NotifySettings {
            webhook_url: Some(format!("http://{ok_addr}/hook?query=secret-ish")),
            webhook_secret_env: Some(secret_env.to_owned()),
            secret_header: None, // default X-Dmuxai-Secret
            timeout_secs: Some(5),
            command: None,
            events: Some(vec!["completed".to_owned()]), // FR-772 filter
            stall_after_secs: None,
        };
        let notifier = Notifier::spawn_with(&settings, workspace.path(), |name| {
            if name == secret_env {
                Some("test-secret-value".to_owned())
            } else {
                None
            }
        })
        .expect("webhook notifier");

        notifier.notify(&filtered); // not in `events`: must never be delivered
        notifier.notify(&delivered);

        let request = ok_server.await.unwrap().expect("mock served one request");
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/hook?query=secret-ish");
        assert_eq!(request.header("content-type"), Some("application/json"));
        assert_eq!(request.header("x-dmuxai-secret"), Some("test-secret-value"));
        let body: EventRecord =
            serde_json::from_slice(&request.body).expect("body is the event JSON");
        assert_eq!(body, delivered);
        // The filtered `running` event never reached the mock: the only
        // delivered record was the `completed` one (FIFO, sequential).
        assert_eq!(body.to_state, "completed");

        // FR-770: a 302 reply is not followed even with a Location header.
        let redirect_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redirect_addr = redirect_listener.local_addr().unwrap();
        let second_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let second_addr = second_listener.local_addr().unwrap();
        let redirect_response = format!(
            "HTTP/1.1 302 Found\r\nLocation: http://{second_addr}/hook\r\nContent-Length: 0\r\n\r\n"
        )
        .into_bytes();
        let redirect_server = tokio::spawn(serve_one(redirect_listener, redirect_response));
        let second_probe = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_millis(700), second_listener.accept())
                .await
                .is_err() // true when nothing connected
        });

        let redirect_settings = NotifySettings {
            webhook_url: Some(format!("http://{redirect_addr}/hook")),
            timeout_secs: Some(5),
            ..Default::default()
        };
        let redirect_notifier =
            Notifier::spawn(&redirect_settings, workspace.path()).expect("redirect notifier");
        redirect_notifier.notify(&sample_record("completed"));

        let redirect_request = redirect_server
            .await
            .unwrap()
            .expect("redirect mock served one request");
        assert_eq!(redirect_request.path, "/hook");
        assert!(
            second_probe.await.unwrap(),
            "302 Location target must not be contacted"
        );
        assert!(
            wait_for_log_contains(&log_path, "302").await,
            "notify.log must record the 302 webhook failure"
        );

        // FR-770: a server that never replies produces a timeout log line.
        let slow_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let slow_addr = slow_listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (_socket, _) = slow_listener.accept().await.unwrap();
            std::future::pending::<()>().await; // hold the connection, never reply
        });
        let slow_settings = NotifySettings {
            webhook_url: Some(format!("http://{slow_addr}/hook")),
            timeout_secs: Some(1),
            ..Default::default()
        };
        let slow_notifier =
            Notifier::spawn(&slow_settings, workspace.path()).expect("slow notifier");
        slow_notifier.notify(&sample_record("completed"));
        assert!(
            wait_for_log_contains(&log_path, "timeout").await,
            "notify.log must record the webhook timeout"
        );

        // Failures are logged without the secret and without the URL query.
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        assert!(!log.contains("test-secret-value"), "log leaked the secret");
        assert!(
            !log.contains("query=secret-ish"),
            "log leaked the URL query"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_hook_receives_event_on_stdin() {
        // TC-773 / FR-771: the hook runs in the workspace directory and reads
        // the event JSON from stdin.
        let workspace = tempfile::tempdir().unwrap();
        let settings = NotifySettings {
            command: Some(vec![
                "sh".to_owned(),
                "-c".to_owned(),
                "cat > out.json".to_owned(),
            ]),
            ..Default::default()
        };
        let notifier = Notifier::spawn(&settings, workspace.path()).expect("command notifier");

        let record = sample_record("completed");
        notifier.notify(&record);

        let out_path = workspace.path().join("out.json");
        let mut received = None;
        for _ in 0..100 {
            if let Ok(data) = std::fs::read_to_string(&out_path)
                && let Ok(parsed) = serde_json::from_str::<EventRecord>(&data)
            {
                received = Some(parsed);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(received, Some(record), "stdin must carry the event JSON");
    }
}
