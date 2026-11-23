//! The client address of a request. Services get only headers, so the edge, which sees the TCP
//! peer, removes any incoming [`CLIENT_IP_HEADER`] and sets it itself ([`resolve`]); services read
//! it with [`from_headers`] (sign-in throttling, audit logs). Behind a proxy (`TRUST_PROXY=on`) the
//! right-most `X-Forwarded-For` entry is the client: the one the proxy appended.
//!
//! Only the edge is reachable from browsers; a process that calls a service port directly could
//! set the header itself, so never use it for access decisions, only for limits and logs.

use std::net::{IpAddr, Ipv6Addr};

use axum::http::HeaderMap;

/// Internal header the edge sets to the client address (never trusted from outside).
pub const CLIENT_IP_HEADER: &str = "x-client-ip";

/// The client address the edge records: the right-most `X-Forwarded-For` entry when
/// `trust_proxy` is on and it is a valid address, else the TCP `peer`.
#[must_use]
pub fn resolve(
    peer: Option<IpAddr>,
    forwarded_for: Option<&str>,
    trust_proxy: bool,
) -> Option<IpAddr> {
    if trust_proxy
        && let Some(last) = forwarded_for.and_then(|v| v.rsplit(',').next())
        && let Ok(ip) = last.trim().parse::<IpAddr>()
    {
        return Some(canonical(ip));
    }
    peer.map(canonical)
}

/// The address set by the edge ([`CLIENT_IP_HEADER`]), if any.
#[must_use]
pub fn from_headers(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get(CLIENT_IP_HEADER)?
        .to_str()
        .ok()?
        .parse()
        .ok()
        .map(canonical)
}

/// A key for per-client limits: the IPv4 address, or the /64 network of an IPv6 address (one
/// IPv6 client usually owns a whole /64). `unknown` without an address.
#[must_use]
pub fn limit_key(ip: Option<IpAddr>) -> String {
    match ip {
        Some(IpAddr::V4(v4)) => v4.to_string(),
        Some(IpAddr::V6(v6)) => {
            let s = v6.segments();
            format!("{}/64", Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
        }
        None => "unknown".to_owned(),
    }
}

/// IPv4-mapped IPv6 addresses (`::ffff:1.2.3.4`, dual-stack sockets) as plain IPv4.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        IpAddr::V4(_) => ip,
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn the_peer_unless_a_trusted_proxy_forwarded_the_client() {
        let peer = Some(ip("127.0.0.1"));
        let xff = Some("6.6.6.6, 203.0.113.7");
        assert_eq!(resolve(peer, xff, false), peer);
        assert_eq!(resolve(peer, xff, true), Some(ip("203.0.113.7")));
        assert_eq!(resolve(peer, Some("garbage"), true), peer);
        assert_eq!(resolve(peer, None, true), peer);
        assert_eq!(
            resolve(Some(ip("::ffff:10.1.2.3")), None, false),
            Some(ip("10.1.2.3"))
        );
    }

    #[test]
    fn reads_the_header_and_groups_ipv6_by_network() {
        let mut headers = HeaderMap::new();
        assert_eq!(from_headers(&headers), None);
        headers.insert(
            CLIENT_IP_HEADER,
            HeaderValue::from_static("2001:db8:1:2:3:4:5:6"),
        );
        let v6 = from_headers(&headers);
        assert_eq!(limit_key(v6), "2001:db8:1:2::/64");
        assert_eq!(limit_key(Some(ip("192.0.2.1"))), "192.0.2.1");
        assert_eq!(limit_key(None), "unknown");
    }
}
