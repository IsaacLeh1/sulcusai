// SPDX-License-Identifier: AGPL-3.0-only
//! Sharing chats live with someone linked. The owner picks chats and who
//! may see them; while both apps are connected those people can open them
//! (they're in their sidebar, marked with who shares them), watch replies
//! being written, and take part: what they write goes into the owner's chat,
//! and their own model, on their PC, answers it there too. Only the titles
//! are kept on their side (for the sidebar), never the messages, so
//! unsharing (or disconnecting) takes it away.
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
    /// Someone it's shared with wrote it (their name), or their model did.
    pub by: String,
    /// For an answer from someone else's model: which.
    pub model: String,
    /// On the watching side: this user wrote it, or this user's model did.
    pub mine: bool,
}

/// A chat someone shares with this user, remembered for the sidebar.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Seen {
    pub contact: String,
    pub chat: SharedChat,
}

/// For the sidebar: a chat someone shares with this user.
#[derive(Debug, Clone, Serialize)]
pub struct SharedHere {
    pub contact_id: String,
    pub contact_name: String,
    /// Connected now (it can be opened).
    pub online: bool,
    pub chat: SharedChat,
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
            let meta = m.meta.as_ref();
            let by = meta.and_then(|v| v.get("from_name")).and_then(Value::as_str).unwrap_or_default().to_string();
            let model = meta.and_then(|v| v.get("model")).and_then(Value::as_str).unwrap_or_default().to_string();
            Some(SharedMsg { id: m.id, role: role.into(), text: text.chars().take(MAX_TEXT).collect(), at: m.created_at, by, model, mine: false })
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
    // Remembered for the sidebar (titles only), until they stop sharing.
    let _ = update(state, |b| {
        b.seen.retain(|s| s.contact != contact);
        b.seen.extend(chats.iter().map(|ch| Seen { contact: contact.to_string(), chat: SharedChat { busy: false, ..ch.clone() } }));
    });
    THEIRS.lock().unwrap().insert(contact.to_string(), chats);
    // Unshared: an open view of it closes.
    VIEWS.lock().unwrap().retain(|k, _| k.0 != contact || ids.contains(&k.1));
    emit(state, "people:shared", json!({ "contact_id": contact }));
}

