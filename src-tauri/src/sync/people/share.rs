// SPDX-License-Identifier: AGPL-3.0-only
//! Sharing chats live with someone linked. The owner picks chats and who
//! may see them; while both apps are connected those people can open them
//! read-only and watch replies being written. Nothing is kept on their side,
//! so unsharing (or disconnecting) takes it away.
//!
//! - Incognito chats can't be shared.
//! - The assistant's steps show as their titles ("Searched the web for …"),
//!   never what the tools returned; pictures show as "[picture]".
//! - Each side's chats are written by its own model; the shared chat names it.

use std::sync::LazyLock;

use serde_json::{json, Value};
use tokio::sync::oneshot;

use super::link::{self, Frame};
use super::*;

/// The most messages a shared chat shows (the latest ones).
const MOST: usize = 300;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Share {
    pub chat_id: String,
    /// The contacts it's shared with.
    pub with: Vec<String>,
}

/// A chat this user shares, as listed in People.
#[derive(Debug, Clone, Serialize)]
pub struct Sharing {
    pub chat_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SharedChat {
    pub id: String,
    pub title: String,
    /// The model that writes its replies.
    pub model: String,
    /// A reply is being written.
    pub busy: bool,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SharedMsg {
    pub id: String,
    /// "user", "assistant", "step" (a tool the assistant used) or "note".
    pub role: String,
    pub text: String,
    pub at: i64,
}

/// A reply being written on the owner's side.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Live {
    /// "loading", "start", "delta", "tool", "resume" or "done".
    pub kind: String,
    pub text: String,
}

/// An open shared chat on the watching side.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SharedView {
    pub contact_id: String,
    pub chat: SharedChat,
    pub messages: Vec<SharedMsg>,
    /// The reply being written: what it's doing, and the text so far.
    pub live: Option<LiveView>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LiveView {
    pub status: String,
    pub content: String,
}

type Key = (String, String);

/// Owner side: who is watching each chat (chat id → contacts).
static WATCHERS: LazyLock<Mutex<HashMap<String, HashSet<String>>>> = LazyLock::new(Default::default);
/// Watching side: what each person shares, and the chats open here.
static THEIRS: LazyLock<Mutex<HashMap<String, Vec<SharedChat>>>> = LazyLock::new(Default::default);
static VIEWS: LazyLock<Mutex<HashMap<Key, SharedView>>> = LazyLock::new(Default::default);
/// `people_watch` calls waiting for their first look.
type Opened = oneshot::Sender<Result<(), String>>;
static OPENING: LazyLock<Mutex<HashMap<Key, Opened>>> = LazyLock::new(Default::default);

// ---------- the owner's side ----------

fn shared_with(b: &Book, chat_id: &str, contact: &str) -> bool {
    b.shares.iter().any(|s| s.chat_id == chat_id && s.with.iter().any(|w| w == contact))
}

fn model_label(state: &AppState, id: Option<&str>) -> String {
    match id {
        None => String::new(),
        Some(id) => state.model_spec(id).map(|s| s.name).unwrap_or_else(|| id.rsplit(':').next().unwrap_or(id).to_string()),
    }
}

/// How a chat is listed for the people it's shared with (`busy`: a turn
/// that's starting or ending says so itself).
fn summary(state: &AppState, chat: &db::Chat, busy: Option<bool>) -> SharedChat {
    SharedChat {
        id: chat.id.clone(),
        title: chat.title.clone(),
        model: model_label(state, chat.model_id.as_deref()),
        busy: busy.unwrap_or_else(|| state.generations.lock().unwrap().contains_key(&chat.id)),
        updated_at: chat.updated_at,
    }
}

/// The chats shared with someone.
fn list_for(state: &AppState, c: &Cipher, contact: &str, turn: Option<(&str, bool)>) -> Vec<SharedChat> {
    let chats: Vec<db::Chat> = {
        let conn = state.db.lock().unwrap();
        let b = book(&conn);
        b.shares
            .iter()
            .filter(|s| s.with.iter().any(|w| w == contact))
            .filter_map(|s| db::chat(&conn, c, &s.chat_id))
            .filter(|ch| !ch.incognito)
            .collect()
    };
    let mut list: Vec<SharedChat> = chats.iter().map(|ch| summary(state, ch, turn.filter(|t| t.0 == ch.id).map(|t| t.1))).collect();
    list.sort_by_key(|ch| std::cmp::Reverse(ch.updated_at));
    list
}

/// What someone watching sees of a chat.
fn messages_of(state: &AppState, c: &Cipher, chat_id: &str) -> Vec<SharedMsg> {
    let all = db::messages(&state.db.lock().unwrap(), c, chat_id);
    let skip = all.len().saturating_sub(MOST);
    all.into_iter()
        .skip(skip)
        .filter_map(|m| {
            let (role, text) = match m.role.as_str() {
                "user" => {
                    let pictures = m.meta.as_ref().and_then(|v| v.get("images")).and_then(Value::as_array).map_or(0, Vec::len);
                    let mut text = m.content.clone();
                    for _ in 0..pictures {
                        text.push_str(" [picture]");
                    }
                    ("user", text)
                }
                "assistant" if crate::compact::is_marker(&m) => ("note", "The earlier part of this chat was summarized here.".to_string()),
                "assistant" if !m.content.trim().is_empty() => ("assistant", m.content.clone()),
                "tool" => {
                    let meta = m.meta.as_ref();
                    let title = meta.and_then(|v| v.get("title")).and_then(Value::as_str).map(str::to_string);
                    let tool = meta.and_then(|v| v.get("tool")).and_then(Value::as_str).unwrap_or("a tool");
                    ("step", title.unwrap_or_else(|| format!("Used {tool}")))
                }
                _ => return None,
            };
            Some(SharedMsg { id: m.id, role: role.into(), text: text.chars().take(MAX_TEXT).collect(), at: m.created_at })
        })
        .collect()
}

fn snapshot_of(state: &AppState, c: &Cipher, chat_id: &str) -> Option<Frame> {
    let chat = db::chat(&state.db.lock().unwrap(), c, chat_id).filter(|ch| !ch.incognito)?;
    Some(Frame::Snapshot { chat: summary(state, &chat, None), messages: messages_of(state, c, chat_id) })
}

/// Sends someone the list of chats shared with them (on connecting, and
/// whenever it changes).
fn send_list(state: &AppState, contact: &str, turn: Option<(&str, bool)>) {
    let Ok(c) = state.work_cipher() else { return };
    link::send(contact, Frame::Shared { chats: list_for(state, &c, contact, turn) });
}

pub(super) fn connected(state: &AppState, contact: &str) {
    send_list(state, contact, None);
}

pub(super) fn disconnected(state: &AppState, contact: &str) {
    for set in WATCHERS.lock().unwrap().values_mut() {
        set.remove(contact);
    }
    THEIRS.lock().unwrap().remove(contact);
    VIEWS.lock().unwrap().retain(|k, _| k.0 != contact);
    emit(state, "people:shared", json!({ "contact_id": contact }));
}

/// They opened a chat this user shares with them.
pub(super) fn watch(state: &AppState, contact: &str, chat_id: &str) {
    let ok = shared_with(&book(&state.db.lock().unwrap()), chat_id, contact);
    let frame = ok.then(|| state.work_cipher().ok().and_then(|c| snapshot_of(state, &c, chat_id))).flatten();
    let Some(frame) = frame else {
        link::send(contact, Frame::Gone { chat_id: chat_id.into() });
        return;
    };
    WATCHERS.lock().unwrap().entry(chat_id.to_string()).or_default().insert(contact.to_string());
    link::send(contact, frame);
    // Mid-reply: what's written so far.
    if let Some(r) = crate::live_reply(chat_id) {
        let text = r.get("content").and_then(Value::as_str).unwrap_or_default().to_string();
        let status = r.get("status").and_then(Value::as_str).unwrap_or("working");
        link::send(contact, Frame::Live { chat_id: chat_id.into(), live: Live { kind: "resume".into(), text: format!("{status}\n{text}") } });
    }
}

pub(super) fn unwatch(contact: &str, chat_id: &str) {
    if let Some(set) = WATCHERS.lock().unwrap().get_mut(chat_id) {
        set.remove(contact);
    }
}

fn watchers(chat_id: &str) -> Vec<String> {
    WATCHERS.lock().unwrap().get(chat_id).map(|s| s.iter().cloned().collect()).unwrap_or_default()
}

/// Every event of a turn in this app: anyone watching that chat sees the
/// reply as it's written, and each step as it lands.
pub fn chat_event(state: &AppState, event: &str, payload: &Value) {
    let Some(chat_id) = payload.get("chat_id").and_then(Value::as_str) else { return };
    let who = watchers(chat_id);
    if who.is_empty() {
        return;
    }
    let live = match event {
        "chat:status" if payload["status"] == "loading" => Live { kind: "loading".into(), text: String::new() },
        "chat:start" => Live { kind: "start".into(), text: String::new() },
        // The model's thinking stays here; only the reply is shown.
        "chat:delta" => match payload.get("content").and_then(Value::as_str) {
            Some(t) => Live { kind: "delta".into(), text: t.into() },
            None => return,
        },
        "agent:tool_start" => Live { kind: "tool".into(), text: payload.get("tool").and_then(Value::as_str).unwrap_or_default().into() },
        // A step was saved: they get the chat again, with it.
        "agent:step" => {
            let Ok(c) = state.work_cipher() else { return };
            if let Some(f) = snapshot_of(state, &c, chat_id) {
                for w in who {
                    link::send(&w, f.clone());
                }
            }
            return;
        }
        _ => return,
    };
    for w in who {
        link::send(&w, Frame::Live { chat_id: chat_id.into(), live: live.clone() });
    }
}

/// A turn started or ended in a chat: the people it's shared with see it
/// busy or not, and those watching get the finished chat.
pub fn turn_changed(state: &AppState, chat_id: &str, busy: bool) {
    let with: Vec<String> = {
        let b = book(&state.db.lock().unwrap());
        b.shares.iter().find(|s| s.chat_id == chat_id).map(|s| s.with.clone()).unwrap_or_default()
    };
    for contact in with.iter().filter(|c| link::connected(c)) {
        send_list(state, contact, Some((chat_id, busy)));
    }
    if !busy {
        let who = watchers(chat_id);
        if who.is_empty() {
            return;
        }
        let Ok(c) = state.work_cipher() else { return };
        for w in who {
            if let Some(f) = snapshot_of(state, &c, chat_id) {
                link::send(&w, f);
            }
            link::send(&w, Frame::Live { chat_id: chat_id.into(), live: Live { kind: "done".into(), text: String::new() } });
        }
    }
}

// ---------- the watching side ----------

/// What someone shares with this user (while connected).
pub(super) fn theirs(contact: &str) -> Vec<SharedChat> {
    THEIRS.lock().unwrap().get(contact).cloned().unwrap_or_default()
}

pub(super) fn listed(state: &AppState, contact: &str, chats: Vec<SharedChat>) {
    let ids: HashSet<String> = chats.iter().map(|c| c.id.clone()).collect();
    THEIRS.lock().unwrap().insert(contact.to_string(), chats);
    // Unshared: an open view of it closes.
    VIEWS.lock().unwrap().retain(|k, _| k.0 != contact || ids.contains(&k.1));
    emit(state, "people:shared", json!({ "contact_id": contact }));
}

pub(super) fn snapshot(state: &AppState, contact: &str, chat: SharedChat, messages: Vec<SharedMsg>) {
    let key = (contact.to_string(), chat.id.clone());
    let opening = OPENING.lock().unwrap().remove(&key);
    let view = {
        let mut views = VIEWS.lock().unwrap();
        // Only chats open here are kept (a late one after closing is dropped).
        if opening.is_none() && !views.contains_key(&key) {
            return;
        }
        let v = views.entry(key.clone()).or_insert_with(|| SharedView { contact_id: contact.into(), ..Default::default() });
        v.chat = chat;
        v.messages = messages;
        if !v.chat.busy {
            v.live = None;
        }
        v.clone()
    };
    if let Some(tx) = opening {
        let _ = tx.send(Ok(()));
    }
    emit(state, "people:snapshot", json!({ "contact_id": contact, "chat_id": key.1, "view": view }));
}

pub(super) fn live(state: &AppState, contact: &str, chat_id: &str, live: Live) {
    let key = (contact.to_string(), chat_id.to_string());
    let shown = {
        let mut views = VIEWS.lock().unwrap();
        let Some(v) = views.get_mut(&key) else { return };
        match live.kind.as_str() {
            "done" => v.live = None,
            "resume" => {
                let (status, text) = live.text.split_once('\n').unwrap_or(("working", ""));
                v.live = Some(LiveView { status: status.into(), content: text.into() });
            }
            kind => {
                let l = v.live.get_or_insert_with(LiveView::default);
                match kind {
                    "loading" => l.status = "loading".into(),
                    "start" => {
                        l.status = "thinking".into();
                        l.content.clear();
                    }
                    "delta" => {
                        l.status = "writing".into();
                        l.content.push_str(&live.text);
                    }
                    "tool" => l.status = "working".into(),
                    _ => {}
                }
            }
        }
        v.live.clone()
    };
    emit(state, "people:live", json!({ "contact_id": contact, "chat_id": chat_id, "live": shown }));
}

pub(super) fn gone(state: &AppState, contact: &str, chat_id: &str) {
    let key = (contact.to_string(), chat_id.to_string());
    VIEWS.lock().unwrap().remove(&key);
    if let Some(list) = THEIRS.lock().unwrap().get_mut(contact) {
        list.retain(|c| c.id != chat_id);
    }
    if let Some(tx) = OPENING.lock().unwrap().remove(&key) {
        let _ = tx.send(Err("They don't share that chat anymore.".into()));
    }
    emit(state, "people:shared", json!({ "contact_id": contact }));
}

/// Opens a chat someone shares: the chat as it is now; changes follow as
/// `people:snapshot` and `people:live` events.
pub(super) async fn open(contact: &str, chat_id: &str) -> Result<SharedView, String> {
    let key = (contact.to_string(), chat_id.to_string());
    let (tx, rx) = oneshot::channel();
    OPENING.lock().unwrap().insert(key.clone(), tx);
    if !link::send(contact, Frame::Watch { chat_id: chat_id.into() }) {
        OPENING.lock().unwrap().remove(&key);
        return Err("They aren't connected right now.".into());
    }
    match timeout(Duration::from_secs(20), rx).await {
        Ok(Ok(Ok(()))) => VIEWS.lock().unwrap().get(&key).cloned().ok_or_else(|| "They don't share that chat anymore.".to_string()),
        Ok(Ok(Err(e))) => Err(e),
        _ => {
            OPENING.lock().unwrap().remove(&key);
            Err("They didn't send the chat. Try again.".into())
        }
    }
}

/// Sets who a chat is shared with (an empty list stops sharing it).
fn set_sharing(state: &AppState, c: &Cipher, chat_id: &str, with: Vec<String>) -> Result<(), String> {
    let chat = db::chat(&state.db.lock().unwrap(), c, chat_id).ok_or("That chat no longer exists.")?;
    if chat.incognito && !with.is_empty() {
        return Err("Incognito chats can't be shared.".into());
    }
    let (before, names) = update(state, |b| {
        let before = b.shares.iter().find(|s| s.chat_id == chat_id).map(|s| s.with.clone()).unwrap_or_default();
        let known: HashSet<&str> = b.contacts.iter().map(|p| p.id.as_str()).collect();
        let with: Vec<String> = with.iter().filter(|w| known.contains(w.as_str())).cloned().collect();
        b.shares.retain(|s| s.chat_id != chat_id);
        if !with.is_empty() {
            b.shares.push(Share { chat_id: chat_id.into(), with: with.clone() });
        }
        let names: Vec<String> = b.contacts.iter().filter(|p| with.contains(&p.id)).map(|p| p.name.clone()).collect();
        (before, names)
    })?;
    let after: HashSet<String> = book(&state.db.lock().unwrap()).shares.iter().find(|s| s.chat_id == chat_id).map(|s| s.with.iter().cloned().collect()).unwrap_or_default();
    // Whoever lost it stops watching it.
    for gone in before.iter().filter(|w| !after.contains(*w)) {
        unwatch(gone, chat_id);
        link::send(gone, Frame::Gone { chat_id: chat_id.into() });
    }
    for w in before.iter().chain(after.iter()).collect::<HashSet<_>>() {
        send_list(state, w, None);
    }
    state.log(
        "people",
        &if names.is_empty() { "Stopped sharing a chat".to_string() } else { format!("Shared a chat with {}", names.join(", ")) },
    );
    changed(state);
    Ok(())
}

// ---------- commands ----------

#[derive(Debug, Serialize)]
pub struct SharingChoice {
    id: String,
    name: String,
    shared: bool,
    online: bool,
}

fn choices(state: &AppState, chat_id: &str) -> Vec<SharingChoice> {
    let b = book(&state.db.lock().unwrap());
    b.contacts
        .iter()
        .map(|p| SharingChoice { id: p.id.clone(), name: p.name.clone(), shared: shared_with(&b, chat_id, &p.id), online: link::connected(&p.id) })
        .collect()
}

/// Who a chat could be shared with, and who it is.
#[tauri::command(async)]
pub fn people_chat_sharing(state: AppStateRef, chat_id: String) -> Result<Vec<SharingChoice>, String> {
    features::require(&state, Feature::People)?;
    state.cipher()?;
    Ok(choices(&state, &chat_id))
}

#[tauri::command(async)]
pub fn people_share_chat(state: AppStateRef, chat_id: String, with: Vec<String>) -> Result<Vec<SharingChoice>, String> {
    features::require(&state, Feature::People)?;
    let c = state.cipher()?;
    set_sharing(&state, &c, &chat_id, with)?;
    Ok(choices(&state, &chat_id))
}

/// Opens a chat someone shares, to watch it.
#[tauri::command]
pub async fn people_watch(state: AppStateRef<'_>, contact_id: String, chat_id: String) -> Result<SharedView, String> {
    features::require(&state, Feature::People)?;
    state.cipher()?;
    open(&contact_id, &chat_id).await
}

#[tauri::command(async)]
pub fn people_unwatch(contact_id: String, chat_id: String) {
    VIEWS.lock().unwrap().remove(&(contact_id.clone(), chat_id.clone()));
    link::send(&contact_id, Frame::Unwatch { chat_id });
}

#[cfg(test)]
pub(super) fn view_of(contact: &str, chat_id: &str) -> Option<SharedView> {
    VIEWS.lock().unwrap().get(&(contact.to_string(), chat_id.to_string())).cloned()
}

#[cfg(test)]
pub(super) fn share_now(state: &AppState, chat_id: &str, with: Vec<String>) -> Result<(), String> {
    set_sharing(state, &state.cipher()?, chat_id, with)
}
