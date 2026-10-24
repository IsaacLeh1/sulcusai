// SPDX-License-Identifier: AGPL-3.0-only
//! The live link with someone: one connection per person, kept open while
//! both apps run (a ping every 30 s; 90 s without a word means it's gone),
//! carrying sealed frames both ways. Messages and shared chats travel on it.
//!
//! - Whichever app can reach the other calls; one reachable side is enough.
//! - If both call at once, both keep the call made by the app with the
//!   smaller id, so they always agree on one.
//! - A frame this version doesn't know is skipped, so newer apps can add kinds.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;

use tauri::AppHandle;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, watch, Notify};

use super::*;

const PING: Duration = Duration::from_secs(30);
const QUIET: Duration = Duration::from_secs(90);
/// Two calls this close together crossed each other.
const RACE: Duration = Duration::from_secs(10);
/// Waiting longer after each failed call, up to this.
const MOST_BETWEEN: Duration = Duration::from_secs(120);

/// What travels on the link.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub(super) enum Frame {
    /// The first frame each way: who this is now, and where it takes calls
    /// (`parts`: it takes big files in parts).
    Hello {
        name: String,
        port: u16,
        #[serde(default)]
        parts: bool,
    },
    Ping,
    Msgs { items: Vec<Item> },
    /// Messages now stored on the other side.
    Got { ids: Vec<String> },
    /// Messages the other person has read (they send read receipts).
    Read { ids: Vec<String> },
    /// Their messages they changed or deleted after sending, and the
    /// changes now stored on the other side.
    Change { items: Vec<Change> },
    Changed { ids: Vec<String> },
    /// The chats the other side shares with this one.
    Shared { chats: Vec<share::SharedChat> },
    Watch { chat_id: String },
    Unwatch { chat_id: String },
    Snapshot { chat: share::SharedChat, messages: Vec<share::SharedMsg> },
    Live { chat_id: String, live: share::Live },
    /// Not shared (anymore).
    Gone { chat_id: String },
    /// A question from their assistant to this one, and what became of it.
    Ask { id: String, question: String },
    Status { id: String, status: String },
    Answer { id: String, text: Option<String>, error: Option<String> },
    Answered { id: String },
    /// Working something out together: the goal, each turn, or the end.
    Task { id: String, goal: String },
    Turn { id: String, text: String, done: bool },
    End { id: String, reason: String },
    /// The relay they may use to reach this app (None: not anymore).
    Relay { url: Option<String> },
    /// Taking part in a chat the other side shares: a message written here,
    /// and the answer this side's model gave to it.
    Post { chat_id: String, id: String, text: String },
    PostReply { chat_id: String, id: String, text: String, model: String },
    /// A big file's parts (files.rs): asked for from a point, a part, how
    /// far they've arrived, and "that file isn't here anymore".
    Want { id: String, from: u64 },
    Part { id: String, at: u64, data: String },
    Have { id: String, upto: u64 },
    NoFile { id: String },
    /// What this side lets the other use in the chats it shares with them:
    /// its models (none: nothing) and the limits.
    Lends { models: Vec<lend::LentModel>, limits: lend::Lend },
    /// A message written in a chat the other side shares, for one of the
    /// models it lends to answer there (`model`; empty: its default).
    Borrow { chat_id: String, id: String, text: String, model: String },
    /// That side's PC didn't answer it, and why.
    Refused { chat_id: String, why: String },
    #[serde(other)]
    Unknown,
}

struct Conn {
    serial: u64,
    tx: mpsc::UnboundedSender<Frame>,
    stop: watch::Sender<bool>,
    opened: Instant,
    /// The app that made the call.
    caller: String,
    /// Messages sent on this connection and not confirmed yet.
    sending: HashSet<String>,
}

static CONNS: LazyLock<Mutex<HashMap<String, Conn>>> = LazyLock::new(Default::default);
static SERIAL: AtomicU64 = AtomicU64::new(1);

/// Calling someone who isn't connected: when it last tried, and why it failed.
#[derive(Default)]
struct Dial {
    tried: Option<Instant>,
    fails: u32,
    error: Option<String>,
}

static DIALS: LazyLock<Mutex<HashMap<String, Dial>>> = LazyLock::new(Default::default);
static WAKE: LazyLock<Notify> = LazyLock::new(Notify::new);

pub(super) fn connected(id: &str) -> bool {
    CONNS.lock().unwrap().contains_key(id)
}

