// SPDX-License-Identifier: AGPL-3.0-only
//! Direct messages between group members, and calls with screen share.
//!
//! Direct messages go through the group's hub (members needn't be linked
//! with each other), sealed with the group key; the hub passes them on
//! without keeping them. Someone who isn't a People contact lands in Message
//! requests until accepted; blocked people and, for teens, strangers are
//! dropped.
//!
//! Calls: the window records the microphone, camera or a shared window in
//! short pieces and hands them here; each piece is sealed with the group key
//! and passed on by the hub, which keeps only each sender's first piece
//! (what a player needs to start) for people who join late.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, Mutex};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::Emitter;

use super::chat::{self, Envelope, Event};
use super::safety::Inbox;
use super::store::{self, Wire};
use super::MemberId;
use crate::{db, AppState, AppStateRef};

const DMS: &str = "social.dms";
/// Direct messages kept on this PC, newest kept.
const KEEP_DMS: usize = 5000;
/// One piece of a call stream at most (about a second of 720p).
const MAX_PIECE: usize = 1 << 20;
pub const MAX_IN_CALL: usize = 25;

// ---------- direct messages ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dm {
    pub group: String,
    pub with: MemberId,
    pub with_name: String,
    pub from_me: bool,
    pub text: String,
    pub at: i64,
    /// Waiting in Message requests.
    pub request: bool,
}

fn dms(state: &AppState) -> Vec<Dm> {
    db::get(&state.db.lock().unwrap(), DMS).unwrap_or_default()
}

fn keep_dm(state: &AppState, c: &crate::crypto::Cipher, mut d: Dm) {
    d.text = c.encrypt(&d.text);
    let conn = state.db.lock().unwrap();
    let mut all: Vec<Dm> = db::get(&conn, DMS).unwrap_or_default();
    all.push(d);
    if all.len() > KEEP_DMS {
        let cut = all.len() - KEEP_DMS;
        all.drain(..cut);
    }
    let _ = db::set(&conn, DMS, &all);
}

fn dm_channel(to: &str) -> String {
    format!("dm:{to}")
}

/// A direct message arriving: at the hub to pass on, or for this user.
pub(super) fn dm_in(state: &Arc<AppState>, from: &str, group: &str, to: &str, env: Envelope) -> Result<(), String> {
    let c = state.work_cipher()?;
    let b = store::book(&state.db.lock().unwrap());
    let r = b.groups.iter().find(|r| r.group.id == group).ok_or("That group isn't on this PC.")?;
    if env.group != group || env.channel != dm_channel(to) {
        return Err("A direct message was addressed wrongly.".into());
    }
    let mine = store::me(state);
    if r.hub == mine && to != mine {
        if env.from != from || !r.group.members.contains_key(from) || !r.group.members.contains_key(to) {
            return Err("Direct messages are between members of the group.".into());
        }
        if !store::send(to, &Wire::Dm { group: group.into(), to: to.into(), env }) {
            return Err("They aren't connected right now.".into());
        }
        return Ok(());
    }
    if to != mine || (from != r.hub && from != env.from) {
        return Ok(());
    }
    let k = store::keys(&c, r);
    let ev = store::envelope_epoch(&env).and_then(|e| k.get(&e)).ok_or("A direct message used a key this PC doesn't have.")?;
    let Event::Post { text, .. } = chat::open(ev, &env)? else { return Ok(()) };
    let contact = crate::sync::people::contact_names(&state.db.lock().unwrap()).iter().any(|(id, _)| *id == env.from);
    let request = match b.personal.route_dm(&env.from, contact) {
        Inbox::Drop => return Ok(()),
        Inbox::Request => true,
        Inbox::Deliver => false,
    };
    let with_name = r.group.members.get(&env.from).map(|m| m.name.clone()).unwrap_or_default();
    keep_dm(state, &c, Dm { group: group.into(), with: env.from.clone(), with_name, from_me: false, text: text.chars().take(chat::MAX_TEXT).collect(), at: env.at, request });
    if let Some(app) = state.app.get() {
        let _ = app.emit("social:dm", json!({ "group": group, "from": env.from, "request": request }));
    }
    Ok(())
}

