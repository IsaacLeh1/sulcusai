// SPDX-License-Identifier: AGPL-3.0-only
//! People: link with someone else's SulcusAI by a code, chat with them
//! directly, and let them watch chats you share (DESIGN.md §4.11). What goes
//! between people never goes to a model unless an assistant is asked.
//!
//! - Linking works like pairing PCs (`wire`: SPAKE2 with a code shown on one
//!   side), but a person is a contact, not a paired PC: a contact can never
//!   sync, or list or copy models.
//! - The apps connect directly and stay connected while both run (`link`):
//!   on the same network, or on a virtual one (Radmin VPN, Tailscale,
//!   ZeroTier) by the address the other side shows. It's enough that one
//!   side can be reached.
//! - Messages are stored encrypted with this PC's data key; files sent with
//!   them are sealed the same way under the data folder.
//! - Shared chats (`share`) are seen live and read-only, never stored there.
//! - Off means off: with the feature off this PC doesn't take calls from
//!   people, doesn't call them, and its commands refuse.

mod link;
pub mod share;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};
use tokio::net::TcpStream;
use tokio::time::timeout;

use super::{clean_code, device_id, lost, new_code, recv_plain, send_plain, transcript, wire, Accept, Busy, Open, PairReply, Pairing};
use super::{ANNOUNCE, AWAY, CODE_FOR, CODE_TRIES, HANDSHAKE, LIVE, NEARBY, PORT};
use crate::crypto::Cipher;
use crate::features::{self, Feature};
use crate::{db, AppState, AppStateRef};

pub(super) use link::{answer, start};
pub use share::{chat_event, turn_changed};

const KEY: &str = "people";
/// What a call asks for (`Open::want`): linking, or the live link.
pub(super) const LINK: &str = "person";
pub(super) const CHAT: &str = "people";
/// The biggest file a message can carry.
pub const MAX_FILE: u64 = 16 << 20;
const MAX_TEXT: usize = 20_000;
/// What one batch of messages carries at most (a frame holds 32 MB); the
/// rest goes once they confirm it.
const SEND_BUDGET: usize = 24 << 20;
const UNEXPECTED: &str = "Their app sent something unexpected.";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Book {
    /// This user's name, as the people they link with see it.
    name: String,
    contacts: Vec<Contact>,
    /// Chats this user shares, and with whom.
    shares: Vec<share::Share>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Contact {
    /// Their app's id (`device_id`).
    id: String,
    name: String,
    /// The link key, sealed with the data key.
    key: String,
    /// Where they last answered, or said they take calls.
    addr: Option<String>,
    added_at: i64,
    last_seen: Option<i64>,
}

fn book(conn: &Connection) -> Book {
    let mut b: Book = db::get(conn, KEY).unwrap_or_default();
    if b.name.trim().is_empty() {
        b.name = default_name();
    }
    b
}

/// The Windows account's name, capitalized ("isaac" → "Isaac").
fn default_name() -> String {
    let user = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default();
    let mut chars = user.trim().chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Someone".into(),
    }
}

fn update<R>(state: &AppState, f: impl FnOnce(&mut Book) -> R) -> Result<R, String> {
    let conn = state.db.lock().unwrap();
    let mut b = book(&conn);
    let r = f(&mut b);
    db::set(&conn, KEY, &b)?;
    Ok(r)
}

fn contact(state: &AppState, id: &str) -> Option<Contact> {
    book(&state.db.lock().unwrap()).contacts.into_iter().find(|p| p.id == id)
}

fn my_name(state: &AppState) -> String {
    book(&state.db.lock().unwrap()).name
}

fn link_key(c: &Cipher, p: &Contact) -> Result<zeroize::Zeroizing<Vec<u8>>, String> {
    let hex = zeroize::Zeroizing::new(c.decrypt(&p.key)?);
    hex::decode(hex.as_str()).map(zeroize::Zeroizing::new).map_err(|_| format!("The link with {} is damaged. Link again.", p.name))
}

/// Ids (app ids, message ids) are short and safe to use as file names.
fn safe_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn clean_name(name: &str) -> String {
    let s: String = name.chars().filter(|c| !c.is_control()).take(60).collect();
    let s = s.trim();
    if s.is_empty() { "Someone".into() } else { s.to_string() }
}

