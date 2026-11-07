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
use super::groups::Role;
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
    let ev = Event::Post { id: uuid::Uuid::new_v4().simple().to_string(), channel: dm_channel(&to), text: text.clone(), reply_to: None, thread: None, mentions: vec![], files: vec![] };
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
    Roster {
        call: String,
        channel: String,
        people: Vec<(MemberId, String)>,
        /// When each person joined (ms), for the time-in-call shown beside them.
        #[serde(default)]
        since: BTreeMap<MemberId, i64>,
        /// Muted or deafened in this call by a moderator.
        #[serde(default)]
        muted: Vec<MemberId>,
        #[serde(default)]
        deafened: Vec<MemberId>,
    },
    /// A moderator mutes, deafens or removes someone from the call (hub checks the role).
    Mod { call: String, target: MemberId, action: ModAction, on: bool },
    /// The hub telling someone they were removed from the call.
    Removed { call: String },
    /// Remote control between two people in the call, passed on by the hub,
    /// which fills in `by` (who sent it) itself.
    Rc { call: String, to: MemberId, #[serde(default)] by: MemberId, msg: RcMsg },
    /// One piece of a stream (`kind`: voice, camera or screen), sealed.
    /// `to` set: a whisper, passed on only to those people. Empty: everyone.
    Piece {
        call: String,
        from: MemberId,
        kind: String,
        first: bool,
        env: Envelope,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        to: Vec<MemberId>,
    },
}

#[derive(Default)]
struct Call {
    group: String,
    channel: String,
    people: BTreeMap<MemberId, String>,
    since: BTreeMap<MemberId, i64>,
    muted: std::collections::BTreeSet<MemberId>,
    deafened: std::collections::BTreeSet<MemberId>,
    /// Each sender's first piece per stream, for those who join later (hub).
    firsts: HashMap<(MemberId, String), (Envelope, bool)>,
}

static CALLS: LazyLock<Mutex<HashMap<String, Call>>> = LazyLock::new(Default::default);
/// The call this PC is in: (group, call).
static MINE: LazyLock<Mutex<Option<(String, String)>>> = LazyLock::new(Default::default);

fn emit(state: &AppState, event: &str, payload: serde_json::Value) {
    if let Some(app) = state.app.get() {
        let _ = app.emit(event, payload);
    }
}

type Roster = (String, String, Vec<(MemberId, String)>, BTreeMap<MemberId, i64>, Vec<MemberId>, Vec<MemberId>);

fn roster(call: &str) -> Option<Roster> {
    let calls = CALLS.lock().unwrap();
    calls.get(call).map(|c| (c.group.clone(), c.channel.clone(), c.people.iter().map(|(a, b)| (a.clone(), b.clone())).collect(), c.since.clone(), c.muted.iter().cloned().collect(), c.deafened.iter().cloned().collect()))
}

/// The hub telling everyone in a call who's in it.
fn spread_roster(state: &AppState, call: &str) {
    let Some((group, channel, people, since, muted, deafened)) = roster(call) else { return };
    let mine = store::me(state);
    for (id, _) in &people {
        if *id != mine {
            store::send(id, &Wire::Call { group: group.clone(), call: CallWire::Roster { call: call.into(), channel: channel.clone(), people: people.clone(), since: since.clone(), muted: muted.clone(), deafened: deafened.clone() } });
        }
    }
    emit(state, "social:call", json!({ "call": call, "group": group, "channel": channel, "people": people, "since": since, "muted": muted, "deafened": deafened }));
}

