// SPDX-License-Identifier: AGPL-3.0-only
//! Invites in the app: making one (its secret kept hashed in a sealed
//! book), reading one someone sent, and using it to become friends. Who
//! joined and the addresses that worked are kept sealed too. Needs the
//! Network switch on; until relays (M4) an invite only carries this PC's
//! own addresses, so it works on the same network or with a forwarded port.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sulcus_net::addrbook::AddressBook;
use sulcus_net::invite::{self, Book, Kind};
use sulcus_net::libp2p::multiaddr::Protocol;
use sulcus_net::libp2p::Multiaddr;
use sulcus_net::{friend, signed};

use crate::crypto::Cipher;
use crate::{db, AppState, AppStateRef};

const BOOK: &str = "p2p_invites";
pub(crate) const FRIENDS: &str = "p2p_friends";
const ADDRS: &str = "p2p_addrbook";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Friend {
    pub peer: String,
    pub name: String,
    pub kind: Kind,
    pub added: u64,
}

pub(crate) fn load_sealed<T: for<'de> Deserialize<'de> + Default>(conn: &rusqlite::Connection, c: &Cipher, key: &str) -> T {
    db::get::<String>(conn, key).and_then(|s| c.decrypt(&s).ok()).and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default()
}

pub(crate) fn save_sealed<T: Serialize>(conn: &rusqlite::Connection, c: &Cipher, key: &str, v: &T) -> Result<(), String> {
    db::set(conn, key, &c.encrypt(&serde_json::to_string(v).map_err(|e| e.to_string())?))
}

/// Addresses worth putting in an invite: not loopback, not "any".
pub fn shareable(addrs: &[Multiaddr], lan_too: bool) -> Vec<Multiaddr> {
    addrs
        .iter()
        .filter(|a| {
            a.iter().all(|p| match p {
                Protocol::Ip4(ip) => !ip.is_loopback() && !ip.is_unspecified() && (lan_too || !ip.is_private()),
                Protocol::Ip6(ip) => !ip.is_loopback() && !ip.is_unspecified(),
                _ => true,
            })
        })
        .cloned()
        .collect()
}

/// Starts answering invites on the running node; called when it starts.
pub fn answer(state: &Arc<AppState>, node: &sulcus_net::NodeHandle) {
    let Some(c) = state.vault.lock().unwrap().cipher() else { return };
    let (book, name) = {
        let conn = state.db.lock().unwrap();
        let mut b: Book = load_sealed(&conn, &c, BOOK);
        b.forget_expired(signed::now());
        (b, db::profile(&conn, &c).name)
    };
    let book = Arc::new(Mutex::new(book));
    let (st, bk) = (state.clone(), book.clone());
    friend::answer_invites(node, book, if name.trim().is_empty() { "SulcusAI".into() } else { name }, None, move |j| {
        let Some(c) = st.vault.lock().unwrap().cipher() else { return };
        let conn = st.db.lock().unwrap();
        let _ = save_sealed(&conn, &c, BOOK, &*bk.lock().unwrap());
        let mut friends: Vec<Friend> = load_sealed(&conn, &c, FRIENDS);
        friends.retain(|f| f.peer != j.peer.to_string());
        friends.push(Friend { peer: j.peer.to_string(), name: j.name.clone(), kind: j.kind, added: signed::now() });
        let _ = save_sealed(&conn, &c, FRIENDS, &friends);
        if let Some(r) = j.record {
            let mut ab: AddressBook = load_sealed(&conn, &c, ADDRS);
            ab.record(r, signed::now());
            let _ = save_sealed(&conn, &c, ADDRS, &ab);
        }
        crate::applog::line("network", &format!("{} used an invite", j.name));
    });
}

#[derive(Serialize)]
pub struct Made {
    text: String,
    /// What the invite shows anyone who gets it.
    warning: String,
    addresses: usize,
}

