//! Host allowlist, CSRF guards, and peer checks (SPEC-web-local-access FR-2952..FR-2955).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use axum::http::header::ORIGIN;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::web::auth::token_matches;
use crate::web::local_peer::{LocalPeerSource, peer_uid_for_connection};
use crate::web::peer_connect::PeerConnectInfo;

pub const CLIENT_HEADER: &str = "x-dmuxai-client";
pub const FETCH_SITE_HEADER: &str = "sec-fetch-site";

/// Effective OS user id for peer-uid checks (FR-2955); skipped on non-Unix builds.
pub fn effective_server_uid() -> u32 {
    #[cfg(unix)]
    {
        unsafe { libc::geteuid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvidedToken {
    Absent,
    Valid,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenerKind {
    Loopback,
    WslInterface,
    Remote,
}

#[derive(Clone)]
pub struct WebAccess {
    pub token_required: bool,
    pub listener: ListenerKind,
    pub allowed_hosts: Arc<Vec<String>>,
    pub wsl_host_peer: Option<IpAddr>,
    pub server_uid: u32,
    pub peer_source: Arc<dyn LocalPeerSource>,
    pub token: Arc<String>,
}

impl WebAccess {
    /// Backward-compatible default for router tests (`build_router`): token always required.
    pub fn legacy_token_required(token: String, allowed_hosts: Option<Vec<String>>) -> Self {
        match allowed_hosts {
            None => Self {
                token_required: true,
                listener: ListenerKind::Remote,
                allowed_hosts: Arc::new(Vec::new()),
                wsl_host_peer: None,
                server_uid: effective_server_uid(),
                peer_source: crate::web::local_peer::shared_peer_source(),
                token: Arc::new(token),
            },
            Some(hosts) => Self {
                token_required: true,
                listener: ListenerKind::Loopback,
                allowed_hosts: Arc::new(hosts),
                wsl_host_peer: None,
                server_uid: effective_server_uid(),
                peer_source: crate::web::local_peer::shared_peer_source(),
                token: Arc::new(token),
            },
        }
    }

    pub fn remote(token: String, peer_source: Arc<dyn LocalPeerSource>) -> Self {
        Self {
            token_required: true,
            listener: ListenerKind::Remote,
            allowed_hosts: Arc::new(Vec::new()),
            wsl_host_peer: None,
            server_uid: effective_server_uid(),
            peer_source,
            token: Arc::new(token),
        }
    }

    pub fn loopback(
        token: String,
        token_required: bool,
        bind_ip: IpAddr,
        port: u16,
        peer_source: Arc<dyn LocalPeerSource>,
    ) -> Self {
        Self {
            token_required,
            listener: ListenerKind::Loopback,
            allowed_hosts: Arc::new(build_allowed_hosts(bind_ip, port)),
            wsl_host_peer: None,
            server_uid: effective_server_uid(),
            peer_source,
            token: Arc::new(token),
        }
    }

    pub fn wsl_interface(
        token: String,
        token_required: bool,
        bind_ip: IpAddr,
        port: u16,
        wsl_host_peer: Option<IpAddr>,
        peer_source: Arc<dyn LocalPeerSource>,
    ) -> Self {
        let mut hosts = build_allowed_hosts(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        hosts.push(host_from_ip(bind_ip, port));
        if port == 80 {
            hosts.push(ip_host_only(bind_ip));
        }
        hosts.sort();
        hosts.dedup();
        Self {
            token_required,
            listener: ListenerKind::WslInterface,
            allowed_hosts: Arc::new(hosts),
            wsl_host_peer,
            server_uid: effective_server_uid(),
            peer_source,
            token: Arc::new(token),
        }
    }
}

pub fn host_from_ip(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(v4) => format!("{v4}:{port}"),
        IpAddr::V6(v6) => format!("[{v6}]:{port}"),
    }
}

/// Allowed `Host` values for a loopback bind on port `port` (FR-2952, FR-2962).
pub fn build_allowed_hosts(bind_ip: IpAddr, port: u16) -> Vec<String> {
    let mut hosts = vec![
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("[::1]:{port}"),
    ];
    if bind_ip != IpAddr::V4(Ipv4Addr::LOCALHOST)
        && bind_ip != IpAddr::V6(Ipv6Addr::LOCALHOST)
        && !bind_ip.is_unspecified()
    {
        hosts.push(host_from_ip(bind_ip, port));
    }
    if port == 80 {
        hosts.push("127.0.0.1".to_owned());
        hosts.push("localhost".to_owned());
        hosts.push("[::1]".to_owned());
        if bind_ip != IpAddr::V4(Ipv4Addr::LOCALHOST)
            && bind_ip != IpAddr::V6(Ipv6Addr::LOCALHOST)
            && !bind_ip.is_unspecified()
        {
            hosts.push(ip_host_only(bind_ip));
        }
    }
    hosts
}

fn ip_host_only(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => format!("[{v6}]"),
    }
}

pub fn host_allowed(access: &WebAccess, host_header: &str) -> bool {
    if access.listener == ListenerKind::Remote {
        return true;
    }
    access.allowed_hosts.iter().any(|h| h == host_header)
}

pub fn normalize_guarded_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.starts_with("//") {
        format!("/{}", trimmed.trim_start_matches('/'))
    } else {
        trimmed.to_owned()
    }
}

pub fn is_guarded_path(path: &str) -> bool {
    let path = normalize_guarded_path(path);
    path.starts_with("/api/") || path == "/api" || path.contains("/api/")
}

pub fn classify_provided_token(
    headers: &HeaderMap,
    query: Option<&str>,
    expected: &str,
) -> ProvidedToken {
    let mut rejected = false;
    if let Some(auth_val) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        let mut parts = auth_val.splitn(2, ' ');
        let scheme = parts.next().unwrap_or_default();
        let tok = parts.next().unwrap_or_default();
        if scheme.eq_ignore_ascii_case("bearer") {
            let tok = tok.trim();
            if tok.is_empty() {
                rejected = true;
            } else if token_matches(expected, tok) {
                return ProvidedToken::Valid;
            } else {
                rejected = true;
            }
        } else {
            rejected = true;
        }
    }
    if let Some(query) = query {
        for pair in query.split('&') {
            if let Some((k, v)) = pair.split_once('=')
                && k == "token"
            {
                if v.is_empty() {
                    rejected = true;
                } else if token_matches(expected, v) {
                    return ProvidedToken::Valid;
                } else {
                    rejected = true;
                }
                break;
            }
        }
    }
    if rejected {
        ProvidedToken::Invalid
    } else {
        ProvidedToken::Absent
    }
}

pub fn tokenless_write_allowed(headers: &HeaderMap, host_header: &str) -> bool {
    let origin = headers.get(ORIGIN).and_then(|v| v.to_str().ok());
    if origin == Some("null") {
        return false;
    }
    let fetch_site = headers.get(FETCH_SITE_HEADER).and_then(|v| v.to_str().ok());
    let client = headers.get(CLIENT_HEADER).and_then(|v| v.to_str().ok());

    if let Some(origin) = origin {
        let expected = format!("http://{host_header}");
        if origin != expected {
            return false;
        }
        if let Some(site) = fetch_site
            && site != "same-origin"
            && site != "none"
        {
            return false;
        }
        return true;
    }

    if client != Some("1") {
        return false;
    }
    match fetch_site {
        None | Some("same-origin") | Some("none") => true,
        Some(_) => false,
    }
}

pub fn tokenless_get_allowed(headers: &HeaderMap) -> bool {
    match headers.get(FETCH_SITE_HEADER).and_then(|v| v.to_str().ok()) {
        None | Some("same-origin") | Some("none") => true,
        Some("cross-site") | Some("same-site") => false,
        Some(_) => false,
    }
}

fn decode_query_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(byte) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(char::from(byte));
            i += 3;
            continue;
        }
        out.push(char::from(bytes[i]));
        i += 1;
    }
    out
}

