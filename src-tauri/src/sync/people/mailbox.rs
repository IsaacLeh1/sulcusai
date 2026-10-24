// SPDX-License-Identifier: AGPL-3.0-only
//! People across the internet without a virtual network (People, phase 5):
//! when two linked apps can't connect directly, messages, questions and the
//! assistants' turns go through a relay mailbox, the kind sync uses
//! (`sync::relay`, `relay/worker.js`).
//!
//! - Either person can let the other use their relay (set in Settings →
//!   Your devices; each person in People has a switch). If both do, both use
//!   the one of the app with the smaller id, so they read the same boxes.
//! - Only at the Cloud level, like sync's relay. Letters are sealed with keys
//!   from the link key, so the relay can't read them. Its address (which can
//!   hold a secret) goes only to the people it's offered to, over the link.
//! - Live things (watching a shared chat) need a direct connection, and so do
//!   files over about a megabyte (a letter holds 2 MB).

use std::sync::LazyLock;

use super::super::relay::{box_id, open, seal, token};
use super::link::{self, Frame};
use super::*;
use crate::net::{self, Connectivity, Purpose};

/// What a letter carries at most before sealing.
const LETTER: usize = 1_400_000;
/// How often to check the mailbox: while something is going on, and otherwise.
const BUSY_EVERY: Duration = Duration::from_secs(5);
const IDLE_EVERY: Duration = Duration::from_secs(30);
/// "Something going on" lasts this long after the last letter either way.
const ACTIVE_FOR: Duration = Duration::from_secs(180);
/// Counts as reachable by mail this long after the last exchange that worked.
const FRESH: Duration = Duration::from_secs(120);

#[derive(Serialize, Deserialize)]
struct Letter {
    frames: Vec<Frame>,
}

#[derive(Deserialize)]
struct Stored {
    seq: u64,
    data: String,
}

#[derive(Default)]
struct Post {
    tried: Option<Instant>,
    ok: Option<Instant>,
    active_until: Option<Instant>,
    error: Option<String>,
    fails: u32,
}

/// Frames waiting for the next exchange, by person.
static OUTBOX: LazyLock<Mutex<HashMap<String, Vec<Frame>>>> = LazyLock::new(Default::default);
/// Messages sent by mail and not confirmed yet, by person.
static SENT: LazyLock<Mutex<HashMap<String, HashSet<String>>>> = LazyLock::new(Default::default);
static POSTS: LazyLock<Mutex<HashMap<String, Post>>> = LazyLock::new(Default::default);
/// People reachable by mail right now (a relay is picked, at the Cloud level).
static OPEN: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

/// What may go by mail: not live views.
pub(super) fn relayable(f: &Frame) -> bool {
    matches!(
        f,
        Frame::Hello { .. }
            | Frame::Msgs { .. }
            | Frame::Got { .. }
            | Frame::Change { .. }
            | Frame::Changed { .. }
            | Frame::Ask { .. }
            | Frame::Status { .. }
            | Frame::Answer { .. }
            | Frame::Answered { .. }
            | Frame::Task { .. }
            | Frame::Turn { .. }
            | Frame::End { .. }
            | Frame::Relay { .. }
    )
}

pub(super) fn open_for(id: &str) -> bool {
    OPEN.lock().unwrap().contains(id)
}

pub(super) fn queue(id: &str, f: Frame) {
    OUTBOX.lock().unwrap().entry(id.to_string()).or_default().push(f);
    nudge(id);
}

/// Something is going on with them: check the mailbox often for a while.
pub(super) fn nudge(id: &str) {
    POSTS.lock().unwrap().entry(id.to_string()).or_default().active_until = Some(Instant::now() + ACTIVE_FOR);
}

/// They confirmed messages (by mail or directly).
pub(super) fn confirmed(id: &str, ids: &[String]) {
    if let Some(set) = SENT.lock().unwrap().get_mut(id) {
        for i in ids {
            set.remove(i);
        }
    }
}

/// Connected directly: what was sent by mail and not confirmed goes again that way.
pub(super) fn direct(id: &str) {
    SENT.lock().unwrap().remove(id);
}

/// The last mail exchange with them worked recently.
pub(super) fn via(id: &str) -> bool {
    POSTS.lock().unwrap().get(id).and_then(|p| p.ok).is_some_and(|t| t.elapsed() < FRESH)
}

pub(super) fn error(id: &str) -> Option<String> {
    POSTS.lock().unwrap().get(id).and_then(|p| p.error.clone())
}

fn cloud(state: &AppState) -> bool {
    state.settings().connectivity == Connectivity::Cloud
}

/// This user's relay (Settings → Your devices), if one is set.
pub(super) fn mine(state: &AppState) -> Option<String> {
    super::super::config(&state.db.lock().unwrap()).relay_url.filter(|u| !u.is_empty())
}

