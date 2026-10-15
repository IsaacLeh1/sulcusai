// SPDX-License-Identifier: AGPL-3.0-only
//! People: link with someone else's SulcusAI by a code and chat with them
//! directly (DESIGN.md §4.11). Messages go app to app and never to a model.
//!
//! - Linking works like pairing PCs (`wire`: SPAKE2 with a code shown on one
//!   side), but a person is a contact, not a paired PC: a contact can only
//!   exchange messages. It never syncs, and can't list or copy models.
//! - The apps connect directly: on the same network, or on a virtual one
//!   (Radmin VPN, Tailscale, ZeroTier) by the address the other side shows.
//! - Whichever side can reach the other calls, and each call delivers what's
//!   waiting both ways, so it's enough that one side can take calls.
//! - Messages are stored encrypted with this PC's data key; files sent with
//!   them are sealed the same way under the data folder.
//! - Off means off: with the feature off this PC doesn't take calls from
//!   people, doesn't call them, and its commands refuse.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio::time::timeout;

use super::{clean_code, device_id, lost, new_code, recv_plain, send_plain, transcript, wire, Accept, Busy, Open, PairReply, Pairing};
use super::{ANNOUNCE, AWAY, CODE_FOR, CODE_TRIES, HANDSHAKE, LIVE, NEARBY, PORT};
use crate::crypto::Cipher;
use crate::features::{self, Feature};
use crate::{db, AppState, AppStateRef};

