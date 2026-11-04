// SPDX-License-Identifier: AGPL-3.0-only
//! The window's commands for chats and their messages: sending, editing,
//! versions, stopping, and the replies running now.

use super::*;

#[tauri::command(async)]
pub(super) fn list_chats(state: AppStateRef) -> Result<Vec<Chat>, String> {
    let c = state.cipher()?;
    Ok(db::list_chats(&state.db.lock().unwrap(), &c).into_iter().map(protected::masked).collect())
}

#[tauri::command(async)]
pub(super) fn create_chat(state: AppStateRef) -> Result<Chat, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let model = db::settings(&conn).default_model;
    db::create_chat(&conn, &c, model)
}

/// Deletes a chat, and tells every window (one may have it open on its own).
#[tauri::command(async)]
pub(super) fn delete_chat(app: tauri::AppHandle, state: AppStateRef, chat_id: String) -> Result<(), String> {
    delete_chat_now(&state, &chat_id)?;
    app.emit("chat:deleted", serde_json::json!({ "chat_id": chat_id })).ok();
    Ok(())
}

/// Deletes a chat: its messages and pasted pictures, its place in a folder,
/// its lock, and (first) its sharing with people.
pub(crate) fn delete_chat_now(state: &AppState, chat_id: &str) -> Result<(), String> {
    let c = state.cipher()?;
    if sync::people::shared_chat_ids(&state.db.lock().unwrap()).contains(chat_id) {
        let _ = sync::people::share::set_chat_sharing(state, &c, chat_id, Vec::new());
    }
    state.contexts.lock().unwrap().remove(chat_id);
    state.approvals.forget_chat(chat_id);
    preview::stop_chat(chat_id);
    let conn = state.db.lock().unwrap();
    db::delete_chat(&conn, chat_id)?;
    media::store::delete_attachments(&conn, &state.paths, chat_id);
    folders::forget_chat(&conn, chat_id);
    cleanup::forget_chat(&conn, chat_id);
    db::log_action(&conn, "chat", "Deleted a chat and its messages");
    Ok(())
}

/// Deletes the incognito chats (but `keep`) and everything they left
/// behind: pasted and made pictures with their descriptions, file backups,
/// what they sent under "What left this PC", folder links, locks, previews.
pub(crate) fn clear_incognito(state: &AppState, keep: Option<&str>) -> Result<(), String> {
    let ids = db::incognito_chat_ids(&state.db.lock().unwrap(), keep);
    for id in &ids {
        state.contexts.lock().unwrap().remove(id);
        state.approvals.forget_chat(id);
        preview::stop_chat(id);
    }
    let conn = state.db.lock().unwrap();
    for id in &ids {
        media::store::delete_chat_items(&conn, &state.paths, id);
        folders::forget_chat(&conn, id);
        cleanup::forget_chat(&conn, id);
        checkpoint::forget_chat(&conn, id);
    }
    if let Ok(c) = state.cipher() {
        outgoing::forget_chats(&conn, &c, &ids);
    }
    db::delete_incognito_chats(&conn, keep)?;
    Ok(())
}

#[tauri::command(async)]
pub(super) fn rename_chat(state: AppStateRef, chat_id: String, title: String) -> Result<(), String> {
    let c = state.cipher()?;
    let title = title.trim();
    if title.is_empty() {
        return Err("A chat needs a name.".into());
    }
    db::set_chat_title(&state.db.lock().unwrap(), &c, &chat_id, title)
}

#[tauri::command(async)]
pub(super) fn set_chat_web(state: AppStateRef, chat_id: String, web: bool) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    db::set_chat_web(&conn, &chat_id, web)?;
    db::log_action(&conn, "privacy", if web { "Turned on web access for one chat" } else { "Turned off web access for one chat" });
    Ok(())
}

