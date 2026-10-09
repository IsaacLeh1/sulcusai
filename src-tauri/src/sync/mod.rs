// SPDX-License-Identifier: AGPL-3.0-only
//! Sync between the user's PCs on the same network (DESIGN.md §4.9).
//!
//! - Off until the user turns it on or pairs a PC. Works on Offline: nothing
//!   leaves the local network.
//! - PCs find each other with a broadcast on the local network (name, port,
//!   whether a pairing code is showing) and pair with a code shown on one of
//!   them (`wire`: SPAKE2, so a wrong guess learns nothing).
//! - A session sends each side what changed since the other last heard from
//!   it (`track`); for an item both changed, the later edit wins. Chats and
//!   their messages, projects, memories, notes, tasks and "About you" sync.
//!   Models, pictures and video, meetings, mail and calendar accounts,
//!   connectors, plugins, schedules and other settings stay on each PC.
//! - Anything stored encrypted travels inside the session's encryption and
//!   is sealed again with the receiving PC's own data key.

pub mod models;
pub mod track;
mod wire;


use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::Notify;
use tokio::time::timeout;

use crate::crypto::Cipher;
use crate::{db, AppState, AppStateRef};

const KEY: &str = "sync";
const PORT: u16 = 47816;
const DISCOVERY_PORT: u16 = 47817;
/// Crockford base32 without I, L, O, U, so codes are easy to read aloud.
const CODE_ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CODE_LEN: usize = 8;
const CODE_FOR: Duration = Duration::from_secs(300);
const CODE_TRIES: u32 = 5;
const BATCH: usize = 200;
/// A PC not heard from in this long counts as away.
const AWAY: Duration = Duration::from_secs(120);
/// Even with nothing new here, check in this often (the other PC may have news).
const CHECK_IN: i64 = 10 * 60 * 1000;
const HANDSHAKE: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    /// This PC's name as the others see it.
    pub name: String,
    /// Changes when this database is restored from a backup, so the other
    /// PCs send everything again instead of trusting old positions.
    pub epoch: String,
    pub peers: Vec<Peer>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Peer {
    pub id: String,
    pub name: String,
    /// The pair key, sealed with the data key.
    key: String,
    /// Where it last answered, for when broadcasts don't get through.
    pub addr: Option<String>,
    pub paired_at: i64,
    pub last_sync: Option<i64>,
    /// How far it has been sent our changes.
    sent: i64,
    /// How far we've had its changes, and in which of its databases.
    recv_epoch: String,
    recv_since: i64,
}

fn config(conn: &rusqlite::Connection) -> Config {
    let mut c: Config = db::get(conn, KEY).unwrap_or_default();
    if c.name.trim().is_empty() {
        c.name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "This PC".into());
    }
    if c.epoch.is_empty() {
        c.epoch = uuid::Uuid::new_v4().simple().to_string();
        let _ = db::set(conn, KEY, &c);
    }
    c
}

fn update<R>(state: &AppState, f: impl FnOnce(&mut Config) -> R) -> Result<R, String> {
    let conn = state.db.lock().unwrap();
    let mut c = config(&conn);
    let r = f(&mut c);
    db::set(&conn, KEY, &c)?;
    Ok(r)
}

/// After a restore: a new epoch, and everything is exchanged again.
pub fn after_restore(conn: &rusqlite::Connection) {
    let mut c = config(conn);
    c.epoch = uuid::Uuid::new_v4().simple().to_string();
    for p in &mut c.peers {
        p.sent = 0;
        p.recv_since = 0;
    }
    let _ = db::set(conn, KEY, &c);
}

/// This PC's id: kept in its own file, so a database restored onto another
/// PC doesn't bring the old PC's identity along.
pub fn device_id(data: &Path) -> String {
    let file = data.join("device-id");
    if let Ok(id) = std::fs::read_to_string(&file) {
        let id = id.trim();
        if id.len() >= 16 && id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return id.to_string();
        }
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    std::fs::write(&file, &id).ok();
    id
}

struct Seen {
    name: String,
    addr: SocketAddr,
    at: Instant,
    pairing: bool,
}

struct Pairing {
    code: String,
    until: Instant,
    tries: u32,
}

struct Live {
    tasks: Vec<tauri::async_runtime::JoinHandle<()>>,
    port: u16,
}

static LIVE: Mutex<Option<Live>> = Mutex::new(None);
static NEARBY: LazyLock<Mutex<HashMap<String, Seen>>> = LazyLock::new(Default::default);
static PAIRING: Mutex<Option<Pairing>> = Mutex::new(None);
static BUSY: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
static ERRORS: LazyLock<Mutex<HashMap<String, (String, Instant)>>> = LazyLock::new(Default::default);
static WAKE: LazyLock<Notify> = LazyLock::new(Notify::new);
static ANNOUNCE: LazyLock<Notify> = LazyLock::new(Notify::new);
static FORCE: AtomicBool = AtomicBool::new(false);

/// Random bytes (for keys shared with phones, too).
pub fn random_bytes<const N: usize>() -> [u8; N] {
    wire::random()
}

fn emit(state: &AppState, event: &str) {
    if let Some(app) = state.app.get() {
        let _ = app.emit(event, ());
    }
}