fn refresh_truthy(value: &str) -> bool {
    matches!(value, "1" | "true" | "yes" | "on")
}

fn jira_refresh_requested(query: Option<&str>) -> bool {
    let Some(query) = query else {
        return false;
    };
    for pair in query.split('&') {
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (pair, ""),
        };
        if k == "refresh" && refresh_truthy(&decode_query_component(v)) {
            return true;
        }
    }
    false
}

pub fn get_request_needs_write_guard(path: &str, query: Option<&str>) -> bool {
    normalize_guarded_path(path) == "/api/jira/issues" && jira_refresh_requested(query)
}

#[allow(clippy::result_large_err)]
fn check_loopback_peer_uid(access: &WebAccess, peer: &PeerConnectInfo) -> Result<(), Response> {
    let mut guard = peer.loopback_uid.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(result) = *guard {
        return result.map_err(|_| forbidden());
    }
    let verdict = if let Some(uid) = peer_uid_for_connection(access.peer_source.as_ref(), peer.addr)
    {
        if uid != access.server_uid && uid != 0 {
            Err(())
        } else {
            Ok(())
        }
    } else {
        Ok(())
    };
    *guard = Some(verdict);
    verdict.map_err(|_| forbidden())
}

pub fn effective_token_required(
    access: &WebAccess,
    peer_ip: Option<IpAddr>,
    has_valid_token: bool,
) -> bool {
    if has_valid_token {
        return false;
    }
    if access.listener == ListenerKind::WslInterface {
        if let (Some(peer), Some(host)) = (peer_ip, access.wsl_host_peer)
            && peer == host
        {
            return access.token_required;
        }
        return true;
    }
    access.token_required
}

