use std::net::{IpAddr, SocketAddr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindCheck {
    Loopback,
    Remote { warning: String },
}

pub fn generate_token() -> String {
    let u1 = uuid::Uuid::new_v4().simple().to_string();
    let u2 = uuid::Uuid::new_v4().simple().to_string();
    format!("{u1}{u2}").to_ascii_lowercase()
}

pub fn dashboard_url(addr: SocketAddr, token: &str, token_required: bool) -> String {
    let port = addr.port();
    let host = dashboard_host(addr);
    if token_required {
        format!("http://{host}:{port}/?token={token}")
    } else {
        format!("http://{host}:{port}/")
    }
}

pub fn dashboard_url_plain(addr: SocketAddr) -> String {
    let port = addr.port();
    let host = dashboard_host(addr);
    format!("http://{host}:{port}/")
}

fn dashboard_host(addr: SocketAddr) -> String {
    match addr.ip() {
        ip if ip.is_unspecified() => {
            if addr.is_ipv4() {
                "127.0.0.1".to_string()
            } else {
                "[::1]".to_string()
            }
        }
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    }
}

/// Whether the dashboard must reject unauthenticated `/api/*` requests (FR-2951).
pub fn resolve_token_required(
    config_require_token: bool,
    bind: IpAddr,
    allow_remote: bool,
    explicit_env_token: bool,
) -> bool {
    if let Ok(value) = std::env::var("DMUXAI_WEB_TOKEN_REQUIRED") {
        if value == "0" {
            if config_require_token || env_require_token_flag() || explicit_env_token {
                return true;
            }
            if bind_is_loopback_for_policy(bind, allow_remote) {
                return false;
            }
        } else if value == "1" {
            return true;
        }
    }
    if config_require_token || env_require_token_flag() || explicit_env_token {
        return true;
    }
    !bind_is_loopback_for_policy(bind, allow_remote)
}

fn env_require_token_flag() -> bool {
    matches!(
        std::env::var("DMUXAI_WEB_REQUIRE_TOKEN").ok().as_deref(),
        Some("1")
    )
}

pub fn bind_is_loopback_for_policy(bind: IpAddr, allow_remote: bool) -> bool {
    if bind.is_loopback() {
        return true;
    }
    if bind.is_unspecified() && !allow_remote {
        return true;
    }
    false
}

pub fn check_bind(ip: IpAddr, allow_remote: bool) -> Result<BindCheck, String> {
    if ip.is_loopback() {
        Ok(BindCheck::Loopback)
    } else if allow_remote {
        Ok(BindCheck::Remote {
            warning:
                "warning: binding to non-loopback address; anyone who can reach the port and has the token can control agents"
                    .to_string(),
        })
    } else {
        Err(format!(
            "refusing to bind to non-loopback address '{ip}' without --allow-remote"
        ))
    }
}

pub(crate) fn token_matches(expected: &str, given: &str) -> bool {
    let exp_bytes = expected.as_bytes();
    let given_bytes = given.as_bytes();

    let len_matches = exp_bytes.len() == given_bytes.len();
    let mut diff = 0u8;

    for i in 0..exp_bytes.len() {
        let b = if i < given_bytes.len() {
            given_bytes[i]
        } else {
            0
        };
        diff |= exp_bytes[i] ^ b;
    }

    len_matches && diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    // TC-521: token_is_random_hex_and_url_has_token
    #[test]
    fn tc_521_token_is_random_hex_and_url_has_token() {
        let t1 = generate_token();
        let t2 = generate_token();

        assert_eq!(t1.len(), 64);
        assert_eq!(t2.len(), 64);
        assert_ne!(t1, t2);
        assert!(
            t1.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        );
        assert!(
            t2.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        );

        let addr_v4: SocketAddr = "127.0.0.1:7420".parse().unwrap();
        let url_v4 = dashboard_url(addr_v4, &t1, true);
        assert_eq!(url_v4, format!("http://127.0.0.1:7420/?token={t1}"));
        let url_v4_plain = dashboard_url(addr_v4, &t1, false);
        assert_eq!(url_v4_plain, "http://127.0.0.1:7420/");

        let addr_v6: SocketAddr = "[::1]:7420".parse().unwrap();
        let url_v6 = dashboard_url(addr_v6, &t1, true);
        assert_eq!(url_v6, format!("http://[::1]:7420/?token={t1}"));

        let addr_unspec_v4: SocketAddr = "0.0.0.0:7420".parse().unwrap();
        let url_unspec_v4 = dashboard_url(addr_unspec_v4, &t1, true);
        assert_eq!(url_unspec_v4, format!("http://127.0.0.1:7420/?token={t1}"));

        let addr_unspec_v6: SocketAddr = "[::]:7420".parse().unwrap();
        let url_unspec_v6 = dashboard_url(addr_unspec_v6, &t1, true);
        assert_eq!(url_unspec_v6, format!("http://[::1]:7420/?token={t1}"));
    }

    // TC-2956 / FR-2956
    #[test]
    fn tc_2956_printed_url_has_no_token_when_not_required() {
        let addr: SocketAddr = "127.0.0.1:7420".parse().unwrap();
        let token = "a".repeat(64);
        assert_eq!(dashboard_url(addr, &token, false), "http://127.0.0.1:7420/");
        assert!(dashboard_url(addr, &token, true).contains("?token="));
    }

    // TC-2951 partial / FR-2951
    #[test]
    fn tc_2951_require_token_config_env_token_remote_bind_and_reexec_enforce_token() {
        let _guard = crate::state_path::test_env_lock();
        let prev_req = std::env::var_os("DMUXAI_WEB_REQUIRE_TOKEN");
        let prev_state = std::env::var_os("DMUXAI_WEB_TOKEN_REQUIRED");
        unsafe { std::env::remove_var("DMUXAI_WEB_REQUIRE_TOKEN") };
        unsafe { std::env::remove_var("DMUXAI_WEB_TOKEN_REQUIRED") };

        assert!(!resolve_token_required(
            false,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            false,
            false
        ));
        assert!(resolve_token_required(
            true,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            false,
            false
        ));

        unsafe { std::env::set_var("DMUXAI_WEB_REQUIRE_TOKEN", "1") };
        assert!(resolve_token_required(
            false,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            false,
            false
        ));
        unsafe { std::env::remove_var("DMUXAI_WEB_REQUIRE_TOKEN") };

        let remote: IpAddr = "192.168.1.1".parse().unwrap();
        assert!(resolve_token_required(false, remote, true, false));

        unsafe { std::env::set_var("DMUXAI_WEB_TOKEN_REQUIRED", "0") };
        assert!(!resolve_token_required(
            false,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            false,
            false
        ));
        assert!(resolve_token_required(
            true,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            false,
            false
        ));
        assert!(resolve_token_required(
            false,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            false,
            true,
        ));
        unsafe { std::env::set_var("DMUXAI_WEB_TOKEN_REQUIRED", "0") };
        assert!(resolve_token_required(
            false,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            false,
            true,
        ));
        unsafe { std::env::set_var("DMUXAI_WEB_TOKEN_REQUIRED", "1") };
        assert!(resolve_token_required(
            false,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            false,
            false
        ));

        match prev_req {
            Some(v) => unsafe { std::env::set_var("DMUXAI_WEB_REQUIRE_TOKEN", v) },
            None => unsafe { std::env::remove_var("DMUXAI_WEB_REQUIRE_TOKEN") },
        }
        match prev_state {
            Some(v) => unsafe { std::env::set_var("DMUXAI_WEB_TOKEN_REQUIRED", v) },
            None => unsafe { std::env::remove_var("DMUXAI_WEB_TOKEN_REQUIRED") },
        }
    }

    // TC-524: bind_policy_requires_opt_in_for_non_loopback
    #[test]
    fn tc_524_bind_policy_requires_opt_in_for_non_loopback() {
        // Loopback v4
        let res_v4 = check_bind(IpAddr::V4(Ipv4Addr::LOCALHOST), false);
        assert_eq!(res_v4, Ok(BindCheck::Loopback));

        // Loopback v6
        let res_v6 = check_bind(IpAddr::V6(Ipv6Addr::LOCALHOST), false);
        assert_eq!(res_v6, Ok(BindCheck::Loopback));

        // Unspecified v4 without allow_remote -> error
        let res_unspecified_disallowed = check_bind(IpAddr::V4(Ipv4Addr::UNSPECIFIED), false);
        assert!(res_unspecified_disallowed.is_err());

        // Unspecified v4 with allow_remote -> Remote with warning
        let res_unspecified_allowed = check_bind(IpAddr::V4(Ipv4Addr::UNSPECIFIED), true);
        assert!(matches!(
            res_unspecified_allowed,
            Ok(BindCheck::Remote { warning }) if warning.contains("anyone who can reach the port and has the token can control agents")
        ));

        // Arbitrary remote IP without allow_remote -> error
        let remote_ip: IpAddr = "192.168.1.50".parse().unwrap();
        assert!(check_bind(remote_ip, false).is_err());

        // Arbitrary remote IP with allow_remote -> ok
        assert!(check_bind(remote_ip, true).is_ok());
    }

    #[test]
    fn test_token_matches_constant_time() {
        let token = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        // exact -> true
        assert!(token_matches(token, token));
        assert!(!token_matches(token, "wrong"));
        // empty -> false
        assert!(!token_matches(token, ""));
        assert!(!token_matches(
            token,
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456788"
        ));
        // prefix -> false
        assert!(!token_matches(token, &token[..32]));
        // expected+1 char -> false
        let token_plus_1 = format!("{token}a");
        assert!(!token_matches(token, &token_plus_1));
        // expected+256 extra chars -> false
        let token_plus_256 = format!("{}{}", token, "a".repeat(256));
        assert!(!token_matches(token, &token_plus_256));
    }
}