/// One session per PC at a time.
struct Busy(String);

impl Busy {
    fn take(id: &str) -> Option<Busy> {
        BUSY.lock().unwrap().insert(id.to_string()).then(|| Busy(id.to_string()))
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.lock().unwrap().remove(&self.0);
    }
}

#[derive(Serialize, Deserialize)]
struct Announce {
    app: String,
    v: u32,
    id: String,
    name: String,
    port: u16,
    #[serde(default)]
    pairing: bool,
}

/// The first frame on a connection (plain): who's calling, and either a
/// session nonce or a pairing message.
#[derive(Serialize, Deserialize)]
struct Open {
    v: u32,
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    pair: Option<String>,
    /// "models" (what can be copied) or "file" (copy one), instead of syncing.
    #[serde(default)]
    want: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Accept {
    id: String,
    nonce: String,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct PairReply {
    id: String,
    name: String,
    spake: String,
    error: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
enum Msg {
    Hello { epoch: String, name: String, want_epoch: String, want_since: i64 },
    Batch { items: Vec<track::Change> },
    End { up_to: i64 },
}

/// Runs sync in the background: listens and announces while it's on, and
/// starts a session with a paired PC when either side has news.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<Arc<AppState>>().inner().clone();
        loop {
            let enabled = config(&state.db.lock().unwrap()).enabled;
            let live = LIVE.lock().unwrap().is_some();
            if enabled && !live {
                go_live(&state).await;
                emit(&state, "sync:status");
            } else if !enabled && live {
                stop();
                emit(&state, "sync:status");
            }
            if enabled {
                tick(&state);
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(15)) => {}
                _ = WAKE.notified() => {}
            }
        }
    });
}

fn stop() {
    if let Some(live) = LIVE.lock().unwrap().take() {
        for t in live.tasks {
            t.abort();
        }
    }
    NEARBY.lock().unwrap().clear();
    *PAIRING.lock().unwrap() = None;
}

async fn go_live(state: &Arc<AppState>) {
    // Windows asks once whether to allow this on private networks.
    let listener = match TcpListener::bind((Ipv4Addr::UNSPECIFIED, PORT)).await {
        Ok(l) => l,
        Err(_) => match TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("sync: can't listen: {e}");
                return;
            }
        },
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(PORT);
    let mut tasks = Vec::new();
    let s = state.clone();
    tasks.push(tauri::async_runtime::spawn(async move {
        let slots = Arc::new(tokio::sync::Semaphore::new(8));
        while let Ok((stream, from)) = listener.accept().await {
            let Ok(slot) = slots.clone().try_acquire_owned() else { continue };
            let s = s.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = timeout(Duration::from_secs(600), answer(&s, stream)).await.unwrap_or(Err("took too long".into())) {
                    eprintln!("sync: from {from}: {e}");
                }
                drop(slot);
            });
        }
    }));
    // Discovery: listen on the shared port if it's free (another copy of the
    // app may hold it), and announce either way.
    let listen = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)).await;
    let sock = match listen {
        Ok(s) => Some(s),
        Err(_) => UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await.ok(),
    };
    if let Some(sock) = sock {
        let sock = Arc::new(sock);
        let _ = sock.set_broadcast(true);
        let s = state.clone();
        let tx = sock.clone();
        tasks.push(tauri::async_runtime::spawn(async move {
            loop {
                let (me, name) = {
                    let conn = s.db.lock().unwrap();
                    (device_id(&s.paths.data), config(&conn).name)
                };
                let pairing = PAIRING.lock().unwrap().as_ref().is_some_and(|p| p.until > Instant::now());
                let msg = Announce { app: "sulcusai".into(), v: 1, id: me, name, port, pairing };
                if let Ok(bytes) = serde_json::to_vec(&msg) {
                    let _ = tx.send_to(&bytes, (Ipv4Addr::BROADCAST, DISCOVERY_PORT)).await;
                }
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                    _ = ANNOUNCE.notified() => {}
                }
            }
        }));
        let s = state.clone();
        tasks.push(tauri::async_runtime::spawn(async move {
            let me = device_id(&s.paths.data);
            let mut buf = vec![0u8; 2048];
            while let Ok((n, from)) = sock.recv_from(&mut buf).await {
                let Ok(a) = serde_json::from_slice::<Announce>(&buf[..n]) else { continue };
                if a.app != "sulcusai" || a.v != 1 || a.id == me || a.id.len() > 64 {
                    continue;
                }
                let new = {
                    let mut near = NEARBY.lock().unwrap();
                    let new = near.get(&a.id).is_none_or(|x| x.pairing != a.pairing || x.at.elapsed() > AWAY);
                    near.insert(a.id, Seen { name: a.name.chars().take(60).collect(), addr: SocketAddr::new(from.ip(), a.port), at: Instant::now(), pairing: a.pairing });
                    new
                };
                if new {
                    emit(&s, "sync:status");
                    WAKE.notify_one();
                }
            }
        }));
    }
    *LIVE.lock().unwrap() = Some(Live { tasks, port });
}

