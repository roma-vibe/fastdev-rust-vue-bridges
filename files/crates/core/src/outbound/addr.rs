//! Which addresses outbound requests may connect to: only public unicast ones. Everything that
//! reaches this machine, the local network, the cloud metadata service or a carrier network is
//! refused, including the IPv6 forms that embed such an IPv4 address.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Whether `ip` must never be connected to.
#[must_use]
pub fn is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => is_blocked_v6(v6),
    }
}

/// IPv4 networks (address, prefix length) that are never public.
const BLOCKED_V4: [(Ipv4Addr, u32); 11] = [
    (Ipv4Addr::new(0, 0, 0, 0), 8),      // "this network", unspecified
    (Ipv4Addr::new(10, 0, 0, 0), 8),     // private
    (Ipv4Addr::new(100, 64, 0, 0), 10),  // carrier-grade NAT
    (Ipv4Addr::new(127, 0, 0, 0), 8),    // loopback
    (Ipv4Addr::new(169, 254, 0, 0), 16), // link-local, cloud metadata
    (Ipv4Addr::new(172, 16, 0, 0), 12),  // private
    (Ipv4Addr::new(192, 0, 0, 0), 24),   // IETF protocol assignments
    (Ipv4Addr::new(192, 168, 0, 0), 16), // private
    (Ipv4Addr::new(198, 18, 0, 0), 15),  // benchmarking
    (Ipv4Addr::new(224, 0, 0, 0), 4),    // multicast
    (Ipv4Addr::new(240, 0, 0, 0), 4),    // reserved, broadcast
];

fn is_blocked_v4(ip: Ipv4Addr) -> bool {
    let bits = u32::from(ip);
    BLOCKED_V4
        .iter()
        .any(|(net, len)| bits >> (32 - len) == u32::from(*net) >> (32 - len))
}

fn is_blocked_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = embedded_v4(ip) {
        return is_blocked_v4(v4);
    }
    let first = ip.segments()[0];
    ip.is_unspecified()
        || ip.is_loopback()
        || (first & 0xfe00) == 0xfc00 // fc00::/7 unique local
        || (first & 0xffc0) == 0xfe80 // fe80::/10 link-local
        || (first & 0xffc0) == 0xfec0 // fec0::/10 deprecated site-local
        || (first & 0xff00) == 0xff00 // ff00::/8 multicast
        || (first == 0x0100 && ip.segments()[1..4] == [0, 0, 0]) // 100::/64 discard
        || (first == 0x2001 && ip.segments()[1] == 0x0db8) // 2001:db8::/32 documentation
}

/// The IPv4 address inside IPv4-mapped (`::ffff:a.b.c.d`), IPv4-translated
/// (`::ffff:0:a.b.c.d`), IPv4-compatible (`::a.b.c.d`),
/// NAT64 (`64:ff9b::a.b.c.d`) and 6to4 (`2002:aabb:ccdd::`) addresses: each one can reach that
/// IPv4 host, so it is judged as that host.
fn embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();
    let tail = Ipv4Addr::new(
        (s[6] >> 8) as u8,
        (s[6] & 0xff) as u8,
        (s[7] >> 8) as u8,
        (s[7] & 0xff) as u8,
    );
    match s {
        [0, 0, 0, 0, 0, 0xffff, ..] | [0, 0, 0, 0, 0xffff, 0, ..] => Some(tail),
        // `::` and `::1` are not IPv4-compatible forms: the caller blocks them as IPv6.
        [0, 0, 0, 0, 0, 0, hi, lo] if hi != 0 || lo > 1 => Some(tail),
        [0x64, 0xff9b, 0, 0, 0, 0, ..] => Some(tail),
        [0x2002, hi, lo, ..] => Some(Ipv4Addr::new(
            (hi >> 8) as u8,
            (hi & 0xff) as u8,
            (lo >> 8) as u8,
            (lo & 0xff) as u8,
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked(s: &str) -> bool {
        is_blocked(s.parse().unwrap())
    }

    #[test]
    fn refuses_local_private_and_special_ipv4() {
        for ip in [
            "127.0.0.1",
            "127.255.255.254",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "100.127.255.255",
            "0.0.0.0",
            "0.1.2.3",
            "224.0.0.1",
            "239.255.255.250",
            "255.255.255.255",
            "198.18.0.1",
        ] {
            assert!(blocked(ip), "{ip} must be blocked");
        }
    }

    #[test]
    fn allows_public_ipv4() {
        for ip in [
            "1.1.1.1",
            "8.8.8.8",
            "93.184.216.34",
            "172.32.0.1",
            "100.128.0.1",
            "192.169.0.1",
        ] {
            assert!(!blocked(ip), "{ip} is public");
        }
    }

    #[test]
    fn refuses_local_and_special_ipv6() {
        for ip in [
            "::",
            "::1",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "febf::1",
            "ff02::1",
            "2001:db8::1",
        ] {
            assert!(blocked(ip), "{ip} must be blocked");
        }
    }

    #[test]
    fn judges_ipv6_forms_of_ipv4_as_their_ipv4() {
        for ip in [
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
            "::ffff:7f00:1",
            "::ffff:0:127.0.0.1",
            "::127.0.0.1",
            "::10.1.1.1",
            "64:ff9b::192.168.0.1",
            "2002:7f00:0001::",
            "2002:c0a8:0101::1",
        ] {
            assert!(blocked(ip), "{ip} embeds a blocked IPv4 address");
        }
        for ip in [
            "::ffff:8.8.8.8",
            "64:ff9b::1.1.1.1",
            "2002:0808:0808::1",
            "2606:4700::1111",
        ] {
            assert!(!blocked(ip), "{ip} reaches a public host");
        }
    }
}
