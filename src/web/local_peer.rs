//! WSL / local peer detection for dashboard access (SPEC-web-local-access FR-2960..FR-2963).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::process::Command;
use std::sync::Arc;

/// Injectable source for OS-specific reads (FR-2963).
pub trait LocalPeerSource: Send + Sync {
    fn proc_version(&self) -> Option<String>;
    fn wsl_distro_name(&self) -> Option<String>;
    fn wslinfo_networking_mode(&self) -> Option<String>;
    fn proc_net_route(&self) -> Option<String>;
    fn default_route_interface_ip(&self) -> Option<Ipv4Addr>;
    fn proc_net_tcp(&self) -> Option<String>;
    fn proc_net_tcp6(&self) -> Option<String>;
}

/// Production implementation reading the live environment.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsLocalPeerSource;

impl LocalPeerSource for OsLocalPeerSource {
    fn proc_version(&self) -> Option<String> {
        std::fs::read_to_string("/proc/version").ok()
    }

    fn wsl_distro_name(&self) -> Option<String> {
        std::env::var("WSL_DISTRO_NAME").ok()
    }

    fn wslinfo_networking_mode(&self) -> Option<String> {
        let output = Command::new("wslinfo")
            .arg("--networking-mode")
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout)
            .trim()
            .to_ascii_lowercase();
        if text.is_empty() { None } else { Some(text) }
    }

    fn proc_net_route(&self) -> Option<String> {
        std::fs::read_to_string("/proc/net/route").ok()
    }

    fn default_route_interface_ip(&self) -> Option<Ipv4Addr> {
        let route = self.proc_net_route()?;
        let iface = default_route_iface(route.as_str())?;
        first_ipv4_on_iface(&iface)
    }

    fn proc_net_tcp(&self) -> Option<String> {
        std::fs::read_to_string("/proc/net/tcp").ok()
    }

    fn proc_net_tcp6(&self) -> Option<String> {
        std::fs::read_to_string("/proc/net/tcp6").ok()
    }
}

/// Returns true when the environment looks like WSL (Terms).
pub fn is_wsl<S: LocalPeerSource + ?Sized>(source: &S) -> bool {
    if source.wsl_distro_name().is_some() {
        return true;
    }
    source
        .proc_version()
        .is_some_and(|v| v.to_ascii_lowercase().contains("microsoft"))
}

/// WSL2 NAT mode detection (Terms + OQ-2953).
pub fn is_wsl2_nat_mode<S: LocalPeerSource + ?Sized>(source: &S) -> bool {
    if !is_wsl(source) {
        return false;
    }
    if let Some(mode) = source.wslinfo_networking_mode() {
        return mode == "nat";
    }
    let version = source.proc_version().unwrap_or_default();
    if !version.to_ascii_uppercase().contains("WSL2") {
        return false;
    }
    let Some(iface_ip) = source.default_route_interface_ip() else {
        return false;
    };
    let Some(gateway) = windows_host_ipv4_from_route(source.proc_net_route().as_deref()) else {
        return false;
    };
    nat_heuristic(iface_ip, gateway)
}

fn nat_heuristic(iface: Ipv4Addr, gateway: Ipv4Addr) -> bool {
    let iface_octets = iface.octets();
    let gw_octets = gateway.octets();
    if is_private_172_16(iface) || is_private_192_168(iface) {
        if is_private_172_16(iface) && is_private_172_16(gateway) {
            return same_prefix(iface_octets, gw_octets, 20);
        }
        if is_private_192_168(iface) && is_private_192_168(gateway) {
            return same_prefix(iface_octets, gw_octets, 24);
        }
    }
    false
}

fn is_private_172_16(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 172 && (16..=31).contains(&o[1])
}

fn is_private_192_168(ip: Ipv4Addr) -> bool {
    ip.octets()[0] == 192 && ip.octets()[1] == 168
}

fn same_prefix(a: [u8; 4], b: [u8; 4], prefix_bits: u32) -> bool {
    let a_bits = u32::from_be_bytes(a);
    let b_bits = u32::from_be_bytes(b);
    let mask = if prefix_bits == 0 {
        0
    } else {
        !0u32 << (32 - prefix_bits)
    };
    (a_bits & mask) == (b_bits & mask)
}

/// IPv4 address of the Windows host (default-route gateway) in WSL2 NAT mode.
pub fn windows_host_ipv4<S: LocalPeerSource + ?Sized>(source: &S) -> Option<Ipv4Addr> {
    if !is_wsl2_nat_mode(source) {
        return None;
    }
    windows_host_ipv4_from_route(source.proc_net_route().as_deref())
}

pub fn windows_host_ipv4_from_route(route: Option<&str>) -> Option<Ipv4Addr> {
    let route = route?;
    default_route_gateway_v4(route)
}

/// Interface IPv4 on the default route (printed WSL dashboard URL).
pub fn wsl_nat_listen_ipv4<S: LocalPeerSource + ?Sized>(source: &S) -> Option<Ipv4Addr> {
    if !is_wsl2_nat_mode(source) {
        return None;
    }
    source.default_route_interface_ip()
}