fn seen_addr(id: &str) -> Option<SocketAddr> {
    NEARBY.lock().unwrap().get(id).filter(|s| s.at.elapsed() < AWAY).map(|s| s.addr)
}

/// Starts sessions with the paired PCs that are due one.
fn tick(state: &Arc<AppState>) {
    if state.work_cipher().is_err() {
        return;
    }
    let force = FORCE.swap(false, Ordering::SeqCst);
    let due: Vec<(String, SocketAddr)> = {
        let conn = state.db.lock().unwrap();
        let now = db::now_ms();
        config(&conn)
            .peers
            .iter()
            .filter_map(|p| {
                let seen = seen_addr(&p.id);
                let addr = seen.or_else(|| p.addr.as_deref()?.parse().ok())?;
                if !force && ERRORS.lock().unwrap().get(&p.id).is_some_and(|(_, at)| at.elapsed() < Duration::from_secs(60)) {
                    return None;
                }
                let stale = seen.is_some() && p.last_sync.is_none_or(|t| now - t > CHECK_IN);
                (force || stale || track::has_news(&conn, p.sent, &p.id)).then(|| (p.id.clone(), addr))
            })
            .collect()
    };
    for (id, addr) in due {
        let s = state.clone();
        tauri::async_runtime::spawn(async move {
            let result = timeout(Duration::from_secs(600), call(&s, &id, addr)).await.unwrap_or(Err("The sync took too long.".into()));
            match result {
                Ok(_) => {
                    ERRORS.lock().unwrap().remove(&id);
                }
                Err(e) if e == BUSY_TEXT => {}
                Err(e) => {
                    ERRORS.lock().unwrap().insert(id, (e, Instant::now()));
                    emit(&s, "sync:status");
                }
            }
        });
    }
}

const BUSY_TEXT: &str = "busy";

fn peer(state: &AppState, id: &str) -> Option<Peer> {
    config(&state.db.lock().unwrap()).peers.into_iter().find(|p| p.id == id)
}

fn pair_key(c: &Cipher, p: &Peer) -> Result<zeroize::Zeroizing<Vec<u8>>, String> {
    let hex = zeroize::Zeroizing::new(c.decrypt(&p.key)?);
    hex::decode(hex.as_str()).map(zeroize::Zeroizing::new).map_err(|_| "This PC's pairing is damaged. Pair them again.".to_string())
}

async fn send_plain<T: Serialize>(stream: &mut TcpStream, msg: &T) -> Result<(), String> {
    wire::write_frame(stream, &serde_json::to_vec(msg).map_err(|e| e.to_string())?).await.map_err(lost)
}

async fn recv_plain<T: for<'de> Deserialize<'de>>(stream: &mut TcpStream) -> Result<T, String> {
    let bytes = timeout(HANDSHAKE, wire::read_frame(stream)).await.map_err(|_| "The other PC didn't answer.".to_string())?.map_err(lost)?;
    serde_json::from_slice(&bytes).map_err(|_| "The other PC sent something unexpected.".to_string())
}

fn lost(e: std::io::Error) -> String {
    format!("The connection was lost ({e}).")
}

/// Calls a paired PC and syncs with it.
async fn call(state: &Arc<AppState>, id: &str, addr: SocketAddr) -> Result<usize, String> {
    let Some(_busy) = Busy::take(id) else { return Err(BUSY_TEXT.into()) };
    let c = state.work_cipher()?;
    let p = peer(state, id).ok_or("That PC isn't paired anymore.")?;
    let key = pair_key(&c, &p)?;
    let me = device_id(&state.paths.data);
    let mut stream = timeout(Duration::from_secs(5), TcpStream::connect(addr))
        .await
        .map_err(|_| format!("{} didn't answer. Is SulcusAI open there, with sync on?", p.name))?
        .map_err(|_| format!("Couldn't reach {}. Is SulcusAI open there, with sync on?", p.name))?;
    let nonce = wire::random::<32>();
    send_plain(&mut stream, &Open { v: 1, id: me, name: String::new(), nonce: Some(hex::encode(nonce)), pair: None, want: None }).await?;
    let accept: Accept = recv_plain(&mut stream).await.map_err(|_| format!("{} didn't accept this PC. If it was unpaired or reset there, pair them again.", p.name))?;
    if accept.id != p.id {
        return Err("A different PC answered at that address.".into());
    }
    let theirs = hex::decode(&accept.nonce).map_err(|_| "The other PC sent something unexpected.".to_string())?;
    let (to, from) = wire::session_keys(&key, &nonce, &theirs);
    session(state, stream, &c, &p, wire::Sealer::new(&to), wire::Sealer::new(&from), Some(addr)).await
}

