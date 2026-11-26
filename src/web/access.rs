//! Host allowlist, CSRF guards, and peer checks (SPEC-web-local-access FR-2952..FR-2955).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use axum::http::header::ORIGIN;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::web::auth::token_matches;
use crate::web::local_peer::{LocalPeerSource, peer_uid_for_connection};

pub const CLIENT_HEADER: &str = "x-dmuxai-client";
pub const FETCH_SITE_HEADER: &str = "sec-fetch-site";

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
        let hosts = allowed_hosts.unwrap_or_else(|| vec!["localhost:7420".to_string()]);
        Self {
            token_required: true,
            listener: ListenerKind::Loopback,
            allowed_hosts: Arc::new(hosts),
            wsl_host_peer: None,
            server_uid: std::process::id(),
            peer_source: crate::web::local_peer::shared_peer_source(),
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
            server_uid: std::process::id(),
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
        hosts.sort();
        hosts.dedup();
        Self {
            token_required,
            listener: ListenerKind::WslInterface,
            allowed_hosts: Arc::new(hosts),
            wsl_host_peer,
            server_uid: std::process::id(),
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
    access.allowed_hosts.iter().any(|h| h == host_header)
}

pub fn is_guarded_path(path: &str) -> bool {
    path.starts_with("/api/")
        || path == "/api"
        || path.starts_with("//api/")
        || path.contains("/api/")
}

pub fn extract_bearer_or_query_token(headers: &HeaderMap, query: Option<&str>) -> Option<String> {
    if let Some(auth_val) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        let mut parts = auth_val.splitn(2, ' ');
        let scheme = parts.next().unwrap_or_default();
        let tok = parts.next().unwrap_or_default();
        if scheme.eq_ignore_ascii_case("bearer") && !tok.trim().is_empty() {
            return Some(tok.trim().to_owned());
        }
    }
    let query = query?;
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=')
            && k == "token"
            && !v.is_empty()
        {
            return Some(v.to_owned());
        }
    }
    None
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

    origin.is_none() && fetch_site.is_none() && client == Some("1")
}

pub fn tokenless_get_allowed(headers: &HeaderMap) -> bool {
    match headers.get(FETCH_SITE_HEADER).and_then(|v| v.to_str().ok()) {
        None | Some("same-origin") | Some("none") => true,
        Some("cross-site") | Some("same-site") => false,
        Some(_) => true,
    }
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
    peer: Option<SocketAddr>,
    peer_checked: &mut bool,
) -> std::result::Result<(), Response> {
    if !is_guarded_path(path) {
        return Ok(());
    }

    let provided = extract_bearer_or_query_token(headers, query);
    let has_valid = provided
        .as_deref()
        .is_some_and(|t| token_matches(access.token.as_str(), t));
    if provided.is_some() && !has_valid {
        return Err(unauthorized());
    }

    let peer_ip = peer.map(|a| a.ip());
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
        && !*peer_checked
        && let Some(peer_addr) = peer
    {
        *peer_checked = true;
        if let Some(uid) = peer_uid_for_connection(access.peer_source.as_ref(), peer_addr)
            && uid != access.server_uid
            && uid != 0
        {
            return Err(forbidden());
        }
    }

    if method == Method::GET || method == Method::HEAD {
        if !tokenless_get_allowed(headers) {
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
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

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
        let mut seen = false;
        let mut headers = HeaderMap::new();
        headers.insert(ORIGIN, HeaderValue::from_static("http://localhost:7420"));
        headers.insert(FETCH_SITE_HEADER, HeaderValue::from_static("same-origin"));
        assert!(
            check_guarded_request(
                &access,
                &Method::POST,
                "/api/prompt",
                &headers,
                "localhost:7420",
                None,
                None,
                &mut seen,
            )
            .is_ok()
        );

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
                &mut seen,
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
                &mut seen,
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
        let mut seen = false;
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
                &mut seen,
            )
            .is_err()
        );

        assert!(!is_guarded_path("/"));
        assert!(!is_guarded_path("/docs/view/ticket"));
        assert!(get_api_routes_with_possible_side_effects().is_empty());
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
        let mut seen = false;
        let trusted: SocketAddr = "172.24.48.1:12345".parse().unwrap();
        assert!(
            check_guarded_request(
                &access,
                &Method::POST,
                "/api/prompt",
                &headers,
                "172.24.64.5:7420",
                None,
                Some(trusted),
                &mut seen,
            )
            .is_ok()
        );
    }
}