pub(super) fn call_in(state: &Arc<AppState>, from: &str, group: &str, w: CallWire) -> Result<(), String> {
    // Only Social itself gates what arrives: a PC whose Calls switch was off
    // used to drop the other side's voice without a word, so calls were one-way.
    if store::on(state).is_err() {
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
            if !r.group.can(from, &channel, super::groups::Perm::Connect) {
                return Err("You can't join this voice channel.".into());
            }
            let limit = ch.user_limit as usize;
            let firsts: Vec<(Envelope, String, MemberId)> = {
                let mut calls = CALLS.lock().unwrap();
                let c = calls.entry(call.clone()).or_insert_with(|| Call { group: group.into(), channel, ..Default::default() });
                if c.people.len() >= MAX_IN_CALL || (limit > 0 && c.people.len() >= limit && !c.people.contains_key(from)) {
                    return Err("This call is full.".into());
                }
                c.people.insert(from.into(), name.chars().take(100).collect());
                c.since.entry(from.into()).or_insert_with(super::now_ms);
                c.firsts.iter().map(|((who, kind), (e, _))| (e.clone(), kind.clone(), who.clone())).collect()
            };
            for (env, kind, who) in firsts {
                store::send(from, &Wire::Call { group: group.into(), call: CallWire::Piece { call: call.clone(), from: who, kind, first: true, env, to: Vec::new() } });
            }
            spread_roster(state, &call);
        }
        CallWire::Leave { call } if hub => {
            let empty = {
                let mut calls = CALLS.lock().unwrap();
                let Some(c) = calls.get_mut(&call) else { return Ok(()) };
                c.people.remove(from);
                c.since.remove(from);
                c.firsts.retain(|(who, _), _| who != from);
                c.people.is_empty()
            };
            if empty {
                CALLS.lock().unwrap().remove(&call);
            }
            spread_roster(state, &call);
        }
        CallWire::Roster { call, channel, people, since, muted, deafened } if from == r.hub => {
            CALLS.lock().unwrap().insert(
                call.clone(),
                Call { group: group.into(), channel: channel.clone(), people: people.iter().cloned().collect(), since: since.clone(), muted: muted.iter().cloned().collect(), deafened: deafened.iter().cloned().collect(), firsts: HashMap::new() },
            );
            emit(state, "social:call", json!({ "call": call, "group": group, "channel": channel, "people": people, "since": since, "muted": muted, "deafened": deafened }));
        }
        CallWire::Mod { call, target, action, on } if hub => {
            // Moderators act on members below them; the owner on anyone.
            let allowed = r.group.role(from).is_some_and(|x| x >= Role::Moderator) && r.group.role(&target).is_some_and(|x| x < r.group.role(from).unwrap_or(Role::Member) || from == target.as_str() && action != ModAction::Remove);
            if !allowed {
                return Err("You can't do that to them in this call.".into());
            }
            let removed = {
                let mut calls = CALLS.lock().unwrap();
                let Some(c) = calls.get_mut(&call) else { return Ok(()) };
                match action {
                    ModAction::Mute => {
                        if on {
                            c.muted.insert(target.clone());
                        } else {
                            c.muted.remove(&target);
                        }
                        false
                    }
                    ModAction::Deafen => {
                        if on {
                            c.deafened.insert(target.clone());
                        } else {
                            c.deafened.remove(&target);
                        }
                        false
                    }
                    ModAction::Remove => {
                        c.people.remove(&target);
                        c.since.remove(&target);
                        c.firsts.retain(|(who, _), _| *who != target);
                        true
                    }
                }
            };
            if removed {
                if target == mine {
                    *MINE.lock().unwrap() = None;
                    emit(state, "social:call_removed", json!({ "call": call }));
                } else {
                    store::send(&target, &Wire::Call { group: group.into(), call: CallWire::Removed { call: call.clone() } });
                }
            }
            spread_roster(state, &call);
        }
        CallWire::Removed { call } if from == r.hub => {
            if MINE.lock().unwrap().as_ref().is_some_and(|(_, c)| *c == call) {
                *MINE.lock().unwrap() = None;
                emit(state, "social:call_removed", json!({ "call": call }));
            }
        }
        CallWire::Rc { call, to, by, msg } => {
            // The hub stamps the sender; others take only what the hub passes on.
            let by = if hub { from.to_string() } else if from == r.hub { by } else { return Ok(()) };
            let both_in = CALLS.lock().unwrap().get(&call).is_some_and(|c| c.people.contains_key(&by) && c.people.contains_key(&to));
            if !both_in {
                return Ok(());
            }
            if to == mine {
                rc_in(state, &call, &by, msg);
            } else if hub {
                store::send(&to.clone(), &Wire::Call { group: group.into(), call: CallWire::Rc { call, to, by, msg } });
            }
        }
        CallWire::Piece { call, from: who, kind, first, env, to } => {
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
                    if (kind == "voice" || kind == "whisper") && (!r.group.can(from, &c.channel, super::groups::Perm::Speak) || c.muted.contains(from)) {
                        return Ok(());
                    }
                    // A whisper's start is not kept for later joiners: it was never theirs.
                    if first && to.is_empty() {
                        c.firsts.insert((who.clone(), kind.clone()), (env.clone(), true));
                    }
                    c.people.keys().filter(|p| to.is_empty() || to.contains(p)).cloned().collect()
                };
                let deaf: Vec<MemberId> = CALLS.lock().unwrap().get(&call).map(|c| c.deafened.iter().cloned().collect()).unwrap_or_default();
                for p in people.iter().filter(|p| **p != who && **p != mine && !deaf.contains(p)) {
                    store::send(p, &Wire::Call { group: group.into(), call: CallWire::Piece { call: call.clone(), from: who.clone(), kind: kind.clone(), first, env: env.clone(), to: to.clone() } });
                }
            } else if from != r.hub {
                return Ok(());
            }
            if MINE.lock().unwrap().as_ref().map(|(_, c)| *c == call) != Some(true) || who == mine || !(to.is_empty() || to.contains(&mine)) {
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
    store::on(&state)?;
    // Joining a call is the ask: whatever else a call needs comes on with it.
    if !store::book(&state.db.lock().unwrap()).switches.calls {
        store::update(&state, |b| {
            b.switches.calls = true;
            Ok(())
        })?;
    }
    let state: Arc<AppState> = state.inner().clone();
    let call = format!("{group}:{channel}");
    // Another channel in the same group counts too, or the hub kept this PC in both.
    if let Some((g, c)) = MINE.lock().unwrap().clone() {
        if c != call {
            let _ = leave(&state, &g, &c);
        }
    }
    let (hub, is_hub) = hub_and_group(&state, &group)?;
    let w = CallWire::Join { call: call.clone(), channel, name };
    // Set before the join goes out: the hub answers at once with everyone's
    // first pieces, and pieces for a call this PC isn't marked in are dropped,
    // which left the joiner unable to hear people already talking.
    let before = MINE.lock().unwrap().replace((group.clone(), call.clone()));
    let sent = if is_hub {
        call_in(&state, &store::me(&state), &group, w)
    } else if !store::send(&hub, &Wire::Call { group: group.clone(), call: w }) {
        Err("The group's owner isn't connected, so the call can't start.".into())
    } else {
        Ok(())
    };
    if let Err(e) = sent {
        *MINE.lock().unwrap() = before;
        return Err(e);
    }
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
pub async fn social_call_piece(state: AppStateRef<'_>, kind: String, first: bool, data: String, to: Option<Vec<MemberId>>) -> Result<(), String> {
    let to = to.unwrap_or_default();
    let state: Arc<AppState> = state.inner().clone();
    let Some((group, call)) = MINE.lock().unwrap().clone() else { return Err("Not in a call.".into()) };
    if !stream_kind_ok(&kind) {
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
    let w = CallWire::Piece { call, from: mine.clone(), kind, first, env, to };
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
    since: BTreeMap<MemberId, i64>,
    muted: Vec<MemberId>,
    deafened: Vec<MemberId>,
}

#[tauri::command]
pub async fn social_call_view() -> Option<CallView> {
    let (_, call) = MINE.lock().unwrap().clone()?;
    roster(&call).map(|(group, channel, people, since, muted, deafened)| CallView { call, group, channel, people, since, muted, deafened })
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

/// What remote control in a call says.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "rc", rename_all = "snake_case")]
pub enum RcMsg {
    Ask,
    Answer { ok: bool, why: String },
    Input { input: sulcus_net::sharing::wire::Input },
    End,
}

/// Up to four windows or screens each, besides voice and camera.
/// "whisper" is voice for a few people only; "sound" is a shared screen's audio; "signal" carries small notes between call
/// windows (recording, drawing, voice changed, quality asked for).
fn stream_kind_ok(kind: &str) -> bool {
    // "camera-low", "screen1-low": a lighter copy for viewers who asked for one.
    let kind = match kind.strip_suffix("-low") {
        Some(base) if base == "camera" || base.starts_with("screen") => base,
        _ => kind,
    };
    matches!(kind, "voice" | "camera" | "screen" | "sound" | "signal" | "whisper") || kind.strip_prefix("screen").and_then(|n| n.parse::<u8>().ok()).is_some_and(|n| (1..=3).contains(&n))
}

fn rc_send(state: &Arc<AppState>, to: &str, msg: RcMsg) -> Result<(), String> {
    let Some((group, call)) = MINE.lock().unwrap().clone() else { return Err("Not in a call.".into()) };
    let (hub, is_hub) = hub_and_group(state, &group)?;
    let w = CallWire::Rc { call, to: to.into(), by: String::new(), msg };
    if is_hub {
        call_in(state, &store::me(state), &group, w)
    } else if store::send(&hub, &Wire::Call { group, call: w }) {
        Ok(())
    } else {
        Err("The group's owner isn't connected.".into())
    }
}

fn name_in_call(call: &str, who: &str) -> String {
    CALLS.lock().unwrap().get(call).and_then(|c| c.people.get(who).cloned()).unwrap_or_default()
}

/// Remote control arriving for this PC.
fn rc_in(state: &Arc<AppState>, call: &str, by: &str, msg: RcMsg) {
    match msg {
        RcMsg::Ask => {
            // The name typed to allow it is the one the call shows, not one the asker chose.
            let name = name_in_call(call, by);
            match crate::p2p::sharing::call_request(by, &name) {
                Ok(()) => emit(state, "social:rc_ask", json!({ "by": by, "name": name })),
                Err(why) => {
                    let _ = rc_send(state, by, RcMsg::Answer { ok: false, why });
                }
            }
        }
        RcMsg::Answer { ok, why } => emit(state, "social:rc_answer", json!({ "by": by, "name": name_in_call(call, by), "ok": ok, "why": why })),
        RcMsg::Input { input } => {
            if crate::p2p::sharing::call_may_input(by) {
                crate::p2p::remote_io::apply(input);
            }
        }
        RcMsg::End => {
            if crate::p2p::sharing::call_controller().as_deref() == Some(by) {
                crate::p2p::sharing::call_stop();
            }
            emit(state, "social:rc_end", json!({ "by": by }));
        }
    }
}

/// Asks someone in the call for control of their PC.
#[tauri::command]
pub async fn social_rc_ask(state: AppStateRef<'_>, to: String) -> Result<(), String> {
    rc_send(state.inner(), &to, RcMsg::Ask)
}

/// Allows control: `typed` must be the asker's name, and the shared monitor
/// (`width` x `height` pixels) is where their input lands.
#[tauri::command]
pub async fn social_rc_accept(state: AppStateRef<'_>, typed: String, width: u32, height: u32) -> Result<(), String> {
    let state: Arc<AppState> = state.inner().clone();
    if let Some(app) = state.app.get() {
        let monitors = app.available_monitors().unwrap_or_default();
        let m = monitors.iter().find(|m| m.size().width == width && m.size().height == height).cloned().or_else(|| app.primary_monitor().ok().flatten());
        if let Some(m) = m {
            crate::p2p::remote_io::set_screen(m.position().x, m.position().y, m.size().width, m.size().height);
        }
    }
    let by = crate::p2p::sharing::call_accept(&typed)?;
    rc_send(&state, &by, RcMsg::Answer { ok: true, why: String::new() })
}

/// Declines a request or ends control (either side).
#[tauri::command]
pub async fn social_rc_end(state: AppStateRef<'_>, to: Option<String>, why: Option<String>) -> Result<(), String> {
    let state: Arc<AppState> = state.inner().clone();
    if let Some(by) = crate::p2p::sharing::call_controller() {
        crate::p2p::sharing::call_stop();
        let _ = rc_send(&state, &by, RcMsg::Answer { ok: false, why: why.unwrap_or_else(|| "They stopped remote control.".into()) });
    }
    if let Some(to) = to {
        rc_send(&state, &to, RcMsg::End)?;
    }
    Ok(())
}

/// The controller's mouse and keys, sent to the PC they control.
#[tauri::command]
pub async fn social_rc_input(state: AppStateRef<'_>, to: String, input: sulcus_net::sharing::wire::Input) -> Result<(), String> {
    rc_send(state.inner(), &to, RcMsg::Input { input })
}

#[cfg(test)]
mod rc_tests {
    #[test]
    fn stream_kinds() {
        for k in ["voice", "camera", "screen", "screen1", "screen3"] {
            assert!(super::stream_kind_ok(k), "{k}");
        }
        for k in ["screen4", "screen0", "screenx", "mic", ""] {
            assert!(!super::stream_kind_ok(k), "{k}");
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModAction {
    Mute,
    Deafen,
    Remove,
}

/// A moderator mutes, deafens (or undoes it) or removes someone from the call.
#[tauri::command]
pub async fn social_call_mod(state: AppStateRef<'_>, target: String, action: ModAction, on: bool) -> Result<(), String> {
    let state: Arc<AppState> = state.inner().clone();
    let Some((group, call)) = MINE.lock().unwrap().clone() else { return Err("Not in a call.".into()) };
    let (hub, is_hub) = hub_and_group(&state, &group)?;
    let w = CallWire::Mod { call, target, action, on };
    if is_hub {
        call_in(&state, &store::me(&state), &group, w)
    } else if store::send(&hub, &Wire::Call { group, call: w }) {
        Ok(())
    } else {
        Err("The group's owner isn't connected.".into())
    }
}

/// The push-to-talk key, held anywhere on the PC (a game in front too).
static PTT: LazyLock<Mutex<Option<tauri_plugin_global_shortcut::Shortcut>>> = LazyLock::new(Default::default);

/// Sets the push-to-talk key (an accelerator such as "F8" or "alt+KeyV"), or none.
#[tauri::command]
pub async fn social_call_ptt(app: tauri::AppHandle, key: Option<String>) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let gs = app.global_shortcut();
    if let Some(old) = PTT.lock().unwrap().take() {
        let _ = gs.unregister(old);
    }
    if let Some(k) = key.filter(|k| !k.trim().is_empty()) {
        let s: tauri_plugin_global_shortcut::Shortcut = k.parse().map_err(|_| "That key can't be used for push to talk.".to_string())?;
        gs.register(s).map_err(|_| "Another program already uses that key.".to_string())?;
        *PTT.lock().unwrap() = Some(s);
    }
    Ok(())
}

/// The window hears both press and release; true when the key was push to talk.
pub fn ptt_event(app: &tauri::AppHandle, shortcut: &tauri_plugin_global_shortcut::Shortcut, pressed: bool) -> bool {
    if !PTT.lock().unwrap().as_ref().is_some_and(|p| p.id() == shortcut.id()) {
        return false;
    }
    let _ = app.emit("social:ptt", pressed);
    true
}
