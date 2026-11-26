//! Reuse client for attaching a workspace to a live web dashboard (SPEC-web-default-mode FR-2972..FR-2973, NFR-2970).

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::web::discovery::{UserDiscovery, process_alive, read_user_discovery};
use crate::web::snapshot::SNAPSHOT_SCHEMA;

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const ATTACH_TIMEOUT: Duration = Duration::from_secs(65);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// Outcome of probing for and attaching to a live web dashboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachOutcome {
    /// Attached to an existing live dashboard; print `url` and exit 0.
    Attached { url: String },
    /// The live dashboard is at its workspace limit; exit 4 with `message`.
    Full { message: String },
    /// The workspace defines repo custom agents and cannot be hosted in-process (FR-2978).
    CustomAgentsNotSupported,
    /// No live web dashboard is available (missing, stale, serve, other uid, unreachable).
    NoLiveDashboard,
}

/// Probe for a live web dashboard and attach `workspace_root` in-process if possible.
///
/// Makes zero network calls when no valid user-level discovery file exists (NFR-2970).
pub fn try_attach_to_live_dashboard(workspace_root: &Path) -> AttachOutcome {
    try_attach_to_live_dashboard_with_connector(workspace_root, |addr, timeout| {
        TcpStream::connect_timeout(&addr, timeout)
    })
}

pub(crate) fn try_attach_to_live_dashboard_with_connector<F>(
    workspace_root: &Path,
    connect: F,
) -> AttachOutcome
where
    F: FnMut(SocketAddr, Duration) -> std::io::Result<TcpStream>,
{
    let Some(discovery) = read_user_discovery() else {
        return AttachOutcome::NoLiveDashboard;
    };
    try_attach_with_discovery_and_connector(&discovery, workspace_root, connect)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn try_attach_with_discovery(
    discovery: &UserDiscovery,
    workspace_root: &Path,
) -> AttachOutcome {
    try_attach_with_discovery_and_connector(discovery, workspace_root, |addr, timeout| {
        TcpStream::connect_timeout(&addr, timeout)
    })
}

fn try_attach_with_discovery_and_connector<F>(
    discovery: &UserDiscovery,
    workspace_root: &Path,
    mut connect: F,
) -> AttachOutcome
where
    F: FnMut(SocketAddr, Duration) -> std::io::Result<TcpStream>,
{
    if discovery.mode != "web" {
        return AttachOutcome::NoLiveDashboard;
    }
    if discovery.uid != crate::web::effective_server_uid() {
        return AttachOutcome::NoLiveDashboard;
    }
    if !process_alive(discovery.pid) {
        return AttachOutcome::NoLiveDashboard;
    }
    let Some((addr, host_header)) = parse_loopback_target(&discovery.base_url, discovery.port)
    else {
        return AttachOutcome::NoLiveDashboard;
    };

    let token = if discovery.token_required {
        discovery.token.as_deref()
    } else {
        None
    };
    if discovery.token_required && token.is_none() {
        return AttachOutcome::NoLiveDashboard;
    }

    if !probe_live_state(addr, &host_header, token, &mut connect) {
        return AttachOutcome::NoLiveDashboard;
    }

    post_attach(
        addr,
        &host_header,
        discovery,
        token,
        workspace_root,
        &mut connect,
    )
}

fn parse_loopback_target(base_url: &str, fallback_port: u16) -> Option<(SocketAddr, String)> {
    let rest = base_url
        .trim()
        .strip_prefix("http://")?
        .trim_end_matches('/');
    let host_port = rest.split('/').next()?;
    if host_port.is_empty() {
        return None;
    }
    if let Ok(addr) = host_port.parse::<SocketAddr>() {
        if !addr.ip().is_loopback() {
            return None;
        }
        return Some((addr, host_port.to_owned()));
    }
    let port_target = if host_port.contains(':') {
        host_port.to_owned()
    } else {
        format!("{host_port}:{fallback_port}")
    };
    let addr = port_target
        .to_socket_addrs()
        .ok()?
        .find(|a| a.ip().is_loopback())?;
    Some((addr, host_port.to_owned()))
}

fn probe_live_state<F>(
    addr: SocketAddr,
    host_header: &str,
    token: Option<&str>,
    connect: &mut F,
) -> bool
where
    F: FnMut(SocketAddr, Duration) -> std::io::Result<TcpStream>,
{
    let mut req =
        format!("GET /api/state HTTP/1.1\r\nHost: {host_header}\r\nConnection: close\r\n");
    if let Some(tok) = token {
        req.push_str(&format!("Authorization: Bearer {tok}\r\n"));
    }
    req.push_str("\r\n");

    let Ok((status, body)) = send_http_request(addr, req.as_bytes(), PROBE_TIMEOUT, connect) else {
        return false;
    };
    if status != 200 {
        return false;
    }
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return false;
    };
    json.get("schema").and_then(|v| v.as_str()) == Some(SNAPSHOT_SCHEMA)
}