/// A file name someone sent: no folders, nothing Windows refuses.
fn clean_file_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let s: String = last.chars().filter(|c| !c.is_control() && !"<>:\"|?*".contains(*c)).take(120).collect();
    let s = s.trim().trim_matches('.').trim();
    if s.is_empty() { "file".into() } else { s.to_string() }
}

fn files_dir(state: &AppState, contact: &str) -> PathBuf {
    state.paths.data.join("people").join(contact)
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Runs a task on the runtime this is called from (the app's, or a test's).
fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(f: F) {
    match tokio::runtime::Handle::try_current() {
        Ok(h) => {
            h.spawn(f);
        }
        Err(_) => {
            tauri::async_runtime::spawn(f);
        }
    }
}

// ---------- what's happening now ----------

/// The code this PC is showing for someone to link with.
static CODE: Mutex<Option<Pairing>> = Mutex::new(None);

fn code_showing() -> bool {
    CODE.lock().unwrap().as_ref().is_some_and(|p| p.until > Instant::now())
}

/// Whether to announce on this network that a code is showing.
pub(super) fn showing_code() -> bool {
    code_showing()
}

/// Whether People needs this PC to take calls: it's on, and someone is
/// linked or a code is showing.
pub(super) fn wants_calls(conn: &Connection) -> bool {
    features::is_on(conn, Feature::People) && (code_showing() || !book(conn).contacts.is_empty())
}

pub(super) fn stopped() {
    *CODE.lock().unwrap() = None;
}

fn listening_port() -> u16 {
    LIVE.lock().unwrap().as_ref().map_or(0, |l| l.port)
}

fn emit(state: &AppState, event: &str, payload: serde_json::Value) {
    if let Some(app) = state.app.get() {
        let _ = app.emit(event, payload);
    }
}

fn changed(state: &AppState) {
    emit(state, "people:changed", serde_json::Value::Null);
}

/// The sync loop listens; it needs a nudge when People starts or stops needing that.
fn listen_again() {
    super::WAKE.notify_one();
    ANNOUNCE.notify_one();
}

// ---------- messages ----------

#[derive(Debug, Clone, Serialize)]
pub struct FileView {
    name: String,
    size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageView {
    id: String,
    from_me: bool,
    at: i64,
    text: String,
    file: Option<FileView>,
    /// For messages from here: they have it.
    delivered: bool,
}

/// A message as it travels (inside the link's encryption).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Item {
    id: String,
    at: i64,
    text: String,
    file: Option<FileItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct FileItem {
    name: String,
    /// Base64.
    data: String,
}

fn row_view(c: &Cipher, r: &rusqlite::Row) -> rusqlite::Result<MessageView> {
    let file_name: Option<String> = r.get(4)?;
    let file_size: Option<i64> = r.get(5)?;
    Ok(MessageView {
        id: r.get(0)?,
        from_me: r.get::<_, i64>(1)? != 0,
        at: r.get(2)?,
        text: c.decrypt_or(&r.get::<_, String>(3)?, ""),
        file: file_name.map(|n| FileView { name: c.decrypt_or(&n, "file"), size: file_size.unwrap_or(0).max(0) as u64 }),
        delivered: r.get::<_, i64>(6)? != 0,
    })
}

const COLUMNS: &str = "id, from_me, at, body, file_name, file_size, sent";

fn messages(conn: &Connection, c: &Cipher, contact: &str) -> Result<Vec<MessageView>, String> {
    let mut stmt = conn
        .prepare(&format!("SELECT {COLUMNS} FROM people_messages WHERE contact_id = ?1 ORDER BY at, rowid"))
        .map_err(err)?;
    let rows = stmt.query_map([contact], |r| row_view(c, r)).map_err(err)?;
    Ok(rows.filter_map(Result::ok).collect())
}

fn last_message(conn: &Connection, c: &Cipher, contact: &str) -> Option<MessageView> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM people_messages WHERE contact_id = ?1 ORDER BY at DESC, rowid DESC LIMIT 1"),
        [contact],
        |r| row_view(c, r),
    )
    .optional()
    .ok()
    .flatten()
}