/// Sends a frame: directly if they're connected, else by mail when they're
/// reached that way (and it isn't a live view).
pub(super) fn send(id: &str, f: Frame) -> bool {
    if let Some(c) = CONNS.lock().unwrap().get(id) {
        return c.tx.send(f).is_ok();
    }
    if mailbox::open_for(id) && mailbox::relayable(&f) {
        mailbox::queue(id, f);
        return true;
    }
    false
}

/// They can be reached now: connected directly, or by mail.
pub(super) fn reachable(id: &str) -> bool {
    connected(id) || mailbox::open_for(id)
}

/// Why the last call to them didn't get through.
pub(super) fn error(id: &str) -> Option<String> {
    DIALS.lock().unwrap().get(id).and_then(|d| d.error.clone())
}

/// Call them on the next tick, whatever happened before.
pub(super) fn reset(id: &str) {
    DIALS.lock().unwrap().insert(id.to_string(), Dial::default());
}

pub(super) fn forget(id: &str) {
    DIALS.lock().unwrap().remove(id);
}

pub(super) fn wake() {
    WAKE.notify_one();
}

/// Ends every connection (fully offline).
pub(super) fn hang_up_all() {
    for (_, c) in CONNS.lock().unwrap().drain() {
        let _ = c.stop.send(true);
    }
}

/// Ends the connection with them, if any.
pub(super) fn hang_up(id: &str) {
    if let Some(c) = CONNS.lock().unwrap().remove(id) {
        let _ = c.stop.send(true);
    }
}

/// Tells everyone connected this user's (new) name.
pub(super) fn hello_all(state: &AppState) {
    let name = my_name(state);
    for c in CONNS.lock().unwrap().values() {
        let _ = c.tx.send(Frame::Hello { name: name.clone(), port: listening_port(), parts: true });
    }
}

/// Sends what's waiting for someone, if they're connected. More goes once
/// they confirm what they got.
pub(super) fn flush(state: &AppState, id: &str) {
    let Ok(c) = state.work_cipher() else { return };
    let skip = match CONNS.lock().unwrap().get(id) {
        Some(conn) => conn.sending.clone(),
        None => return,
    };
    let Ok(items) = outgoing(state, &c, id, &skip) else { return };
    if items.is_empty() {
        return;
    }
    let mut conns = CONNS.lock().unwrap();
    if let Some(conn) = conns.get_mut(id) {
        conn.sending.extend(items.iter().map(|i| i.id.clone()));
        let _ = conn.tx.send(Frame::Msgs { items });
    }
}

/// Keeps one connection per person. False: this one lost a crossed call.
fn register(id: &str, serial: u64, caller: &str, tx: mpsc::UnboundedSender<Frame>, stop: watch::Sender<bool>) -> bool {
    let mut conns = CONNS.lock().unwrap();
    if let Some(old) = conns.get(id) {
        if old.opened.elapsed() < RACE && old.caller.as_str() < caller {
            return false;
        }
        let _ = old.stop.send(true);
    }
    conns.insert(id.to_string(), Conn { serial, tx, stop, opened: Instant::now(), caller: caller.to_string(), sending: HashSet::new() });
    true
}

fn unregister(id: &str, serial: u64) -> bool {
    let mut conns = CONNS.lock().unwrap();
    if conns.get(id).is_some_and(|c| c.serial == serial) {
        conns.remove(id);
        return true;
    }
    false
}

async fn read(r: &mut (impl AsyncRead + Unpin), inn: &mut wire::Sealer, wait: Duration) -> Result<Frame, String> {
    let sealed = timeout(wait, wire::read_frame(r)).await.map_err(|_| "They went quiet.".to_string())?.map_err(lost)?;
    let plain = inn.open(&sealed)?;
    // A kind this version doesn't know (or can't read) is skipped.
    Ok(serde_json::from_slice(&plain).unwrap_or(Frame::Unknown))
}

