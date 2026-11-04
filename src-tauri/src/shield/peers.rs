// SPDX-License-Identifier: AGPL-3.0-only
//! "Don't show my address to peers". A direct connection between two PCs
//! shows each one the other's address, whichever side calls. With this on,
//! connections to other people (People, a VPN with friends, file sharing)
//! go only through a relay, so each side sees the relay's address. Linking
//! with your own devices isn't covered: they are yours.
//!
//! When no relay can be reached, the connection waits; it never falls back
//! to direct. People's live links use SulcusAI's built-in relay: both apps
//! must wait in the same room, and the other app, shield or not, waits
//! there. Letters go through whichever relay the two agreed on as before.

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
}
