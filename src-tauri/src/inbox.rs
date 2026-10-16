// SPDX-License-Identifier: AGPL-3.0-only
//! Messages typed while a reply is being written. "Add now" (Enter) puts one
//! in the chat's inbox, which the assistant reads at its next step (and a
//! reply that was about to end carries on to answer it). "Queue" (Ctrl+Enter)
//! waits for the reply to finish, then goes out as the next message, even
//! when the person is in another chat. Stopping a reply keeps the queue
//! (and anything it hadn't read yet) until they send it.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::{AppState, AppStateRef};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub images: Vec<String>,
    pub at: i64,
}

/// Inboxes of the chats whose reply is running (present only meanwhile).
static OPEN: LazyLock<Mutex<HashMap<String, Vec<Item>>>> = LazyLock::new(Default::default);
/// What waits for each chat's reply to finish.
static QUEUED: LazyLock<Mutex<HashMap<String, Vec<Item>>>> = LazyLock::new(Default::default);
/// Going out as the chat's next message, until that message is saved.
static SENDING: LazyLock<Mutex<HashMap<String, Vec<Item>>>> = LazyLock::new(Default::default);

fn item(text: &str, images: Vec<String>) -> Item {
    Item { id: uuid::Uuid::new_v4().simple().to_string()[..10].to_string(), text: text.trim().to_string(), images, at: crate::db::now_ms() }
}

/// A reply starts: messages can be added to it.
pub fn open(chat_id: &str) {
    OPEN.lock().unwrap().entry(chat_id.to_string()).or_default();
}

/// What was added since the last look (the assistant reads these now).
pub fn take(chat_id: &str) -> Vec<Item> {
    OPEN.lock().unwrap().get_mut(chat_id).map(std::mem::take).unwrap_or_default()
}

/// Whether something was added that the assistant hasn't read yet.
pub fn waiting(chat_id: &str) -> bool {
    OPEN.lock().unwrap().get(chat_id).is_some_and(|v| !v.is_empty())
}

/// The reply ended: nothing more can be added; returns what it didn't read.
fn close(chat_id: &str) -> Vec<Item> {
    OPEN.lock().unwrap().remove(chat_id).unwrap_or_default()
}

pub fn queue(chat_id: &str) -> Vec<Item> {
    QUEUED.lock().unwrap().get(chat_id).cloned().unwrap_or_default()
}

/// Messages added to a running reply that it hasn't read yet, and those
/// about to go out as the next message.
pub fn unread(chat_id: &str) -> Vec<Item> {
    let mut all = SENDING.lock().unwrap().get(chat_id).cloned().unwrap_or_default();
    all.extend(OPEN.lock().unwrap().get(chat_id).cloned().unwrap_or_default());
    all
}

/// The next message is saved in the chat: it no longer waits.
pub fn sent(app: &AppHandle, chat_id: &str) {
    if SENDING.lock().unwrap().remove(chat_id).is_some() {
        told(app, chat_id);
    }
}

/// Tells the window what's waiting for the model to read, so it shows in the chat.
pub fn told(app: &AppHandle, chat_id: &str) {
    app.emit("chat:inbox", json!({ "chat_id": chat_id, "waiting": unread(chat_id) })).ok();
}

fn changed(app: &AppHandle, chat_id: &str) {
    app.emit("chat:queue", json!({ "chat_id": chat_id, "queue": queue(chat_id) })).ok();
}

/// Closes a chat's inbox when its reply ends, however it ends. Unless the
/// reply finished normally (`finish`), unread messages join the front of the
/// queue, to send when the person chooses.
pub struct Opened {
    chat_id: String,
    app: AppHandle,
    done: bool,
}

impl Opened {
    pub fn new(app: &AppHandle, chat_id: &str) -> Opened {
        open(chat_id);
        Opened { chat_id: chat_id.to_string(), app: app.clone(), done: false }
    }

    /// The reply ended. Finished (not stopped): what it didn't read and the
    /// queue go out next, as one message. Stopped: they wait in the queue.
    pub fn finish(mut self, state: &Arc<AppState>, stopped: bool) {
        self.done = true;
        let unread = close(&self.chat_id);
        let next = {
            let mut q = QUEUED.lock().unwrap();
            let list = q.entry(self.chat_id.clone()).or_default();
            list.splice(0..0, unread);
            if stopped || list.is_empty() { Vec::new() } else { std::mem::take(list) }
        };
        if !next.is_empty() {
            send_later(self.app.clone(), state.clone(), self.chat_id.clone(), next);
        }
        changed(&self.app, &self.chat_id);
        told(&self.app, &self.chat_id);
    }
}

impl Drop for Opened {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let unread = close(&self.chat_id);
        if !unread.is_empty() {
            QUEUED.lock().unwrap().entry(self.chat_id.clone()).or_default().splice(0..0, unread);
            changed(&self.app, &self.chat_id);
            told(&self.app, &self.chat_id);
        }
    }
}