/// After their hello: who they are now and where they take calls.
fn heard_from(state: &AppState, p: &Contact, addr: Option<SocketAddr>, name: &str) {
    DIALS.lock().unwrap().remove(&p.id);
    let name = clean_name(name);
    let addr = addr.map(|a| a.to_string());
    let now = db::now_ms();
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

/// One connection, from either side, once the keys are agreed: runs until
/// it breaks, goes quiet, or another one takes its place. `stream`: a
/// network connection (`from`: the address it came from), or the pipe
/// through the relay (`pipe`).
#[allow(clippy::too_many_arguments)]
async fn run<S: AsyncRead + AsyncWrite + Send + 'static>(
    state: Arc<AppState>,
    p: Contact,
    stream: S,
    mut out: wire::Sealer,
    mut inn: wire::Sealer,
    caller: String,
    called: Option<SocketAddr>,
    from: Option<SocketAddr>,
    busy: Option<Busy>,
) {
    let (mut r, mut w) = tokio::io::split(stream);
    let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();
    let _ = tx.send(Frame::Hello { name: my_name(&state), port: listening_port(), parts: true });
    let writer = tokio::spawn(async move {
        while let Some(f) = rx.recv().await {
            let Ok(bytes) = serde_json::to_vec(&f) else { continue };
            if wire::write_frame(&mut w, &out.seal(&bytes)).await.is_err() {
                break;
            }
        }
        let _ = w.shutdown().await;
    });
    let Ok(Frame::Hello { name, port, parts }) = read(&mut r, &mut inn, HANDSHAKE).await else {
        writer.abort();
        return;
    };
    let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
    let (stop, mut stopped) = watch::channel(false);
    if !register(&p.id, serial, &caller, tx.clone(), stop) {
        writer.abort();
        return;
    }
    drop(busy);
    files::set_capable(&p.id, parts);
    let addr = called.or_else(|| from.filter(|_| port != 0).map(|a| SocketAddr::new(a.ip(), port)));
    heard_from(&state, &p, addr, &name);
    mailbox::direct(&p.id);
    let _ = tx.send(Frame::Relay { url: mailbox::offer(&state, &p) });
    flush(&state, &p.id);
    super::send_change(&state, &p.id);
    share::connected(&state, &p.id);
    ask::connected(&state, &p.id);
    if let Some(p) = contact(&state, &p.id) {
        lend::connected(&state, &p);
    }
    // Big files from them that didn't all arrive carry on.
    files::ask_for_missing(&state, &p.id);
    changed(&state);

    // Pings keep it open through routers and VPNs, and show when it's gone.
    let ping = tx.clone();
    let mut ping_stop = stopped.clone();
    let pinger = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(PING) => {
                    if ping.send(Frame::Ping).is_err() {
                        break;
                    }
                }
                _ = ping_stop.changed() => break,
            }
        }
    });
    drop(tx);
    loop {
        let next = tokio::select! {
            _ = stopped.changed() => break,
            f = read(&mut r, &mut inn, QUIET) => f,
        };
        match next {
            Ok(f) => dispatch(&state, &p, f),
            Err(_) => break,
        }
    }
    pinger.abort();
    writer.abort();
    if unregister(&p.id, serial) {
        let now = db::now_ms();
        let _ = update(&state, |b| {
            if let Some(x) = b.contacts.iter_mut().find(|x| x.id == p.id) {
                x.last_seen = Some(now);
            }
        });
        share::disconnected(&state, &p.id);
        lend::disconnected(&p.id);
        files::set_capable(&p.id, false);
    }
    changed(&state);
}