fn post_attach<F>(
    addr: SocketAddr,
    host_header: &str,
    discovery: &UserDiscovery,
    token: Option<&str>,
    workspace_root: &Path,
    connect: &mut F,
) -> AttachOutcome
where
    F: FnMut(SocketAddr, Duration) -> std::io::Result<TcpStream>,
{
    let payload = serde_json::json!({
        "path": workspace_root.to_string_lossy(),
    })
    .to_string();

    let mut req = format!(
        "POST /api/workspace/attach HTTP/1.1\r\nHost: {host_header}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        payload.len()
    );
    if let Some(tok) = token {
        req.push_str(&format!("Authorization: Bearer {tok}\r\n"));
    } else {
        req.push_str("X-Dmuxai-Client: 1\r\n");
    }
    req.push_str("\r\n");
    req.push_str(&payload);

    let Ok((status, body)) = send_http_request(addr, req.as_bytes(), ATTACH_TIMEOUT, connect)
    else {
        return AttachOutcome::NoLiveDashboard;
    };
    let json = serde_json::from_slice::<serde_json::Value>(&body).ok();
    let code = json
        .as_ref()
        .and_then(|v| v.get("code"))
        .and_then(|v| v.as_str());

    if status == 200
        && let Some(id) = json
            .as_ref()
            .and_then(|v| v.get("id"))
            .and_then(|v| v.as_str())
    {
        let base = discovery.base_url.trim_end_matches('/');
        let url = match token {
            Some(tok) => format!("{base}/?ws={id}&token={tok}"),
            None => format!("{base}/?ws={id}"),
        };
        return AttachOutcome::Attached { url };
    }

    if code == Some("custom_agents_not_supported") {
        return AttachOutcome::CustomAgentsNotSupported;
    }

    if code == Some("full") {
        let message = json
            .as_ref()
            .and_then(|v| v.get("error"))
            .and_then(|v| v.as_str())
            .unwrap_or("too many open folders; close one in the dashboard first")
            .to_owned();
        return AttachOutcome::Full { message };
    }

    AttachOutcome::NoLiveDashboard
}

fn send_http_request<F>(
    addr: SocketAddr,
    request_bytes: &[u8],
    timeout: Duration,
    connect: &mut F,
) -> std::io::Result<(u16, Vec<u8>)>
where
    F: FnMut(SocketAddr, Duration) -> std::io::Result<TcpStream>,
{
    let deadline = Instant::now() + timeout;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "request timed out",
        ));
    }
    let mut stream = connect(addr, remaining.min(PROBE_TIMEOUT))?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "request timed out",
        ));
    }
    stream.set_read_timeout(Some(remaining))?;
    stream.set_write_timeout(Some(remaining))?;
    stream.write_all(request_bytes)?;
    stream.flush()?;

    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "request timed out",
            ));
        }
        stream.set_read_timeout(Some(remaining))?;
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if raw.len().saturating_add(n) > MAX_RESPONSE_BYTES {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "response too large",
                    ));
                }
                raw.extend_from_slice(&buf[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }

    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "missing headers"))?;
    let head = std::str::from_utf8(&raw[..split])
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid headers"))?;
    let mut lines = head.lines();
    let status_line = lines
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "empty status"))?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad status"))?;

    let is_chunked = lines.any(|line| {
        line.split_once(':').is_some_and(|(k, v)| {
            k.trim().eq_ignore_ascii_case("transfer-encoding")
                && v.to_ascii_lowercase().contains("chunked")
        })
    });
    let body_slice = &raw[split + 4..];
    let body = if is_chunked {
        decode_chunked(body_slice)
    } else {
        body_slice.to_vec()
    };
    Ok((status, body))
}