pub(super) fn snapshot(state: &AppState, contact: &str, chat: SharedChat, mut messages: Vec<SharedMsg>) {
    let key = (contact.to_string(), chat.id.clone());
    // What this user (or their model) wrote there carries this app's id.
    let me = format!("{}:", crate::sync::device_id(&state.paths.data));
    for m in &mut messages {
        m.mine = m.id.starts_with(&me);
    }
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
    let _ = update(state, |b| b.seen.retain(|s| !(s.contact == contact && s.chat.id == chat_id)));
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

/// Someone the chat is shared with wrote in it, or (`reply`: its name)
/// their model answered what they wrote. It goes into the chat here, marked
/// as theirs, and everyone watching sees it.
pub(super) fn posted(state: &AppState, contact: &str, chat_id: &str, id: &str, text: &str, reply: Option<&str>) {
    let Ok(c) = state.work_cipher() else { return };
    let (ok, name, with) = {
        let b = book(&state.db.lock().unwrap());
        let name = b.contacts.iter().find(|p| p.id == contact).map(|p| p.name.clone()).unwrap_or_default();
        let with = b.shares.iter().find(|s| s.chat_id == chat_id).map(|s| s.with.clone()).unwrap_or_default();
        (shared_with(&b, chat_id, contact), name, with)
    };
    if !ok {
        link::send(contact, Frame::Gone { chat_id: chat_id.into() });
        return;
    }
    let text: String = text.trim().chars().take(MAX_TEXT).collect();
    if text.is_empty() || id.len() > 64 {
        return;
    }
    let mut meta = json!({ "from_person": contact, "from_name": name });
    if let Some(model) = reply {
        meta["model"] = json!(model.chars().take(80).collect::<String>());
    }
    let msg = db::Message {
        id: format!("{contact}:{id}"),
        chat_id: chat_id.into(),
        role: if reply.is_some() { "assistant" } else { "user" }.into(),
        content: text,
        created_at: db::now_ms(),
        meta: Some(meta),
        ..Default::default()
    };
    // Sent twice (a resend): kept once.
    if db::add_message(&state.db.lock().unwrap(), &c, &msg).is_err() {
        return;
    }
    if let Some(app) = state.app.get() {
        use tauri::Emitter;
        app.emit("chat:changed", json!({ "chat_id": chat_id })).ok();
    }
    if let Some(f) = snapshot_of(state, &c, chat_id) {
        for w in watchers(chat_id) {
            link::send(&w, f.clone());
        }
    }
    for w in with.iter().filter(|w| link::connected(w)) {
        send_list(state, w, None);
    }
}

/// Answers what this user wrote in a chat someone shares: this user's own
/// model, here, from what the chat holds so far. Sends the answer back to
/// go into their chat.
async fn answer_here(state: &Arc<AppState>, contact: &str, view: &SharedView, text: &str) -> Result<(), String> {
    let chat_id = view.chat.id.clone();
    let key = format!("shared:{contact}:{chat_id}");
    let Some((cancel, _guard)) = crate::claim(&state.generations, &key) else {
        return Err("Your model is still answering in this chat.".into());
    };
    let say = |status: &str, delta: Option<&str>| {
        if let Some(app) = state.app.get() {
            use tauri::Emitter;
            app.emit("people:post_live", json!({ "contact_id": contact, "chat_id": chat_id, "status": status, "delta": delta })).ok();
        }
    };
    say("loading", None);
    let (ep, model) = {
        #[cfg(test)]
        let test = super::ask::TEST_ENDPOINT.lock().unwrap().clone().map(|ep| (ep, "Test model".to_string()));
        #[cfg(not(test))]
        let test: Option<(crate::engine::Endpoint, String)> = None;
        match test {
            Some(t) => t,
            None => crate::llm_endpoint(state, None, false, || {}).await.map(|(ep, spec, _)| (ep, spec.name))?,
        }
    };
    let me = my_name(state);
    let owner = contact_name(state, contact);
    // The chat so far, as a transcript (the latest part that fits).
    let lines: Vec<String> = view
        .messages
        .iter()
        .filter_map(|m| match m.role.as_str() {
            "user" if m.by.is_empty() => Some(format!("{owner}: {}", m.text)),
            "user" => Some(format!("{}: {}", m.by, m.text)),
            "assistant" if m.by.is_empty() => Some(format!("{owner}'s assistant: {}", m.text)),
            "assistant" => Some(format!("{}'s assistant: {}", m.by, m.text)),
            "note" => Some("(The earlier part of the chat was summarized.)".into()),
            _ => None,
        })
        .collect();
    let budget = (ep.ctx as usize).saturating_mul(2);
    let mut transcript = String::new();
    for line in lines.iter().rev() {
        if transcript.len() + line.len() > budget {
            break;
        }
        transcript.insert_str(0, &format!("{line}\n\n"));
    }
    let profile = db::profile(&state.db.lock().unwrap(), &state.work_cipher()?);
    let today = chrono::Local::now().format("%A, %B %-d, %Y").to_string();
    let (base, _) = crate::chat::system_prompt(&profile, &today);
    let system = format!(
        "{base}\n\nYou're {me}'s assistant, on {me}'s PC. {owner} shares one of their chats with {me}, and {me} is taking part in it. \
         The chat so far (the newest at the end):\n\n{transcript}Answer {me}'s new message below, in the context of that chat. \
         Your answer goes into the shared chat, so {owner} reads it too."
    );
    let messages = vec![json!({ "role": "system", "content": system }), json!({ "role": "user", "content": text })];
    let finished = crate::chat::stream(&ep, messages, None, &cancel, |d| {
        if let crate::chat::Delta::Content(t) = d {
            say("writing", Some(&t));
        }
    })
    .await;
    say("done", None);
    let finished = finished?;
    let answer = finished.content.trim().to_string();
    if finished.cancelled || answer.is_empty() {
        return Ok(());
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    if !link::send(contact, Frame::PostReply { chat_id: chat_id.clone(), id, text: answer, model }) {
        return Err(format!("{owner} went offline, so your model's answer couldn't go into their chat."));
    }
    Ok(())
}

fn contact_name(state: &AppState, id: &str) -> String {
    contact(state, id).map(|p| p.name).unwrap_or_else(|| "They".into())
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

/// The chats others share with this user, for the sidebar.
#[tauri::command(async)]
pub fn people_shared_chats(state: AppStateRef) -> Vec<SharedHere> {
    if !features::is_on(&state.db.lock().unwrap(), Feature::People) {
        return Vec::new();
    }
    let b = book(&state.db.lock().unwrap());
    let mut list: Vec<SharedHere> = b
        .seen
        .iter()
        .filter_map(|s| {
            let p = b.contacts.iter().find(|p| p.id == s.contact)?;
            let online = link::connected(&p.id);
            let live = theirs(&p.id).into_iter().find(|c| c.id == s.chat.id);
            Some(SharedHere { contact_id: p.id.clone(), contact_name: p.name.clone(), online, chat: live.unwrap_or_else(|| s.chat.clone()) })
        })
        .collect();
    list.sort_by_key(|s| std::cmp::Reverse(s.chat.updated_at));
    list
}

/// This user writes in a chat someone shares: it goes into their chat, and
/// this user's model answers it there (it streams here as
/// `people:post_live`).
#[tauri::command]
pub async fn people_shared_post(state: AppStateRef<'_>, contact_id: String, chat_id: String, text: String) -> Result<(), String> {
    features::require(&state, Feature::People)?;
    state.cipher()?;
    post_here(state.inner(), &contact_id, &chat_id, &text).await
}

pub(super) async fn post_here(state: &Arc<AppState>, contact_id: &str, chat_id: &str, text: &str) -> Result<(), String> {
    let (contact_id, chat_id) = (contact_id.to_string(), chat_id.to_string());
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Type a message first.".into());
    }
    let view = VIEWS.lock().unwrap().get(&(contact_id.clone(), chat_id.clone())).cloned().ok_or("Open the chat first.")?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    if !link::send(&contact_id, Frame::Post { chat_id: chat_id.clone(), id, text: text.clone() }) {
        return Err(format!("{} isn't connected right now, so this can't go into their chat.", contact_name(state, &contact_id)));
    }
    answer_here(state, &contact_id, &view, &text).await
}

/// Stops this user's model answering in a shared chat.
#[tauri::command(async)]
pub fn people_shared_stop(state: AppStateRef, contact_id: String, chat_id: String) {
    if let Some(flag) = state.generations.lock().unwrap().get(&format!("shared:{contact_id}:{chat_id}")) {
        flag.store(true, std::sync::atomic::Ordering::Relaxed);
    }
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