/// Opens an authenticated link to a paired PC for a request other than
/// syncing; `ask` (if not null) is sent first, sealed.
async fn request(
    state: &Arc<AppState>,
    p: &Peer,
    addr: SocketAddr,
    want: &str,
    ask: &serde_json::Value,
) -> Result<(tokio::net::tcp::OwnedReadHalf, tokio::net::tcp::OwnedWriteHalf, wire::Sealer, wire::Sealer), String> {
    let c = state.work_cipher()?;
    let key = pair_key(&c, p)?;
    let me = device_id(&state.paths.data);
    let mut stream = timeout(Duration::from_secs(5), TcpStream::connect(addr))
        .await
        .map_err(|_| format!("{} didn't answer.", p.name))?
        .map_err(|_| format!("Couldn't reach {}.", p.name))?;
    let nonce = wire::random::<32>();
    send_plain(&mut stream, &Open { v: 1, id: me, name: String::new(), nonce: Some(hex::encode(nonce)), pair: None, want: Some(want.into()) }).await?;
    let accept: Accept = recv_plain(&mut stream).await.map_err(|_| format!("{} didn't accept this PC.", p.name))?;
    if accept.id != p.id {
        return Err("A different PC answered at that address.".into());
    }
    let theirs = hex::decode(&accept.nonce).map_err(|e| e.to_string())?;
    let (to, from) = wire::session_keys(&key, &nonce, &theirs);
    let (mut out, inn) = (wire::Sealer::new(&to), wire::Sealer::new(&from));
    if !ask.is_null() {
        wire::write_frame(&mut stream, &out.seal(&serde_json::to_vec(ask).map_err(|e| e.to_string())?)).await.map_err(lost)?;
    }
    let (r, w) = stream.into_split();
    Ok((r, w, inn, out))
}

/// Answers a connection: a pairing attempt or a paired PC's session.
async fn answer(state: &Arc<AppState>, mut stream: TcpStream) -> Result<(), String> {
    let open: Open = recv_plain(&mut stream).await?;
    if open.v != 1 {
        return Err("another version".into());
    }
    if let Some(theirs) = &open.pair {
        return pair_answer(state, stream, &open, theirs).await;
    }
    // Unknown PCs, and anything while this PC is locked, get no answer.
    let c = state.work_cipher()?;
    let p = peer(state, &open.id).ok_or("not paired")?;
    let key = pair_key(&c, &p)?;
    // Both PCs may call each other at once; both sessions are harmless.
    let _busy = Busy::take(&p.id);
    let theirs = hex::decode(open.nonce.as_deref().unwrap_or_default()).map_err(|e| e.to_string())?;
    let nonce = wire::random::<32>();
    send_plain(&mut stream, &Accept { id: device_id(&state.paths.data), nonce: hex::encode(nonce) }).await?;
    let (from_them, to_them) = wire::session_keys(&key, &theirs, &nonce);
    let (mut out, mut inn) = (wire::Sealer::new(&to_them), wire::Sealer::new(&from_them));
    match open.want.as_deref() {
        Some("models") => {
            let list = serde_json::to_vec(&models::offered(state)).map_err(|e| e.to_string())?;
            wire::write_frame(&mut stream, &out.seal(&list)).await.map_err(lost)
        }
        Some("file") => {
            let ask = timeout(HANDSHAKE, wire::read_frame(&mut stream)).await.map_err(|_| "no request".to_string())?.map_err(lost)?;
            let ask: models::FileAsk = serde_json::from_slice(&inn.open(&ask)?).map_err(|e| e.to_string())?;
            let (_r, mut w) = stream.into_split();
            models::serve(state, ask, &mut w, &mut out).await
        }
        Some(_) => Err("unknown request".into()),
        None => session(state, stream, &c, &p, out, inn, None).await.map(|_| ()),
    }
}

async fn send_msg(w: &mut (impl tokio::io::AsyncWrite + Unpin), out: &mut wire::Sealer, msg: &Msg) -> Result<(), String> {
    let bytes = serde_json::to_vec(msg).map_err(|e| e.to_string())?;
    wire::write_frame(w, &out.seal(&bytes)).await.map_err(lost)
}

async fn recv_msg(r: &mut (impl tokio::io::AsyncRead + Unpin), inn: &mut wire::Sealer) -> Result<Msg, String> {
    let sealed = timeout(Duration::from_secs(120), wire::read_frame(r)).await.map_err(|_| "The other PC stopped answering.".to_string())?.map_err(lost)?;
    serde_json::from_slice(&inn.open(&sealed)?).map_err(|_| "The other PC sent something unexpected.".to_string())
}