pub fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        r#"{"error":"unauthorized"}"#,
    )
        .into_response()
}

pub fn forbidden() -> Response {
    StatusCode::FORBIDDEN.into_response()
}

/// Core middleware decision for guarded paths.
#[allow(clippy::too_many_arguments, clippy::result_large_err)]
pub fn check_guarded_request(
    access: &WebAccess,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    host_header: &str,
    query: Option<&str>,
    peer: Option<&PeerConnectInfo>,
) -> std::result::Result<(), Response> {
    let path = normalize_guarded_path(path);
    if !is_guarded_path(&path) {
        return Ok(());
    }

    let token_state = classify_provided_token(headers, query, access.token.as_str());
    if token_state == ProvidedToken::Invalid {
        return Err(unauthorized());
    }
    let has_valid = token_state == ProvidedToken::Valid;

    let peer_ip = peer.map(|p| p.addr.ip());
    if effective_token_required(access, peer_ip, has_valid) {
        if !has_valid {
            return Err(unauthorized());
        }
        return Ok(());
    }

    if has_valid {
        return Ok(());
    }

    // Tokenless path
    if access.listener == ListenerKind::Loopback
        && let Some(peer_info) = peer
    {
        check_loopback_peer_uid(access, peer_info)?;
    }

    if method == Method::GET || method == Method::HEAD {
        if !tokenless_get_allowed(headers) {
            return Err(forbidden());
        }
        if get_request_needs_write_guard(&path, query)
            && !tokenless_write_allowed(headers, host_header)
        {
            return Err(forbidden());
        }
        return Ok(());
    }

    if tokenless_write_allowed(headers, host_header) {
        return Ok(());
    }
    Err(forbidden())
}

/// FR-2954: GET `/api/*` routes must not perform side effects; list any exceptions.
pub fn get_api_routes_with_possible_side_effects() -> Vec<&'static str> {
    vec!["GET /api/jira/issues?refresh=1"]
}

