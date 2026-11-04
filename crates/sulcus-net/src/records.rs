// SPDX-License-Identifier: AGPL-3.0-only
//! Address records (design 5.2: where a peer can be reached, signed by that
//! peer) and rosters (3.2: a person's devices, signed by their host PC).
//! A newer `seq` replaces an older one; an older or equal one is ignored, so
//! a stale record passed around can't undo a fresh one.

use libp2p::identity::Keypair;
use libp2p::PeerId;
use serde::{Deserialize, Serialize};

use crate::signed::{self, Bad};

const ADDR_PREFIX: &str = "sulcus:a1";
const ADDR_DOMAIN: &str = "SulcusAI address v1\n";
const ROSTER_PREFIX: &str = "sulcus:r1";
const ROSTER_DOMAIN: &str = "SulcusAI roster v1\n";
pub const MAX_ADDRS: usize = 16;
pub const MAX_DEVICES: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressRecord {
    pub peer: String,
    pub addrs: Vec<String>,
    /// Relays this peer holds reservations on.
    #[serde(default)]
    pub relays: Vec<String>,
    pub seq: u64,
    pub issued: u64,
}

impl AddressRecord {
    pub fn seal(&self, key: &Keypair) -> String {
        signed::seal(ADDR_PREFIX, ADDR_DOMAIN, key, self)
    }

    pub fn open(text: &str) -> Result<AddressRecord, Bad> {
        let (r, signer): (AddressRecord, PeerId) = signed::open(ADDR_PREFIX, ADDR_DOMAIN, text)?;
        if r.peer != signer.to_string() {
            return Err(Bad::WrongSigner);
        }
        if r.addrs.len() > MAX_ADDRS || r.relays.len() > MAX_ADDRS {
            return Err(Bad::Body("too many addresses".into()));
        }
        Ok(r)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterDevice {
    pub node_id: String,
    pub name: String,
    /// "pc" or "phone".
    pub kind: String,
    pub added: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Roster {
    pub version: u64,
    pub person_name: String,
    pub devices: Vec<RosterDevice>,
    #[serde(default)]
    pub revoked: Vec<String>,
    pub issued: u64,
    /// Room for a person-level key later (phase 15); the host PC for now.
    #[serde(default)]
    pub signer: Option<String>,
}

impl Roster {
    pub fn seal(&self, key: &Keypair) -> String {
        signed::seal(ROSTER_PREFIX, ROSTER_DOMAIN, key, self)
    }

    /// A roster and the host PC that signed it. Whether that host is the one
    /// already trusted for this person is the caller's check (`newer`).
    pub fn open(text: &str) -> Result<(Roster, PeerId), Bad> {
        let (r, signer): (Roster, PeerId) = signed::open(ROSTER_PREFIX, ROSTER_DOMAIN, text)?;
        if r.devices.len() > MAX_DEVICES || r.revoked.len() > MAX_DEVICES * 4 {
            return Err(Bad::Body("too many devices".into()));
        }
        if !r.devices.iter().any(|d| d.node_id == signer.to_string()) {
            return Err(Bad::WrongSigner);
        }
        Ok((r, signer))
    }

    pub fn trusts(&self, node: &PeerId) -> bool {
        let id = node.to_string();
        self.devices.iter().any(|d| d.node_id == id) && !self.revoked.contains(&id)
    }
}

/// Whether `incoming` (signed by `signer`) should replace `known` (whose
/// host is `known_host`): same host and a higher version. A different host
/// is the "This person's PC key changed" case, which never replaces silently.
pub fn newer(known: Option<(&Roster, &PeerId)>, incoming: &Roster, signer: &PeerId) -> Result<bool, &'static str> {
    match known {
        None => Ok(true),
        Some((k, host)) if host == signer => Ok(incoming.version > k.version),
        Some(_) => Err("This person's PC key changed. Check with them before trusting it."),
    }
}