/// Both sides send what the other hasn't seen, at the same time.
async fn session(state: &Arc<AppState>, stream: TcpStream, c: &Cipher, p: &Peer, mut out: wire::Sealer, mut inn: wire::Sealer, addr: Option<SocketAddr>) -> Result<usize, String> {
    let me = device_id(&state.paths.data);
    let (epoch, name, up_to) = {
        let conn = state.db.lock().unwrap();
        let cfg = config(&conn);
        (cfg.epoch, cfg.name, track::max_seq(&conn))
    };
    let (mut r, mut w) = stream.into_split();
    send_msg(&mut w, &mut out, &Msg::Hello { epoch: epoch.clone(), name, want_epoch: p.recv_epoch.clone(), want_since: p.recv_since }).await?;
    let Msg::Hello { epoch: their_epoch, name: their_name, want_epoch, want_since } = recv_msg(&mut r, &mut inn).await? else {
        return Err("The other PC sent something unexpected.".into());
    };
    // Positions from before a restore (or from another database) don't count.
    let since = if want_epoch == epoch && want_since <= up_to { want_since } else { 0 };
    let sending = async {
        let mut after: track::Mark = (since, "\u{10FFFF}".into(), String::new());
        loop {
            let (items, end) = {
                let conn = state.db.lock().unwrap();
                track::changes(&conn, c, &me, &p.id, &after, up_to, BATCH)?
            };
            if !items.is_empty() {
                send_msg(&mut w, &mut out, &Msg::Batch { items }).await?;
            }
            match end {
                Some(m) => after = m,
                None => break,
            }
        }
        send_msg(&mut w, &mut out, &Msg::End { up_to }).await
    };
    let receiving = async {
        let mut cols = track::Columns::default();
        let mut taken = 0;
        let mut later = Vec::new();
        loop {
            match recv_msg(&mut r, &mut inn).await? {
                Msg::Batch { items } => {
                    let conn = state.db.lock().unwrap();
                    let (n, l) = track::apply(&conn, c, &me, &p.id, &items, &mut cols)?;
                    taken += n;
                    later.extend(l);
                }
                Msg::End { up_to } => {
                    // A message whose chat came later; anything still failing
                    // (its chat was deleted here) is dropped.
                    let conn = state.db.lock().unwrap();
                    taken += track::apply(&conn, c, &me, &p.id, &later, &mut cols)?.0;
                    return Ok::<_, String>((taken, up_to));
                }
                Msg::Hello { .. } => return Err("The other PC sent something unexpected.".to_string()),
            }
        }
    };
    let (sent, received) = tokio::join!(sending, receiving);
    sent?;
    let (taken, their_up_to) = received?;
    update(state, |cfg| {
        if let Some(x) = cfg.peers.iter_mut().find(|x| x.id == p.id) {
            x.sent = up_to;
            x.recv_epoch = their_epoch;
            x.recv_since = their_up_to;
            x.last_sync = Some(db::now_ms());
            x.name = their_name.chars().take(60).collect();
            if let Some(a) = addr {
                x.addr = Some(a.to_string());
            }
        }
    })?;
    if taken > 0 {
        state.log("sync", &format!("Synced {taken} change{} from {}", if taken == 1 { "" } else { "s" }, p.name));
        emit(state, "sync:changed");
    }
    emit(state, "sync:status");
    Ok(taken)
}

fn transcript(client: &str, server: &str, client_msg: &[u8], server_msg: &[u8]) -> Vec<u8> {
    serde_json::to_vec(&[client, server, &B64.encode(client_msg), &B64.encode(server_msg)]).unwrap_or_default()
}

fn keep_peer(state: &AppState, c: &Cipher, id: &str, name: &str, key: &[u8], addr: Option<SocketAddr>) -> Result<(), String> {
    let sealed = c.encrypt(&zeroize::Zeroizing::new(hex::encode(key)));
    update(state, |cfg| {
        cfg.enabled = true;
        cfg.peers.retain(|p| p.id != id);
        cfg.peers.push(Peer {
            id: id.to_string(),
            name: name.chars().take(60).collect(),
            key: sealed,
            addr: addr.map(|a| a.to_string()),
            paired_at: db::now_ms(),
            ..Default::default()
        });
    })?;
    state.log("sync", &format!("Paired with {name}"));
    Ok(())
}

/// The PC showing the code: answers one pairing attempt.
async fn pair_answer(state: &Arc<AppState>, mut stream: TcpStream, open: &Open, theirs: &str) -> Result<(), String> {
    let code = {
        let mut p = PAIRING.lock().unwrap();
        match p.as_mut() {
            Some(x) if x.until > Instant::now() && x.tries < CODE_TRIES => {
                x.tries += 1;
                Some(x.code.clone())
            }
            _ => None,
        }
    };
    let Some(code) = code else {
        let reply = PairReply { error: Some("That PC isn't showing a pairing code right now.".into()), ..Default::default() };
        return send_plain(&mut stream, &reply).await;
    };
    let c = state.work_cipher()?;
    let theirs = B64.decode(theirs).map_err(|e| e.to_string())?;
    let me = device_id(&state.paths.data);
    let name = config(&state.db.lock().unwrap()).name;
    let (spake, mine) = wire::pair_start(&code);
    send_plain(&mut stream, &PairReply { id: me.clone(), name, spake: B64.encode(&mine), error: None }).await?;
    let shared = wire::pair_finish(spake, &theirs)?;
    let t = transcript(&open.id, &me, &theirs, &mine);
    let proof = timeout(Duration::from_secs(30), wire::read_frame(&mut stream)).await.map_err(|_| "no proof".to_string())?.map_err(lost)?;
    if !wire::confirm_ok(&shared, "client", &t, &proof) {
        emit(state, "sync:status");
        return Err("wrong code".into());
    }
    wire::write_frame(&mut stream, &wire::confirm(&shared, "server", &t)).await.map_err(lost)?;
    keep_peer(state, &c, &open.id, &open.name, &*wire::pair_key(&shared, &t), None)?;
    *PAIRING.lock().unwrap() = None;
    ANNOUNCE.notify_one();
    emit(state, "sync:status");
    Ok(())
}

