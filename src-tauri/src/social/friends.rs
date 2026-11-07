// SPDX-License-Identifier: AGPL-3.0-only
//! Friends: a request goes straight to the other person's PC, and both sides
//! keep the list on their own PC. Only people sharing a group can ask, so a
//! stranger can't reach anyone with a request.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::Emitter;

use super::store::{book, me, on, send, Wire};
use super::MemberId;
use crate::{db, AppState, AppStateRef};

const KEY: &str = "social.friends";
/// Requests waiting either way, so one person can't flood another's list.
const MAX_PENDING: usize = 200;
const MAX_NOTE: usize = 256;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Friends {
    /// Friend id to the name they gave.
    pub friends: BTreeMap<MemberId, String>,
    /// Requests to this PC, waiting for an answer.
    pub incoming: BTreeMap<MemberId, String>,
    /// Requests from this PC, waiting on the other side.
    pub outgoing: BTreeMap<MemberId, String>,
    /// Private notes about anyone, never sent.
    pub notes: BTreeMap<MemberId, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "f", rename_all = "snake_case")]
pub enum FriendWire {
    Ask { name: String },
    Accept { name: String },
    /// A request withdrawn or declined, or a friendship ended.
    Remove,
}

fn load(state: &AppState) -> Friends {
    db::get(&state.db.lock().unwrap(), KEY).unwrap_or_default()
}

fn save(state: &AppState, f: &Friends) -> Result<(), String> {
    db::set(&state.db.lock().unwrap(), KEY, f)
}

fn changed(state: &AppState) {
    if let Some(app) = state.app.get() {
        let _ = app.emit("social:friends", json!({}));
    }
}

/// The name a person goes by in a shared group, and whether there is one.
fn shared_name(state: &AppState, who: &str) -> Option<String> {
    let b = book(&state.db.lock().unwrap());
    if b.personal.blocked.contains(who) {
        return None;
    }
    b.groups.iter().find_map(|r| r.group.members.get(who).map(|m| m.name.clone()))
}

fn clean(name: &str) -> String {
    name.trim().chars().filter(|c| !c.is_control()).take(100).collect()
}

pub(super) fn friend_in(state: &Arc<AppState>, from: &str, msg: FriendWire) -> Result<(), String> {
    let Some(group_name) = shared_name(state, from) else { return Ok(()) };
    let mut f = load(state);
    match msg {
        FriendWire::Ask { name } => {
            let name = if clean(&name).is_empty() { group_name } else { clean(&name) };
            if f.friends.contains_key(from) {
                return Ok(());
            }
            // Both asked at once: that's a yes from each side.
            if f.outgoing.remove(from).is_some() {
                f.friends.insert(from.into(), name.clone());
                send(from, &Wire::Friend { msg: FriendWire::Accept { name: my_name(state) } });
            } else if f.incoming.len() < MAX_PENDING {
                f.incoming.insert(from.into(), name);
            }
        }
        FriendWire::Accept { name } => {
            if f.outgoing.remove(from).is_some() {
                f.friends.insert(from.into(), if clean(&name).is_empty() { group_name } else { clean(&name) });
            }
        }
        FriendWire::Remove => {
            f.friends.remove(from);
            f.incoming.remove(from);
            f.outgoing.remove(from);
        }
    }
    save(state, &f)?;
    changed(state);
    Ok(())
}

fn my_name(state: &AppState) -> String {
    db::get::<super::profile::Profile>(&state.db.lock().unwrap(), super::profile::MINE).map(|p| p.name).unwrap_or_default()
}

#[tauri::command]
pub async fn social_friends(state: AppStateRef<'_>) -> Result<Friends, String> {
    Ok(load(&state))
}

/// Sends a friend request to someone in a shared group.
#[tauri::command]
pub async fn social_friend_ask(state: AppStateRef<'_>, who: String, my_name: String) -> Result<(), String> {
    on(&state)?;
    if who == me(&state) {
        return Err("That's you.".into());
    }
    let name = shared_name(&state, &who).ok_or("You can ask people you share a group with.")?;
    let mut f = load(&state);
    if f.friends.contains_key(&who) {
        return Err("You're already friends.".into());
    }
    if f.outgoing.len() >= MAX_PENDING {
        return Err("Too many requests are waiting. Withdraw some first.".into());
    }
    // They asked first: asking back is accepting.
    if let Some(theirs) = f.incoming.remove(&who) {
        f.friends.insert(who.clone(), theirs);
        save(&state, &f)?;
        send(&who, &Wire::Friend { msg: FriendWire::Accept { name: clean(&my_name) } });
        changed(&state);
        return Ok(());
    }
    if !send(&who, &Wire::Friend { msg: FriendWire::Ask { name: clean(&my_name) } }) {
        return Err("They aren't connected right now. Try when their app is open.".into());
    }
    f.outgoing.insert(who, name);
    save(&state, &f)?;
    changed(&state);
    Ok(())
}

#[tauri::command]
pub async fn social_friend_answer(state: AppStateRef<'_>, who: String, accept: bool, my_name: String) -> Result<(), String> {
    let mut f = load(&state);
    let name = f.incoming.remove(&who).ok_or("That request isn't here any more.")?;
    if accept {
        f.friends.insert(who.clone(), name);
    }
    save(&state, &f)?;
    // Declining says only that the request is gone, the same as withdrawing.
    send(&who, &Wire::Friend { msg: if accept { FriendWire::Accept { name: clean(&my_name) } } else { FriendWire::Remove } });
    changed(&state);
    Ok(())
}

/// Ends a friendship or withdraws a request.
#[tauri::command]
pub async fn social_friend_remove(state: AppStateRef<'_>, who: String) -> Result<(), String> {
    let mut f = load(&state);
    let was = f.friends.remove(&who).is_some() | f.outgoing.remove(&who).is_some();
    save(&state, &f)?;
    if was {
        send(&who, &Wire::Friend { msg: FriendWire::Remove });
    }
    changed(&state);
    Ok(())
}

#[tauri::command]
pub async fn social_friend_note(state: AppStateRef<'_>, who: String, note: String) -> Result<(), String> {
    let mut f = load(&state);
    let note: String = note.chars().take(MAX_NOTE).collect();
    if note.trim().is_empty() {
        f.notes.remove(&who);
    } else {
        f.notes.insert(who, note);
    }
    save(&state, &f)?;
    changed(&state);
    Ok(())
}
