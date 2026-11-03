// SPDX-License-Identifier: AGPL-3.0-only
//! "Don't show my address to peers". A direct connection between two PCs
//! shows each one the other's address, whichever side calls. With this on,
//! connections to other people (People, a VPN with friends, file sharing)
//! go only through a relay, so each side sees the relay's address. Linking
//! with your own devices isn't covered: they are yours.
//!
//! When no relay can be reached, the connection waits; it never falls back
//! to direct.

use super::ShieldSettings;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRules {
    /// Call another person's PC at its address.
    pub dial_direct: bool,
    /// Take calls from other people on this PC's listener (own devices
    /// still connect).
    pub accept_direct: bool,
    /// Put this PC's addresses in what's shared with others (contact
    /// details, invitations, announcements to people).
    pub share_addresses: bool,
}

pub fn rules(s: &ShieldSettings) -> PeerRules {
    let hide = s.enabled && s.peers_relays_only;
    PeerRules { dial_direct: !hide, accept_direct: !hide, share_addresses: !hide }
}

/// Which relays to try, in order: the person's own relay first (it's
/// theirs), then the friend's, then SulcusAI's built-in one. Empty entries
/// and repeats are left out.
pub fn relay_order<'a>(own: Option<&'a str>, friends: Option<&'a str>, built_in: &'a str) -> Vec<&'a str> {
    let mut out: Vec<&str> = Vec::new();
    for r in [own, friends, Some(built_in)].into_iter().flatten().map(str::trim) {
        if !r.is_empty() && !out.contains(&r) {
            out.push(r);
        }
    }
    out
}

/// Ok with the relay to use, or why the connection waits instead.
pub fn pick_relay<'a>(order: &[&'a str], reachable: impl Fn(&str) -> bool) -> Result<&'a str, String> {
    order.iter().copied().find(|r| reachable(r)).ok_or_else(|| "No relay is answering, so this waits instead of connecting directly (\"Don't show my address to peers\" is on).".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_by_default_and_hides_everything_when_on() {
        assert_eq!(rules(&ShieldSettings::default()), PeerRules { dial_direct: true, accept_direct: true, share_addresses: true });
        let only_switch = ShieldSettings { peers_relays_only: true, ..Default::default() };
        assert!(rules(&only_switch).dial_direct, "needs the shield on too");
        let on = ShieldSettings { enabled: true, peers_relays_only: true, ..Default::default() };
        assert_eq!(rules(&on), PeerRules { dial_direct: false, accept_direct: false, share_addresses: false });
    }

    #[test]
    fn own_relay_first_without_repeats() {
        assert_eq!(relay_order(Some("mine.example"), Some("friend.example"), "builtin.example"), vec!["mine.example", "friend.example", "builtin.example"]);
        assert_eq!(relay_order(Some(" "), None, "builtin.example"), vec!["builtin.example"]);
        assert_eq!(relay_order(Some("builtin.example"), Some("builtin.example"), "builtin.example"), vec!["builtin.example"]);
    }

    #[test]
    fn no_relay_means_wait_not_direct() {
        let order = relay_order(Some("mine.example"), None, "builtin.example");
        assert_eq!(pick_relay(&order, |r| r == "builtin.example"), Ok("builtin.example"));
        let e = pick_relay(&order, |_| false).unwrap_err();
        assert!(e.contains("instead of connecting directly"));
    }
}