/// The relay two people use: whichever is offered; if both are, the one of
/// the app with the smaller id.
pub(super) fn pick(state: &AppState, c: &Cipher, p: &Contact) -> Option<(String, &'static str)> {
    let mine = p.offer_relay.then(|| mine(state)).flatten();
    let theirs = p.their_relay.as_deref().and_then(|s| c.decrypt(s).ok()).filter(|u| !u.is_empty());
    match (mine, theirs) {
        (Some(m), Some(t)) => Some(if device_id(&state.paths.data) < p.id { (m, "mine") } else { (t, "theirs") }),
        (Some(m), None) => Some((m, "mine")),
        (None, Some(t)) => Some((t, "theirs")),
        (None, None) => None,
    }
}

/// What this app offers someone: its relay, if the user lets them use it.
pub(super) fn offer(state: &AppState, p: &Contact) -> Option<String> {
    p.offer_relay.then(|| mine(state)).flatten()
}

/// Their relay (or that they withdrew it).
pub(super) fn offered(state: &AppState, contact: &str, url: Option<String>) {
    let Ok(c) = state.work_cipher() else { return };
    let url = url.map(|u| u.trim().trim_end_matches('/').to_string()).filter(|u| crate::net::private_ok(u));
    let _ = update(state, |b| {
        if let Some(x) = b.contacts.iter_mut().find(|x| x.id == contact) {
            x.their_relay = url.as_deref().map(|u| c.encrypt(u));
        }
    });
    changed(state);
}

/// Checks the mailbox for everyone not connected directly who can be
/// reached by mail.
pub(super) fn tick(state: &Arc<AppState>) {
    let Ok(c) = state.work_cipher() else { return };
    let contacts = book(&state.db.lock().unwrap()).contacts;
    let cloud = cloud(state);
    let mut open = HashSet::new();
    for p in contacts {
        if link::connected(&p.id) || !cloud {
            continue;
        }
        let Some((url, _)) = pick(state, &c, &p) else { continue };
        open.insert(p.id.clone());
        let waiting = OUTBOX.lock().unwrap().get(&p.id).is_some_and(|f| !f.is_empty());
        {
            let mut posts = POSTS.lock().unwrap();
            let post = posts.entry(p.id.clone()).or_default();
            let active = waiting || post.active_until.is_some_and(|t| Instant::now() < t);
            let wait = if post.fails > 0 {
                (Duration::from_secs(30) * (1 << (post.fails - 1).min(3))).min(Duration::from_secs(300))
            } else if active {
                BUSY_EVERY
            } else {
                IDLE_EVERY
            };
            if post.tried.is_some_and(|t| t.elapsed() < wait) {
                continue;
            }
            post.tried = Some(Instant::now());
        }
        let s = state.clone();
        spawn(async move {
            let Some(_busy) = Busy::take(&format!("mail:{}", p.id)) else { return };
            let result = exchange(&s, &p, &url).await;
            let mut posts = POSTS.lock().unwrap();
            let post = posts.entry(p.id.clone()).or_default();
            match result {
                Ok(n) => {
                    post.ok = Some(Instant::now());
                    post.error = None;
                    post.fails = 0;
                    if n > 0 {
                        post.active_until = Some(Instant::now() + ACTIVE_FOR);
                    }
                }
                Err(e) => {
                    post.fails = post.fails.saturating_add(1);
                    post.error = Some(e);
                }
            }
            drop(posts);
            changed(&s);
        });
    }
    *OPEN.lock().unwrap() = open;
}

/// Splits frames into letters that fit (one frame too big for a letter is
/// left out: it waits for a direct connection).
fn letters(frames: Vec<Frame>) -> Vec<Vec<Frame>> {
    let mut out: Vec<Vec<Frame>> = vec![Vec::new()];
    let mut size = 0;
    for f in frames {
        let n = serde_json::to_vec(&f).map(|v| v.len()).unwrap_or(usize::MAX);
        if n > LETTER {
            continue;
        }
        if size + n > LETTER && !out.last().unwrap().is_empty() {
            out.push(Vec::new());
            size = 0;
        }
        size += n;
        out.last_mut().unwrap().push(f);
    }
    out.retain(|l| !l.is_empty());
    out
}