/// Every `GET` route registered under `/api/*` on the dashboard router (for FR-2954 tests).
pub fn all_api_get_routes() -> Vec<&'static str> {
    vec![
        "/api/state",
        "/api/workspaces",
        "/api/workspace",
        "/api/workspace/dirs",
        "/api/skills",
        "/api/agent-prompts",
        "/api/memory",
        "/api/docs",
        "/api/docs/tree",
        "/api/docs/file",
        "/api/docs/diff",
        "/api/docs/render",
        "/api/docs/changes",
        "/api/events",
        "/api/events/log",
        "/api/runs",
        "/api/runs/{id}",
        "/api/runs/{id}/stream",
        "/api/todos",
        "/api/jira/issues",
        "/api/mcp/servers",
        "/api/attachments/{name}",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::net::SocketAddr;

    // TC-2950 / FR-2950 (middleware helpers)
    #[test]
    fn tc_2950_loopback_api_without_token_is_accepted_and_wrong_token_rejected() {
        let access = WebAccess::loopback(
            "secret".to_owned(),
            false,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            7420,
            crate::web::local_peer::shared_peer_source(),
        );
        let mut headers = HeaderMap::new();
        headers.insert(CLIENT_HEADER, HeaderValue::from_static("1"));
        assert!(
            check_guarded_request(
                &access,
                &Method::GET,
                "/api/state",
                &headers,
                "localhost:7420",
                None,
                None,
            )
            .is_ok()
        );

        let mut bad = HeaderMap::new();
        bad.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer wrong"),
        );
        assert!(
            check_guarded_request(
                &access,
                &Method::GET,
                "/api/state",
                &bad,
                "localhost:7420",
                None,
                None,
            )
            .is_err()
        );

        let mut empty_bearer = HeaderMap::new();
        empty_bearer.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer "),
        );
        assert_eq!(
            classify_provided_token(&empty_bearer, None, access.token.as_str()),
            ProvidedToken::Invalid
        );
    }

    // TC-2952 / FR-2952
    #[test]
    fn tc_2952_host_allowlist_covers_bound_ip_and_port_80_and_rejects_foreign() {
        let hosts = build_allowed_hosts(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 42)), 7420);
        assert!(hosts.iter().any(|h| h == "127.0.0.42:7420"));

        let port80 = build_allowed_hosts(IpAddr::V4(Ipv4Addr::LOCALHOST), 80);
        assert!(port80.contains(&"localhost".to_owned()));
        assert!(port80.contains(&"127.0.0.1".to_owned()));

        let access = WebAccess::loopback(
            "tok".to_owned(),
            false,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            7420,
            crate::web::local_peer::shared_peer_source(),
        );
        assert!(host_allowed(&access, "localhost:7420"));
        assert!(!host_allowed(&access, "evil.test:7420"));
    }

    // TC-2953 / FR-2953
    #[test]
    fn tc_2953_tokenless_post_needs_same_origin_or_client_header_and_rejects_null_origin() {
        let access = WebAccess::loopback(
            "secret".to_owned(),
            false,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            7420,
            crate::web::local_peer::shared_peer_source(),
        );
        let mut headers = HeaderMap::new();
        headers.insert(ORIGIN, HeaderValue::from_static("http://localhost:7420"));
        headers.insert(FETCH_SITE_HEADER, HeaderValue::from_static("same-origin"));
        for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            assert!(
                check_guarded_request(
                    &access,
                    &method,
                    "/api/prompt",
                    &headers,
                    "localhost:7420",
                    None,
                    None,
                )
                .is_ok(),
                "{method}"
            );
        }

        headers.insert(ORIGIN, HeaderValue::from_static("null"));
        assert!(
            check_guarded_request(
                &access,
                &Method::POST,
                "/api/prompt",
                &headers,
                "localhost:7420",
                None,
                None,
            )
            .is_err()
        );

        let mut bare = HeaderMap::new();
        bare.insert(CLIENT_HEADER, HeaderValue::from_static("1"));
        assert!(
            check_guarded_request(
                &access,
                &Method::POST,
                "/api/prompt",
                &bare,
                "localhost:7420",
                None,
                None,
            )
            .is_ok()
        );
    }

    // TC-2954 / FR-2954
    #[test]
    fn tc_2954_tokenless_cross_site_get_rejected_shell_and_tickets_allowed_get_routes_side_effect_free()
     {
        let access = WebAccess::loopback(
            "t".to_owned(),
            false,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            7420,
            crate::web::local_peer::shared_peer_source(),
        );
        let mut headers = HeaderMap::new();
        headers.insert(FETCH_SITE_HEADER, HeaderValue::from_static("cross-site"));
        assert!(
            check_guarded_request(
                &access,
                &Method::GET,
                "/api/state",
                &headers,
                "localhost:7420",
                None,
                None,
            )
            .is_err()
        );

        headers.insert(FETCH_SITE_HEADER, HeaderValue::from_static("other"));
        assert!(
            check_guarded_request(
                &access,
                &Method::GET,
                "/api/state",
                &headers,
                "localhost:7420",
                None,
                None,
            )
            .is_err()
        );

        let mut jira = HeaderMap::new();
        assert!(
            check_guarded_request(
                &access,
                &Method::GET,
                "/api/jira/issues",
                &jira,
                "localhost:7420",
                Some("refresh=1"),
                None,
            )
            .is_err()
        );
        jira.insert(CLIENT_HEADER, HeaderValue::from_static("1"));
        assert!(
            check_guarded_request(
                &access,
                &Method::GET,
                "/api/jira/issues",
                &jira,
                "localhost:7420",
                Some("refresh=1"),
                None,
            )
            .is_ok()
        );

        let mut jira_originless = HeaderMap::new();
        jira_originless.insert(CLIENT_HEADER, HeaderValue::from_static("1"));
        jira_originless.insert(FETCH_SITE_HEADER, HeaderValue::from_static("same-origin"));
        assert!(
            check_guarded_request(
                &access,
                &Method::GET,
                "/api/jira/issues",
                &jira_originless,
                "localhost:7420",
                Some("refresh=1"),
                None,
            )
            .is_ok()
        );

        assert!(
            check_guarded_request(
                &access,
                &Method::GET,
                "//api/jira/issues",
                &jira_originless,
                "localhost:7420",
                Some("refresh=%31"),
                None,
            )
            .is_ok()
        );

        for route in get_api_routes_with_possible_side_effects() {
            let path = route
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .split('?')
                .next()
                .unwrap_or("");
            assert!(
                all_api_get_routes().contains(&path),
                "side-effect route {route} missing from router GET list"
            );
        }

        assert!(!is_guarded_path("/"));
        assert!(!is_guarded_path("/docs/view/ticket"));
        assert!(!get_api_routes_with_possible_side_effects().is_empty());
    }

    // TC-2951 partial / FR-2951
    #[test]
    fn tc_2951_require_token_config_env_token_remote_bind_and_reexec_enforce_token() {
        let access = WebAccess::remote(
            "tok".to_owned(),
            crate::web::local_peer::shared_peer_source(),
        );
        assert!(access.token_required);
        assert_eq!(access.listener, ListenerKind::Remote);
        assert!(host_allowed(&access, "any.host:1"));
        let headers = HeaderMap::new();
        assert!(
            check_guarded_request(
                &access,
                &Method::GET,
                "/api/state",
                &headers,
                "any.host:1",
                None,
                None,
            )
            .is_err()
        );
    }

    // TC-2962 / FR-2962
    #[test]
    fn tc_2962_wsl_listener_host_and_origin_allowlist() {
        let wsl_ip = IpAddr::V4(Ipv4Addr::new(172, 24, 64, 5));
        let access = WebAccess::wsl_interface(
            "t".to_owned(),
            false,
            wsl_ip,
            7420,
            Some(IpAddr::V4(Ipv4Addr::new(172, 24, 48, 1))),
            crate::web::local_peer::shared_peer_source(),
        );
        assert!(
            access
                .allowed_hosts
                .contains(&"172.24.64.5:7420".to_owned())
        );
        let mut headers = HeaderMap::new();
        headers.insert(ORIGIN, HeaderValue::from_static("http://172.24.64.5:7420"));
        headers.insert(FETCH_SITE_HEADER, HeaderValue::from_static("same-origin"));
        let trusted: SocketAddr = "172.24.48.1:12345".parse().unwrap();
        assert!(
            check_guarded_request(
                &access,
                &Method::POST,
                "/api/prompt",
                &headers,
                "172.24.64.5:7420",
                None,
                Some(&PeerConnectInfo::new(trusted)),
            )
            .is_ok()
        );
    }

    // TC-2955 / FR-2955
    #[test]
    fn tc_2955_peer_uid_mismatch_rejected_root_allowed_once_per_connection() {
        use crate::web::local_peer::LocalPeerSource;

        #[derive(Clone, Default)]
        struct MockPeer {
            tcp: String,
        }

        impl LocalPeerSource for MockPeer {
            fn proc_version(&self) -> Option<String> {
                None
            }
            fn wsl_distro_name(&self) -> Option<String> {
                None
            }
            fn wslinfo_networking_mode(&self) -> Option<String> {
                None
            }
            fn proc_net_route(&self) -> Option<String> {
                None
            }
            fn default_route_interface_ip(&self) -> Option<Ipv4Addr> {
                None
            }
            fn proc_net_tcp(&self) -> Option<String> {
                Some(self.tcp.clone())
            }
            fn proc_net_tcp6(&self) -> Option<String> {
                None
            }
        }

        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
             0: 0100007F:C351 0100007F:C350 01 00000000:00000000 00:00000000 00000000  1000        0 123 1 00000000\n";
        let peer_src = Arc::new(MockPeer {
            tcp: table.to_owned(),
        }) as Arc<dyn LocalPeerSource>;
        let server_uid = 2000_u32;
        let access = WebAccess {
            token_required: false,
            listener: ListenerKind::Loopback,
            allowed_hosts: Arc::new(vec!["localhost:7420".to_owned()]),
            wsl_host_peer: None,
            server_uid,
            peer_source: peer_src,
            token: Arc::new("tok".to_owned()),
        };
        let peer: SocketAddr = "127.0.0.1:50001".parse().unwrap();
        let peer_info = PeerConnectInfo::new(peer);
        let mut headers = HeaderMap::new();
        headers.insert(CLIENT_HEADER, HeaderValue::from_static("1"));

        assert!(
            check_guarded_request(
                &access,
                &Method::GET,
                "/api/state",
                &headers,
                "localhost:7420",
                None,
                Some(&peer_info),
            )
            .is_err()
        );
        assert!(
            check_guarded_request(
                &access,
                &Method::GET,
                "/api/state",
                &headers,
                "localhost:7420",
                None,
                Some(&peer_info),
            )
            .is_err(),
            "rejected peer stays rejected on keep-alive"
        );

        let root_table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
             0: 0100007F:C351 0100007F:C350 01 00000000:00000000 00:00000000 00000000     0        0 123 1 00000000\n";
        let root_src = Arc::new(MockPeer {
            tcp: root_table.to_owned(),
        }) as Arc<dyn LocalPeerSource>;
        let root_access = WebAccess {
            token_required: false,
            listener: ListenerKind::Loopback,
            allowed_hosts: Arc::new(vec!["localhost:7420".to_owned()]),
            wsl_host_peer: None,
            server_uid,
            peer_source: root_src,
            token: Arc::new("tok".to_owned()),
        };
        let root_peer = PeerConnectInfo::new(peer);
        assert!(
            check_guarded_request(
                &root_access,
                &Method::GET,
                "/",
                &headers,
                "localhost:7420",
                None,
                Some(&root_peer),
            )
            .is_ok(),
            "root path skips guarded peer check"
        );

        let v6_peer: SocketAddr = "[::ffff:127.0.0.1]:50001".parse().unwrap();
        let v6_info = PeerConnectInfo::new(v6_peer);
        assert!(
            check_guarded_request(
                &access,
                &Method::GET,
                "/api/state",
                &headers,
                "localhost:7420",
                None,
                Some(&v6_info),
            )
            .is_err()
        );

        let token_access = WebAccess {
            token_required: true,
            listener: ListenerKind::Loopback,
            allowed_hosts: Arc::new(vec!["localhost:7420".to_owned()]),
            wsl_host_peer: None,
            server_uid,
            peer_source: Arc::new(MockPeer {
                tcp: table.to_owned(),
            }) as Arc<dyn LocalPeerSource>,
            token: Arc::new("sekrit".to_owned()),
        };
        let mut with_token = headers.clone();
        with_token.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer sekrit"),
        );
        assert!(
            check_guarded_request(
                &token_access,
                &Method::GET,
                "/api/state",
                &with_token,
                "localhost:7420",
                None,
                Some(&peer_info),
            )
            .is_ok(),
            "valid token skips peer uid check"
        );
    }
}
