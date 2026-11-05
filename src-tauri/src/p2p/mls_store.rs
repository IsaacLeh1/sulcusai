// SPDX-License-Identifier: AGPL-3.0-only
//! This PC's MLS groups (`sulcus_net::mls`), kept sealed with the data key:
//! a group's secrets never touch the disk in the clear, and every change
//! (an add, a remove, a letter sent or opened, which moves the ratchets) is
//! saved before its result leaves this module, so a crash can't make this
//! PC reuse a key or forget an epoch. A group is named by the app's own id
//! (a People group, or `person:<id>` for one person).

use std::collections::BTreeMap;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use rusqlite::Connection;
use sha2::Digest as _;
use sulcus_net::mls::{Group, Member, Received};

use crate::crypto::Cipher;
use crate::db;

/// Joined groups, by app id.
const GROUPS: &str = "mls_groups";
/// Members whose key package went out, waiting for a welcome.
const WAITING: &str = "mls_waiting";

fn load(conn: &Connection, key: &str) -> BTreeMap<String, String> {
    db::get(conn, key).unwrap_or_default()
}

fn open(c: &Cipher, stored: &str) -> Result<Vec<u8>, String> {
    c.open_bytes(&B64.decode(stored).map_err(|_| "Saved group keys are damaged.")?)
}

fn seal(c: &Cipher, raw: &[u8]) -> String {
    B64.encode(c.seal_bytes(raw))
}

fn group(conn: &Connection, c: &Cipher, id: &str) -> Result<Group, String> {
    let all = load(conn, GROUPS);
    let stored = all.get(id).ok_or("This PC isn't in that group's keys yet.")?;
    Group::from_bytes(&open(c, stored)?)
}

fn save(conn: &Connection, c: &Cipher, id: &str, g: &Group) -> Result<(), String> {
    let mut all = load(conn, GROUPS);
    all.insert(id.to_string(), seal(c, &g.to_bytes()?));
    db::set(conn, GROUPS, &all)
}

/// Runs `f` on a group and saves it before handing back the result.
fn with<R>(conn: &Connection, c: &Cipher, id: &str, f: impl FnOnce(&mut Group) -> Result<R, String>) -> Result<R, String> {
    let mut g = group(conn, c, id)?;
    let out = f(&mut g)?;
    save(conn, c, id, &g)?;
    Ok(out)
}

pub fn has(conn: &Connection, id: &str) -> bool {
    load(conn, GROUPS).contains_key(id)
}

/// Starts a group with only this PC (`me` is its node id).
pub fn create(conn: &Connection, c: &Cipher, id: &str, me: &str) -> Result<(), String> {
    if has(conn, id) {
        return Err("That group already has keys.".into());
    }
    save(conn, c, id, &Group::create(Member::new(me)?)?)
}

/// A key package for whoever adds this PC to group `id`; the member that
/// made it is kept (sealed) for the welcome.
pub fn key_package(conn: &Connection, c: &Cipher, id: &str, me: &str) -> Result<Vec<u8>, String> {
    let m = Member::new(me)?;
    let kp = m.key_package()?;
    let mut waiting = load(conn, WAITING);
    waiting.insert(id.to_string(), seal(c, &m.to_bytes()?));
    db::set(conn, WAITING, &waiting)?;
    Ok(kp)
}

/// Joins group `id` from the welcome that answered this PC's key package.
pub fn join(conn: &Connection, c: &Cipher, id: &str, welcome: &[u8]) -> Result<(), String> {
    let mut waiting = load(conn, WAITING);
    let stored = waiting.get(id).ok_or("No invitation to that group is waiting.")?;
    let g = Group::join(Member::from_bytes(&open(c, stored)?)?, welcome)?;
    save(conn, c, id, &g)?;
    waiting.remove(id);
    db::set(conn, WAITING, &waiting)
}

/// Adds a PC; returns the commit for current members and the newcomer's welcome.
pub fn add(conn: &Connection, c: &Cipher, id: &str, key_package: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
    with(conn, c, id, |g| g.add(key_package))
}