#[tauri::command(async)]
pub(super) fn set_chat_model(state: AppStateRef, chat_id: String, model_id: String) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    if !model_known(&conn, &model_id) {
        return Err(if cloud::is_cloud(&model_id) {
            "That cloud model isn't turned on. Check its provider and key in Settings → Cloud models.".into()
        } else {
            "That model isn't installed.".into()
        });
    }
    state.contexts.lock().unwrap().remove(&chat_id);
    db::set_chat_model(&conn, &chat_id, &model_id)?;
    drop(conn);
    // A cloud provider's chats may go to people (Settings, Cloud models).
    if let Ok(c) = state.cipher() {
        sync::people::share::follow_provider(&state, &c, &chat_id);
    }
    Ok(())
}

#[tauri::command(async)]
pub(super) fn get_messages(state: AppStateRef, chat_id: String) -> Result<Vec<Message>, String> {
    let c = state.cipher()?;
    protected::check(&state, &chat_id)?;
    let messages = db::messages(&state.db.lock().unwrap(), &c, &chat_id);
    // Opened just now (the automatic cleanup counts from the last use).
    cleanup::touch_chat(&state, &chat_id);
    Ok(messages)
}

/// How full a chat's context window is: as counted at its last reply (this
/// run, or kept since), else worked out from its saved messages. Chats not
/// used since SulcusAI started all showed 0.
#[tauri::command(async)]
pub(super) fn get_context(state: AppStateRef, chat_id: String) -> Option<ContextInfo> {
    if let Some(c) = state.contexts.lock().unwrap().get(&chat_id).cloned() {
        return Some(c);
    }
    if let Some(c) = db::get::<ContextInfo>(&state.db.lock().unwrap(), &format!("context:{chat_id}")) {
        return Some(c);
    }
    estimate_context(&state, &chat_id)
}

/// A chat's use of its window from the saved messages (about four
/// characters a token) since its last compaction, with the instructions and
/// tools as the last counted reply had them, at the window its model runs
/// with now.
pub(super) fn estimate_context(state: &AppState, chat_id: &str) -> Option<ContextInfo> {
    let cipher = state.cipher().ok()?;
    let (chat, messages, last, profile) = {
        let conn = state.db.lock().unwrap();
        (db::chat(&conn, &cipher, chat_id)?, db::messages(&conn, &cipher, chat_id), db::get::<ContextInfo>(&conn, "context:last"), db::profile(&conn, &cipher))
    };
    if messages.is_empty() {
        return None;
    }
    let id = chat.model_id.or_else(|| state.settings().default_model)?;
    let spec = state.model_spec(&id)?;
    let quant = db::installed_model(&state.db.lock().unwrap(), &id).map(|m| m.quant).unwrap_or_else(|| spec.variants[0].quant.clone());
    let ctx = plan_with(state, &spec, &quant, 0, true).ctx;
    let tokens = |s: &str| (s.chars().count() as u32).div_ceil(4);
    let from = messages.iter().rposition(|m| m.meta.as_ref().is_some_and(|v| v.get("compacted").is_some())).unwrap_or(0);
    let mut parts = chat::HistoryParts::default();
    let mut history = 0;
    for m in &messages[from..] {
        let n = tokens(&m.content) + m.tool_calls.iter().flatten().map(|c| tokens(&c.arguments) + 4).sum::<u32>() + 4;
        parts.add(m, n);
        history += n;
    }
    let (base, about) = chat::system_prompt(&profile, "");
    let (system_tokens, profile_tokens, tools_tokens) = match last {
        Some(l) => (l.system_tokens, l.profile_tokens, l.tools_tokens),
        None => (tokens(&base), tokens(&about), 0),
    };
    Some(ContextInfo { ctx, system_tokens, profile_tokens, tools_tokens, history_tokens: history, reply_reserve: (ctx / 4).clamp(256, 4096), estimated: true, history: parts, ..Default::default() })
}

#[tauri::command(async)]
pub(super) fn stop_generation(state: AppStateRef, chat_id: String) {
    if let Some(flag) = state.generations.lock().unwrap().get(&chat_id) {
        flag.store(true, Ordering::Relaxed);
    }
}