/// The PC where the code is typed: pairs with the one showing it.
async fn pair_call(state: &Arc<AppState>, code: &str, addr: SocketAddr) -> Result<String, String> {
    let c = state.work_cipher()?;
    let me = device_id(&state.paths.data);
    let name = config(&state.db.lock().unwrap()).name;
    let mut stream = timeout(Duration::from_secs(5), TcpStream::connect(addr))
        .await
        .map_err(|_| "That PC didn't answer. Is SulcusAI open there?".to_string())?
        .map_err(|_| "Couldn't reach that PC. Is SulcusAI open there, and is it on this network?".to_string())?;
    let (spake, mine) = wire::pair_start(code);
    send_plain(&mut stream, &Open { v: 1, id: me.clone(), name, nonce: None, pair: Some(B64.encode(&mine)), want: None }).await?;
    let reply: PairReply = recv_plain(&mut stream).await?;
    if let Some(e) = reply.error {
        return Err(e);
    }
    let theirs = B64.decode(&reply.spake).map_err(|_| "The other PC sent something unexpected.".to_string())?;
    let shared = wire::pair_finish(spake, &theirs)?;
    let t = transcript(&me, &reply.id, &mine, &theirs);
    wire::write_frame(&mut stream, &wire::confirm(&shared, "client", &t)).await.map_err(lost)?;
    let wrong = "That code didn't match. Check the code on the other PC and try again.";
    let proof = timeout(HANDSHAKE, wire::read_frame(&mut stream)).await.map_err(|_| wrong.to_string())?.map_err(|_| wrong.to_string())?;
    if !wire::confirm_ok(&shared, "server", &t, &proof) {
        return Err(wrong.into());
    }
    keep_peer(state, &c, &reply.id, &reply.name, &*wire::pair_key(&shared, &t), Some(addr))?;
    Ok(reply.name)
}

/// "k7q2-9xmb" → "K7Q29XMB"; the letters people mix up read as digits.
fn clean_code(code: &str) -> Option<String> {
    let s: String = code
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        })
        .collect();
    (s.len() == CODE_LEN && s.bytes().all(|b| CODE_ALPHABET.contains(&b))).then_some(s)
}

fn new_code() -> String {
    let bytes = wire::random::<CODE_LEN>();
    bytes.iter().map(|b| CODE_ALPHABET[(*b as usize) % CODE_ALPHABET.len()] as char).collect()
}

#[derive(Serialize)]
pub struct PeerView {
    id: String,
    name: String,
    last_sync: Option<i64>,
    online: bool,
    syncing: bool,
    error: Option<String>,
}

#[derive(Serialize)]
pub struct NearbyView {
    id: String,
    name: String,
    pairing: bool,
}

#[derive(Serialize)]
pub struct SyncView {
    enabled: bool,
    name: String,
    listening: bool,
    port: Option<u16>,
    /// This PC's address on the local network, to type on the other PC when
    /// broadcasts don't get through.
    address: Option<String>,
    peers: Vec<PeerView>,
    nearby: Vec<NearbyView>,
    /// The code this PC is showing ("K7Q2-9XMB") and its seconds left.
    code: Option<String>,
    code_left: u64,
}

/// The address this PC's default route uses. Connecting a UDP socket only
/// looks up the route; nothing is sent.
fn local_ip() -> Option<IpAddr> {
    let s = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    s.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    s.local_addr().ok().map(|a| a.ip()).filter(|ip| !ip.is_unspecified() && !ip.is_loopback())
}

fn view(state: &AppState) -> SyncView {
    let cfg = config(&state.db.lock().unwrap());
    let live = LIVE.lock().unwrap().as_ref().map(|l| l.port);
    let busy = BUSY.lock().unwrap().clone();
    let errors = ERRORS.lock().unwrap();
    let near = NEARBY.lock().unwrap();
    let paired: HashSet<&str> = cfg.peers.iter().map(|p| p.id.as_str()).collect();
    let pairing = PAIRING.lock().unwrap();
    let pairing = pairing.as_ref().filter(|p| p.until > Instant::now());
    SyncView {
        enabled: cfg.enabled,
        name: cfg.name.clone(),
        listening: live.is_some(),
        port: live,
        address: live.and_then(|port| local_ip().map(|ip| if port == PORT { ip.to_string() } else { SocketAddr::new(ip, port).to_string() })),
        peers: cfg
            .peers
            .iter()
            .map(|p| PeerView {
                id: p.id.clone(),
                name: p.name.clone(),
                last_sync: p.last_sync,
                online: near.get(&p.id).is_some_and(|s| s.at.elapsed() < AWAY),
                syncing: busy.contains(&p.id),
                error: errors.get(&p.id).map(|e| e.0.clone()),
            })
            .collect(),
        nearby: near
            .iter()
            .filter(|(id, s)| !paired.contains(id.as_str()) && s.at.elapsed() < AWAY)
            .map(|(id, s)| NearbyView { id: id.clone(), name: s.name.clone(), pairing: s.pairing })
            .collect(),
        code: pairing.map(|p| format!("{}-{}", &p.code[..4], &p.code[4..])),
        code_left: pairing.map_or(0, |p| p.until.saturating_duration_since(Instant::now()).as_secs()),
    }
}

