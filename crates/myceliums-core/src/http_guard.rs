//! Request checks shared by the local HTTP servers (MCP transport and
//! dashboard).
//!
//! Both servers run on the developer's machine and expose powerful
//! operations. A web page in the developer's browser must not reach them:
//!
//! * **DNS rebinding** — a page on `evil.example` re-points its own hostname
//!   at `127.0.0.1`. The browser then sends `Host: evil.example:<port>` to the
//!   local server. [`is_loopback_host`] rejects that.
//! * **Cross-origin requests** — a page sends a request straight to
//!   `http://127.0.0.1:<port>`. The browser adds `Origin: https://evil.example`.
//!   [`is_loopback_origin`] rejects that. Requests without an `Origin` header
//!   (CLI tools, MCP clients that are not browsers) are not affected.
//!
//! The functions here are pure string checks so both servers apply the same
//! rules; each server wires them into its own middleware.

use std::net::{IpAddr, SocketAddr};

/// Splits an HTTP authority (`host`, `host:port`, `[v6]:port`) into its host.
fn authority_host(authority: &str) -> Option<&str> {
    let authority = authority.trim();
    if authority.is_empty() || authority.contains(['/', '@', ' ']) {
        return None;
    }
    if let Some(rest) = authority.strip_prefix('[') {
        // Bracketed IPv6 literal, optionally followed by `:port`.
        let end = rest.find(']')?;
        let tail = &rest[end + 1..];
        if !(tail.is_empty() || (tail.starts_with(':') && valid_port(&tail[1..]))) {
            return None;
        }
        return Some(&rest[..end]);
    }
    match authority.rsplit_once(':') {
        // More than one colon without brackets is an unbracketed IPv6
        // literal; a valid `Host` header never looks like that.
        Some((host, _)) if host.contains(':') => None,
        Some((host, port)) if valid_port(port) => Some(host),
        Some(_) => None,
        None => Some(authority),
    }
}

fn valid_port(port: &str) -> bool {
    !port.is_empty() && port.parse::<u16>().is_ok()
}

/// True for `localhost` and loopback IP literals (`127.0.0.0/8`, `::1`).
pub fn is_loopback_hostname(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    // `to_canonical` maps `::ffff:127.0.0.1` to `127.0.0.1`.
    host.parse::<IpAddr>()
        .is_ok_and(|ip| ip.to_canonical().is_loopback())
}

/// True if a `Host` header value names this machine via loopback.
///
/// Accepts `localhost`, `127.x.x.x` and `[::1]`, each with or without a port.
pub fn is_loopback_host(host_header: &str) -> bool {
    authority_host(host_header).is_some_and(is_loopback_hostname)
}

/// True if an `Origin` header value is an `http`/`https` origin on loopback.
///
/// `Origin: null` (sandboxed frames, `file://` pages) is **not** loopback.
pub fn is_loopback_origin(origin: &str) -> bool {
    let origin = origin.trim();
    let rest = if let Some(rest) = origin.strip_prefix("http://") {
        rest
    } else if let Some(rest) = origin.strip_prefix("https://") {
        rest
    } else {
        return false;
    };
    is_loopback_host(rest)
}

/// Parses a listen address (`127.0.0.1:9999`, `[::1]:9999`,
/// `localhost:9999`, `0.0.0.0:9999`) and reports whether it is loopback-only.
///
/// Returns `None` if the address can not be parsed. `localhost` counts as
/// loopback; any other host name does not, because it may resolve to a
/// public interface.
pub fn bind_addr_is_loopback(addr: &str) -> Option<bool> {
    if let Ok(sock) = addr.parse::<SocketAddr>() {
        return Some(sock.ip().to_canonical().is_loopback());
    }
    let (host, port) = addr.rsplit_once(':')?;
    if !valid_port(port) || host.is_empty() || host.contains(':') {
        return None;
    }
    Some(host.eq_ignore_ascii_case("localhost"))
}

/// Extracts the token from an `Authorization: Bearer <token>` header value.
pub fn bearer_token(authorization: &str) -> Option<&str> {
    let (scheme, token) = authorization.trim().split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

/// Compares two secrets in time that depends only on their lengths.
pub fn token_matches(provided: &str, expected: &str) -> bool {
    let (a, b) = (provided.as_bytes(), expected.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_hosts_are_accepted() {
        for host in [
            "localhost",
            "LOCALHOST:9999",
            "127.0.0.1",
            "127.0.0.1:3000",
            "127.1.2.3:80",
            "[::1]",
            "[::1]:9999",
            "[::ffff:127.0.0.1]:9999",
        ] {
            assert!(is_loopback_host(host), "{host}");
        }
    }

    #[test]
    fn foreign_hosts_are_rejected() {
        for host in [
            "",
            "evil.example",
            "evil.example:9999",
            "localhost.evil.example",
            "127.0.0.1.evil.example",
            "0.0.0.0:9999",
            "192.168.1.10:9999",
            "[::]:9999",
            "[::ffff:10.0.0.1]:9999",
            "::1",
            "localhost:notaport",
            "user@localhost",
            "[::1]x",
        ] {
            assert!(!is_loopback_host(host), "{host:?}");
        }
    }

    #[test]
    fn loopback_origins_are_accepted() {
        for origin in [
            "http://localhost",
            "http://localhost:6274",
            "https://127.0.0.1:8443",
            "http://[::1]:3000",
        ] {
            assert!(is_loopback_origin(origin), "{origin}");
        }
    }

    #[test]
    fn foreign_origins_are_rejected() {
        for origin in [
            "null",
            "https://evil.example",
            "http://localhost.evil.example",
            "http://evil.example:9999",
            "file://",
            "chrome-extension://abc",
            "localhost:3000",
        ] {
            assert!(!is_loopback_origin(origin), "{origin}");
        }
    }

    #[test]
    fn bind_addresses_are_classified() {
        assert_eq!(bind_addr_is_loopback("127.0.0.1:9999"), Some(true));
        assert_eq!(bind_addr_is_loopback("[::1]:9999"), Some(true));
        assert_eq!(bind_addr_is_loopback("localhost:9999"), Some(true));
        assert_eq!(bind_addr_is_loopback("0.0.0.0:9999"), Some(false));
        assert_eq!(bind_addr_is_loopback("[::]:9999"), Some(false));
        assert_eq!(bind_addr_is_loopback("[::ffff:127.0.0.1]:9999"), Some(true));
        assert_eq!(bind_addr_is_loopback("[::ffff:0.0.0.0]:9999"), Some(false));
        assert_eq!(bind_addr_is_loopback("192.168.1.5:80"), Some(false));
        assert_eq!(bind_addr_is_loopback("myhost:80"), Some(false));
        assert_eq!(bind_addr_is_loopback("9999"), None);
        assert_eq!(bind_addr_is_loopback("127.0.0.1"), None);
    }

    #[test]
    fn bearer_tokens_are_parsed() {
        assert_eq!(bearer_token("Bearer abc"), Some("abc"));
        assert_eq!(bearer_token("bearer  abc "), Some("abc"));
        assert_eq!(bearer_token("Basic abc"), None);
        assert_eq!(bearer_token("Bearer "), None);
        assert_eq!(bearer_token("abc"), None);
    }

    #[test]
    fn tokens_compare_exactly() {
        assert!(token_matches("s3cret", "s3cret"));
        assert!(!token_matches("s3cret", "s3creT"));
        assert!(!token_matches("s3cre", "s3cret"));
        assert!(!token_matches("", "s3cret"));
    }
}