/// Replies in progress, by chat: what has streamed so far, so a chat opened
/// again mid-reply shows it (the reply keeps going when the user leaves).
pub(super) fn live_replies() -> &'static Mutex<HashMap<String, serde_json::Value>> {
    static LIVE: std::sync::OnceLock<Mutex<HashMap<String, serde_json::Value>>> = std::sync::OnceLock::new();
    LIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn track_live(event: &str, payload: &serde_json::Value) {
    let Some(chat) = payload.get("chat_id").and_then(|c| c.as_str()) else { return };
    let mut live = live_replies().lock().unwrap();
    match event {
        "chat:start" => {
            live.insert(chat.to_string(), json!({ "message_id": payload["message_id"], "content": "", "thinking": "", "status": "thinking" }));
        }
        "chat:delta" => {
            if let Some(r) = live.get_mut(chat) {
                for key in ["content", "thinking"] {
                    if let Some(add) = payload.get(key).and_then(|v| v.as_str()) {
                        let joined = format!("{}{add}", r[key].as_str().unwrap_or(""));
                        r[key] = json!(joined);
                    }
                }
                if payload.get("content").is_some_and(|v| v.is_string()) {
                    r["status"] = json!("writing");
                }
            }
        }
        "agent:step" => {
            live.insert(chat.to_string(), json!({ "message_id": null, "content": "", "thinking": "", "status": "working" }));
        }
        "chat:status" if payload["status"] == "loading" => {
            live.insert(chat.to_string(), json!({ "message_id": null, "content": "", "thinking": "", "status": "loading" }));
        }
        _ => {}
    }
}

/// Clears a chat's live reply when its turn ends, however it ends.
pub(super) struct LiveGuard(pub(super) String);
impl Drop for LiveGuard {
    fn drop(&mut self) {
        live_replies().lock().unwrap().remove(&self.0);
    }
}

/// What a chat's reply in progress has written so far (for people watching it).
pub(crate) fn live_reply(chat_id: &str) -> Option<serde_json::Value> {
    live_replies().lock().unwrap().get(chat_id).cloned()
}

/// While a turn runs, the people a chat is shared with see it busy.
pub(super) struct SharedTurn(pub(super) Arc<AppState>, pub(super) String);
impl Drop for SharedTurn {
    fn drop(&mut self) {
        sync::people::turn_changed(&self.0, &self.1, false);
    }
}

/// The reply a chat is in the middle of, if any.
#[tauri::command(async)]
pub(super) fn chat_live(state: AppStateRef, chat_id: String) -> Option<serde_json::Value> {
    if !state.generations.lock().unwrap().contains_key(&chat_id) {
        return None;
    }
    Some(live_replies().lock().unwrap().get(&chat_id).cloned().unwrap_or_else(|| json!({ "message_id": null, "content": "", "thinking": "", "status": "working" })))
}

#[tauri::command]
pub(super) async fn send_message(app: AppHandle, state: AppStateRef<'_>, chat_id: String, text: String, images: Option<Vec<String>>) -> Result<Option<Message>, String> {
    protected::check(&state, &chat_id)?;
    // A new chat on a cloud model whose provider shares its chats with people.
    if let Ok(c) = state.cipher() {
        sync::people::share::follow_provider(&state, &c, &chat_id);
    }
    // The turn is built on the heap: a command starts on the main thread,
    // and in 0.9.26 sending a message overflowed its stack (the app closed).
    Box::pin(run_turn(&app, state.inner(), chat_id, text, images.unwrap_or_default(), None)).await
}

/// Asking again from a message of the person's; what followed it is kept
/// as a version (`versions`).
pub(crate) enum Again {
    /// Another reply to this message.
    Reply(String),
    /// This message, edited: it and what followed are set aside.
    Edit(String),
}

/// The person's own message `id` in a chat.
pub(super) fn own_message(state: &AppState, chat_id: &str, id: &str) -> Result<Message, String> {
    let c = state.cipher()?;
    protected::check(state, chat_id)?;
    db::messages(&state.db.lock().unwrap(), &c, chat_id)
        .into_iter()
        .find(|m| m.id == id && m.role == "user" && m.meta.as_ref().is_none_or(|v| v.get("from_person").is_none() && v.get("from_chat").is_none()))
        .ok_or_else(|| "That message isn't in this chat any more.".to_string())
}

