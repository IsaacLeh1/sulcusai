// SPDX-License-Identifier: AGPL-3.0-only

use rusqlite::Connection;

use super::*;
use crate::crypto::Cipher;

fn conn() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    crate::db::init_for_test(&c);
    c
}

#[test]
fn network_starts_off() {
    let s = settings::load(&conn());
    assert!(!s.network);
    assert!(!allowed(&s, false));
}

#[test]
fn fully_offline_without_the_local_network_keeps_the_node_off() {
    let on = P2pSettings { network: true };
    assert!(allowed(&on, false));
    assert!(!allowed(&on, true));
}

#[test]
fn node_key_is_made_once_sealed_and_kept() {
    let (db, c) = (conn(), Cipher::for_test());
    let first = keys::node_key(&db, &c).unwrap();
    let stored: String = crate::db::get(&db, "p2p_node_key").unwrap();
    assert!(Cipher::is_encrypted(&stored));
    assert!(!stored.contains(&hex::encode(&first[..])));
    assert_eq!(keys::node_key(&db, &c).unwrap(), first);
}

#[test]
fn an_unreadable_node_key_is_not_replaced() {
    let (db, c) = (conn(), Cipher::for_test());
    crate::db::set(&db, "p2p_node_key", &"plain text".to_string()).unwrap();
    assert!(keys::node_key(&db, &c).is_err());
    let still: String = crate::db::get(&db, "p2p_node_key").unwrap();
    assert_eq!(still, "plain text");
}