#[tauri::command]
pub fn p2p_make_invite(state: AppStateRef, kind: Kind, days: Option<u64>, uses: Option<u32>) -> Result<Made, String> {
    let node = super::running().ok_or("Turn on Network first.")?;
    let c = state.vault.lock().unwrap().cipher().ok_or("Unlock SulcusAI first.")?;
    let conn = state.db.lock().unwrap();
    let key = super::keys::node_key(&conn, &c)?;
    let key = sulcus_net::identity::load(&key).map_err(|e| e.to_string())?;
    let hide = !crate::shield::peers::rules(&crate::shield::live::current()).share_addresses;
    let addrs: Vec<Multiaddr> = shareable(&node.listening(), true).into_iter().filter(|a| !hide || a.iter().any(|p| matches!(p, Protocol::P2pCircuit))).collect();
    if addrs.is_empty() {
        return Err(if hide { "Your address is hidden from peers, so an invite needs a relay first (add one below).".into() } else { "This PC has no address others can reach yet. Check the network connection.".to_string() });
    }
    let name = db::profile(&conn, &c).name;
    let days = days.unwrap_or(invite::DEFAULT_DAYS).clamp(1, 30);
    let (text, hash) = invite::make(&key, kind, if name.trim().is_empty() { "SulcusAI" } else { &name }, &addrs, Vec::new(), Some(days), None);
    let mut book: Book = load_sealed(&conn, &c, BOOK);
    book.add(hash, kind, uses.unwrap_or(1).clamp(1, 20), signed::now() + days * 86_400);
    save_sealed(&conn, &c, BOOK, &book)?;
    Ok(Made { text, warning: "This invite contains your PC's internet address. Anyone who gets it can see it.".into(), addresses: addrs.len() })
}

#[derive(Serialize)]
pub struct Read {
    kind: Kind,
    name: String,
    from: String,
    expires: u64,
}

#[tauri::command(async)]
pub fn p2p_read_invite(text: String) -> Result<Read, String> {
    let c = invite::read(&text, signed::now()).map_err(|e| e.to_string())?;
    Ok(Read { kind: c.invite.kind, name: c.invite.name, from: c.from.to_string(), expires: c.invite.expires })
}

#[tauri::command]
pub async fn p2p_use_invite(state: AppStateRef<'_>, text: String) -> Result<Friend, String> {
    let node = super::running().ok_or("Turn on Network first.")?;
    let checked = invite::read(&text, signed::now()).map_err(|e| e.to_string())?;
    if checked.from == node.peer_id() {
        return Err("That's this PC's own invite.".into());
    }
    let (c, me) = {
        let c = state.vault.lock().unwrap().cipher().ok_or("Unlock SulcusAI first.")?;
        let me = db::profile(&state.db.lock().unwrap(), &c).name;
        (c, me)
    };
    // "Don't show my address to peers": reach them only through relays.
    let hide = !crate::shield::peers::rules(&crate::shield::live::current()).dial_direct;
    let (name, record) = friend::use_invite(&node, &checked, if me.trim().is_empty() { "SulcusAI" } else { &me }, None, hide).await?;
    let f = Friend { peer: checked.from.to_string(), name: if name.is_empty() { checked.invite.name.clone() } else { name }, kind: checked.invite.kind, added: signed::now() };
    let conn = state.db.lock().unwrap();
    let mut friends: Vec<Friend> = load_sealed(&conn, &c, FRIENDS);
    friends.retain(|x| x.peer != f.peer);
    friends.push(f.clone());
    save_sealed(&conn, &c, FRIENDS, &friends)?;
    let mut ab: AddressBook = load_sealed(&conn, &c, ADDRS);
    for a in &checked.addrs {
        ab.worked(&f.peer, &a.to_string(), signed::now());
    }
    if let Some(r) = record {
        ab.record(r, signed::now());
    }
    save_sealed(&conn, &c, ADDRS, &ab)?;
    Ok(f)
}

/// Friends' network ids, for who this PC relays for.
pub(crate) fn friend_peers(state: &AppState) -> Vec<String> {
    let Some(c) = state.vault.lock().unwrap().cipher() else { return Vec::new() };
    load_sealed::<Vec<Friend>>(&state.db.lock().unwrap(), &c, FRIENDS).into_iter().map(|f| f.peer).collect()
}

#[tauri::command(async)]
pub fn p2p_friends(state: AppStateRef) -> Result<Vec<Friend>, String> {
    let c = state.vault.lock().unwrap().cipher().ok_or("Unlock SulcusAI first.")?;
    Ok(load_sealed(&state.db.lock().unwrap(), &c, FRIENDS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invites_carry_reachable_addresses_only() {
        let addrs: Vec<Multiaddr> = ["/ip4/127.0.0.1/tcp/1", "/ip4/0.0.0.0/tcp/2", "/ip4/192.168.1.5/tcp/3", "/ip4/8.8.8.8/udp/4/quic-v1", "/ip6/::1/tcp/5"].iter().map(|a| a.parse().unwrap()).collect();
        let lan: Vec<String> = shareable(&addrs, true).iter().map(|a| a.to_string()).collect();
        assert_eq!(lan, vec!["/ip4/192.168.1.5/tcp/3", "/ip4/8.8.8.8/udp/4/quic-v1"]);
        let public: Vec<String> = shareable(&addrs, false).iter().map(|a| a.to_string()).collect();
        assert_eq!(public, vec!["/ip4/8.8.8.8/udp/4/quic-v1"]);
    }
}
