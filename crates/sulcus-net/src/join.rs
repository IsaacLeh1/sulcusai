// SPDX-License-Identifier: AGPL-3.0-only
//! Joining someone's relay with an invite key (design section 10). The
//! relay keeps each key's secret (sealed at rest by its store, since the
//! HMAC check needs the secret itself, not a hash of it); a newcomer proves
//! it has the secret by answering a fresh challenge with an HMAC bound to both
//! PeerIds, so an answer seen once can't be replayed on another connection.
//! A good proof only puts the newcomer in line: the owner approves each one.
//! Wrong proofs count toward a lockout per key id.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::lockout::Lockout;

const DOMAIN: &[u8] = b"SulcusAI join v1";
pub const MAX_PENDING: usize = 50;
pub const PENDING_SECS: u64 = 7 * 86_400;

/// HMAC-SHA256, written out so no extra crate is needed for one use.
pub fn hmac(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let (mut ipad, mut opad) = ([0x36u8; 64], [0x5cu8; 64]);
    for i in 0..64 {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let inner = Sha256::new().chain_update(ipad).chain_update(msg).finalize();
    Sha256::new().chain_update(opad).chain_update(inner).finalize().into()
}

/// The newcomer's answer to a challenge.
pub fn proof(secret: &[u8], relay: &str, newcomer: &str, challenge: &[u8; 32]) -> [u8; 32] {
    let mut msg = DOMAIN.to_vec();
    msg.extend_from_slice(relay.as_bytes());
    msg.push(0);
    msg.extend_from_slice(newcomer.as_bytes());
    msg.push(0);
    msg.extend_from_slice(challenge);
    hmac(secret, &msg)
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InviteKey {
    /// Sent in clear so the relay knows which key to check.
    pub id: String,
    /// The secret itself, kept by the relay: the HMAC needs it. Sealed at
    /// rest by whoever stores the book.
    pub secret: String,
    pub label: String,
    pub created: u64,
    pub expires: Option<u64>,
    pub max_uses: Option<u32>,
    pub uses: u32,
    pub disabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pending {
    pub peer: String,
    pub name: String,
    pub kind: String,
    pub key_label: String,
    pub at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Admissions {
    pub keys: Vec<InviteKey>,
    pub pending: Vec<Pending>,
    pub members: Vec<String>,
    pub blocked: Vec<String>,
    pub lockouts: HashMap<String, Lockout>,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// In line for the owner's approval.
    Waiting,
    AlreadyMember,
    Refused(&'static str),
    /// Too many wrong proofs for this key: wait this many seconds.
    Wait(u64),
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).expect("the OS random source works");
    b
}

pub fn challenge() -> [u8; 32] {
    random_bytes()
}

impl Admissions {
    /// A new key for a circle ("Family"); returns its id and secret (hex),
    /// which go into the `kind = relay` invite.
    pub fn new_key(&mut self, label: &str, now: u64, expires: Option<u64>, max_uses: Option<u32>) -> (String, String) {
        let id = loop {
            let b: [u8; 2] = random_bytes();
            let id = hex::encode(b);
            if !self.keys.iter().any(|k| k.id == id) {
                break id;
            }
        };
        let secret = hex::encode(random_bytes::<16>());
        self.keys.push(InviteKey { id: id.clone(), secret: secret.clone(), label: label.trim().into(), created: now, expires, max_uses, uses: 0, disabled: false });
        (id, secret)
    }

    /// "New key" for a circle: the old ones stop admitting newcomers;
    /// members they admitted stay.
    pub fn rotate(&mut self, label: &str, now: u64) -> (String, String) {
        for k in self.keys.iter_mut().filter(|k| k.label == label) {
            k.disabled = true;
        }
        self.new_key(label, now, None, None)
    }

    /// Checks a join request: the proof for `challenge` with key `key_id`.
    pub fn request(&mut self, key_id: &str, answer: &[u8], challenge: &[u8; 32], relay: &str, peer: &str, name: &str, kind: &str, now: u64) -> Outcome {
        if self.blocked.iter().any(|b| b == peer) {
            return Outcome::Refused("blocked by the relay's owner");
        }
        if self.members.iter().any(|m| m == peer) {
            return Outcome::AlreadyMember;
        }
        let lock = self.lockouts.entry(key_id.to_string()).or_default();
        if let Err(w) = lock.ready((now * 1000) as i64) {
            return Outcome::Wait(w.secs);
        }
        let Some(key) = self.keys.iter_mut().find(|k| k.id == key_id) else {
            lock.failed((now * 1000) as i64);
            return Outcome::Refused("that invite key isn't known here");
        };
        let secret = hex::decode(&key.secret).unwrap_or_default();
        if !same(&proof(&secret, relay, peer, challenge), answer) {
            lock.failed((now * 1000) as i64);
            return Outcome::Refused("the invite key doesn't match");
        }
        lock.succeeded();
        if key.disabled || key.expires.is_some_and(|e| e < now) || key.max_uses.is_some_and(|m| key.uses >= m) {
            return Outcome::Refused("that invite key no longer admits anyone; ask for a new one");
        }
        key.uses += 1;
        let label = key.label.clone();
        self.pending.retain(|p| p.peer != peer && now.saturating_sub(p.at) <= PENDING_SECS);
        if self.pending.len() >= MAX_PENDING {
            return Outcome::Refused("too many requests are waiting; try again later");
        }
        self.pending.push(Pending { peer: peer.into(), name: name.chars().take(60).collect(), kind: kind.chars().take(20).collect(), key_label: label, at: now });
        Outcome::Waiting
    }

    /// The owner's answer to a waiting request.
    pub fn decide(&mut self, peer: &str, approve: bool) -> bool {
        let before = self.pending.len();
        self.pending.retain(|p| p.peer != peer);
        if before == self.pending.len() {
            return false;
        }
        if approve && !self.members.iter().any(|m| m == peer) {
            self.members.push(peer.into());
        }
        true
    }

    /// Remove (and with `block`, refuse from now on).
    pub fn remove(&mut self, peer: &str, block: bool) {
        self.members.retain(|m| m != peer);
        self.pending.retain(|p| p.peer != peer);
        if block && !self.blocked.iter().any(|b| b == peer) {
            self.blocked.push(peer.into());
        }
    }

    pub fn is_member(&self, peer: &str) -> bool {
        self.members.iter().any(|m| m == peer) && !self.blocked.iter().any(|b| b == peer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_the_standard() {
        // RFC 4231 test case 2.
        let mac = hmac(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(hex::encode(mac), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    #[test]
    fn a_good_proof_waits_for_the_owner_and_a_replay_fails() {
        let mut a = Admissions::default();
        let (id, secret) = a.new_key("Family", 100, None, None);
        let secret = hex::decode(secret).unwrap();
        let c = challenge();
        let good = proof(&secret, "relay", "ana", &c);
        assert_eq!(a.request(&id, &good, &c, "relay", "ana", "Ana's laptop", "pc", 100), Outcome::Waiting);
        assert!(!a.is_member("ana"), "a proof alone isn't membership");
        // The same answer for another peer, or on a new challenge, fails.
        assert!(matches!(a.request(&id, &good, &c, "relay", "eve", "Eve", "pc", 100), Outcome::Refused(_)));
        assert!(matches!(a.request(&id, &good, &challenge(), "relay", "ana", "Ana", "pc", 100), Outcome::Refused(_)));
        assert!(a.decide("ana", true));
        assert!(a.is_member("ana"));
        assert_eq!(a.request(&id, &good, &c, "relay", "ana", "Ana", "pc", 101), Outcome::AlreadyMember);
        a.remove("ana", true);
        assert!(!a.is_member("ana"));
        assert!(matches!(a.request(&id, &good, &c, "relay", "ana", "Ana", "pc", 102), Outcome::Refused("blocked by the relay's owner")));
    }

    #[test]
    fn rotation_and_lockout() {
        let mut a = Admissions::default();
        let (old, old_secret) = a.new_key("Friends", 0, None, None);
        let (new, new_secret) = a.rotate("Friends", 1);
        let c = challenge();
        let p = |s: &str, who: &str| proof(&hex::decode(s).unwrap(), "relay", who, &c);
        assert!(matches!(a.request(&old, &p(&old_secret, "bo"), &c, "relay", "bo", "Bo", "pc", 2), Outcome::Refused(_)), "the old key stopped");
        assert_eq!(a.request(&new, &p(&new_secret, "bo"), &c, "relay", "bo", "Bo", "pc", 2), Outcome::Waiting);
        for _ in 0..3 {
            assert!(matches!(a.request(&new, &[0u8; 32], &c, "relay", "x", "X", "pc", 3), Outcome::Refused(_)));
        }
        assert!(matches!(a.request(&new, &[0u8; 32], &c, "relay", "x", "X", "pc", 3), Outcome::Refused(_)));
        assert!(matches!(a.request(&new, &p(&new_secret, "cy"), &c, "relay", "cy", "Cy", "pc", 3), Outcome::Wait(_)), "guessing locks the key for a while");
    }
}