#[tauri::command]
pub async fn social_dm_send(state: AppStateRef<'_>, group: String, to: String, text: String) -> Result<(), String> {
    store::on(&state)?;
    if text.trim().is_empty() || text.chars().count() > chat::MAX_TEXT {
        return Err("Messages are 1 to 4000 characters.".into());
    }
    let c = state.work_cipher()?;
    let b = store::book(&state.db.lock().unwrap());
    if b.personal.blocked.contains(&to) {
        return Err("You blocked them. Unblock to write to them.".into());
    }
    let r = b.groups.iter().find(|r| r.group.id == group).ok_or("That group isn't on this PC.")?;
    let to_name = r.group.members.get(&to).map(|m| m.name.clone()).ok_or("They aren't in this group.")?;
    let k = store::current_key(&c, r)?;
    let mine = store::me(&state);
    let at = super::now_ms();
    let ev = Event::Post { id: uuid::Uuid::new_v4().simple().to_string(), channel: dm_channel(&to), text: text.clone(), reply_to: None, thread: None, mentions: vec![] };
    let env = chat::seal(&k, &group, &dm_channel(&to), &mine, at, &ev);
    // From the hub it goes straight to them; else through the hub.
    let via = if r.hub == mine { to.clone() } else { r.hub.clone() };
    if !store::send(&via, &Wire::Dm { group: group.clone(), to: to.clone(), env }) {
        return Err("The group's owner isn't connected right now.".into());
    }
    // Writing to someone accepts their requests.
    store::update(&state, |b| {
        b.personal.accepted.insert(to.clone());
        Ok(())
    })?;
    keep_dm(&state, &c, Dm { group, with: to, with_name: to_name, from_me: true, text, at, request: false });
    Ok(())
}

#[tauri::command]
pub async fn social_dms(state: AppStateRef<'_>) -> Result<Vec<Dm>, String> {
    let c = state.work_cipher()?;
    let blocked = store::book(&state.db.lock().unwrap()).personal.blocked;
    Ok(dms(&state).into_iter().filter(|d| !blocked.contains(&d.with)).map(|mut d| {
        d.text = c.decrypt_or(&d.text, "");
        d
    }).collect())
}

/// Accepts (or declines, which blocks) someone's message requests.
#[tauri::command]
pub async fn social_dm_answer(state: AppStateRef<'_>, from: String, accept: bool) -> Result<(), String> {
    store::update(&state, |b| {
        if accept {
            b.personal.accepted.insert(from.clone());
        } else {
            b.personal.block(&from);
        }
        Ok(())
    })?;
    let conn = state.db.lock().unwrap();
    let mut all: Vec<Dm> = db::get(&conn, DMS).unwrap_or_default();
    if accept {
        all.iter_mut().filter(|d| d.with == from).for_each(|d| d.request = false);
    } else {
        all.retain(|d| d.with != from);
    }
    db::set(&conn, DMS, &all)
}

// ---------- calls ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "c", rename_all = "snake_case")]
pub enum CallWire {
    /// Someone joined (or started) the call in a voice channel.
    Join { call: String, channel: String, name: String },
    Leave { call: String },
    /// Who's in the call, from the hub.
    Roster { call: String, channel: String, people: Vec<(MemberId, String)> },
    /// One piece of a stream (`kind`: voice, camera or screen), sealed.
    Piece { call: String, from: MemberId, kind: String, first: bool, env: Envelope },
}

#[derive(Default)]
struct Call {
    group: String,
    channel: String,
    people: BTreeMap<MemberId, String>,
    /// Each sender's first piece per stream, for those who join later (hub).
    firsts: HashMap<(MemberId, String), (Envelope, bool)>,
}

static CALLS: LazyLock<Mutex<HashMap<String, Call>>> = LazyLock::new(Default::default);
/// The call this PC is in: (group, call).
static MINE: LazyLock<Mutex<Option<(String, String)>>> = LazyLock::new(Default::default);

fn calls_on(state: &AppState) -> Result<(), String> {
    store::on(state)?;
    if !store::book(&state.db.lock().unwrap()).switches.calls {
        return Err("Calls are off. Turn them on in the group settings.".into());
    }
    Ok(())
}

fn emit(state: &AppState, event: &str, payload: serde_json::Value) {
    if let Some(app) = state.app.get() {
        let _ = app.emit(event, payload);
    }
}

fn roster(call: &str) -> Option<(String, String, Vec<(MemberId, String)>)> {
    let calls = CALLS.lock().unwrap();
    calls.get(call).map(|c| (c.group.clone(), c.channel.clone(), c.people.iter().map(|(a, b)| (a.clone(), b.clone())).collect()))
}