/// Several messages as one: their texts one after another, all the pictures.
fn joined(items: Vec<Item>) -> (String, Vec<String>) {
    let text = items.iter().map(|i| i.text.as_str()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n");
    (text, items.into_iter().flat_map(|i| i.images).collect())
}

/// Sends `items` as the chat's next message once its reply has let go.
fn send_later(app: AppHandle, state: Arc<AppState>, chat_id: String, items: Vec<Item>) {
    SENDING.lock().unwrap().insert(chat_id.clone(), items.clone());
    tauri::async_runtime::spawn(async move {
        for _ in 0..100 {
            if !state.generations.lock().unwrap().contains_key(&chat_id) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let (text, images) = joined(items.clone());
        let result = crate::run_turn(&app, &state, chat_id.clone(), text, images, None).await;
        let unsent = SENDING.lock().unwrap().remove(&chat_id).is_some();
        if unsent {
            told(&app, &chat_id);
        }
        if let Err(e) = result {
            // Couldn't start (say, the app got locked): back in the queue.
            if unsent {
                QUEUED.lock().unwrap().entry(chat_id.clone()).or_default().splice(0..0, items);
            }
            changed(&app, &chat_id);
            app.emit("chat:done", json!({ "chat_id": chat_id, "error": e })).ok();
        }
    });
}

/// Enter while a reply is written: "added" (it reads it at its next step),
/// or "idle" when no reply is running any more (send it normally).
#[tauri::command(async)]
pub fn chat_add(app: AppHandle, state: AppStateRef, chat_id: String, text: String, images: Option<Vec<String>>) -> Result<&'static str, String> {
    state.cipher()?;
    if text.trim().is_empty() && images.as_ref().is_none_or(|i| i.is_empty()) {
        return Err("Type a message first.".into());
    }
    if !add(&chat_id, &text, images.unwrap_or_default()) {
        return Ok("idle");
    }
    told(&app, &chat_id);
    Ok("added")
}

/// What's waiting for the running reply to read, for a window opening the chat.
#[tauri::command]
pub fn chat_inbox(chat_id: String) -> Vec<Item> {
    unread(&chat_id)
}

/// Adds a message to a running reply; false when none is running.
pub(crate) fn add(chat_id: &str, text: &str, images: Vec<String>) -> bool {
    match OPEN.lock().unwrap().get_mut(chat_id) {
        Some(inbox) => {
            inbox.push(item(text, images));
            true
        }
        None => false,
    }
}

/// Ctrl+Enter while a reply is written: it goes out when the reply is done.
#[tauri::command(async)]
pub fn chat_queue_add(app: AppHandle, state: AppStateRef, chat_id: String, text: String, images: Option<Vec<String>>) -> Result<Vec<Item>, String> {
    state.cipher()?;
    if text.trim().is_empty() && images.as_ref().is_none_or(|i| i.is_empty()) {
        return Err("Type a message first.".into());
    }
    QUEUED.lock().unwrap().entry(chat_id.clone()).or_default().push(item(&text, images.unwrap_or_default()));
    changed(&app, &chat_id);
    Ok(queue(&chat_id))
}

#[tauri::command(async)]
pub fn chat_queue(chat_id: String) -> Vec<Item> {
    queue(&chat_id)
}

#[tauri::command(async)]
pub fn chat_queue_remove(app: AppHandle, chat_id: String, id: String) -> Vec<Item> {
    if let Some(list) = QUEUED.lock().unwrap().get_mut(&chat_id) {
        list.retain(|i| i.id != id);
    }
    changed(&app, &chat_id);
    queue(&chat_id)
}

/// Sends what's queued now (after a stopped reply), as one message.
#[tauri::command(async)]
pub fn chat_queue_send(app: AppHandle, state: AppStateRef, chat_id: String) -> Result<(), String> {
    state.cipher()?;
    if state.generations.lock().unwrap().contains_key(&chat_id) {
        return Err("Still answering; it goes out when the reply is done.".into());
    }
    let items = QUEUED.lock().unwrap().remove(&chat_id).unwrap_or_default();
    if !items.is_empty() {
        send_later(app.clone(), state.inner().clone(), chat_id.clone(), items);
        told(&app, &chat_id);
    }
    changed(&app, &chat_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn added_messages_wait_for_the_assistant_while_a_reply_runs() {
        let chat = format!("c-{}", uuid::Uuid::new_v4());
        assert!(OPEN.lock().unwrap().get(&chat).is_none(), "no reply running: nothing can be added");
        open(&chat);
        OPEN.lock().unwrap().get_mut(&chat).unwrap().push(item(" also check the footer ", vec![]));
        assert!(waiting(&chat));
        let got = take(&chat);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, "also check the footer");
        assert!(!waiting(&chat) && take(&chat).is_empty(), "each is read once");
        OPEN.lock().unwrap().get_mut(&chat).unwrap().push(item("one more", vec!["p1".into()]));
        let unread = close(&chat);
        assert_eq!(unread.len(), 1);
        assert!(OPEN.lock().unwrap().get(&chat).is_none(), "closed with the reply");
        let (text, images) = joined(vec![item("first", vec![]), item("second", vec!["p2".into()]), item("", vec!["p3".into()])]);
        assert_eq!(text, "first\n\nsecond");
        assert_eq!(images, vec!["p2".to_string(), "p3".to_string()]);
    }
}