pub(super) fn dispatch(state: &Arc<AppState>, p: &Contact, f: Frame) {
    match f {
        Frame::Hello { name, parts, .. } => {
            files::set_capable(&p.id, parts);
            let name = clean_name(&name);
            let _ = update(state, |b| {
                if let Some(x) = b.contacts.iter_mut().find(|x| x.id == p.id) {
                    // A name this user gave them stays.
                    if !x.renamed {
                        x.name = name.clone();
                    }
                    x.their_name = name;
                }
            });
            changed(state);
        }
        Frame::Ping | Frame::Unknown => {}
        Frame::Msgs { items } => {
            let Ok(c) = state.work_cipher() else { return };
            // The latest name, for the notification.
            let p = contact(state, &p.id).unwrap_or_else(|| p.clone());
            match take(state, &c, &p, items) {
                Ok((got, fresh)) => {
                    send(&p.id, Frame::Got { ids: got });
                    arrived(state, &p, &fresh);
                    changed(state);
                }
                Err(e) => eprintln!("people: couldn't keep messages: {e}"),
            }
        }
        Frame::Got { ids } => {
            if let Some(conn) = CONNS.lock().unwrap().get_mut(&p.id) {
                for id in &ids {
                    conn.sending.remove(id);
                }
            }
            let _ = mark_delivered(state, &p.id, &ids);
            mailbox::confirmed(&p.id, &ids);
            changed(state);
            flush(state, &p.id);
            // Changed while on its way: the change goes now it's there.
            if let Ok(c) = state.work_cipher() {
                let late: Vec<Change> = super::changes(state, &c, &p.id, &HashSet::new()).into_iter().filter(|ch| ids.contains(&ch.id)).collect();
                if !late.is_empty() {
                    send(&p.id, Frame::Change { items: late });
                }
            }
        }
        Frame::Change { items } => {
            let Ok(c) = state.work_cipher() else { return };
            let done = super::take_changes(state, &c, p, items);
            if !done.is_empty() {
                send(&p.id, Frame::Changed { ids: done });
            }
        }
        Frame::Changed { ids } => {
            super::changes_confirmed(state, &p.id, &ids);
            mailbox::confirmed(&p.id, &ids.iter().map(|i| format!("edit:{i}")).collect::<Vec<_>>());
            changed(state);
        }
        Frame::Read { ids } => {
            super::they_read(state, &p.id, &ids);
            changed(state);
        }
        Frame::Shared { chats } => share::listed(state, &p.id, chats),
        Frame::Post { chat_id, id, text } => share::posted(state, &p.id, &chat_id, &id, &text, None),
        Frame::PostReply { chat_id, id, text, model } => share::posted(state, &p.id, &chat_id, &id, &text, Some(&model)),
        Frame::Lends { models, limits } => lend::lends(state, &p.id, models, limits),
        Frame::Want { id, from } => files::want(state, &p.id, &id, from),
        Frame::Part { id, at, data } => files::part(state, &p.id, &id, at, &data),
        Frame::Have { id, upto } => files::have(state, &p.id, &id, upto),
        Frame::NoFile { id } => files::gone(state, &p.id, &id),
        Frame::Borrow { chat_id, id, text, model } => {
            share::posted(state, &p.id, &chat_id, &id, &text, None);
            if let Some(p) = contact(state, &p.id) {
                if share::is_shared_with(state, &chat_id, &p.id) {
                    lend::borrow(state, &p, &chat_id, &text, &model);
                }
            }
        }
        Frame::Refused { chat_id, why } => lend::refused(state, &p.id, &chat_id, &why),
        Frame::Watch { chat_id } => share::watch(state, &p.id, &chat_id),
        Frame::Unwatch { chat_id } => share::unwatch(&p.id, &chat_id),
        Frame::Snapshot { chat, messages } => share::snapshot(state, &p.id, chat, messages),
        Frame::Live { chat_id, live } => share::live(state, &p.id, &chat_id, live),
        Frame::Gone { chat_id } => share::gone(state, &p.id, &chat_id),
        Frame::Ask { id, question } => {
            // The latest settings for what their assistant may do.
            if let Some(p) = contact(state, &p.id) {
                ask::incoming(state, &p, &id, &question);
            }
        }
        Frame::Status { id, status } => ask::status(state, &p.id, &id, &status),
        Frame::Answer { id, text, error } => ask::answered(state, &p.id, &id, text, error),
        Frame::Answered { id } => ask::delivered(state, &p.id, &id),
        Frame::Task { id, goal } => {
            if let Some(p) = contact(state, &p.id) {
                together::incoming(state, &p, &id, &goal);
            }
        }
        Frame::Turn { id, text, done } => together::turn(state, &p.id, &id, &text, done),
        Frame::End { id, reason } => together::ended(state, &p.id, &id, &reason),
        Frame::Relay { url } => mailbox::offered(state, &p.id, url),
    }
}

/// Calls someone and keeps the connection open until it ends.
pub(super) async fn call(state: &Arc<AppState>, id: &str, addr: SocketAddr) -> Result<(), String> {
    let Some(busy) = Busy::take(&format!("person:{id}")) else { return Err(BUSY.into()) };
    let stream = connect(addr).await?;
    let from = stream.peer_addr().ok();
    call_on(state, id, stream, Some(addr), from, busy).await
}