fn unread(conn: &Connection) -> HashMap<String, i64> {
    let Ok(mut stmt) = conn.prepare("SELECT contact_id, COUNT(*) FROM people_messages WHERE from_me = 0 AND read = 0 GROUP BY contact_id") else {
        return HashMap::new();
    };
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

/// What's waiting for someone (leaving out what's already on its way), up
/// to what one batch carries.
fn outgoing(state: &AppState, c: &Cipher, contact: &str, skip: &HashSet<String>) -> Result<Vec<Item>, String> {
    let rows: Vec<(String, i64, String, Option<String>)> = {
        let conn = state.db.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, at, body, file_name FROM people_messages WHERE contact_id = ?1 AND from_me = 1 AND sent = 0 ORDER BY at, rowid")
            .map_err(err)?;
        let rows = stmt.query_map([contact], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map_err(err)?;
        rows.filter_map(Result::ok).filter(|r: &(String, i64, String, Option<String>)| !skip.contains(&r.0)).collect()
    };
    let mut items = Vec::new();
    let mut size = 0;
    for (id, at, body, file_name) in rows {
        let text = c.decrypt(&body)?;
        // A file that can't be read anymore goes without it rather than holding up the rest.
        let file = file_name.and_then(|n| {
            let blob = std::fs::read(files_dir(state, contact).join(&id)).ok()?;
            let data = c.open_bytes(&blob).ok()?;
            Some(FileItem { name: c.decrypt_or(&n, "file"), data: B64.encode(data) })
        });
        let n = text.len() + file.as_ref().map_or(0, |f| f.data.len() + f.name.len()) + 256;
        if !items.is_empty() && size + n > SEND_BUDGET {
            break;
        }
        size += n;
        items.push(Item { id, at, text, file });
    }
    Ok(items)
}

fn mark_delivered(state: &AppState, contact: &str, ids: &[String]) -> Result<usize, String> {
    let conn = state.db.lock().unwrap();
    let mut n = 0;
    for id in ids {
        n += conn
            .execute("UPDATE people_messages SET sent = 1 WHERE contact_id = ?1 AND id = ?2 AND from_me = 1", params![contact, id])
            .map_err(err)?;
    }
    Ok(n)
}

/// Keeps what someone sent. Returns the ids stored here (new, or already
/// here from before) and the new messages.
fn take(state: &AppState, c: &Cipher, p: &Contact, items: Vec<Item>) -> Result<(Vec<String>, Vec<Item>), String> {
    let now = db::now_ms();
    let mut got = Vec::new();
    let mut fresh = Vec::new();
    // Whatever is turned away is still counted as handled, so it isn't sent again.
    for mut it in items {
        if !safe_id(&it.id) {
            continue;
        }
        let here = {
            let conn = state.db.lock().unwrap();
            conn.query_row("SELECT 1 FROM people_messages WHERE contact_id = ?1 AND id = ?2", params![p.id, it.id], |_| Ok(()))
                .optional()
                .map_err(err)?
                .is_some()
        };
        if here {
            got.push(it.id);
            continue;
        }
        it.text = it.text.chars().take(MAX_TEXT).collect();
        let mut file = None;
        if let Some(f) = it.file.take() {
            let data = B64.decode(f.data.as_bytes()).ok().filter(|d| d.len() as u64 <= MAX_FILE);
            let Some(data) = data else {
                got.push(it.id);
                continue;
            };
            let dir = files_dir(state, &p.id);
            std::fs::create_dir_all(&dir).map_err(err)?;
            std::fs::write(dir.join(&it.id), c.seal_bytes(&data)).map_err(err)?;
            file = Some((clean_file_name(&f.name), data.len() as i64));
            it.file = Some(FileItem { name: clean_file_name(&f.name), data: String::new() });
        }
        if it.text.trim().is_empty() && file.is_none() {
            got.push(it.id);
            continue;
        }
        it.at = it.at.clamp(0, now);
        let conn = state.db.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO people_messages (contact_id, id, from_me, at, body, file_name, file_size, sent, read)
             VALUES (?1, ?2, 0, ?3, ?4, ?5, ?6, 1, 0)",
            params![p.id, it.id, it.at, c.encrypt(&it.text), file.as_ref().map(|f| c.encrypt(&f.0)), file.as_ref().map(|f| f.1)],
        )
        .map_err(err)?;
        got.push(it.id.clone());
        fresh.push(it);
    }
    Ok((got, fresh))
}

/// New messages arrived: the window hears, and so does the user if the
/// app isn't in front.
fn arrived(state: &AppState, p: &Contact, fresh: &[Item]) {
    if fresh.is_empty() {
        return;
    }
    emit(state, "people:message", serde_json::json!({ "contact_id": p.id }));
    let last = &fresh[fresh.len() - 1];
    let text: String = match &last.file {
        Some(f) if last.text.trim().is_empty() => format!("Sent a file: {}", f.name),
        _ => last.text.chars().take(140).collect(),
    };
    tell(state, &p.name, &text);
}

/// A Windows notification, unless the app is in front.
fn tell(state: &AppState, title: &str, text: &str) {
    let Some(app) = state.app.get() else { return };
    if app.get_webview_window("main").and_then(|w| w.is_focused().ok()).unwrap_or(false) {
        return;
    }
    // While the screen is locked, nothing about who or what.
    if state.cipher().is_err() {
        crate::notify::show(app, "SulcusAI", "Something new in People. Unlock SulcusAI to see it.");
        return;
    }
    crate::notify::show(app, title, text);
}

async fn connect(addr: SocketAddr) -> Result<TcpStream, String> {
    let them = "Is SulcusAI open there, with People turned on?";
    timeout(Duration::from_secs(5), TcpStream::connect(addr))
        .await
        .map_err(|_| format!("They didn't answer. {them}"))?
        .map_err(|_| format!("Couldn't reach them at {addr}. {them}"))
}

// ---------- linking ----------

fn keep(state: &AppState, c: &Cipher, id: &str, name: &str, key: &[u8], addr: Option<SocketAddr>) -> Result<String, String> {
    let sealed = c.encrypt(&zeroize::Zeroizing::new(hex::encode(key)));
    let name = clean_name(name);
    let now = db::now_ms();
    update(state, |b| {
        let before = b.contacts.iter().find(|p| p.id == id).cloned();
        b.contacts.retain(|p| p.id != id);
        b.contacts.push(Contact {
            id: id.to_string(),
            name: name.clone(),
            key: sealed,
            addr: addr.map(|a| a.to_string()).or(before.as_ref().and_then(|p| p.addr.clone())),
            added_at: before.as_ref().map_or(now, |p| p.added_at),
            last_seen: Some(now),
        });
    })?;
    // A new key: any connection on the old one goes, and the app calls now.
    link::hang_up(id);
    link::reset(id);
    state.log("people", &format!("Linked with {name}"));
    listen_again();
    link::wake();
    Ok(name)
}

/// The app showing the code: answers one attempt to link.
pub(super) async fn pair_answer(state: &Arc<AppState>, mut stream: TcpStream, open: &Open, theirs: &str) -> Result<(), String> {
    let on = features::is_on(&state.db.lock().unwrap(), Feature::People);
    let code = {
        let mut p = CODE.lock().unwrap();
        match p.as_mut() {
            Some(x) if on && x.until > Instant::now() && x.tries < CODE_TRIES => {
                x.tries += 1;
                Some(x.code.clone())
            }
            _ => None,
        }
    };
    let Some(code) = code else {
        let reply = PairReply { error: Some("They aren't showing a code right now. Ask them for a new one.".into()), ..Default::default() };
        return send_plain(&mut stream, &reply).await;
    };
    if !safe_id(&open.id) {
        return Err("bad id".into());
    }
    let c = state.work_cipher()?;
    let theirs = B64.decode(theirs).map_err(err)?;
    let me = device_id(&state.paths.data);
    let (spake, mine) = wire::pair_start(&code);
    send_plain(&mut stream, &PairReply { id: me.clone(), name: my_name(state), spake: B64.encode(&mine), error: None }).await?;
    let shared = wire::pair_finish(spake, &theirs)?;
    let t = transcript(&open.id, &me, &theirs, &mine);
    let proof = timeout(Duration::from_secs(30), wire::read_frame(&mut stream)).await.map_err(|_| "no proof".to_string())?.map_err(lost)?;
    if !wire::confirm_ok(&shared, "client", &t, &proof) {
        changed(state);
        return Err("wrong code".into());
    }
    wire::write_frame(&mut stream, &wire::confirm(&shared, "server", &t)).await.map_err(lost)?;
    // Their call right after says which port they take calls on.
    let name = keep(state, &c, &open.id, &open.name, &*wire::pair_key(&shared, &t), None)?;
    *CODE.lock().unwrap() = None;
    ANNOUNCE.notify_one();
    changed(state);
    if let Some(app) = state.app.get() {
        crate::notify::show(app, "SulcusAI", &format!("{name} linked with you. You can chat in People."));
    }
    Ok(())
}

/// The app where the code is typed: links with the one showing it.
async fn pair_call(state: &Arc<AppState>, code: &str, addr: SocketAddr) -> Result<String, String> {
    let c = state.work_cipher()?;
    let me = device_id(&state.paths.data);
    let mut stream = connect(addr).await?;
    let (spake, mine) = wire::pair_start(code);
    send_plain(&mut stream, &Open { v: 1, id: me.clone(), name: my_name(state), nonce: None, pair: Some(B64.encode(&mine)), want: Some(LINK.into()) }).await?;
    let reply: PairReply = recv_plain(&mut stream).await?;
    if let Some(e) = reply.error {
        return Err(e);
    }
    if !safe_id(&reply.id) || reply.id == me {
        return Err(UNEXPECTED.into());
    }
    let theirs = B64.decode(&reply.spake).map_err(|_| UNEXPECTED.to_string())?;
    let shared = wire::pair_finish(spake, &theirs)?;
    let t = transcript(&me, &reply.id, &mine, &theirs);
    wire::write_frame(&mut stream, &wire::confirm(&shared, "client", &t)).await.map_err(lost)?;
    let wrong = "That code didn't match. Check the code they're showing and try again.";
    let proof = timeout(HANDSHAKE, wire::read_frame(&mut stream)).await.map_err(|_| wrong.to_string())?.map_err(|_| wrong.to_string())?;
    if !wire::confirm_ok(&shared, "server", &t, &proof) {
        return Err(wrong.into());
    }
    keep(state, &c, &reply.id, &reply.name, &*wire::pair_key(&shared, &t), Some(addr))
}

// ---------- addresses ----------

#[derive(Debug, Serialize)]
pub struct AddressView {
    /// What to type on the other side ("26.10.4.7", or with ":port").
    address: String,
    /// The network it's on ("Wi-Fi", "Radmin VPN").
    network: String,
}

/// Virtual networks people use to connect PCs, by adapter name or address range.
fn network_label(adapter: &str, ip: Ipv4Addr) -> String {
    let a = adapter.to_lowercase();
    let [b0, b1, ..] = ip.octets();
    if a.contains("radmin") {
        "Radmin VPN".into()
    } else if a.contains("tailscale") || (b0 == 100 && (64..128).contains(&b1)) {
        "Tailscale".into()
    } else if a.contains("zerotier") {
        "ZeroTier".into()
    } else if a.contains("hamachi") {
        "Hamachi".into()
    } else {
        adapter.to_string()
    }
}

/// Adapters no one else can reach this PC through (virtual machines, WSL).
fn private_adapter(adapter: &str) -> bool {
    let a = adapter.to_lowercase();
    ["vethernet", "wsl", "hyper-v", "virtualbox", "vmware", "loopback", "docker"].iter().any(|x| a.contains(x))
}

/// Where others can reach this PC, the network it's on first.
fn addresses(port: u16) -> Vec<AddressView> {
    let main = super::local_ip();
    let nets = sysinfo::Networks::new_with_refreshed_list();
    let mut found: Vec<(bool, AddressView)> = Vec::new();
    for (adapter, data) in nets.iter() {
        if private_adapter(adapter) {
            continue;
        }
        for n in data.ip_networks() {
            let IpAddr::V4(ip) = n.addr else { continue };
            if ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() {
                continue;
            }
            let address = if port == PORT || port == 0 { ip.to_string() } else { SocketAddr::new(IpAddr::V4(ip), port).to_string() };
            if found.iter().any(|(_, a)| a.address == address) {
                continue;
            }
            found.push((Some(IpAddr::V4(ip)) == main, AddressView { address, network: network_label(adapter, ip) }));
        }
    }
    found.sort_by_key(|(first, a)| (!first, a.network.clone()));
    found.into_iter().map(|(_, a)| a).collect()
}

/// "26.10.4.7", "26.10.4.7:47820", a nearby app's id, or a name like
/// "isaacs-pc" (Tailscale and ZeroTier give PCs names).
async fn resolve(target: &str) -> Result<SocketAddr, String> {
    if let Some(a) = NEARBY.lock().unwrap().get(target).map(|s| s.addr) {
        return Ok(a);
    }
    if let Ok(a) = target.parse::<SocketAddr>() {
        return Ok(a);
    }
    if let Ok(ip) = target.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, PORT));
    }
    let valid = !target.is_empty() && target.len() <= 253 && target.chars().all(|c| c.is_ascii_alphanumeric() || ".-:".contains(c));
    let help = "Type the address they see in People, like 26.10.4.7.";
    if !valid {
        return Err(help.into());
    }
    let with_port = if target.contains(':') { target.to_string() } else { format!("{target}:{PORT}") };
    let found = timeout(Duration::from_secs(5), tokio::net::lookup_host(with_port)).await.map_err(|_| help.to_string())?;
    let mut addrs = found.map_err(|_| format!("Couldn't find \"{target}\". {help}"))?;
    addrs.find(|a| a.is_ipv4()).ok_or_else(|| help.to_string())
}