pub(super) fn images_of(m: &Message) -> Vec<String> {
    m.meta.as_ref().and_then(|v| v.get("images")).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

/// Another reply to the person's message `message_id`; the one it had is
/// kept as a version.
#[tauri::command]
pub(super) async fn chat_regenerate(app: AppHandle, state: AppStateRef<'_>, chat_id: String, message_id: String) -> Result<Option<Message>, String> {
    let m = own_message(&state, &chat_id, &message_id)?;
    let images = images_of(&m);
    Box::pin(run_turn_with(&app, state.inner(), chat_id, m.content, images, None, None, Some(Again::Reply(message_id)))).await
}

/// Sends the person's message `message_id` again as `text` (its pictures
/// stay); it and what followed are kept as a version.
#[tauri::command]
pub(super) async fn chat_edit(app: AppHandle, state: AppStateRef<'_>, chat_id: String, message_id: String, text: String) -> Result<Option<Message>, String> {
    let m = own_message(&state, &chat_id, &message_id)?;
    let images = images_of(&m);
    Box::pin(run_turn_with(&app, state.inner(), chat_id, text, images, None, None, Some(Again::Edit(message_id)))).await
}

/// The points in a chat that have other versions.
#[tauri::command(async)]
pub(super) fn chat_versions(state: AppStateRef, chat_id: String) -> Result<Vec<versions::Versions>, String> {
    let c = state.cipher()?;
    protected::check(&state, &chat_id)?;
    Ok(versions::list(&state.db.lock().unwrap(), &c, &chat_id))
}

/// Shows version `to` of what follows `anchor` in a chat.
#[tauri::command(async)]
pub(super) fn chat_switch_version(app: AppHandle, state: AppStateRef, chat_id: String, anchor: String, to: usize) -> Result<(), String> {
    if state.generations.lock().unwrap().contains_key(&chat_id) {
        return Err("Wait for the reply to finish first.".into());
    }
    let c = state.cipher()?;
    protected::check(&state, &chat_id)?;
    versions::switch(&state.db.lock().unwrap(), &c, &chat_id, &anchor, to)?;
    state.contexts.lock().unwrap().remove(&chat_id);
    app.emit("chat:changed", json!({ "chat_id": chat_id })).ok();
    Ok(())
}

/// Also receives every event of a turn (voice mode speaks the reply from it).
pub(crate) type Tee = Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync>;

/// Saves the user's message and runs one agent turn in the chat.
pub(crate) async fn run_turn(
    app: &AppHandle,
    state: &Arc<AppState>,
    chat_id: String,
    text: String,
    images: Vec<String>,
    tee: Option<Tee>,
) -> Result<Option<Message>, String> {
    run_turn_with(app, state, chat_id, text, images, tee, None, None).await
}

/// Another chat's assistant asking this chat something: it answers with the
/// model that chat has loaded (one model at a time), and can't ask on.
pub(crate) struct Asking {
    pub ep: engine::Endpoint,
    pub tools: bool,
    pub vision: bool,
    /// The asking chat: id and title.
    pub from: (String, String),
}

pub(crate) async fn run_asked(app: &AppHandle, state: &Arc<AppState>, chat_id: String, text: String, asking: Asking) -> Result<Option<Message>, String> {
    run_turn_with(app, state, chat_id, text, Vec::new(), None, Some(asking), None).await
}

#[tauri::command(async)]
pub(super) fn manager_get(state: AppStateRef, chat_id: String) -> crate::agent::manager::Settings {
    crate::agent::manager::load(&state.paths.data, &chat_id)
}

#[tauri::command(async)]
pub(super) fn manager_set(state: AppStateRef, chat_id: String, settings: crate::agent::manager::Settings) -> Result<(), String> {
    crate::agent::manager::save(&state.paths.data, &chat_id, &settings)?;
    db::log_action(&state.db.lock().unwrap(), "agent", if settings.on { "Turned on Manage itself for one chat" } else { "Turned off Manage itself for one chat" });
    Ok(())
}
