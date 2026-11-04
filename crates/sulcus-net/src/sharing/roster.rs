// SPDX-License-Identifier: AGPL-3.0-only
//! A virtual LAN's member list passed between its members. An admin signs
//! each change; a member keeps a list only if an admin it already trusts
//! signed it (or, for a network it hasn't seen, an admin named in the list
//! that lists this PC), and only if it is newer than the one it has.

use libp2p::identity::Keypair;

use super::vlan::Network;
use crate::signed;

pub const PREFIX: &str = "svn1";
const DOMAIN: &str = "sulcus-vlan-roster-v1\n";

pub fn seal(key: &Keypair, n: &Network) -> String {
    let mut n = n.clone();
    // Who is waiting to join stays with the admin who was asked.
    n.waiting.clear();
    signed::seal(PREFIX, DOMAIN, key, &n)
}

/// Which network a list is for, before deciding whether to keep it.
pub fn network_id(text: &str) -> Option<String> {
    signed::open::<Network>(PREFIX, DOMAIN, text).ok().map(|(n, _)| n.id)
}

/// The list to keep after `text` arrives, or None to ignore it.
pub fn accept(have: Option<&Network>, text: &str, me: &str) -> Option<Network> {
    let (mut n, signer): (Network, _) = signed::open(PREFIX, DOMAIN, text).ok()?;
    let signer = signer.to_string();
    match have {
        Some(old) => {
            if old.id != n.id || !old.admins.contains(&signer) || n.version <= old.version {
                return None;
            }
            n.waiting = old.waiting.clone();
        }
        None => {
            if !n.admins.contains(&signer) || !n.members.contains_key(me) {
                return None;
            }
        }
    }
    if !n.admins.iter().all(|a| n.members.contains_key(a)) {
        return None;
    }
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(k: &Keypair) -> String {
        k.public().to_peer_id().to_string()
    }

    #[test]
    fn only_newer_lists_from_trusted_admins_are_kept() {
        let (admin, bob, eve) = (Keypair::generate_ed25519(), Keypair::generate_ed25519(), Keypair::generate_ed25519());
        let mut n = Network::create("n1", "games", &id(&admin));
        n.ask(&id(&bob));
        n.admit(&id(&admin), &id(&bob)).unwrap();
        let text = seal(&admin, &n);

        assert_eq!(network_id(&text).as_deref(), Some("n1"));
        let got = accept(None, &text, &id(&bob)).unwrap();
        assert_eq!(got.members, n.members);
        assert!(got.waiting.is_empty());
        assert!(accept(None, &text, &id(&eve)).is_none(), "not listed: not joined");

        assert!(accept(Some(&got), &text, &id(&bob)).is_none(), "same version again");
        let mut newer = n.clone();
        newer.admit(&id(&admin), "carol").unwrap();
        assert_eq!(accept(Some(&got), &seal(&admin, &newer), &id(&bob)).unwrap().members.len(), 3);

        // Eve makes herself an admin of a copy and signs it: not trusted.
        let mut forged = newer.clone();
        forged.members.insert(id(&eve), "10.77.9.9".parse().unwrap());
        forged.admins.insert(id(&eve));
        forged.version += 5;
        assert!(accept(Some(&got), &seal(&eve, &forged), &id(&bob)).is_none());
        assert!(accept(Some(&got), "svn1.bad", &id(&bob)).is_none());
    }
}