// ---------- commands ----------

#[derive(Debug, Serialize)]
pub struct ContactView {
    id: String,
    name: String,
    /// Connected right now.
    online: bool,
    last_seen: Option<i64>,
    address: Option<String>,
    unread: i64,
    last: Option<MessageView>,
    /// Why the last call from here didn't get through.
    error: Option<String>,
    /// Chats they share with this user (while connected).
    shared: Vec<share::SharedChat>,
    /// Chats this user shares with them.
    sharing: Vec<share::Sharing>,
}

#[derive(Debug, Serialize)]
pub struct NearbyPerson {
    id: String,
    name: String,
}

#[derive(Debug, Serialize)]
pub struct PeopleView {
    name: String,
    /// Taking calls (this PC can be reached).
    listening: bool,
    addresses: Vec<AddressView>,
    /// The code this PC is showing ("K7Q2-9XMB") and its seconds left.
    code: Option<String>,
    code_left: u64,
    contacts: Vec<ContactView>,
    /// Apps on this network showing a code.
    nearby: Vec<NearbyPerson>,
    unread: i64,
}

fn view(state: &AppState, c: &Cipher) -> PeopleView {
    // The database first and on its own (the sync page holds NEARBY while it reads it).
    let (b, counts, lasts, chats) = {
        let conn = state.db.lock().unwrap();
        let b = book(&conn);
        let lasts: Vec<Option<MessageView>> = b.contacts.iter().map(|p| last_message(&conn, c, &p.id)).collect();
        let chats: HashMap<String, String> = b.shares.iter().filter_map(|s| db::chat(&conn, c, &s.chat_id).map(|ch| (ch.id, ch.title))).collect();
        (b, unread(&conn), lasts, chats)
    };
    let port = listening_port();
    let mut contacts: Vec<ContactView> = b
        .contacts
        .iter()
        .zip(lasts)
        .map(|(p, last)| {
            let online = link::connected(&p.id);
            ContactView {
                id: p.id.clone(),
                name: p.name.clone(),
                online,
                last_seen: p.last_seen,
                address: p.addr.clone(),
                unread: counts.get(&p.id).copied().unwrap_or(0),
                last,
                error: if online { None } else { link::error(&p.id) },
                shared: if online { share::theirs(&p.id) } else { vec![] },
                sharing: b
                    .shares
                    .iter()
                    .filter(|s| s.with.contains(&p.id))
                    .filter_map(|s| chats.get(&s.chat_id).map(|t| share::Sharing { chat_id: s.chat_id.clone(), title: t.clone() }))
                    .collect(),
            }
        })
        .collect();
    // Latest conversation first.
    contacts.sort_by_key(|p| std::cmp::Reverse(p.last.as_ref().map_or(p.last_seen.unwrap_or(0), |m| m.at)));
    let linked: HashSet<&str> = b.contacts.iter().map(|p| p.id.as_str()).collect();
    let code = CODE.lock().unwrap();
    let code = code.as_ref().filter(|p| p.until > Instant::now());
    PeopleView {
        name: b.name.clone(),
        listening: port != 0,
        addresses: if port != 0 { addresses(port) } else { vec![] },
        code: code.map(|p| format!("{}-{}", &p.code[..4], &p.code[4..])),
        code_left: code.map_or(0, |p| p.until.saturating_duration_since(Instant::now()).as_secs()),
        unread: contacts.iter().map(|p| p.unread).sum(),
        contacts,
        nearby: NEARBY
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, s)| s.person && s.at.elapsed() < AWAY && !linked.contains(id.as_str()))
            .map(|(id, s)| NearbyPerson { id: id.clone(), name: s.name.clone() })
            .collect(),
    }
}

