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

pub fn dashboard_url(addr: SocketAddr, token: &str) -> String {
    let port = addr.port();
    let host = match addr.ip() {
        ip if ip.is_unspecified() => {
            if addr.is_ipv4() {
                "127.0.0.1".to_string()
            } else {
                "[::1]".to_string()
            }
        }
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    format!("http://{host}:{port}/?token={token}")
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
    fn token_is_random_hex_and_url_has_token() {
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
        let url_v4 = dashboard_url(addr_v4, &t1);
        assert_eq!(url_v4, format!("http://127.0.0.1:7420/?token={t1}"));

        let addr_v6: SocketAddr = "[::1]:7420".parse().unwrap();
        let url_v6 = dashboard_url(addr_v6, &t1);
        assert_eq!(url_v6, format!("http://[::1]:7420/?token={t1}"));

        let addr_unspec_v4: SocketAddr = "0.0.0.0:7420".parse().unwrap();
        let url_unspec_v4 = dashboard_url(addr_unspec_v4, &t1);
        assert_eq!(url_unspec_v4, format!("http://127.0.0.1:7420/?token={t1}"));

        let addr_unspec_v6: SocketAddr = "[::]:7420".parse().unwrap();
        let url_unspec_v6 = dashboard_url(addr_unspec_v6, &t1);
        assert_eq!(url_unspec_v6, format!("http://[::1]:7420/?token={t1}"));
    }

    // TC-524: bind_policy_requires_opt_in_for_non_loopback
    #[test]
    fn bind_policy_requires_opt_in_for_non_loopback() {
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