fn decode_chunked(mut input: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(pos) = input.windows(2).position(|w| w == b"\r\n") {
        let Ok(size_str) = std::str::from_utf8(&input[..pos]) else {
            break;
        };
        let size_hex = size_str.split(';').next().unwrap_or("").trim();
        let Ok(size) = usize::from_str_radix(size_hex, 16) else {
            break;
        };
        if size == 0 {
            break;
        }
        let data_start = pos + 2;
        let Some(data_end) = data_start.checked_add(size) else {
            break;
        };
        if data_end > input.len() {
            break;
        }
        out.extend_from_slice(&input[data_start..data_end]);
        let next = data_end.saturating_add(2);
        if next > input.len() {
            break;
        }
        input = &input[next..];
    }
    out
}

/// Returns true when `ip` is loopback (helper for tests).
#[allow(dead_code)]
fn is_loopback_ip(ip: IpAddr) -> bool {
    ip.is_loopback()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct EnvVarGuard {
        key: &'static str,
        prev: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &std::ffi::OsStr) -> Self {
            let prev = std::env::var_os(key);
            // SAFETY: callers hold `crate::state_path::test_env_lock()`.
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: callers hold `crate::state_path::test_env_lock()`.
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    fn test_discovery(port: u16) -> UserDiscovery {
        UserDiscovery {
            base_url: format!("http://127.0.0.1:{port}"),
            windows_url: None,
            port,
            pid: std::process::id(),
            uid: crate::web::effective_server_uid(),
            mode: "web".to_owned(),
            token_required: false,
            token: None,
            workspaces: Vec::new(),
            started_at_ms: 0,
        }
    }

    fn spawn_two_step_mock(
        second_status: &'static str,
        second_body: &'static str,
    ) -> (u16, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            for idx in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let (status, body) = if idx == 0 {
                    ("200 OK", format!(r#"{{"schema":"{SNAPSHOT_SCHEMA}"}}"#))
                } else {
                    (second_status, second_body.to_owned())
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        (port, handle)
    }

    // TC-2978 / NFR-2970: missing user discovery file skips any network probe.
    #[test]
    fn tc_2978_no_discovery_file_skips_reuse_probe() {
        let _lock = crate::state_path::test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let _env_guard = EnvVarGuard::set("DMUXAI_STATE_DIR", tmp.path().as_os_str());

        let connect_calls = AtomicUsize::new(0);
        let outcome = try_attach_to_live_dashboard_with_connector(tmp.path(), |addr, timeout| {
            connect_calls.fetch_add(1, Ordering::SeqCst);
            TcpStream::connect_timeout(&addr, timeout)
        });
        assert_eq!(outcome, AttachOutcome::NoLiveDashboard);
        assert_eq!(
            connect_calls.load(Ordering::SeqCst),
            0,
            "must make zero connection attempts when discovery file is missing"
        );
    }

    // TC-2973 / FR-2973: view-only (`mode = "serve"`), stale pid, unreachable server,
    // explicit error code mapping, 2s probe deadline, and 1 MiB response cap.
    #[test]
    fn tc_2973_full_view_only_or_stale_dashboard_handling() {
        let tmp = tempfile::tempdir().unwrap();
        let uid = crate::web::effective_server_uid();

        // 1. mode = "serve" is never reused.
        let serve_disc = UserDiscovery {
            mode: "serve".to_owned(),
            ..test_discovery(1)
        };
        assert_eq!(
            try_attach_with_discovery(&serve_disc, tmp.path()),
            AttachOutcome::NoLiveDashboard
        );

        // 2. Stale pid (dead pid) is never reused.
        let stale_disc = UserDiscovery {
            pid: 999_999_999,
            ..test_discovery(1)
        };
        assert_eq!(
            try_attach_with_discovery(&stale_disc, tmp.path()),
            AttachOutcome::NoLiveDashboard
        );

        // 3. Other uid is never reused.
        let other_uid_disc = UserDiscovery {
            uid: uid.wrapping_add(1),
            ..test_discovery(1)
        };
        assert_eq!(
            try_attach_with_discovery(&other_uid_disc, tmp.path()),
            AttachOutcome::NoLiveDashboard
        );

        // 4. Unreachable /api/state (port closed) returns NoLiveDashboard.
        let dead_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let dead_port = dead_listener.local_addr().unwrap().port();
        drop(dead_listener);
        assert_eq!(
            try_attach_with_discovery(&test_discovery(dead_port), tmp.path()),
            AttachOutcome::NoLiveDashboard
        );

        // 5. Full response (409 + code = "full") returns AttachOutcome::Full.
        let (port, handle) = spawn_two_step_mock(
            "409 Conflict",
            r#"{"error":"too many open folders (max 8); close one first","code":"full"}"#,
        );
        assert_eq!(
            try_attach_with_discovery(&test_discovery(port), tmp.path()),
            AttachOutcome::Full {
                message: "too many open folders (max 8); close one first".to_owned()
            }
        );
        handle.join().unwrap();

        // 6. 409 without code = "full" (e.g. shutting down host) falls back to NoLiveDashboard.
        let (port, handle) =
            spawn_two_step_mock("409 Conflict", r#"{"error":"requires dmuxai --web"}"#);
        assert_eq!(
            try_attach_with_discovery(&test_discovery(port), tmp.path()),
            AttachOutcome::NoLiveDashboard
        );
        handle.join().unwrap();

        // 7. 422 with code = "custom_agents_not_supported" -> CustomAgentsNotSupported,
        //    422 without that code -> NoLiveDashboard.
        let (port, handle) = spawn_two_step_mock(
            "422 Unprocessable Entity",
            r#"{"error":"custom agents","code":"custom_agents_not_supported"}"#,
        );
        assert_eq!(
            try_attach_with_discovery(&test_discovery(port), tmp.path()),
            AttachOutcome::CustomAgentsNotSupported
        );
        handle.join().unwrap();

        let (port, handle) =
            spawn_two_step_mock("422 Unprocessable Entity", r#"{"error":"other"}"#);
        assert_eq!(
            try_attach_with_discovery(&test_discovery(port), tmp.path()),
            AttachOutcome::NoLiveDashboard
        );
        handle.join().unwrap();

        // 8. Trickling /api/state exceeds the 2 s overall probe deadline -> NoLiveDashboard.
        let slow_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let slow_port = slow_listener.local_addr().unwrap().port();
        let slow_handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = slow_listener.accept() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n");
                for _ in 0..8 {
                    std::thread::sleep(Duration::from_millis(400));
                    if stream.write_all(b" ").is_err() {
                        break;
                    }
                }
            }
        });
        let started = Instant::now();
        assert_eq!(
            try_attach_with_discovery(&test_discovery(slow_port), tmp.path()),
            AttachOutcome::NoLiveDashboard
        );
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "probe must respect 2s overall deadline, took {:?}",
            started.elapsed()
        );
        slow_handle.join().unwrap();

        // 9. Oversized response (> 1 MiB) is rejected -> NoLiveDashboard.
        let big_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let big_port = big_listener.local_addr().unwrap().port();
        let big_handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = big_listener.accept() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let body_len = MAX_RESPONSE_BYTES + 1024;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {body_len}\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(head.as_bytes());
                let chunk = vec![b'a'; 65536];
                for _ in 0..(body_len / chunk.len() + 1) {
                    if stream.write_all(&chunk).is_err() {
                        break;
                    }
                }
            }
        });
        assert_eq!(
            try_attach_with_discovery(&test_discovery(big_port), tmp.path()),
            AttachOutcome::NoLiveDashboard
        );
        big_handle.join().unwrap();
    }
}