/// The hub telling everyone in a call who's in it.
fn spread_roster(state: &AppState, call: &str) {
    let Some((group, channel, people)) = roster(call) else { return };
    let mine = store::me(state);
    for (id, _) in &people {
        if *id != mine {
            store::send(id, &Wire::Call { group: group.clone(), call: CallWire::Roster { call: call.into(), channel: channel.clone(), people: people.clone() } });
        }
    }
    emit(state, "social:call", json!({ "call": call, "group": group, "channel": channel, "people": people }));
}

pub(super) fn call_in(state: &Arc<AppState>, from: &str, group: &str, w: CallWire) -> Result<(), String> {
    if calls_on(state).is_err() {
        return Ok(());
    }
    let b = store::book(&state.db.lock().unwrap());
    let r = b.groups.iter().find(|r| r.group.id == group).ok_or("That group isn't on this PC.")?;
    let mine = store::me(state);
    let hub = r.hub == mine;
    match w {
        CallWire::Join { call, channel, name } if hub => {
            let ch = r.group.channel(&channel).ok_or("No such channel.")?;
            if ch.kind != super::groups::ChannelKind::Voice || !r.group.members.contains_key(from) {
                return Err("Calls are in the group's voice channels, for members.".into());
            }
            let firsts: Vec<(Envelope, String, MemberId)> = {
                let mut calls = CALLS.lock().unwrap();
                let c = calls.entry(call.clone()).or_insert_with(|| Call { group: group.into(), channel, ..Default::default() });
                if c.people.len() >= MAX_IN_CALL {
                    return Err("This call is full.".into());
                }
                c.people.insert(from.into(), name.chars().take(100).collect());
                c.firsts.iter().map(|((who, kind), (e, _))| (e.clone(), kind.clone(), who.clone())).collect()
            };
            for (env, kind, who) in firsts {
                store::send(from, &Wire::Call { group: group.into(), call: CallWire::Piece { call: call.clone(), from: who, kind, first: true, env } });
            }
            spread_roster(state, &call);
        }
        CallWire::Leave { call } if hub => {
            let empty = {
                let mut calls = CALLS.lock().unwrap();
                let Some(c) = calls.get_mut(&call) else { return Ok(()) };
                c.people.remove(from);
                c.firsts.retain(|(who, _), _| who != from);
                c.people.is_empty()
            };
            if empty {
                CALLS.lock().unwrap().remove(&call);
            }
            spread_roster(state, &call);
        }
        CallWire::Roster { call, channel, people } if from == r.hub => {
            CALLS.lock().unwrap().insert(call.clone(), Call { group: group.into(), channel: channel.clone(), people: people.iter().cloned().collect(), firsts: HashMap::new() });
            emit(state, "social:call", json!({ "call": call, "group": group, "channel": channel, "people": people }));
        }
        CallWire::Piece { call, from: who, kind, first, env } => {
            if env.blob.len() > MAX_PIECE + 64 || env.group != group {
                return Ok(());
            }
            if hub {
                if who != from {
                    return Ok(());
                }
                let people: Vec<MemberId> = {
                    let mut calls = CALLS.lock().unwrap();
                    let Some(c) = calls.get_mut(&call) else { return Ok(()) };
                    if !c.people.contains_key(from) {
                        return Ok(());
                    }
                    if first {
                        c.firsts.insert((who.clone(), kind.clone()), (env.clone(), true));
                    }
                    c.people.keys().cloned().collect()
                };
                for p in people.iter().filter(|p| **p != who && **p != mine) {
                    store::send(p, &Wire::Call { group: group.into(), call: CallWire::Piece { call: call.clone(), from: who.clone(), kind: kind.clone(), first, env: env.clone() } });
                }
            } else if from != r.hub {
                return Ok(());
            }
            if MINE.lock().unwrap().as_ref().map(|(_, c)| *c == call) != Some(true) || who == mine {
                return Ok(());
            }
            let c = state.work_cipher()?;
            let k = store::keys(&c, r);
            let Some(key) = store::envelope_epoch(&env).and_then(|e| k.get(&e)) else { return Ok(()) };
            let Ok(data) = key.open(&format!("{}|{}|{}|{}", env.group, env.channel, env.from, env.at), &env.blob) else { return Ok(()) };
            emit(state, "social:call_piece", json!({ "call": call, "from": who, "kind": kind, "first": first, "data": B64.encode(data) }));
        }
        _ => {}
    }
    Ok(())
}