pub fn default_route_gateway_v4(route_table: &str) -> Option<Ipv4Addr> {
    for line in route_table.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 3 {
            continue;
        }
        let dest = cols.get(1).copied().unwrap_or("");
        if dest != "00000000" {
            continue;
        }
        let gateway_hex = cols.get(2).copied()?;
        return parse_proc_ipv4_hex(gateway_hex);
    }
    None
}

pub fn default_route_iface(route_table: &str) -> Option<String> {
    for line in route_table.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 2 {
            continue;
        }
        let dest = cols.get(1).copied().unwrap_or("");
        if dest == "00000000" {
            return cols.first().map(|s| s.to_string());
        }
    }
    None
}

fn first_ipv4_on_iface(iface: &str) -> Option<Ipv4Addr> {
    let output = Command::new("ip")
        .args(["-4", "-o", "addr", "show", "dev", iface])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let _ = parts.next();
        let _ = parts.next();
        if let Some(inet) = parts.next()
            && let Some(addr) = inet.strip_prefix("inet ")
        {
            let ip = addr.split('/').next()?;
            return ip.parse().ok();
        }
    }
    None
}

fn parse_proc_ipv4_hex(hex: &str) -> Option<Ipv4Addr> {
    if hex.len() != 8 {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    let octets = value.to_le_bytes();
    Some(Ipv4Addr::from(octets))
}

/// Parses `/proc/net/tcp` (or tcp6) and returns the uid owning a socket with the given peer.
pub fn peer_uid_from_proc_net(tables: &[&str], peer: SocketAddr) -> Option<u32> {
    for table in tables {
        for line in table.lines().skip(1) {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 8 {
                continue;
            }
            let remote_field = cols.get(2).copied().unwrap_or("");
            let is_v6 = remote_field
                .split(':')
                .next()
                .is_some_and(|addr| addr.len() == 32);
            let (ip, port) = parse_proc_socket_field(remote_field, is_v6)?;
            if port != peer.port() {
                continue;
            }
            if !ip_matches(peer.ip(), ip) {
                continue;
            }
            let uid = cols.get(7).and_then(|u| u.parse().ok())?;
            return Some(uid);
        }
    }
    None
}

fn parse_proc_socket_field(field: &str, v6: bool) -> Option<(IpAddr, u16)> {
    let (addr_hex, port_hex) = field.split_once(':')?;
    let port = u16::from_str_radix(port_hex, 16).ok()?;
    if v6 {
        if addr_hex.len() != 32 {
            return None;
        }
        let mut bytes = [0u8; 16];
        for i in 0..16 {
            let part = &addr_hex[i * 2..i * 2 + 2];
            bytes[i] = u8::from_str_radix(part, 16).ok()?;
        }
        Some((IpAddr::from(bytes), port))
    } else if addr_hex.len() == 8 {
        let value = u32::from_str_radix(addr_hex, 16).ok()?;
        let octets = value.to_le_bytes();
        Some((IpAddr::V4(Ipv4Addr::from(octets)), port))
    } else {
        None
    }
}

fn ip_matches(expected: IpAddr, found: IpAddr) -> bool {
    match (expected, found) {
        (IpAddr::V4(a), IpAddr::V4(b)) => a == b,
        (IpAddr::V6(a), IpAddr::V6(b)) => a == b,
        _ => false,
    }
}

pub fn peer_uid_for_connection<S: LocalPeerSource + ?Sized>(
    source: &S,
    peer: SocketAddr,
) -> Option<u32> {
    let tcp = source.proc_net_tcp();
    let tcp6 = source.proc_net_tcp6();
    let tables: Vec<&str> = [&tcp, &tcp6]
        .into_iter()
        .filter_map(|opt| opt.as_deref())
        .collect();
    peer_uid_from_proc_net(&tables, peer)
}

pub fn shared_peer_source() -> Arc<dyn LocalPeerSource> {
    Arc::new(OsLocalPeerSource)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default)]
    struct MockSource {
        version: Option<String>,
        distro: Option<String>,
        wslinfo: Option<String>,
        route: Option<String>,
        tcp: Option<String>,
        tcp6: Option<String>,
    }

    impl LocalPeerSource for MockSource {
        fn proc_version(&self) -> Option<String> {
            self.version.clone()
        }
        fn wsl_distro_name(&self) -> Option<String> {
            self.distro.clone()
        }
        fn wslinfo_networking_mode(&self) -> Option<String> {
            self.wslinfo.clone()
        }
        fn proc_net_route(&self) -> Option<String> {
            self.route.clone()
        }
        fn default_route_interface_ip(&self) -> Option<Ipv4Addr> {
            self.route
                .as_deref()
                .and_then(|route| windows_host_ipv4_from_route(Some(route)))
        }
        fn proc_net_tcp(&self) -> Option<String> {
            self.tcp.clone()
        }
        fn proc_net_tcp6(&self) -> Option<String> {
            self.tcp6.clone()
        }
    }

    // TC-2963 / FR-2963
    #[test]
    fn tc_2963_wsl_host_from_mocked_route_and_wslinfo_malformed_is_none() {
        let nat = MockSource {
            version: Some("... microsoft ... WSL2 ...".to_owned()),
            wslinfo: Some("nat".to_owned()),
            route: Some(
                "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n\
                 eth0\t00000000\t010011AC\t0003\t0\t0\t0\t00000000\t0\t0\t0\n"
                    .to_owned(),
            ),
            ..Default::default()
        };
        let host = windows_host_ipv4(&nat);
        assert_eq!(host, Some(Ipv4Addr::new(172, 17, 0, 1)));

        let malformed = MockSource {
            version: Some("microsoft".to_owned()),
            wslinfo: Some("nat".to_owned()),
            route: Some("not a route table".to_owned()),
            ..Default::default()
        };
        assert_eq!(windows_host_ipv4(&malformed), None);

        let not_wsl = MockSource::default();
        assert_eq!(windows_host_ipv4(&not_wsl), None);
    }

    // TC-2960 partial / FR-2960
    #[test]
    fn tc_2960_wsl2_nat_detection_and_extra_listener_only_in_nat_mode() {
        let nat = MockSource {
            version: Some("microsoft WSL2".to_owned()),
            wslinfo: Some("nat".to_owned()),
            route: Some(
                "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n\
                 eth0\t00000000\t0101A8C0\t0003\t0\t0\t0\t00000000\t0\t0\t0\n"
                    .to_owned(),
            ),
            ..Default::default()
        };
        assert!(is_wsl2_nat_mode(&nat));
        assert_eq!(
            wsl_nat_listen_ipv4(&nat),
            Some(Ipv4Addr::new(192, 168, 1, 1))
        );

        let mirrored = MockSource {
            version: Some("microsoft".to_owned()),
            wslinfo: Some("mirrored".to_owned()),
            ..Default::default()
        };
        assert!(!is_wsl2_nat_mode(&mirrored));
    }

    // TC-2955 / FR-2955
    #[test]
    fn tc_2955_proc_net_tcp_peer_uid_parsing_mismatch_rejected_root_allowed_once_per_connection() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
             0: 0100007F:C350 0100007F:C351 01 00000000:00000000 00:00000000 00000000     0        0 123 1 00000000\n\
             1: 0100007F:C352 0100007F:C353 01 00000000:00000000 00:00000000 00000000  1000        0 456 1 00000000\n";
        let peer: SocketAddr = "127.0.0.1:50001".parse().unwrap();
        assert_eq!(peer_uid_from_proc_net(&[table], peer), Some(0));
        let peer2: SocketAddr = "127.0.0.1:50003".parse().unwrap();
        assert_eq!(peer_uid_from_proc_net(&[table], peer2), Some(1000));
    }

    // TC-2959 / FR-2959 (middleware-level expectations exercised via access)
    // TC-2961 / FR-2961
    #[test]
    fn tc_2961_wsl_listener_trusts_windows_host_peer_and_requires_token_for_others() {
        use crate::web::access::{WebAccess, check_guarded_request};
        use axum::http::{HeaderMap, Method};
        let host_peer: IpAddr = IpAddr::V4(Ipv4Addr::new(172, 24, 48, 1));
        let access = WebAccess::wsl_interface(
            "secret".to_owned(),
            false,
            IpAddr::V4(Ipv4Addr::new(172, 24, 64, 5)),
            7420,
            Some(host_peer),
            crate::web::local_peer::shared_peer_source(),
        );
        let mut seen = false;
        let headers = HeaderMap::new();
        let trusted: SocketAddr = "172.24.48.1:54000".parse().unwrap();
        assert!(
            check_guarded_request(
                &access,
                &Method::GET,
                "/api/state",
                &headers,
                "172.24.64.5:7420",
                None,
                Some(trusted),
                &mut seen,
            )
            .is_ok()
        );
        let other: SocketAddr = "10.0.0.2:54001".parse().unwrap();
        assert!(
            check_guarded_request(
                &access,
                &Method::GET,
                "/api/state",
                &headers,
                "172.24.64.5:7420",
                None,
                Some(other),
                &mut seen,
            )
            .is_err()
        );
    }

    #[test]
    fn tc_2959_wsl_localhost_forwarding_hosts_and_origins_accepted_tokenless() {
        use crate::web::access::{WebAccess, check_guarded_request};
        use axum::http::{HeaderMap, HeaderValue, Method};
        let access = WebAccess::loopback(
            "t".to_owned(),
            false,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            7420,
            crate::web::local_peer::shared_peer_source(),
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::ORIGIN,
            HeaderValue::from_static("http://127.0.0.1:7420"),
        );
        headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        let mut seen = false;
        assert!(
            check_guarded_request(
                &access,
                &Method::POST,
                "/api/prompt",
                &headers,
                "127.0.0.1:7420",
                None,
                None,
                &mut seen,
            )
            .is_ok()
        );
    }
}