/// The call on a stream already open: a network connection, or the pipe.
pub(super) async fn call_on<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    state: &Arc<AppState>,
    id: &str,
    mut stream: S,
    addr: Option<SocketAddr>,
    from: Option<SocketAddr>,
    busy: Busy,
) -> Result<(), String> {
    let c = state.work_cipher()?;
    let p = contact(state, id).ok_or("They aren't linked anymore.")?;
    let key = link_key(&c, &p)?;
    let me = device_id(&state.paths.data);
    let nonce = wire::random::<32>();
    send_plain(&mut stream, &Open { v: 1, id: me.clone(), name: String::new(), nonce: Some(hex::encode(nonce)), pair: None, want: Some(CHAT.into()), tag: None, sealed: false }).await?;
    let accept: Accept = recv_plain(&mut stream)
        .await
        .map_err(|_| format!("{} didn't take the call. If they removed you, link again.", p.name))?;
    if accept.id != p.id {
        return Err("Someone else answered at that address.".into());
    }
    let theirs = hex::decode(&accept.nonce).map_err(|_| UNEXPECTED.to_string())?;
    let (to_them, from_them) = wire::session_keys(&key, &nonce, &theirs);
    run(state.clone(), p, stream, wire::Sealer::new(&to_them), wire::Sealer::new(&from_them), me, addr, from, Some(busy)).await;
    Ok(())
}

pub(super) const BUSY: &str = "busy";

/// Answers a call from someone linked: the connection runs on its own task.
pub(in crate::sync) async fn answer<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(state: &Arc<AppState>, mut stream: S, open: &Open, from: Option<SocketAddr>) -> Result<(), String> {
    if !features::is_on(&state.db.lock().unwrap(), Feature::People) {
        return Err("People is off".into());
    }
    let c = state.work_cipher()?;
    if codes::is_blocked(&book(&state.db.lock().unwrap()), &open.id) {
        return Err("blocked".into());
    }
    let p = contact(state, &open.id).ok_or("not linked")?;
    let key = link_key(&c, &p)?;
    let theirs = hex::decode(open.nonce.as_deref().unwrap_or_default()).map_err(err)?;
    let nonce = wire::random::<32>();
    send_plain(&mut stream, &Accept { id: device_id(&state.paths.data), nonce: hex::encode(nonce) }).await?;
    let (from_them, to_them) = wire::session_keys(&key, &theirs, &nonce);
    let (state, caller) = (state.clone(), open.id.clone());
    spawn(async move {
        run(state, p, stream, wire::Sealer::new(&to_them), wire::Sealer::new(&from_them), caller, None, from, None).await;
    });
    Ok(())
}

/// Calls whoever isn't connected, waiting longer after each failed try.
fn tick(state: &Arc<AppState>) {
    if state.work_cipher().is_err() || crate::offline::blocks(crate::offline::Keep::LocalNetwork) {
        return;
    }
    if features::is_on(&state.db.lock().unwrap(), Feature::People) {
        mailbox::tick(state);
        // Through SulcusAI's relay, for anyone not reachable directly.
        super::pipe::tick(state);
    }
    let (on, contacts) = {
        let conn = state.db.lock().unwrap();
        (features::is_on(&conn, Feature::People), book(&conn).contacts)
    };
    if !on {
        // Off means off: nobody stays connected.
        let ids: Vec<String> = CONNS.lock().unwrap().keys().cloned().collect();
        for id in ids {
            hang_up(&id);
        }
        return;
    }
    for p in contacts {
        if connected(&p.id) {
            continue;
        }
        let Some(addr) = super::super::seen_addr(&p.id).or_else(|| p.addr.as_deref()?.parse().ok()) else { continue };
        {
            let mut dials = DIALS.lock().unwrap();
            let d = dials.entry(p.id.clone()).or_default();
            let wait = if d.fails == 0 { Duration::ZERO } else { (Duration::from_secs(10) * (1 << (d.fails - 1).min(4))).min(MOST_BETWEEN) };
            if d.tried.is_some_and(|t| t.elapsed() < wait) {
                continue;
            }
            d.tried = Some(Instant::now());
        }
        let s = state.clone();
        spawn(async move {
            let result = call(&s, &p.id, addr).await;
            let mut dials = DIALS.lock().unwrap();
            let d = dials.entry(p.id.clone()).or_default();
            match result {
                // It was connected, and now it ended: try again (after a pause if that fails).
                Ok(()) => {
                    d.fails = 0;
                    d.error = None;
                }
                Err(e) if e == BUSY => {}
                Err(e) => {
                    d.fails = d.fails.saturating_add(1);
                    d.error = Some(e);
                    drop(dials);
                    changed(&s);
                }
            }
        });
    }
}

/// Runs People in the background (the sync loop does the listening).
pub(in crate::sync) fn start(app: AppHandle) {
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
