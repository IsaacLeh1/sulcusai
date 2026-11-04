// SPDX-License-Identifier: AGPL-3.0-only
//! Peers seen before (design 5.2): per PeerId, the addresses that worked
//! last and the newest signed address record. Capped at 500 peers and 16
//! addresses each, aged out after 90 days unseen. The app keeps it sealed in
//! its database; this crate only shapes it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::records::AddressRecord;

pub const MAX_PEERS: usize = 500;
pub const MAX_ADDRS: usize = 16;
pub const AGE_OUT_SECS: u64 = 90 * 86_400;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Newest first.
    pub worked: Vec<String>,
    pub record: Option<AddressRecord>,
    pub seen: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressBook {
    pub peers: HashMap<String, Entry>,
}

impl AddressBook {
    /// An address that just connected moves to the front.
    pub fn worked(&mut self, peer: &str, addr: &str, now: u64) {
        let e = self.peers.entry(peer.to_string()).or_default();
        e.worked.retain(|a| a != addr);
        e.worked.insert(0, addr.to_string());
        e.worked.truncate(MAX_ADDRS);
        e.seen = now;
        self.trim();
    }

    /// A checked record; an older or equal `seq` than the one kept is ignored.
    pub fn record(&mut self, r: AddressRecord, now: u64) -> bool {
        let e = self.peers.entry(r.peer.clone()).or_default();
        if e.record.as_ref().is_some_and(|k| k.seq >= r.seq) {
            return false;
        }
        e.record = Some(r);
        e.seen = now;
        self.trim();
        true
    }

    /// Where to try a peer: what worked, then its record's addresses.
    pub fn addresses(&self, peer: &str) -> Vec<String> {
        let Some(e) = self.peers.get(peer) else { return Vec::new() };
        let mut out = e.worked.clone();
        for a in e.record.iter().flat_map(|r| r.addrs.iter()) {
            if !out.contains(a) {
                out.push(a.clone());
            }
        }
        out.truncate(MAX_ADDRS);
        out
    }

    pub fn age_out(&mut self, now: u64) {
        self.peers.retain(|_, e| now.saturating_sub(e.seen) <= AGE_OUT_SECS);
    }

    /// Over the cap, the least recently seen go.
    fn trim(&mut self) {
        if self.peers.len() <= MAX_PEERS {
            return;
        }
        let mut by_seen: Vec<(String, u64)> = self.peers.iter().map(|(k, e)| (k.clone(), e.seen)).collect();
        by_seen.sort_by_key(|(_, s)| *s);
        for (k, _) in by_seen.into_iter().take(self.peers.len() - MAX_PEERS) {
            self.peers.remove(&k);
        }
    }
}