#[tauri::command(async)]
pub fn people_view(state: AppStateRef) -> Result<PeopleView, String> {
    features::require(&state, Feature::People)?;
    let c = state.cipher()?;
    Ok(view(&state, &c))
}

/// Unread messages, for the badge in the sidebar (0 while People is off or the app is locked).
#[tauri::command(async)]
pub fn people_unread(state: AppStateRef) -> i64 {
    let conn = state.db.lock().unwrap();
    if !features::is_on(&conn, Feature::People) {
        return 0;
    }
    unread(&conn).values().sum()
}

#[tauri::command(async)]
pub fn set_people_name(state: AppStateRef, name: String) -> Result<PeopleView, String> {
    features::require(&state, Feature::People)?;
    let c = state.cipher()?;
    let name = name.trim();
    if name.is_empty() {
        return Err("Type a name for others to see.".into());
    }
    let name = clean_name(name);
    update(&state, |b| b.name = name.clone())?;
    // The people connected now see it right away.
    link::hello_all(&state);
    Ok(view(&state, &c))
}

/// Shows a code for five minutes for someone to link with this app.
#[tauri::command(async)]
pub fn people_show_code(state: AppStateRef) -> Result<PeopleView, String> {
    features::require(&state, Feature::People)?;
    let c = state.cipher()?;
    *CODE.lock().unwrap() = Some(Pairing { code: new_code(), until: Instant::now() + CODE_FOR, tries: 0 });
    listen_again();
    Ok(view(&state, &c))
}