fn hub_and_group(state: &AppState, group: &str) -> Result<(MemberId, bool), String> {
    let b = store::book(&state.db.lock().unwrap());
    let r = b.groups.iter().find(|r| r.group.id == group).ok_or("That group isn't on this PC.")?;
    Ok((r.hub.clone(), r.hub == store::me(state)))
}

/// Joins (or starts) the call in a voice channel. Returns the call's id.
#[tauri::command]
pub async fn social_call_join(state: AppStateRef<'_>, group: String, channel: String, name: String) -> Result<String, String> {
    calls_on(&state)?;
    let state: Arc<AppState> = state.inner().clone();
    if let Some((g, c)) = MINE.lock().unwrap().clone() {
        if g != group {
            let _ = leave(&state, &g, &c);
        }
    }
    let call = format!("{group}:{channel}");
    let (hub, is_hub) = hub_and_group(&state, &group)?;
    let w = CallWire::Join { call: call.clone(), channel, name };
    if is_hub {
        call_in(&state, &store::me(&state), &group, w)?;
    } else if !store::send(&hub, &Wire::Call { group: group.clone(), call: w }) {
        return Err("The group's owner isn't connected, so the call can't start.".into());
    }
    *MINE.lock().unwrap() = Some((group, call.clone()));
    Ok(call)
}

fn leave(state: &Arc<AppState>, group: &str, call: &str) -> Result<(), String> {
    *MINE.lock().unwrap() = None;
    let (hub, is_hub) = hub_and_group(state, group)?;
    let w = CallWire::Leave { call: call.into() };
    if is_hub {
        call_in(state, &store::me(state), group, w)
    } else {
        store::send(&hub, &Wire::Call { group: group.into(), call: w });
        Ok(())
    }
}

#[tauri::command]
pub async fn social_call_leave(state: AppStateRef<'_>) -> Result<(), String> {
    let state: Arc<AppState> = state.inner().clone();
    let Some((group, call)) = MINE.lock().unwrap().clone() else { return Ok(()) };
    leave(&state, &group, &call)
}

/// One recorded piece from the window (`data` base64), sealed and sent.
#[tauri::command]
pub async fn social_call_piece(state: AppStateRef<'_>, kind: String, first: bool, data: String) -> Result<(), String> {
    let state: Arc<AppState> = state.inner().clone();
    let Some((group, call)) = MINE.lock().unwrap().clone() else { return Err("Not in a call.".into()) };
    if !matches!(kind.as_str(), "voice" | "camera" | "screen") {
        return Err("Unknown stream.".into());
    }
    let raw = B64.decode(data).map_err(|_| "A call piece was damaged.".to_string())?;
    if raw.len() > MAX_PIECE {
        return Err("A call piece was too big.".into());
    }
    let c = state.work_cipher()?;
    let r = store::book(&state.db.lock().unwrap()).groups.into_iter().find(|r| r.group.id == group).ok_or("That group isn't on this PC.")?;
    let k = store::current_key(&c, &r)?;
    let mine = store::me(&state);
    let at = super::now_ms();
    let channel = format!("call:{call}");
    let env = Envelope { group: group.clone(), channel: channel.clone(), from: mine.clone(), at, blob: k.seal(&format!("{group}|{channel}|{mine}|{at}"), &raw) };
    let w = CallWire::Piece { call, from: mine.clone(), kind, first, env };
    if r.hub == mine {
        call_in(&state, &mine, &group, w)
    } else {
        store::send(&r.hub, &Wire::Call { group, call: w });
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CallView {
    call: String,
    group: String,
    channel: String,
    people: Vec<(MemberId, String)>,
}

#[tauri::command]
pub async fn social_call_view() -> Option<CallView> {
    let (_, call) = MINE.lock().unwrap().clone()?;
    roster(&call).map(|(group, channel, people)| CallView { call, group, channel, people })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_wire_keeps_its_shape() {
        let w = CallWire::Join { call: "g:v".into(), channel: "v".into(), name: "Ana".into() };
        let s = serde_json::to_string(&w).unwrap();
        assert!(matches!(serde_json::from_str::<CallWire>(&s).unwrap(), CallWire::Join { .. }));
        assert_eq!(dm_channel("bob"), "dm:bob");
    }
}
