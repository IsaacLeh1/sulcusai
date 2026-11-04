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
}

pub fn load(conn: &rusqlite::Connection) -> P2pSettings {
    db::get(conn, KEY).unwrap_or_default()
}

pub fn save(conn: &rusqlite::Connection, s: &P2pSettings) -> Result<(), String> {
    db::set(conn, KEY, s)
}
