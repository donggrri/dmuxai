//! Reuse client for attaching a workspace to a live web dashboard (SPEC-web-default-mode FR-2972..FR-2973, NFR-2970).

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

use crate::web::discovery::{UserDiscovery, process_alive, read_user_discovery};
use crate::web::snapshot::SNAPSHOT_SCHEMA;

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const ATTACH_TIMEOUT: Duration = Duration::from_secs(10);

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
    let Some(discovery) = read_user_discovery() else {
        return AttachOutcome::NoLiveDashboard;
    };
    try_attach_with_discovery(&discovery, workspace_root)
}

pub(crate) fn try_attach_with_discovery(
    discovery: &UserDiscovery,
    workspace_root: &Path,
) -> AttachOutcome {
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

    if !probe_live_state(addr, &host_header, token) {
        return AttachOutcome::NoLiveDashboard;
    }

    post_attach(addr, &host_header, discovery, token, workspace_root)
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

fn probe_live_state(addr: SocketAddr, host_header: &str, token: Option<&str>) -> bool {
    let mut req =
        format!("GET /api/state HTTP/1.1\r\nHost: {host_header}\r\nConnection: close\r\n");
    if let Some(tok) = token {
        req.push_str(&format!("Authorization: Bearer {tok}\r\n"));
    }
    req.push_str("\r\n");

    let Ok((status, body)) = send_http_request(addr, req.as_bytes(), PROBE_TIMEOUT) else {
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

fn post_attach(
    addr: SocketAddr,
    host_header: &str,
    discovery: &UserDiscovery,
    token: Option<&str>,
    workspace_root: &Path,
) -> AttachOutcome {
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

    let Ok((status, body)) = send_http_request(addr, req.as_bytes(), ATTACH_TIMEOUT) else {
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

    if code == Some("custom_agents_not_supported") || status == 422 {
        return AttachOutcome::CustomAgentsNotSupported;
    }

    if code == Some("full") || status == 409 {
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

fn send_http_request(
    addr: SocketAddr,
    request_bytes: &[u8],
    timeout: Duration,
) -> std::io::Result<(u16, Vec<u8>)> {
    let mut stream = TcpStream::connect_timeout(&addr, PROBE_TIMEOUT)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(request_bytes)?;
    stream.flush()?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;

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
    use std::net::TcpListener;

    // TC-2978 / NFR-2970: missing user discovery file skips any network probe.
    #[test]
    fn tc_2978_no_discovery_file_skips_reuse_probe() {
        let _guard = crate::state_path::test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let prev_state = std::env::var_os("DMUXAI_STATE_DIR");
        // SAFETY: serialized by test_env_lock().
        unsafe { std::env::set_var("DMUXAI_STATE_DIR", tmp.path()) };

        let outcome = try_attach_to_live_dashboard(tmp.path());
        assert_eq!(outcome, AttachOutcome::NoLiveDashboard);

        // SAFETY: serialized by test_env_lock().
        unsafe {
            match prev_state {
                Some(v) => std::env::set_var("DMUXAI_STATE_DIR", v),
                None => std::env::remove_var("DMUXAI_STATE_DIR"),
            }
        }
    }

    // TC-2973 / FR-2973: view-only (`mode = "serve"`), stale pid, or unreachable server is ignored.
    #[test]
    fn tc_2973_full_view_only_or_stale_dashboard_handling() {
        let tmp = tempfile::tempdir().unwrap();
        let uid = crate::web::effective_server_uid();
        let pid = std::process::id();

        // 1. mode = "serve" is never reused.
        let serve_disc = UserDiscovery {
            base_url: "http://127.0.0.1:1".to_owned(),
            windows_url: None,
            port: 1,
            pid,
            uid,
            mode: "serve".to_owned(),
            token_required: false,
            token: None,
            workspaces: Vec::new(),
            started_at_ms: 0,
        };
        assert_eq!(
            try_attach_with_discovery(&serve_disc, tmp.path()),
            AttachOutcome::NoLiveDashboard
        );

        // 2. Stale pid (u32::MAX / dead pid) is never reused.
        let stale_disc = UserDiscovery {
            mode: "web".to_owned(),
            pid: 999_999_999,
            ..serve_disc.clone()
        };
        assert_eq!(
            try_attach_with_discovery(&stale_disc, tmp.path()),
            AttachOutcome::NoLiveDashboard
        );

        // 3. Other uid is never reused.
        let other_uid_disc = UserDiscovery {
            mode: "web".to_owned(),
            uid: uid.wrapping_add(1),
            ..serve_disc.clone()
        };
        assert_eq!(
            try_attach_with_discovery(&other_uid_disc, tmp.path()),
            AttachOutcome::NoLiveDashboard
        );

        // 4. Full response (409 / code = "full") returns AttachOutcome::Full.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            for idx in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let body = if idx == 0 {
                    format!(r#"{{"schema":"{SNAPSHOT_SCHEMA}"}}"#)
                } else {
                    r#"{"error":"too many open folders (max 8); close one first","code":"full"}"#
                        .to_owned()
                };
                let status = if idx == 0 { "200 OK" } else { "409 Conflict" };
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(resp.as_bytes()).unwrap();
            }
        });

        let full_disc = UserDiscovery {
            base_url: format!("http://127.0.0.1:{port}"),
            windows_url: None,
            port,
            pid,
            uid,
            mode: "web".to_owned(),
            token_required: false,
            token: None,
            workspaces: Vec::new(),
            started_at_ms: 0,
        };
        assert_eq!(
            try_attach_with_discovery(&full_disc, tmp.path()),
            AttachOutcome::Full {
                message: "too many open folders (max 8); close one first".to_owned()
            }
        );
        handle.join().unwrap();
    }
}