const KEY: &str = "people";
/// What a call asks for (`Open::want`): linking, or exchanging messages.
pub(super) const LINK: &str = "person";
pub(super) const CHAT: &str = "people";
/// The biggest file a message can carry.
pub const MAX_FILE: u64 = 16 << 20;
const MAX_TEXT: usize = 20_000;
/// What one call carries at most (a frame holds 32 MB); the rest goes next.
const CALL_BUDGET: usize = 24 << 20;
/// How often to check in with each person while the People page is open,
/// and otherwise.
const WATCHING_EVERY: Duration = Duration::from_secs(10);
const IDLE_EVERY: Duration = Duration::from_secs(60);
const WATCHING_FOR: Duration = Duration::from_secs(90);
/// Someone counts as online this long after a call either way got through.
const ONLINE_FOR: Duration = Duration::from_secs(150);
const UNEXPECTED: &str = "Their app sent something unexpected.";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Book {
    /// This user's name, as the people they link with see it.
    name: String,
    contacts: Vec<Contact>,
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

// ---------- what's happening now ----------

/// The code this PC is showing for someone to link with.
static CODE: Mutex<Option<Pairing>> = Mutex::new(None);
static WAKE: LazyLock<Notify> = LazyLock::new(Notify::new);
/// When the People page last asked for anything: while it's open, check in more often.
static WATCHING: Mutex<Option<Instant>> = Mutex::new(None);
static LINKS: LazyLock<Mutex<HashMap<String, Link>>> = LazyLock::new(Default::default);

#[derive(Default)]
struct Link {
    /// The last call either way that got through.
    ok: Option<Instant>,
    /// The last call from here, and why it failed (if it did).
    tried: Option<Instant>,
    error: Option<String>,
    /// Failed calls in a row, to wait longer between tries.
    fails: u32,
}

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

/// What travels in a call (inside the session's encryption).
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Hello {
    name: String,
    /// The port this side takes calls on (0 if it doesn't).
    port: u16,
    items: Vec<Item>,
    /// The ids of the other side's items now stored here.
    got: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Item {
    id: String,
    at: i64,
    text: String,
    file: Option<FileItem>,
}

#[derive(Default, Serialize, Deserialize)]
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

/// The people with messages from here they don't have yet.
fn waiting_for(conn: &Connection) -> HashSet<String> {
    let Ok(mut stmt) = conn.prepare("SELECT DISTINCT contact_id FROM people_messages WHERE from_me = 1 AND sent = 0") else {
        return HashSet::new();
    };
    stmt.query_map([], |r| r.get(0)).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

/// What's waiting for someone, up to what one call carries.
fn outgoing(state: &AppState, c: &Cipher, contact: &str) -> Result<Vec<Item>, String> {
    let rows: Vec<(String, i64, String, Option<String>)> = {
        let conn = state.db.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, at, body, file_name FROM people_messages WHERE contact_id = ?1 AND from_me = 1 AND sent = 0 ORDER BY at, rowid")
            .map_err(err)?;
        let rows = stmt.query_map([contact], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map_err(err)?;
        rows.filter_map(Result::ok).collect()
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
        if !items.is_empty() && size + n > CALL_BUDGET {
            break;
        }
        size += n;
        items.push(Item { id, at, text, file });
    }
    Ok(items)
}

fn mark_delivered(state: &AppState, contact: &str, sent: &[String], got: &[String]) -> Result<usize, String> {
    let conn = state.db.lock().unwrap();
    let mut n = 0;
    for id in got.iter().filter(|g| sent.contains(g)) {
        n += conn
            .execute("UPDATE people_messages SET sent = 1 WHERE contact_id = ?1 AND id = ?2 AND from_me = 1", params![contact, id])
            .map_err(err)?;
    }
    Ok(n)
}

/// Keeps what someone sent. Returns the ids stored here (new, or already
/// here from an earlier call) and the new messages.
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

/// After a call either way got through: who they are now, where they take
/// calls, and anything new from them.
fn heard_from(state: &AppState, p: &Contact, addr: Option<SocketAddr>, name: &str, fresh: &[Item]) {
    {
        let mut links = LINKS.lock().unwrap();
        let l = links.entry(p.id.clone()).or_default();
        l.ok = Some(Instant::now());
        l.error = None;
        l.fails = 0;
    }
    let name = clean_name(name);
    let addr = addr.map(|a| a.to_string());
    let now = db::now_ms();
    // Only write when something changed, or now and then for "last seen".
    let stale = p.name != name || (addr.is_some() && p.addr != addr) || p.last_seen.is_none_or(|t| now - t > 5 * 60 * 1000);
    if stale {
        let _ = update(state, |b| {
            if let Some(x) = b.contacts.iter_mut().find(|x| x.id == p.id) {
                x.name = name.clone();
                x.last_seen = Some(now);
                if addr.is_some() {
                    x.addr = addr.clone();
                }
            }
        });
    }
    changed(state);
    if !fresh.is_empty() {
        emit(state, "people:message", serde_json::json!({ "contact_id": p.id }));
        tell(state, &name, fresh);
    }
}

/// A Windows notification for new messages, unless the app is in front.
fn tell(state: &AppState, name: &str, fresh: &[Item]) {
    let Some(app) = state.app.get() else { return };
    if app.get_webview_window("main").and_then(|w| w.is_focused().ok()).unwrap_or(false) {
        return;
    }
    // While the screen is locked, nothing about who or what.
    if state.cipher().is_err() {
        crate::notify::show(app, "SulcusAI", "A new message. Unlock SulcusAI to read it.");
        return;
    }
    let last = &fresh[fresh.len() - 1];
    let text: String = match &last.file {
        Some(f) if last.text.trim().is_empty() => format!("Sent a file: {}", f.name),
        _ => last.text.chars().take(140).collect(),
    };
    crate::notify::show(app, name, &text);
}

// ---------- calls ----------

async fn send_sealed<T: Serialize>(stream: &mut TcpStream, out: &mut wire::Sealer, msg: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec(msg).map_err(err)?;
    wire::write_frame(stream, &out.seal(&bytes)).await.map_err(lost)
}

async fn recv_sealed<T: for<'de> Deserialize<'de>>(stream: &mut TcpStream, inn: &mut wire::Sealer) -> Result<T, String> {
    let sealed = timeout(Duration::from_secs(120), wire::read_frame(stream))
        .await
        .map_err(|_| "They stopped answering.".to_string())?
        .map_err(lost)?;
    serde_json::from_slice(&inn.open(&sealed)?).map_err(|_| UNEXPECTED.to_string())
}

async fn connect(addr: SocketAddr) -> Result<TcpStream, String> {
    let them = "Is SulcusAI open there, with People turned on?";
    timeout(Duration::from_secs(5), TcpStream::connect(addr))
        .await
        .map_err(|_| format!("They didn't answer. {them}"))?
        .map_err(|_| format!("Couldn't reach them at {addr}. {them}"))
}

/// Calls someone: delivers what's waiting for them and picks up what's
/// waiting here.
async fn call(state: &Arc<AppState>, id: &str, addr: SocketAddr) -> Result<(), String> {
    let Some(_busy) = Busy::take(&format!("person:{id}")) else { return Ok(()) };
    let c = state.work_cipher()?;
    let p = contact(state, id).ok_or("They aren't linked anymore.")?;
    let key = link_key(&c, &p)?;
    let me = device_id(&state.paths.data);
    let mut stream = connect(addr).await?;
    let nonce = wire::random::<32>();
    send_plain(&mut stream, &Open { v: 1, id: me, name: String::new(), nonce: Some(hex::encode(nonce)), pair: None, want: Some(CHAT.into()) }).await?;
    let accept: Accept = recv_plain(&mut stream)
        .await
        .map_err(|_| format!("{} didn't take the call. If they removed you, link again.", p.name))?;
    if accept.id != p.id {
        return Err("Someone else answered at that address.".into());
    }
    let theirs = hex::decode(&accept.nonce).map_err(|_| UNEXPECTED.to_string())?;
    let (to, from) = wire::session_keys(&key, &nonce, &theirs);
    let (mut out, mut inn) = (wire::Sealer::new(&to), wire::Sealer::new(&from));
    let items = outgoing(state, &c, id)?;
    let sent: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
    send_sealed(&mut stream, &mut out, &Hello { name: my_name(state), port: listening_port(), items, got: vec![] }).await?;
    let reply: Hello = recv_sealed(&mut stream, &mut inn).await?;
    let delivered = mark_delivered(state, id, &sent, &reply.got)?;
    let (got, fresh) = take(state, &c, &p, reply.items)?;
    send_sealed(&mut stream, &mut out, &Hello { got, ..Default::default() }).await?;
    heard_from(state, &p, Some(addr), &reply.name, &fresh);
    // More than one call carries: go again.
    if delivered > 0 && waiting_for(&state.db.lock().unwrap()).contains(id) {
        WAKE.notify_one();
    }
    Ok(())
}

/// Answers a call from someone linked.
pub(super) async fn answer(state: &Arc<AppState>, mut stream: TcpStream, open: &Open) -> Result<(), String> {
    if !features::is_on(&state.db.lock().unwrap(), Feature::People) {
        return Err("People is off".into());
    }
    let c = state.work_cipher()?;
    let p = contact(state, &open.id).ok_or("not linked")?;
    let key = link_key(&c, &p)?;
    let from = stream.peer_addr().ok();
    let theirs = hex::decode(open.nonce.as_deref().unwrap_or_default()).map_err(err)?;
    let nonce = wire::random::<32>();
    send_plain(&mut stream, &Accept { id: device_id(&state.paths.data), nonce: hex::encode(nonce) }).await?;
    let (from_them, to_them) = wire::session_keys(&key, &theirs, &nonce);
    let (mut out, mut inn) = (wire::Sealer::new(&to_them), wire::Sealer::new(&from_them));
    let hello: Hello = recv_sealed(&mut stream, &mut inn).await?;
    let (got, fresh) = take(state, &c, &p, hello.items)?;
    let items = outgoing(state, &c, &p.id)?;
    let sent: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
    send_sealed(&mut stream, &mut out, &Hello { name: my_name(state), port: listening_port(), items, got }).await?;
    // They take calls where they called from, at the port they named.
    let addr = from.filter(|_| hello.port != 0).map(|a| SocketAddr::new(a.ip(), hello.port));
    heard_from(state, &p, addr, &hello.name, &fresh);
    let last: Hello = recv_sealed(&mut stream, &mut inn).await?;
    mark_delivered(state, &p.id, &sent, &last.got)?;
    changed(state);
    Ok(())
}

/// Calls whoever is due: anyone with messages waiting for them right away,
/// everyone else now and then to see who's around.
fn tick(state: &Arc<AppState>) {
    if state.work_cipher().is_err() {
        return;
    }
    let (contacts, waiting) = {
        let conn = state.db.lock().unwrap();
        if !features::is_on(&conn, Feature::People) {
            return;
        }
        (book(&conn).contacts, waiting_for(&conn))
    };
    let watching = WATCHING.lock().unwrap().is_some_and(|t| t.elapsed() < WATCHING_FOR);
    let every = if watching { WATCHING_EVERY } else { IDLE_EVERY };
    for p in contacts {
        let Some(addr) = super::seen_addr(&p.id).or_else(|| p.addr.as_deref()?.parse().ok()) else { continue };
        {
            let mut links = LINKS.lock().unwrap();
            let l = links.entry(p.id.clone()).or_default();
            let wait = if l.fails > 0 {
                // 15 s, 30 s, 1 min… up to 5 minutes between failed tries.
                Duration::from_secs((15u64 << (l.fails - 1).min(5)).min(300))
            } else if waiting.contains(&p.id) {
                Duration::ZERO
            } else {
                every
            };
            // A call from them counts as checking in.
            let since = l.tried.max(l.ok);
            if since.is_some_and(|t| t.elapsed() < wait) {
                continue;
            }
            l.tried = Some(Instant::now());
        }
        let s = state.clone();
        tauri::async_runtime::spawn(async move {
            let result = timeout(Duration::from_secs(300), call(&s, &p.id, addr)).await.unwrap_or(Err("The call took too long.".into()));
            if let Err(e) = result {
                let mut links = LINKS.lock().unwrap();
                let l = links.entry(p.id.clone()).or_default();
                l.fails = l.fails.saturating_add(1);
                l.error = Some(e);
                drop(links);
                changed(&s);
            }
        });
    }
}

/// Runs People in the background (the sync loop does the listening).
pub(super) fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<Arc<AppState>>().inner().clone();
        loop {
            tick(&state);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                _ = WAKE.notified() => {}
            }
        }
    });
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
            added_at: before.map_or(now, |p| p.added_at),
            last_seen: Some(now),
        });
    })?;
    // A fresh start: the next tick calls them (so they learn where this app takes calls).
    LINKS.lock().unwrap().insert(id.to_string(), Link::default());
    state.log("people", &format!("Linked with {name}"));
    listen_again();
    WAKE.notify_one();
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
    // Their first call says which port they take calls on.
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
    online: bool,
    last_seen: Option<i64>,
    address: Option<String>,
    unread: i64,
    last: Option<MessageView>,
    /// Why the last call from here didn't get through.
    error: Option<String>,
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
    let (b, counts, lasts) = {
        let conn = state.db.lock().unwrap();
        let b = book(&conn);
        let lasts: Vec<Option<MessageView>> = b.contacts.iter().map(|p| last_message(&conn, c, &p.id)).collect();
        (b, unread(&conn), lasts)
    };
    let port = listening_port();
    let links = LINKS.lock().unwrap();
    let mut contacts: Vec<ContactView> = b
        .contacts
        .iter()
        .zip(lasts)
        .map(|(p, last)| {
            let l = links.get(&p.id);
            let online = l.and_then(|l| l.ok).is_some_and(|t| t.elapsed() < ONLINE_FOR);
            ContactView {
                id: p.id.clone(),
                name: p.name.clone(),
                online,
                last_seen: p.last_seen,
                address: p.addr.clone(),
                unread: counts.get(&p.id).copied().unwrap_or(0),
                last,
                error: if online { None } else { l.and_then(|l| l.error.clone()) },
            }
        })
        .collect();
    drop(links);
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