#[tauri::command(async)]
pub fn people_cancel_code(state: AppStateRef) -> Result<PeopleView, String> {
    let c = state.cipher()?;
    *CODE.lock().unwrap() = None;
    listen_again();
    Ok(view(&state, &c))
}

/// Links with whoever is showing `code`: at the address typed (or a nearby
/// app's id), or the only app nearby showing a code. Returns their name.
#[tauri::command]
pub async fn people_connect(state: AppStateRef<'_>, code: String, target: Option<String>) -> Result<String, String> {
    features::require(&state, Feature::People)?;
    state.cipher()?;
    let code = clean_code(&code).ok_or("A code is 8 letters and numbers, like K7Q2-9XMB.")?;
    let target = target.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let addr = match target {
        Some(t) => resolve(&t).await?,
        None => {
            let near = NEARBY.lock().unwrap();
            let showing: Vec<SocketAddr> = near.values().filter(|s| s.person && s.at.elapsed() < AWAY).map(|s| s.addr).collect();
            match showing.as_slice() {
                [one] => *one,
                [] => return Err("No one on this network is showing a code. Type the address they see in People.".into()),
                _ => return Err("More than one person here is showing a code. Pick which one.".into()),
            }
        }
    };
    let state = state.inner().clone();
    let name = pair_call(&state, &code, addr).await?;
    changed(&state);
    Ok(name)
}

