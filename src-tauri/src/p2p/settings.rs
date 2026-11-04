// SPDX-License-Identifier: AGPL-3.0-only
//! Network settings. Everything starts off: with Network off, no part of
//! the peer-to-peer node runs.

use serde::{Deserialize, Serialize};

use crate::db;

const KEY: &str = "p2p";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct P2pSettings {
    pub network: bool,
    /// Find SulcusAI PCs on the same network (mDNS).
    pub lan_discovery: bool,
    /// Ask the router to forward a port to this PC (UPnP).
    pub router_ports: bool,
    /// Relay for people who can't reach each other directly.
    pub relay_for_others: bool,
    /// Relays this PC stays reachable through (addresses ending in /p2p/<id>).
    pub relays: Vec<String>,
    /// The port "Run a relay on this PC" last used.
    pub local_relay_port: Option<u16>,
    /// The guide for hosting a relay on one's own Oracle or Cloudflare account.
    pub own_hosting: bool,
}

pub fn load(conn: &rusqlite::Connection) -> P2pSettings {
    db::get(conn, KEY).unwrap_or_default()
}

pub fn save(conn: &rusqlite::Connection, s: &P2pSettings) -> Result<(), String> {
    db::set(conn, KEY, s)
}