/// One exchange with someone through the relay at `base`: sends what's
/// waiting, then takes (and deletes) what they left. Returns the frames
/// that went either way.
pub(super) async fn exchange(state: &Arc<AppState>, p: &Contact, base: &str) -> Result<usize, String> {
    let base = base.trim_end_matches('/');
    let c = state.work_cipher()?;
    let key = link_key(&c, p)?;
    let me = device_id(&state.paths.data);
    let http = net::external_client(state.settings().connectivity, Purpose::Cloud, false)?;

    // Send: frames waiting, and messages they haven't confirmed (small enough for a letter).
    let mut frames: Vec<Frame> = OUTBOX.lock().unwrap().remove(&p.id).unwrap_or_default();
    let skip = SENT.lock().unwrap().get(&p.id).cloned().unwrap_or_default();
    let items: Vec<Item> = outgoing(state, &c, &p.id, &skip)?
        .into_iter()
        .filter(|i| i.text.len() + i.file.as_ref().map_or(0, |f| f.data.len()) < LETTER - 4_096)
        .collect();
    if !items.is_empty() {
        SENT.lock().unwrap().entry(p.id.clone()).or_default().extend(items.iter().map(|i| i.id.clone()));
        frames.push(Frame::Msgs { items });
    }
    // Changes to messages they have, not confirmed yet (kept apart from messages by their mark).
    let edits: HashSet<String> = skip.iter().filter_map(|s| s.strip_prefix("edit:").map(str::to_string)).collect();
    let changed = changes(state, &c, &p.id, &edits);
    if !changed.is_empty() {
        SENT.lock().unwrap().entry(p.id.clone()).or_default().extend(changed.iter().map(|ch| format!("edit:{}", ch.id)));
        frames.push(Frame::Change { items: changed });
    }
    let mut moved = frames.len();
    let outbox = box_id(&key, &me);
    let out_token = token(&key, &outbox);
    let mut next = p.box_next;
    if !frames.is_empty() {
        // Who's writing, so a new name shows.
        frames.insert(0, Frame::Hello { name: my_name(state), port: 0, parts: true });
        let mut pending = letters(frames).into_iter();
        while let Some(letter) = pending.next() {
            let seq = next + 1;
            let body = seal(&key, &outbox, seq, &serde_json::to_vec(&Letter { frames: letter.clone() }).map_err(err)?);
            let sent = http.put(format!("{base}/v1/box/{outbox}/{seq}")).bearer_auth(&out_token).timeout(Duration::from_secs(60)).body(body).send().await;
            match sent {
                Ok(r) if r.status().is_success() => next = seq,
                other => {
                    // Not sent: back in line for the next try.
                    let mut back: Vec<Frame> = letter.into_iter().filter(|f| !matches!(f, Frame::Hello { .. } | Frame::Msgs { .. } | Frame::Change { .. })).collect();
                    back.extend(pending.flatten().filter(|f| !matches!(f, Frame::Hello { .. } | Frame::Msgs { .. } | Frame::Change { .. })));
                    OUTBOX.lock().unwrap().entry(p.id.clone()).or_default().splice(0..0, back);
                    SENT.lock().unwrap().remove(&p.id);
                    let _ = save_positions(state, &p.id, next, None);
                    return Err(match other {
                        Ok(r) => format!("The relay refused a letter ({}).", r.status()),
                        Err(e) => format!("Couldn't reach the relay: {e}"),
                    });
                }
            }
        }
    }

    // Receive.
    let inbox = box_id(&key, &p.id);
    let in_token = token(&key, &inbox);
    let mut got = p.box_got;
    loop {
        let resp = http
            .get(format!("{base}/v1/box/{inbox}?after={got}"))
            .bearer_auth(&in_token)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| format!("Couldn't reach the relay: {e}"))?;
        // A box no one has written to yet.
        if resp.status().as_u16() == 404 {
            break;
        }
        if !resp.status().is_success() {
            return Err(format!("The relay refused to hand over letters ({}).", resp.status()));
        }
        let batch: Vec<Stored> = resp.json().await.map_err(err)?;
        if batch.is_empty() {
            break;
        }
        for m in batch {
            if m.seq <= got {
                continue;
            }
            let sealed = B64.decode(&m.data).map_err(|_| "A letter was damaged.".to_string())?;
            let letter: Letter = serde_json::from_slice(&open(&key, &inbox, m.seq, &sealed)?).map_err(err)?;
            // The latest settings for what their assistant may do.
            let p = contact(state, &p.id).unwrap_or_else(|| p.clone());
            for f in letter.frames {
                if relayable(&f) {
                    moved += 1;
                    link::dispatch(state, &p, f);
                }
            }
            got = m.seq;
        }
    }
    if got > p.box_got {
        let _ = http.delete(format!("{base}/v1/box/{inbox}?through={got}")).bearer_auth(&in_token).timeout(Duration::from_secs(30)).send().await;
    }
    save_positions(state, &p.id, next, Some(got))?;
    Ok(moved)
}

fn save_positions(state: &AppState, id: &str, next: u64, got: Option<u64>) -> Result<(), String> {
    update(state, |b| {
        if let Some(x) = b.contacts.iter_mut().find(|x| x.id == id) {
            x.box_next = next;
            if let Some(g) = got {
                x.box_got = g;
            }
        }
    })
}

/// Lets someone reach this app through this user's relay (or stops it).
#[tauri::command(async)]
pub fn people_set_relay_offer(state: AppStateRef, id: String, on: bool) -> Result<(), String> {
    features::require(&state, Feature::People)?;
    if on && mine(&state).is_none() {
        return Err("Set your relay first, in Settings → Your devices.".into());
    }
    let p = update(&state, |b| {
        b.contacts.iter_mut().find(|x| x.id == id).map(|x| {
            x.offer_relay = on;
            x.clone()
        })
    })?
    .ok_or("They aren't linked anymore.")?;
    link::send(&p.id, Frame::Relay { url: offer(&state, &p) });
    state.log("people", &if on { format!("Let {} reach you through your relay", p.name) } else { format!("Stopped letting {} use your relay", p.name) });
    changed(&state);
    Ok(())
}

/// Reachable by mail now (the tick works this out; tests say it).
#[cfg(test)]
pub(super) fn open_now(id: &str) {
    OPEN.lock().unwrap().insert(id.to_string());
}
