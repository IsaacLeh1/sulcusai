// SPDX-License-Identifier: AGPL-3.0-only
//! Invites (design section 5.1): a signed record carrying who invites, how
//! to reach them and a one-time secret. Text form `sulcus:i1.…`, which the
//! app also opens from a `sulcus:` link. The inviter keeps only a hash of
//! each secret, with how many uses are left and when it expires.

use std::collections::HashMap;

use libp2p::identity::Keypair;
use libp2p::{Multiaddr, PeerId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::signed::{self, Bad};

pub const PREFIX: &str = "sulcus:i1";
const DOMAIN: &str = "SulcusAI invite v1\n";
pub const MAX_ADDRS: usize = 8;
pub const MAX_RELAYS: usize = 4;
pub const DEFAULT_DAYS: u64 = 7;
const MAX_NAME: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Friend,
    Group,
    Relay,
    Device,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relay {
    pub peer: String,
    pub addrs: Vec<String>,
}

/// The invite's body, with short keys so the QR code stays small.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    #[serde(rename = "k")]
    pub kind: Kind,
    #[serde(rename = "f")]
    pub from: String,
    #[serde(rename = "n")]
    pub name: String,
    #[serde(rename = "a", default)]
    pub addrs: Vec<String>,
    #[serde(rename = "r", default)]
    pub relays: Vec<Relay>,
    /// 16 random bytes, hex.
    #[serde(rename = "s")]
    pub secret: String,
    #[serde(rename = "g", default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Unix seconds; 0 means until the inviter rotates it (relay keys).
    #[serde(rename = "e")]
    pub expires: u64,
}

/// What an invite offers, once its signature and contents checked out.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    pub invite: Invite,
    pub from: PeerId,
    pub addrs: Vec<Multiaddr>,
}

fn random_secret() -> [u8; 16] {
    let mut s = [0u8; 16];
    getrandom::getrandom(&mut s).expect("the OS random source works");
    s
}

pub fn secret_hash(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.trim().to_ascii_lowercase().as_bytes()))
}

/// A new invite from `key`, and the secret's hash for the inviter's book.
pub fn make(key: &Keypair, kind: Kind, name: &str, addrs: &[Multiaddr], relays: Vec<Relay>, days: Option<u64>, group: Option<String>) -> (String, String) {
    let secret = hex::encode(random_secret());
    let expires = days.map_or(0, |d| signed::now() + d.max(1) * 86_400);
    let inv = Invite {
        kind,
        from: key.public().to_peer_id().to_string(),
        name: name.trim().chars().take(MAX_NAME).collect(),
        addrs: addrs.iter().take(MAX_ADDRS).map(|a| a.to_string()).collect(),
        relays: relays.into_iter().take(MAX_RELAYS).collect(),
        secret: secret.clone(),
        group,
        expires,
    };
    (signed::seal(PREFIX, DOMAIN, key, &inv), secret_hash(&secret))
}

#[derive(Debug, PartialEq)]
pub enum InviteError {
    Bad(Bad),
    Expired,
    Contents(&'static str),
}

impl std::fmt::Display for InviteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InviteError::Bad(b) => write!(f, "This isn't a SulcusAI invite: {b}."),
            InviteError::Expired => f.write_str("This invite has expired. Ask for a new one."),
            InviteError::Contents(why) => write!(f, "This invite can't be used: {why}."),
        }
    }
}

/// Reads an invite from its text (or a `sulcus:` link's text) and checks it.
pub fn read(text: &str, now: u64) -> Result<Checked, InviteError> {
    let (inv, signer): (Invite, PeerId) = signed::open(PREFIX, DOMAIN, text).map_err(InviteError::Bad)?;
    if inv.from != signer.to_string() {
        return Err(InviteError::Bad(Bad::WrongSigner));
    }
    if inv.expires != 0 && inv.expires < now {
        return Err(InviteError::Expired);
    }
    if inv.secret.len() != 32 || !inv.secret.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(InviteError::Contents("its secret is malformed"));
    }
    if inv.addrs.len() > MAX_ADDRS || inv.relays.len() > MAX_RELAYS || inv.relays.iter().any(|r| r.addrs.len() > MAX_ADDRS) {
        return Err(InviteError::Contents("it lists too many addresses"));
    }
    if inv.name.chars().count() > MAX_NAME {
        return Err(InviteError::Contents("its name is too long"));
    }
    let addrs = inv.addrs.iter().filter_map(|a| a.parse::<Multiaddr>().ok()).collect();
    Ok(Checked { from: signer, addrs, invite: inv })
}

/// The inviter's record of invites made: secrets hashed, with uses left.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Book {
    pub open: HashMap<String, Issued>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Issued {
    pub kind: Kind,
    pub uses_left: u32,
    pub expires: u64,
}

impl Book {
    pub fn add(&mut self, hash: String, kind: Kind, uses: u32, expires: u64) {
        self.open.insert(hash, Issued { kind, uses_left: uses.max(1), expires });
    }

    /// A secret someone presented: the kind it was made for, using one use.
    pub fn redeem(&mut self, secret: &str, now: u64) -> Option<Kind> {
        let hash = secret_hash(secret);
        let issued = self.open.get_mut(&hash)?;
        if issued.expires != 0 && issued.expires < now {
            self.open.remove(&hash);
            return None;
        }
        let kind = issued.kind;
        issued.uses_left -= 1;
        if issued.uses_left == 0 {
            self.open.remove(&hash);
        }
        Some(kind)
    }

    pub fn forget_expired(&mut self, now: u64) {
        self.open.retain(|_, i| i.expires == 0 || i.expires >= now);
    }
}