fn watching() {
    *WATCHING.lock().unwrap() = Some(Instant::now());
}

#[tauri::command(async)]
pub fn people_view(state: AppStateRef) -> Result<PeopleView, String> {
    features::require(&state, Feature::People)?;
    let c = state.cipher()?;
    watching();
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
    update(&state, |b| b.name = name)?;
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
        name
    })?;
    state.db.lock().unwrap().execute("DELETE FROM people_messages WHERE contact_id = ?1", [&id]).map_err(err)?;
    if safe_id(&id) {
        let _ = std::fs::remove_dir_all(files_dir(&state, &id));
    }
    LINKS.lock().unwrap().remove(&id);
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
    if let Some(l) = LINKS.lock().unwrap().get_mut(&id) {
        l.fails = 0;
        l.tried = None;
    }
    WAKE.notify_one();
    Ok(())
}

/// A conversation, marking what they sent as read.
#[tauri::command(async)]
pub fn people_messages(state: AppStateRef, id: String) -> Result<Vec<MessageView>, String> {
    features::require(&state, Feature::People)?;
    let c = state.cipher()?;
    watching();
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

/// Sends a message (and a file, if one is picked). It waits here until
/// they're reachable.
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
    let msg_id = uuid::Uuid::new_v4().simple().to_string();
    if let Some((_, data)) = &file {
        let dir = files_dir(&state, &p.id);
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
            params![p.id, msg_id, at, c.encrypt(&text), file.as_ref().map(|f| c.encrypt(&f.0)), file.as_ref().map(|f| f.1.len() as i64)],
        )
        .map_err(err)?;
    // Try right away, even after failed tries.
    if let Some(l) = LINKS.lock().unwrap().get_mut(&p.id) {
        l.fails = 0;
    }
    WAKE.notify_one();
    Ok(MessageView {
        id: msg_id,
        from_me: true,
        at,
        text,
        file: file.map(|(name, data)| FileView { name, size: data.len() as u64 }),
        delivered: false,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_file_names_are_made_safe() {
        assert_eq!(clean_file_name("..\\..\\Windows\\evil.exe"), "evil.exe");
        assert_eq!(clean_file_name("a/b/report: final?.pdf"), "report final.pdf");
        assert_eq!(clean_file_name("..."), "file");
        assert_eq!(clean_name("  Sam\u{0007}  "), "Sam");
        assert_eq!(clean_name(""), "Someone");
        assert!(safe_id("0f9a-77"));
        assert!(!safe_id("../x") && !safe_id(""));
    }

    #[test]
    fn virtual_networks_are_named() {
        assert_eq!(network_label("Radmin VPN", Ipv4Addr::new(26, 1, 2, 3)), "Radmin VPN");
        assert_eq!(network_label("Ethernet 3", Ipv4Addr::new(100, 101, 1, 2)), "Tailscale");
        assert_eq!(network_label("Wi-Fi", Ipv4Addr::new(192, 168, 1, 20)), "Wi-Fi");
        assert!(private_adapter("vEthernet (WSL)"));
        assert!(!private_adapter("Wi-Fi"));
        // This PC's own (whatever networks it's on).
        let mine = addresses(PORT);
        eprintln!("{mine:?}");
        assert!(mine.iter().all(|a| !a.address.starts_with("127.") && !a.address.contains(':')));
    }

    /// Two people's apps (two data folders) link by code over loopback,
    /// chat both ways, send a file, and a person can't sync.
    #[cfg(windows)]
    #[tokio::test]
    async fn two_people_link_and_chat() {
        use tokio::net::TcpListener;
        let a = crate::e2e::scratch_state();
        let b = crate::e2e::scratch_state();
        update(&a, |x| x.name = "Ana".into()).unwrap();
        update(&b, |x| x.name = "Ben".into()).unwrap();
        let (ca, cb) = (a.cipher().unwrap(), b.cipher().unwrap());

        // Both take calls.
        let mut addrs = Vec::new();
        for s in [&a, &b] {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
            addrs.push(listener.local_addr().unwrap());
            let s = s.clone();
            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let s = s.clone();
                    tokio::spawn(async move {
                        if let Err(e) = super::super::answer(&s, stream).await {
                            eprintln!("answered: {e}");
                        }
                    });
                }
            });
        }
        let (addr_a, addr_b) = (addrs[0], addrs[1]);

        // Ana shows a code; a wrong code fails, the right one links.
        *CODE.lock().unwrap() = Some(Pairing { code: "K7Q29XMB".into(), until: Instant::now() + CODE_FOR, tries: 0 });
        let wrong = pair_call(&b, "K7Q29XMC", addr_a).await.unwrap_err();
        assert!(wrong.contains("didn't match"), "{wrong}");
        assert!(book(&a.db.lock().unwrap()).contacts.is_empty());
        assert_eq!(pair_call(&b, "K7Q29XMB", addr_a).await.unwrap(), "Ana");
        assert!(CODE.lock().unwrap().is_none(), "a code works once");
        let (id_a, id_b) = (device_id(&a.paths.data), device_id(&b.paths.data));
        assert_eq!(book(&a.db.lock().unwrap()).contacts[0].name, "Ben");
        assert_eq!(book(&b.db.lock().unwrap()).contacts[0].id, id_a);

        // A contact isn't a paired PC: syncing with Ana is refused.
        assert!(super::super::config(&a.db.lock().unwrap()).peers.is_empty());
        assert!(super::super::config(&b.db.lock().unwrap()).peers.is_empty());

        // Ben writes while Ana's app can't reach him (she has no address yet); it waits.
        let file = std::env::temp_dir().join(format!("people-{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&file, b"the plan").unwrap();
        let sent = people_send_now(&b, &id_a, "Hi Ana", Some(file.to_string_lossy().into_owned()));
        assert!(!sent.delivered);
        // Ana writes too; her message waits for Ben's call.
        people_send_now(&a, &id_b, "Hey Ben", None);

        // Ben calls Ana: his message arrives there, hers arrives here, in one
        // call. (Both apps share LIVE in this test; it names Ben's port.)
        *LIVE.lock().unwrap() = Some(super::super::Live { tasks: vec![], port: addr_b.port() });
        call(&b, &id_a, addr_a).await.unwrap();
        let at_a = messages(&a.db.lock().unwrap(), &ca, &id_b).unwrap();
        let texts: Vec<&str> = at_a.iter().map(|m| m.text.as_str()).collect();
        assert!(texts.contains(&"Hi Ana") && texts.contains(&"Hey Ben"), "{texts:?}");
        let theirs = at_a.iter().find(|m| m.text == "Hi Ana").unwrap();
        assert!(!theirs.from_me);
        assert_eq!(theirs.file.as_ref().unwrap().name, file.file_name().unwrap().to_string_lossy());
        let at_b = messages(&b.db.lock().unwrap(), &cb, &id_a).unwrap();
        assert!(at_b.iter().all(|m| m.delivered), "both sides have everything");
        assert_eq!(at_b.len(), 2);
        // Ana's app learned where Ben takes calls, from his call.
        let ben = contact(&a, &id_b).unwrap();
        let ben_at: SocketAddr = ben.addr.as_deref().unwrap().parse().unwrap();
        assert_eq!(ben_at, addr_b);
        // So she can call him too.
        people_send_now(&a, &id_b, "Got it", None);
        call(&a, &id_b, ben_at).await.unwrap();
        assert!(messages(&b.db.lock().unwrap(), &cb, &id_a).unwrap().iter().any(|m| m.text == "Got it" && !m.from_me));
        *LIVE.lock().unwrap() = None;

        // The file comes out as it went in.
        let out = std::env::temp_dir().join(format!("people-out-{}.txt", uuid::Uuid::new_v4()));
        let blob = std::fs::read(files_dir(&a, &id_b).join(&theirs.id)).unwrap();
        assert_ne!(blob, b"the plan", "stored sealed");
        std::fs::write(&out, ca.open_bytes(&blob).unwrap()).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"the plan");

        // Calling again repeats nothing.
        call(&b, &id_a, addr_a).await.unwrap();
        assert_eq!(messages(&a.db.lock().unwrap(), &ca, &id_b).unwrap().len(), 3);

        // With People off, Ana's app turns calls away.
        features::set(&a.db.lock().unwrap(), Feature::People, false).unwrap();
        assert!(call(&b, &id_a, addr_a).await.is_err());
        features::set(&a.db.lock().unwrap(), Feature::People, true).unwrap();

        // Removed: the conversation goes, and their calls are turned away.
        update(&a, |x| x.contacts.clear()).unwrap();
        assert!(call(&b, &id_a, addr_a).await.is_err());
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_file(&out);
    }

    /// `people_send` without the Tauri wrapper.
    #[cfg(windows)]
    fn people_send_now(s: &Arc<AppState>, id: &str, text: &str, file: Option<String>) -> MessageView {
        let c = s.cipher().unwrap();
        let msg_id = uuid::Uuid::new_v4().simple().to_string();
        let mut name = None;
        if let Some(f) = &file {
            let data = std::fs::read(f).unwrap();
            let dir = files_dir(s, id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(&msg_id), c.seal_bytes(&data)).unwrap();
            name = Some((clean_file_name(f), data.len() as i64));
        }
        s.db.lock()
            .unwrap()
            .execute(
                "INSERT INTO people_messages (contact_id, id, from_me, at, body, file_name, file_size, sent, read) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, 0, 1)",
                params![id, msg_id, db::now_ms(), c.encrypt(text), name.as_ref().map(|n| c.encrypt(&n.0)), name.as_ref().map(|n| n.1)],
            )
            .unwrap();
        messages(&s.db.lock().unwrap(), &c, id).unwrap().into_iter().find(|m| m.id == msg_id).unwrap()
    }
}