#[tauri::command]
pub fn sync_view(state: AppStateRef) -> SyncView {
    view(&state)
}

#[tauri::command]
pub fn set_sync(state: AppStateRef, enabled: bool, name: Option<String>) -> Result<SyncView, String> {
    update(&state, |c| {
        c.enabled = enabled;
        if let Some(n) = name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            c.name = n.chars().take(60).collect();
        }
    })?;
    state.log("sync", if enabled { "Turned on sync with my other PCs" } else { "Turned off sync with my other PCs" });
    WAKE.notify_one();
    ANNOUNCE.notify_one();
    Ok(view(&state))
}

/// Shows a code on this PC for five minutes (and turns sync on, so the other
/// PC can reach it).
#[tauri::command]
pub fn start_pairing(state: AppStateRef) -> Result<SyncView, String> {
    state.work_cipher()?;
    update(&state, |c| c.enabled = true)?;
    *PAIRING.lock().unwrap() = Some(Pairing { code: new_code(), until: Instant::now() + CODE_FOR, tries: 0 });
    WAKE.notify_one();
    ANNOUNCE.notify_one();
    Ok(view(&state))
}

#[tauri::command]
pub fn cancel_pairing(state: AppStateRef) -> SyncView {
    *PAIRING.lock().unwrap() = None;
    ANNOUNCE.notify_one();
    view(&state)
}

/// Pairs with the PC showing `code`: the one named (a nearby PC's id or an
/// address), or the only one nearby showing a code.
#[tauri::command]
pub async fn pair_device(state: AppStateRef<'_>, code: String, target: Option<String>) -> Result<SyncView, String> {
    let code = clean_code(&code).ok_or("A code is 8 letters and numbers, like K7Q2-9XMB.")?;
    let target = target.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let addr = match target {
        Some(t) => match NEARBY.lock().unwrap().get(&t).map(|s| s.addr) {
            Some(a) => a,
            None => t
                .parse::<SocketAddr>()
                .ok()
                .or_else(|| t.parse::<IpAddr>().ok().map(|ip| SocketAddr::new(ip, PORT)))
                .ok_or("Type the other PC's address as shown there, like 192.168.1.20.")?,
        },
        None => {
            let near = NEARBY.lock().unwrap();
            let showing: Vec<SocketAddr> = near.values().filter(|s| s.pairing && s.at.elapsed() < AWAY).map(|s| s.addr).collect();
            match showing.as_slice() {
                [one] => *one,
                [] => return Err("No PC on this network is showing a code yet. On the other PC, choose Show a code, or type its address here.".into()),
                _ => return Err("More than one PC is showing a code. Pick which one.".into()),
            }
        }
    };
    let state = state.inner().clone();
    pair_call(&state, &code, addr).await?;
    WAKE.notify_one();
    FORCE.store(true, Ordering::SeqCst);
    emit(&state, "sync:status");
    Ok(view(&state))
}

#[tauri::command]
pub fn remove_device(state: AppStateRef, id: String) -> Result<SyncView, String> {
    let name = update(&state, |c| {
        let name = c.peers.iter().find(|p| p.id == id).map(|p| p.name.clone());
        c.peers.retain(|p| p.id != id);
        name
    })?;
    if let Some(n) = name {
        state.log("sync", &format!("Unpaired {n}"));
    }
    Ok(view(&state))
}

