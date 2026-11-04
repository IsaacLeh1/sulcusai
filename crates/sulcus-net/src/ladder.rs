// SPDX-License-Identifier: AGPL-3.0-only
//! The connection ladder (design section 6): which of a peer's addresses to
//! try, in what order. Same network first (fast, never leaves the house),
//! then the peer's public and router-mapped addresses, then through relays,
//! where hole punching may upgrade the path afterwards. QUIC before TCP at
//! each rung. With "Don't show my address to peers" on, only relays are used
//! so the peer never sees this PC's address.

use libp2p::core::multiaddr::{Multiaddr, Protocol};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rung {
    Lan,
    Public,
    Relayed,
}

pub fn rung_of(a: &Multiaddr) -> Option<Rung> {
    if a.iter().any(|p| matches!(p, Protocol::P2pCircuit)) {
        return Some(Rung::Relayed);
    }
    for p in a.iter() {
        match p {
            Protocol::Ip4(ip) if ip.is_unspecified() || ip.is_link_local() => return None,
            // This same PC (another copy of the app) counts as the local network.
            Protocol::Ip4(ip) if ip.is_private() || ip.is_loopback() => return Some(Rung::Lan),
            Protocol::Ip4(_) => return Some(Rung::Public),
            Protocol::Ip6(ip) if ip.is_unspecified() => return None,
            Protocol::Ip6(ip) if ip.is_loopback() => return Some(Rung::Lan),
            // fe80::/10 (link-local) and fc00::/7 (unique local) stay on the network.
            Protocol::Ip6(ip) if (ip.segments()[0] & 0xffc0) == 0xfe80 || (ip.segments()[0] & 0xfe00) == 0xfc00 => return Some(Rung::Lan),
            Protocol::Ip6(_) => return Some(Rung::Public),
            Protocol::Dns(_) | Protocol::Dns4(_) | Protocol::Dns6(_) => return Some(Rung::Public),
            _ => {}
        }
    }
    None
}

fn quic(a: &Multiaddr) -> bool {
    a.iter().any(|p| matches!(p, Protocol::QuicV1))
}

/// The addresses to try, best first; unusable ones (loopback, "any") are
/// dropped, and only relayed ones are kept when addresses mustn't show.
pub fn order(addrs: &[Multiaddr], relays_only: bool) -> Vec<Multiaddr> {
    let mut usable: Vec<(Rung, bool, Multiaddr)> = addrs
        .iter()
        .filter_map(|a| rung_of(a).map(|r| (r, !quic(a), a.clone())))
        .filter(|(r, _, _)| !relays_only || *r == Rung::Relayed)
        .collect();
    usable.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    let mut out: Vec<Multiaddr> = Vec::new();
    for (_, _, a) in usable {
        if !out.contains(&a) {
            out.push(a);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(s: &str) -> Multiaddr {
        s.parse().unwrap()
    }

    #[test]
    fn same_network_then_public_then_relays_quic_first() {
        let relay = "/ip4/9.9.9.9/tcp/4001/p2p/12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN/p2p-circuit";
        let addrs = vec![m(relay), m("/ip4/8.8.8.8/tcp/1"), m("/ip4/8.8.8.8/udp/1/quic-v1"), m("/ip4/192.168.1.5/tcp/2"), m("/ip4/0.0.0.0/tcp/4"), m("/ip6/fe80::1/tcp/5")];
        let got: Vec<String> = order(&addrs, false).iter().map(|a| a.to_string()).collect();
        assert_eq!(got, vec!["/ip4/192.168.1.5/tcp/2", "/ip6/fe80::1/tcp/5", "/ip4/8.8.8.8/udp/1/quic-v1", "/ip4/8.8.8.8/tcp/1", relay]);
        assert_eq!(rung_of(&m("/ip4/127.0.0.1/tcp/3")), Some(Rung::Lan), "another copy on this PC");
        let hidden: Vec<String> = order(&addrs, true).iter().map(|a| a.to_string()).collect();
        assert_eq!(hidden, vec![relay], "with addresses hidden, only relays");
    }
}