/// Removes someone, with your messages with them, on this PC.
#[tauri::command(async)]
pub fn people_remove(state: AppStateRef, id: String) -> Result<PeopleView, String> {
    let c = state.cipher()?;
    let name = update(&state, |b| {
        let name = b.contacts.iter().find(|p| p.id == id).map(|p| p.name.clone());
        b.contacts.retain(|p| p.id != id);
        for s in &mut b.shares {
            s.with.retain(|w| *w != id);
        }
        b.shares.retain(|s| !s.with.is_empty());
        name
    })?;
    state.db.lock().unwrap().execute("DELETE FROM people_messages WHERE contact_id = ?1", [&id]).map_err(err)?;
    if safe_id(&id) {
        let _ = std::fs::remove_dir_all(files_dir(&state, &id));
    }
    link::hang_up(&id);
    link::forget(&id);
    if let Some(n) = name {
        state.log("people", &format!("Removed {n} from People"));
    }
    listen_again();
    changed(&state);
    Ok(view(&state, &c))
}

/// Where to reach someone, when their address changed.
#[tauri::command]
pub async fn people_set_address(state: AppStateRef<'_>, id: String, address: String) -> Result<(), String> {
    features::require(&state, Feature::People)?;
    let addr = resolve(address.trim()).await?;
    update(&state, |b| {
        if let Some(p) = b.contacts.iter_mut().find(|p| p.id == id) {
            p.addr = Some(addr.to_string());
        }
    })?;
    link::reset(&id);
    link::wake();
    Ok(())
}