/// Removes a PC by node id; the commit re-keys the group without it.
pub fn remove(conn: &Connection, c: &Cipher, id: &str, peer: &str) -> Result<Vec<u8>, String> {
    with(conn, c, id, |g| g.remove(peer))
}

pub fn members(conn: &Connection, c: &Cipher, id: &str) -> Result<Vec<String>, String> {
    Ok(group(conn, c, id)?.members())
}

/// Seals a letter for the group's members now.
pub fn send(conn: &Connection, c: &Cipher, id: &str, body: &[u8]) -> Result<Vec<u8>, String> {
    with(conn, c, id, |g| g.send(body))
}

/// Hashes of letters already opened, per group. OpenMLS refuses a replay
/// only while the group stays in memory; its saved state doesn't record
/// which messages were used, so after a reload this list does.
const SEEN: &str = "mls_seen";
const SEEN_KEEP: usize = 5000;

/// Opens a letter or applies a commit. A failed open saves nothing.
pub fn receive(conn: &Connection, c: &Cipher, id: &str, raw: &[u8]) -> Result<Received, String> {
    let h = hex::encode(sha2::Sha256::digest(raw));
    let mut seen: BTreeMap<String, Vec<String>> = db::get(conn, SEEN).unwrap_or_default();
    let list = seen.entry(id.to_string()).or_default();
    if list.contains(&h) {
        return Err("That group letter was already opened once.".into());
    }
    let out = with(conn, c, id, |g| g.receive(raw))?;
    list.push(h);
    if list.len() > SEEN_KEEP {
        list.drain(..list.len() - SEEN_KEEP);
    }
    db::set(conn, SEEN, &seen)?;
    Ok(out)
}

/// Forgets a group's keys (after leaving it or being removed).
pub fn forget(conn: &Connection, id: &str) -> Result<(), String> {
    let mut seen: BTreeMap<String, Vec<String>> = db::get(conn, SEEN).unwrap_or_default();
    seen.remove(id);
    db::set(conn, SEEN, &seen)?;
    let mut all = load(conn, GROUPS);
    all.remove(id);
    db::set(conn, GROUPS, &all)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init_for_test(&conn);
        conn
    }

    #[test]
    fn groups_survive_on_disk_sealed() {
        let c = Cipher::for_test();
        // Two PCs, each with its own database.
        let (ana, bo) = (mem(), mem());
        create(&ana, &c, "g1", "ana").unwrap();
        let kp = key_package(&bo, &c, "g1", "bo").unwrap();
        let (_, welcome) = add(&ana, &c, "g1", &kp).unwrap();
        join(&bo, &c, "g1", &welcome).unwrap();
        assert_eq!(members(&bo, &c, "g1").unwrap().len(), 2);

        let letter = send(&ana, &c, "g1", b"hello group").unwrap();
        assert_eq!(receive(&bo, &c, "g1", &letter).unwrap(), Received::Letter { from: "ana".into(), body: b"hello group".to_vec() });
        // The saved ratchet moved on, so a replay is refused after reloading.
        assert!(receive(&bo, &c, "g1", &letter).is_err());

        // Nothing readable on disk, and another data key opens nothing.
        let stored: BTreeMap<String, String> = db::get(&bo, GROUPS).unwrap();
        let raw = B64.decode(&stored["g1"]).unwrap();
        assert_ne!(open(&c, &stored["g1"]).unwrap(), raw, "what is stored is sealed");
        assert!(group(&bo, &Cipher::random(), "g1").is_err());

        let commit = remove(&ana, &c, "g1", "bo").unwrap();
        assert_eq!(receive(&bo, &c, "g1", &commit).unwrap(), Received::Changed);
        let after = send(&ana, &c, "g1", b"later").unwrap();
        assert!(receive(&bo, &c, "g1", &after).is_err());
        forget(&bo, "g1").unwrap();
        assert!(!has(&bo, "g1"));
    }
}