#[tauri::command]
pub fn sync_now(state: AppStateRef) -> SyncView {
    ERRORS.lock().unwrap().clear();
    FORCE.store(true, Ordering::SeqCst);
    WAKE.notify_one();
    view(&state)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two PCs (two data folders) on this machine pair over the loopback
    /// network and sync both ways. Uses DPAPI like the real app.
    #[cfg(windows)]
    #[tokio::test]
    #[ignore]
    async fn e2e_sync_two_pcs() {
        let a = crate::e2e::temp_state(None);
        let b = crate::e2e::temp_state(None);
        update(&a, |c| c.name = "PC-A".into()).unwrap();
        update(&b, |c| c.name = "PC-B".into()).unwrap();
        let (ca, cb) = (a.cipher().unwrap(), b.cipher().unwrap());
        let chat = crate::db::create_chat_in(&a.db.lock().unwrap(), &ca, None, None, false).unwrap();
        crate::db::set_chat_title(&a.db.lock().unwrap(), &ca, &chat.id, "Trip plans").unwrap();
        let note = crate::notes::NoteBody { title: "Shopping".into(), body: "eggs".into(), folder: String::new(), tags: vec![] };
        crate::notes::save_note(&b.db.lock().unwrap(), &cb, None, note).unwrap();

        // A listens and shows a code.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let sa = a.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let s = sa.clone();
                tokio::spawn(async move {
                    if let Err(e) = answer(&s, stream).await {
                        eprintln!("A answered: {e}");
                    }
                });
            }
        });
        *PAIRING.lock().unwrap() = Some(Pairing { code: "K7Q29XMB".into(), until: Instant::now() + CODE_FOR, tries: 0 });

        let wrong = pair_call(&b, "K7Q29XMC", addr).await.unwrap_err();
        assert!(wrong.contains("didn't match"), "{wrong}");
        assert!(config(&a.db.lock().unwrap()).peers.is_empty());
        assert_eq!(pair_call(&b, "K7Q29XMB", addr).await.unwrap(), "PC-A");
        assert!(PAIRING.lock().unwrap().is_none(), "a code works once");
        let a_id = device_id(&a.paths.data);
        let b_id = device_id(&b.paths.data);
        assert_eq!(config(&a.db.lock().unwrap()).peers[0].name, "PC-B");

        let t = Instant::now();
        let taken = call(&b, &a_id, addr).await.unwrap();
        eprintln!("first sync: {taken} change(s) to B in {:?}", t.elapsed());
        assert!(taken >= 1);
        let titles: Vec<String> = crate::db::list_chats(&b.db.lock().unwrap(), &cb).into_iter().map(|c| c.title).collect();
        assert!(titles.contains(&"Trip plans".to_string()), "{titles:?}");
        let notes = crate::notes::list_notes(&a.db.lock().unwrap(), &ca);
        assert!(notes.iter().any(|n| n.data.title == "Shopping" && n.data.body == "eggs"));
        assert!(config(&b.db.lock().unwrap()).peers[0].last_sync.is_some());

        // Nothing new: nothing moves. Then one edit on A moves.
        assert_eq!(call(&b, &a_id, addr).await.unwrap(), 0);
        crate::db::set_chat_title(&a.db.lock().unwrap(), &ca, &chat.id, "Italy trip").unwrap();
        {
            let conn = a.db.lock().unwrap();
            assert!(track::has_news(&conn, config(&conn).peers[0].sent, &b_id));
        }
        assert_eq!(call(&b, &a_id, addr).await.unwrap(), 1);
        let titles: Vec<String> = crate::db::list_chats(&b.db.lock().unwrap(), &cb).into_iter().map(|c| c.title).collect();
        assert!(titles.contains(&"Italy trip".to_string()), "{titles:?}");

        // Models: A offers its installed catalog model; B copies its file.
        let spec = a.catalog.model("qwen3-1.7b").unwrap().clone();
        let v = spec.variant("Q4_K_M").unwrap().clone();
        let file = a.paths.models.join(&spec.id).join(&v.file);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::File::create(&file).unwrap().set_len(v.size).unwrap();
        db::save_installed(&a.db.lock().unwrap(), &db::InstalledModel { model_id: spec.id.clone(), quant: v.quant.clone(), path: file.display().to_string(), size: v.size, installed_at: 1, tps: None }).unwrap();
        let pa = peer(&b, &a_id).unwrap();
        let (mut r, _w, mut inn, _out) = request(&b, &pa, addr, "models", &serde_json::Value::Null).await.unwrap();
        let list: Vec<models::Offered> = serde_json::from_slice(&inn.open(&wire::read_frame(&mut r).await.unwrap()).unwrap()).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].files[0], (v.file.clone(), v.size));
        let t = Instant::now();
        let ask = serde_json::json!({ "model_id": spec.id, "file": v.file });
        let (mut r, _w, mut inn, _out) = request(&b, &pa, addr, "file", &ask).await.unwrap();
        let dest = b.paths.models.join(&spec.id).join(&v.file);
        let mut last = 0;
        models::receive_for_test(&mut r, &mut inn, &dest, &mut |got| last = got).await.unwrap();
        eprintln!("copied {} MB in {:?}", v.size >> 20, t.elapsed());
        assert_eq!(dest.metadata().unwrap().len(), v.size);
        assert_eq!(last, v.size);
        // Files that aren't an installed model's are refused.
        let ask = serde_json::json!({ "model_id": spec.id, "file": "../../sulcusai.db" });
        let (mut r, _w, mut inn, _out) = request(&b, &pa, addr, "file", &ask).await.unwrap();
        assert!(models::receive_for_test(&mut r, &mut inn, &b.paths.data.join("stolen"), &mut |_| {}).await.is_err());

        // A PC that was never paired gets nowhere.
        let c = crate::e2e::temp_state(None);
        let fake = Peer { id: a_id.clone(), name: "PC-A".into(), key: c.cipher().unwrap().encrypt(&hex::encode([7u8; 32])), ..Default::default() };
        update(&c, |cfg| cfg.peers.push(fake)).unwrap();
        let e = call(&c, &a_id, addr).await.unwrap_err();
        eprintln!("unpaired: {e}");
        // After B is removed on A, B's calls fail too.
        let mut cfg = config(&a.db.lock().unwrap());
        cfg.peers.clear();
        db::set(&a.db.lock().unwrap(), KEY, &cfg).unwrap();
        assert!(call(&b, &a_id, addr).await.is_err());
    }

    #[test]
    fn codes_read_forgivingly() {
        assert_eq!(clean_code("k7q2-9xmb").as_deref(), Some("K7Q29XMB"));
        assert_eq!(clean_code("K7Q2 9XMO").as_deref(), Some("K7Q29XM0"));
        assert_eq!(clean_code("K7Q2-9XM").as_deref(), None);
        assert_eq!(clean_code("K7Q2-9XMU"), None);
        let c = new_code();
        assert_eq!(clean_code(&c).as_deref(), Some(c.as_str()));
    }
}