/// A conversation, marking what they sent as read.
#[tauri::command(async)]
pub fn people_messages(state: AppStateRef, id: String) -> Result<Vec<MessageView>, String> {
    features::require(&state, Feature::People)?;
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let read = conn
        .execute("UPDATE people_messages SET read = 1 WHERE contact_id = ?1 AND from_me = 0 AND read = 0", [&id])
        .map_err(err)?;
    let list = messages(&conn, &c, &id)?;
    drop(conn);
    if read > 0 {
        changed(&state);
    }
    Ok(list)
}

/// Stores a message for someone; it goes right away if they're connected,
/// or waits until they are.
fn queue(state: &AppState, c: &Cipher, contact: &str, text: &str, file: Option<(String, Vec<u8>)>) -> Result<MessageView, String> {
    let msg_id = uuid::Uuid::new_v4().simple().to_string();
    if let Some((_, data)) = &file {
        let dir = files_dir(state, contact);
        std::fs::create_dir_all(&dir).map_err(err)?;
        std::fs::write(dir.join(&msg_id), c.seal_bytes(data)).map_err(err)?;
    }
    let at = db::now_ms();
    state
        .db
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO people_messages (contact_id, id, from_me, at, body, file_name, file_size, sent, read) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, 0, 1)",
            params![contact, msg_id, at, c.encrypt(text), file.as_ref().map(|f| c.encrypt(&f.0)), file.as_ref().map(|f| f.1.len() as i64)],
        )
        .map_err(err)?;
    if link::connected(contact) {
        link::flush(state, contact);
    } else {
        // Try right away, even after failed tries.
        link::reset(contact);
        link::wake();
    }
    Ok(MessageView {
        id: msg_id,
        from_me: true,
        at,
        text: text.to_string(),
        file: file.map(|(name, data)| FileView { name, size: data.len() as u64 }),
        delivered: false,
    })
}

/// Sends a message (and a file, if one is picked).
#[tauri::command(async)]
pub fn people_send(state: AppStateRef, id: String, text: String, file: Option<String>) -> Result<MessageView, String> {
    features::require(&state, Feature::People)?;
    let c = state.cipher()?;
    let p = contact(&state, &id).ok_or("They aren't linked anymore.")?;
    let text: String = text.trim().chars().take(MAX_TEXT).collect();
    let file = match file.filter(|f| !f.trim().is_empty()) {
        Some(path) => {
            let path = PathBuf::from(path);
            let size = std::fs::metadata(&path).map_err(|_| "That file couldn't be read.".to_string())?.len();
            if size > MAX_FILE {
                return Err(format!("That file is {} MB. Files up to {} MB can be sent.", size >> 20, MAX_FILE >> 20));
            }
            let data = std::fs::read(&path).map_err(|_| "That file couldn't be read.".to_string())?;
            let name = clean_file_name(&path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            Some((name, data))
        }
        None => None,
    };
    if text.is_empty() && file.is_none() {
        return Err("Type a message or pick a file.".into());
    }
    queue(&state, &c, &p.id, &text, file)
}

/// Saves a file someone sent (or you sent) where the user picked.
#[tauri::command(async)]
pub fn people_save_file(state: AppStateRef, id: String, message: String, to: String) -> Result<(), String> {
    features::require(&state, Feature::People)?;
    let c = state.cipher()?;
    if !safe_id(&id) || !safe_id(&message) {
        return Err("That file isn't here.".into());
    }
    let blob = std::fs::read(files_dir(&state, &id).join(&message)).map_err(|_| "That file isn't on this PC anymore.".to_string())?;
    let data = c.open_bytes(&blob)?;
    std::fs::write(&to, data).map_err(|e| format!("Couldn't save it there ({e})."))?;
    Ok(())
}
