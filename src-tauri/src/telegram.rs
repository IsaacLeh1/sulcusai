// SPDX-License-Identifier: AGPL-3.0-only
//! Texting the assistant from Telegram. The person makes a bot with
//! @BotFather and pastes its token here; the first account to send the bot
//! the code this PC shows is the one it answers, and everyone else is ignored.
//! Their messages (text, pictures, voice notes) go to a chat here and
//! the answer, with any pictures made, goes back. Approvals come as buttons.
//! Bot chats aren't end-to-end encrypted: Telegram's servers see them, and
//! the setup says so.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::agent::Decision;
use crate::{db, AppState, AppStateRef};

const KEY: &str = "telegram";

/// What the bot says itself. The window's tables translate these like its
/// own texts, and `scripts/telegram_texts.py` copies them into
/// `telegram_texts.json`, so the bot speaks the app's language.
pub mod say {
    pub const LINKED: &str = "Linked. Write to me here and I'll answer from SulcusAI on your PC. /new starts a new chat, /stop stops an answer.";
    pub const HELP: &str = "Write to me like you would in SulcusAI: text, pictures or voice notes. /new starts a new chat, /stop stops an answer.";
    pub const NEW_CHAT: &str = "Your next message starts a new chat.";
    pub const COULD_NOT: &str = "I couldn't answer that.";
    pub const STOPPED: &str = "Stopped before answering.";
    pub const TOO_BIG: &str = "Something I made is too big to send here; it's in the gallery on the PC.";
    pub const MAY_I: &str = "May I do this?";
    pub const ALLOW: &str = "Allow";
    pub const DENY: &str = "Deny";
    pub const ALLOWED: &str = "Allowed, it goes ahead.";
    pub const DENIED: &str = "Denied, it won't be done.";
    pub const ANSWERED: &str = "That was already answered.";
    pub const NOTHING: &str = "Done, with nothing to say.";
    pub const DIDNT_RUN: &str = "This scheduled task didn't run.";
}

static TEXTS: std::sync::LazyLock<std::collections::HashMap<String, std::collections::HashMap<String, String>>> =
    std::sync::LazyLock::new(|| serde_json::from_str(include_str!("telegram_texts.json")).unwrap_or_default());

/// A text of the bot's in the app's language (English when it has none).
fn tr(state: &AppState, english: &str) -> String {
    let lang: String = db::get(&state.db.lock().unwrap(), "ui_language").unwrap_or_default();
    TEXTS.get(&lang).and_then(|t| t.get(english)).cloned().unwrap_or_else(|| english.to_string())
}
const API: &str = "https://api.telegram.org";

/// Where Telegram is reached (a stand-in server in tests).
#[cfg(not(test))]
fn api() -> String {
    API.to_string()
}
#[cfg(test)]
static TEST_API: Mutex<Option<String>> = Mutex::new(None);
#[cfg(test)]
fn api() -> String {
    TEST_API.lock().unwrap().clone().unwrap_or_else(|| API.to_string())
}

/// Runs one turn in a chat here (the window's own turn; a stand-in in tests).
pub type Runner = Arc<dyn Fn(Arc<AppState>, String, String, Vec<String>) -> futures_util::future::BoxFuture<'static, Result<Option<db::Message>, String>> + Send + Sync>;

fn window_runner(app: AppHandle) -> Runner {
    Arc::new(move |state, chat_id, text, images| {
        let app = app.clone();
        Box::pin(async move { crate::run_turn(&app, &state, chat_id, text, images, None).await })
    })
}
/// How long one ask for new messages waits on Telegram's side.
const POLL_SECS: u64 = 25;
/// Telegram takes 4096 characters a message; a little less leaves room.
const MAX_TEXT: usize = 4000;
/// Telegram's limit for files a bot sends.
const MAX_FILE: usize = 50 * 1024 * 1024;

#[derive(Serialize, Deserialize, Clone, Default)]
struct Link {
    token: String,
    /// The bot's @name.
    bot: String,
    /// The Telegram chat of the person it answers, once they sent the code.
    person: Option<i64>,
    person_name: String,
    /// The code that links the bot to a person, until one has sent it.
    code: Option<String>,
    /// The chat here that their messages go to.
    chat_id: Option<String>,
    /// Scheduled tasks' results are sent too.
    tasks: bool,
    /// The next update to ask Telegram for.
    offset: i64,
}

fn load(state: &AppState) -> Option<Link> {
    let c = state.cipher().ok()?;
    let stored: String = db::get(&state.db.lock().unwrap(), KEY)?;
    serde_json::from_str(&c.decrypt(&stored).ok()?).ok()
}

fn save(state: &AppState, link: &Link) -> Result<(), String> {
    let c = state.cipher()?;
    let text = serde_json::to_string(link).map_err(|e| e.to_string())?;
    db::set(&state.db.lock().unwrap(), KEY, &c.encrypt(&text))
}

/// The running loop's stop flag, and the last problem it met (for the window).
static RUNNING: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

pub fn running() -> bool {
    RUNNING.lock().unwrap().as_ref().is_some_and(|stop| !stop.load(Ordering::Relaxed))
}
static PROBLEM: Mutex<Option<String>> = Mutex::new(None);

fn problem(p: Option<String>) {
    *PROBLEM.lock().unwrap() = p;
}

fn client(state: &AppState) -> Result<reqwest::Client, String> {
    crate::net::service_client(state.settings().connectivity).map_err(|_| "Telegram needs web access: switch to Local AI + Web (or Cloud) for it to work.".to_string())
}

async fn call(client: &reqwest::Client, token: &str, method: &str, body: Value) -> Result<Value, String> {
    let r = client
        .post(format!("{}/bot{token}/{method}", api()))
        .json(&body)
        .timeout(Duration::from_secs(POLL_SECS + 20))
        .send()
        .await
        .map_err(|e| format!("Couldn't reach Telegram ({}).", e.without_url()))?;
    let v: Value = r.json().await.map_err(|_| "Telegram's answer couldn't be read.".to_string())?;
    if v["ok"].as_bool() != Some(true) {
        return Err(match v["error_code"].as_i64() {
            Some(401 | 404) => "Telegram doesn't know that bot token. Copy it again from @BotFather.".into(),
            _ => format!("Telegram said: {}", v["description"].as_str().unwrap_or("no reason given")),
        });
    }
    Ok(v["result"].clone())
}

async fn send_text(client: &reqwest::Client, token: &str, to: i64, text: &str) {
    for part in pieces(text, MAX_TEXT) {
        let _ = call(client, token, "sendMessage", json!({ "chat_id": to, "text": part })).await;
    }
}

/// The options the assistant last offered each person, by their place in
/// the buttons (Telegram's button data is short, so it carries the place).
static OFFERED: Mutex<Option<(i64, Vec<String>)>> = Mutex::new(None);

/// A reply's ```choices block (the window shows it as buttons), taken out:
/// the rest of the reply, and the options.
pub(crate) fn split_choices(reply: &str) -> (String, Vec<String>) {
    let Some(start) = reply.find("```choices") else { return (reply.to_string(), Vec::new()) };
    let body = &reply[start + "```choices".len()..];
    let Some(end) = body.find("```") else { return (reply.to_string(), Vec::new()) };
    let options: Vec<String> = body[..end]
        .lines()
        .map(|l| l.trim().trim_start_matches(['-', '*']).trim().to_string())
        .filter(|l| !l.is_empty())
        .take(8)
        .collect();
    let rest = format!("{}{}", &reply[..start], &body[end + 3..]).trim().to_string();
    (rest, options)
}

/// The reply, with its options as buttons under its last piece.
async fn send_reply(client: &reqwest::Client, token: &str, to: i64, reply: &str) {
    let (text, options) = split_choices(reply);
    if options.is_empty() {
        return send_text(client, token, to, reply).await;
    }
    let mut parts = pieces(&text, MAX_TEXT);
    let last = parts.pop().unwrap_or_default();
    for part in parts {
        let _ = call(client, token, "sendMessage", json!({ "chat_id": to, "text": part })).await;
    }
    let rows: Vec<Value> = options.iter().enumerate().map(|(i, o)| json!([{ "text": o, "callback_data": format!("c:{i}") }])).collect();
    let shown = if last.is_empty() { options.join(" / ") } else { last };
    let _ = call(client, token, "sendMessage", json!({ "chat_id": to, "text": shown, "reply_markup": { "inline_keyboard": rows } })).await;
    *OFFERED.lock().unwrap() = Some((to, options));
}

/// Splits at line breaks (else spaces) so no piece is longer than `max`.
fn pieces(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text.trim();
    while rest.chars().count() > max {
        let cut = rest.char_indices().nth(max).map_or(rest.len(), |(i, _)| i);
        let at = rest[..cut].rfind('\n').or_else(|| rest[..cut].rfind(' ')).filter(|i| *i > cut / 2).unwrap_or(cut);
        out.push(rest[..at].trim_end().to_string());
        rest = rest[at..].trim_start();
    }
    if !rest.is_empty() {
        out.push(rest.to_string());
    }
    out
}

async fn download(client: &reqwest::Client, token: &str, file_id: &str) -> Result<Vec<u8>, String> {
    let f = call(client, token, "getFile", json!({ "file_id": file_id })).await?;
    let path = f["file_path"].as_str().ok_or("Telegram gave no file.")?;
    // The token is in the address: errors are given without it.
    let r = client.get(format!("{}/file/bot{token}/{path}", api())).timeout(Duration::from_secs(120)).send().await.map_err(|e| e.without_url().to_string())?;
    let bytes = r.bytes().await.map_err(|e| e.without_url().to_string())?;
    if bytes.len() > MAX_FILE {
        return Err("That file is too big.".into());
    }
    Ok(bytes.to_vec())
}

/// Starts (or starts again) the loop that reads the bot's messages; it
/// stops when the link is removed.
pub fn start(app: AppHandle, state: Arc<AppState>) {
    run(window_runner(app), state);
}

fn run(runner: Runner, state: Arc<AppState>) {
    let stop = Arc::new(AtomicBool::new(false));
    if let Some(old) = RUNNING.lock().unwrap().replace(stop.clone()) {
        old.store(true, Ordering::Relaxed);
    }
    tauri::async_runtime::spawn(async move {
        let mut wait = 2;
        let mut change = crate::quiet::change_watcher();
        while !stop.load(Ordering::Relaxed) {
            crate::quiet::awake().await;
            // Locked: nothing can be read or answered until it's unlocked
            // (unlocking counts as a change).
            if state.cipher().is_err() {
                change.borrow_and_update();
                crate::quiet::until_changed(&mut change, crate::quiet::BACKSTOP).await;
                continue;
            }
            let Some(link) = load(&state) else { break };
            let client = match client(&state) {
                Ok(c) => c,
                Err(e) => {
                    problem(Some(e));
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    continue;
                }
            };
            let got = call(&client, &link.token, "getUpdates", json!({ "offset": link.offset, "timeout": POLL_SECS, "allowed_updates": ["message", "callback_query"] })).await;
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match got {
                Ok(updates) => {
                    problem(None);
                    wait = 2;
                    for u in updates.as_array().into_iter().flatten() {
                        let Some(mut link) = load(&state) else { break };
                        link.offset = link.offset.max(u["update_id"].as_i64().unwrap_or(0) + 1);
                        let _ = save(&state, &link);
                        handle(&runner, &state, &client, link, u).await;
                    }
                }
                Err(e) => {
                    problem(Some(e));
                    tokio::time::sleep(Duration::from_secs(wait)).await;
                    wait = (wait * 2).min(60);
                }
            }
        }
    });
}

pub(crate) fn stop() {
    if let Some(old) = RUNNING.lock().unwrap().take() {
        old.store(true, Ordering::Relaxed);
    }
}

async fn handle(runner: &Runner, state: &Arc<AppState>, client: &reqwest::Client, mut link: Link, u: &Value) {
    let picked;
    let m = if let Some(q) = u.get("callback_query") {
        if q["from"]["id"].as_i64() != link.person {
            return;
        }
        // One of the assistant's options, tapped: sent as the person's answer.
        let chosen = q["data"].as_str().and_then(|d| d.strip_prefix("c:")).and_then(|i| i.parse::<usize>().ok()).and_then(|i| {
            OFFERED.lock().unwrap().as_ref().filter(|(to, _)| Some(*to) == link.person).and_then(|(_, o)| o.get(i).cloned())
        });
        let Some(chosen) = chosen else {
            answer_button(state, client, &link, q).await;
            return;
        };
        let _ = call(client, &link.token, "answerCallbackQuery", json!({ "callback_query_id": q["id"] })).await;
        picked = json!({ "chat": { "id": link.person }, "from": q["from"], "text": chosen });
        &picked
    } else {
        let Some(m) = u.get("message") else { return };
        m
    };
    let from = m["chat"]["id"].as_i64().unwrap_or(0);
    let text = m["text"].as_str().or(m["caption"].as_str()).unwrap_or("").trim().to_string();
    let Some(person) = link.person else {
        // Not linked yet: only the code links it; anything else gets no answer.
        if link.code.as_deref().is_some_and(|c| text == c) {
            link.person = Some(from);
            link.person_name = m["from"]["first_name"].as_str().unwrap_or("").to_string();
            link.code = None;
            let _ = save(state, &link);
            state.log("telegram", "A Telegram account was linked to the bot");
            send_text(client, &link.token, from, &tr(state, say::LINKED)).await;
        }
        return;
    };
    if from != person {
        return;
    }
    match text.as_str() {
        "/start" | "/help" => {
            send_text(client, &link.token, person, &tr(state, say::HELP)).await;
            return;
        }
        "/new" => {
            link.chat_id = None;
            let _ = save(state, &link);
            send_text(client, &link.token, person, &tr(state, say::NEW_CHAT)).await;
            return;
        }
        "/stop" => {
            if let Some(id) = &link.chat_id {
                if let Some(flag) = state.generations.lock().unwrap().get(id) {
                    flag.store(true, Ordering::Relaxed);
                }
            }
            return;
        }
        _ => {}
    }
    let (runner, state, client, m) = (runner.clone(), state.clone(), client.clone(), m.clone());
    tauri::async_runtime::spawn(async move {
        if let Err(e) = answer(&runner, &state, &client, link.clone(), &m, text).await {
            send_text(&client, &link.token, person, &format!("{} {e}", tr(&state, say::COULD_NOT))).await;
        }
    });
}

/// The chat here that Telegram messages go to, made when there's none.
fn chat_for(state: &AppState, link: &mut Link) -> Result<String, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    if let Some(id) = link.chat_id.as_ref().filter(|id| db::chat(&conn, &c, id).is_some()) {
        return Ok(id.clone());
    }
    let model = db::settings(&conn).default_model;
    let chat = db::create_chat(&conn, &c, model)?;
    db::set_chat_title(&conn, &c, &chat.id, "From Telegram")?;
    drop(conn);
    link.chat_id = Some(chat.id.clone());
    save(state, link)?;
    Ok(chat.id)
}

async fn answer(runner: &Runner, state: &Arc<AppState>, client: &reqwest::Client, mut link: Link, m: &Value, mut text: String) -> Result<(), String> {
    let person = link.person.unwrap_or_default();
    let chat_id = chat_for(state, &mut link)?;
    let c = state.cipher()?;
    let mut images = Vec::new();
    // The biggest size Telegram made of a picture is the last.
    if let Some(id) = m["photo"].as_array().and_then(|p| p.last()).and_then(|p| p["file_id"].as_str()) {
        let bytes = download(client, &link.token, id).await?;
        images.push(crate::media::add(state, &c, bytes, "image", "image/jpeg", "paste", "", None, Some(chat_id.clone()), true)?.id);
    }
    if let Some(id) = m["voice"]["file_id"].as_str().or(m["audio"]["file_id"].as_str()) {
        let heard = hear(state, &download(client, &link.token, id).await?).await?;
        text = if text.is_empty() { heard } else { format!("{text}\n\n{heard}") };
    }
    if text.is_empty() && images.is_empty() {
        return Err("only text, pictures and voice notes can be read here".into());
    }
    let started = db::now_ms();
    let done = Arc::new(AtomicBool::new(false));
    tauri::async_runtime::spawn(typing(client.clone(), link.token.clone(), person, done.clone()));
    tauri::async_runtime::spawn(ask_approvals(state.clone(), client.clone(), link.token.clone(), person, chat_id.clone(), done.clone()));
    let result = runner(state.clone(), chat_id.clone(), text, images).await;
    done.store(true, Ordering::Relaxed);
    let reply = match result {
        Ok(Some(msg)) => msg.content,
        Ok(None) => tr(state, say::STOPPED),
        Err(e) => return Err(e),
    };
    // Pictures, clips and sounds made while answering go along.
    let made: Vec<crate::media::MediaItem> = db::messages(&state.db.lock().unwrap(), &c, &chat_id)
        .into_iter()
        .filter(|msg| msg.role == "tool" && msg.created_at >= started)
        .filter_map(|msg| msg.meta.and_then(|v| serde_json::from_value::<Vec<crate::media::MediaItem>>(v["media"].clone()).ok()))
        .flatten()
        .collect();
    for item in made {
        if let Ok(bytes) = crate::media::store::read(&state.paths, &c, &item.id) {
            send_file(client, &link.token, person, &item, bytes, &tr(state, say::TOO_BIG)).await;
        }
    }
    send_reply(client, &link.token, person, &reply).await;
    state.log("telegram", "Answered a message from Telegram");
    Ok(())
}

async fn send_file(client: &reqwest::Client, token: &str, to: i64, item: &crate::media::MediaItem, bytes: Vec<u8>, too_big: &str) {
    if bytes.len() > MAX_FILE {
        send_text(client, token, to, too_big).await;
        return;
    }
    let (method, field) = if item.kind == "image" && bytes.len() < 10 * 1024 * 1024 { ("sendPhoto", "photo") } else { ("sendDocument", "document") };
    let ext = item.mime.split('/').nth(1).unwrap_or("bin").replace("mpeg", "mp3");
    let Ok(part) = reqwest::multipart::Part::bytes(bytes).file_name(format!("{}.{ext}", item.kind)).mime_str(&item.mime) else { return };
    let form = reqwest::multipart::Form::new().text("chat_id", to.to_string()).part(field, part);
    let _ = client.post(format!("{}/bot{token}/{method}", api())).multipart(form).timeout(Duration::from_secs(300)).send().await;
}

/// "typing…" in Telegram while the answer is written (it fades after 5 s).
async fn typing(client: reqwest::Client, token: String, to: i64, done: Arc<AtomicBool>) {
    while !done.load(Ordering::Relaxed) {
        let _ = call(&client, &token, "sendChatAction", json!({ "chat_id": to, "action": "typing" })).await;
        tokio::time::sleep(Duration::from_millis(4500)).await;
    }
}

/// Each card the turn waits on goes to Telegram as a question with buttons.
async fn ask_approvals(state: Arc<AppState>, client: reqwest::Client, token: String, to: i64, chat_id: String, done: Arc<AtomicBool>) {
    let mut asked = HashSet::new();
    while !done.load(Ordering::Relaxed) {
        for p in state.approvals.pending(&chat_id) {
            if !asked.insert(p.call_id.clone()) {
                continue;
            }
            let text = format!("{}\n{}", tr(&state, say::MAY_I), p.preview.title);
            let buttons = json!({ "inline_keyboard": [[
                { "text": tr(&state, say::ALLOW), "callback_data": format!("ok:{}", p.call_id) },
                { "text": tr(&state, say::DENY), "callback_data": format!("no:{}", p.call_id) },
            ]] });
            let _ = call(&client, &token, "sendMessage", json!({ "chat_id": to, "text": text, "reply_markup": buttons })).await;
        }
        tokio::time::sleep(Duration::from_millis(700)).await;
    }
}

async fn answer_button(state: &AppState, client: &reqwest::Client, link: &Link, q: &Value) {
    let data = q["data"].as_str().unwrap_or("");
    let (decision, call_id, said) = match data.split_once(':') {
        Some(("ok", id)) => (Decision::Allow, id, tr(state, say::ALLOWED)),
        Some(("no", id)) => (Decision::Deny, id, tr(state, say::DENIED)),
        _ => return,
    };
    let took = state.approvals.answer(call_id, decision);
    let _ = call(client, &link.token, "answerCallbackQuery", json!({ "callback_query_id": q["id"], "text": if took { said.clone() } else { tr(state, say::ANSWERED) } })).await;
    if let (Some(chat), Some(msg)) = (q["message"]["chat"]["id"].as_i64(), q["message"]["message_id"].as_i64()) {
        let before = q["message"]["text"].as_str().unwrap_or("");
        let after = if took { format!("{before}\n{said}") } else { before.to_string() };
        let _ = call(client, &link.token, "editMessageText", json!({ "chat_id": chat, "message_id": msg, "text": after })).await;
    }
}

/// A voice note as words, heard on this PC.
async fn hear(state: &AppState, bytes: &[u8]) -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("sulcusai-voice-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("note.ogg");
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let read = crate::media::audio_in::decode(&path);
    let _ = std::fs::remove_dir_all(&dir);
    let (samples, rate, channels) = read.map_err(|_| "this PC couldn't play the voice note".to_string())?;
    let mono: Vec<f32> = samples.chunks(channels.max(1) as usize).map(|f| f.iter().sum::<f32>() / f.len() as f32).collect();
    let ep = crate::speech::endpoint(state, crate::speech::Use::Live).await?;
    let t = crate::speech::transcribe(&ep, &crate::audio::resample(&mono, rate, crate::audio::RATE), &crate::speech::Options::default()).await?;
    let words = t.text.trim().to_string();
    if words.is_empty() {
        return Err("no words were heard in the voice note".into());
    }
    Ok(words)
}

/// A scheduled task finished: its answer goes to Telegram when that's on.
pub fn task_done(state: &Arc<AppState>, name: &str, outcome: Result<&str, &str>) {
    let Some(link) = load(state).filter(|l| l.tasks && l.person.is_some()) else { return };
    let Ok(client) = client(state) else { return };
    let Ok(c) = state.cipher() else { return };
    let text = match outcome {
        Ok(chat_id) => {
            let last = db::messages(&state.db.lock().unwrap(), &c, chat_id).into_iter().rev().find(|m| m.role == "assistant" && !m.content.trim().is_empty());
            format!("{name}\n\n{}", last.map(|m| m.content).unwrap_or_else(|| tr(state, say::NOTHING)))
        }
        Err(e) => format!("{name}\n{} {e}", tr(state, say::DIDNT_RUN)),
    };
    let person = link.person.unwrap_or_default();
    tauri::async_runtime::spawn(async move { send_text(&client, &link.token, person, &text).await });
}

#[derive(Serialize)]
pub struct TelegramView {
    connected: bool,
    bot: Option<String>,
    linked_to: Option<String>,
    code: Option<String>,
    tasks: bool,
    problem: Option<String>,
}

fn view(state: &AppState) -> TelegramView {
    let link = load(state);
    TelegramView {
        connected: link.is_some(),
        bot: link.as_ref().map(|l| l.bot.clone()),
        linked_to: link.as_ref().filter(|l| l.person.is_some()).map(|l| l.person_name.clone()),
        code: link.as_ref().and_then(|l| l.code.clone()),
        tasks: link.as_ref().is_some_and(|l| l.tasks),
        problem: link.is_some().then(|| PROBLEM.lock().unwrap().clone()).flatten(),
    }
}

#[tauri::command(async)]
pub fn telegram_view(state: AppStateRef) -> Result<TelegramView, String> {
    state.cipher()?;
    Ok(view(&state))
}

/// Checks the token with Telegram, then waits for the code.
#[tauri::command]
pub async fn telegram_connect(app: AppHandle, state: AppStateRef<'_>, token: String) -> Result<TelegramView, String> {
    let token = token.trim().to_string();
    let well_formed = token.split_once(':').is_some_and(|(id, rest)| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) && rest.len() >= 30 && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    if !well_formed {
        return Err("That doesn't look like a bot token. @BotFather gives one like 123456789:AAH…".into());
    }
    let me = call(&client(&state)?, &token, "getMe", json!({})).await?;
    let code = format!("{:06}", uuid::Uuid::new_v4().as_u128() % 1_000_000);
    let link = Link { token, bot: me["username"].as_str().unwrap_or("").to_string(), code: Some(code), tasks: true, ..Default::default() };
    save(&state, &link)?;
    state.log("telegram", &format!("Connected the Telegram bot @{}", link.bot));
    problem(None);
    start(app, state.inner().clone());
    Ok(view(&state))
}

#[tauri::command(async)]
pub fn telegram_set_tasks(state: AppStateRef, on: bool) -> Result<TelegramView, String> {
    let mut link = load(&state).ok_or("Telegram isn't connected.")?;
    link.tasks = on;
    save(&state, &link)?;
    Ok(view(&state))
}

#[tauri::command(async)]
pub fn telegram_disconnect(state: AppStateRef) -> Result<(), String> {
    state.cipher()?;
    stop();
    state.db.lock().unwrap().execute("DELETE FROM settings WHERE key = ?1", [KEY]).map_err(|e| e.to_string())?;
    problem(None);
    state.log("telegram", "Disconnected the Telegram bot");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for Telegram: hands out queued updates and keeps what the
    /// bot sends.
    #[derive(Default)]
    struct Fake {
        updates: Mutex<Vec<Value>>,
        sent: Mutex<Vec<Value>>,
    }

    async fn fake_telegram() -> (Arc<Fake>, String) {
        use axum::{extract::{Path, State}, routing::post, Json, Router};
        let fake = Arc::new(Fake::default());
        let app = Router::new()
            .route(
                "/{bot}/{method}",
                post(|State(f): State<Arc<Fake>>, Path((_bot, method)): Path<(String, String)>, Json(body): Json<Value>| async move {
                    let result = match method.as_str() {
                        "getMe" => json!({ "username": "test_bot" }),
                        "getUpdates" => {
                            let offset = body["offset"].as_i64().unwrap_or(0);
                            let ready: Vec<Value> = f.updates.lock().unwrap().iter().filter(|u| u["update_id"].as_i64().unwrap_or(0) >= offset).cloned().collect();
                            if ready.is_empty() {
                                tokio::time::sleep(Duration::from_millis(200)).await;
                            }
                            json!(ready)
                        }
                        _ => {
                            f.sent.lock().unwrap().push(json!({ "method": method, "body": body }));
                            json!({ "message_id": 1 })
                        }
                    };
                    Json(json!({ "ok": true, "result": result }))
                }),
            )
            .with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (fake, format!("http://{addr}"))
    }

    fn text(id: i64, from: i64, words: &str) -> Value {
        json!({ "update_id": id, "message": { "chat": { "id": from }, "from": { "first_name": if from == 7 { "Sam" } else { "Stranger" } }, "text": words } })
    }

    async fn sent_to(fake: &Fake, to: i64, n: usize) -> Vec<String> {
        for _ in 0..100 {
            let got: Vec<String> = fake.sent.lock().unwrap().iter().filter(|s| s["method"] == "sendMessage" && s["body"]["chat_id"] == to).map(|s| s["body"]["text"].as_str().unwrap_or("").to_string()).collect();
            if got.len() >= n {
                return got;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("the bot sent {to} fewer than {n} messages: {:?}", fake.sent.lock().unwrap());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_person_links_with_the_code_and_gets_answers_and_buttons() {
        let (fake, base) = fake_telegram().await;
        *TEST_API.lock().unwrap() = Some(base);
        let state = crate::testutil::scratch_state();
        db::update_settings(&state.db.lock().unwrap(), |s| s.connectivity = crate::net::Connectivity::Web).unwrap();
        save(&state, &Link { token: "123:abc".into(), bot: "test_bot".into(), code: Some("424242".into()), tasks: true, ..Default::default() }).unwrap();
        // The turn: answers with what it was asked, or first asks to write a file.
        let runner: Runner = Arc::new(|state: Arc<AppState>, chat_id: String, text: String, _images: Vec<String>| {
            Box::pin(async move {
                if text.contains("write") {
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    state.approvals.add_for_test(
                        crate::agent::PendingApproval {
                            chat_id,
                            call_id: "call-1".into(),
                            tool: "write_file".into(),
                            risk: crate::tools::Risk::Write,
                            preview: crate::tools::Preview { title: "Write notes.txt".into(), kind: "text", detail: None, note: None },
                            once: false,
                            rule: None,
                        },
                        tx,
                    );
                    let d = rx.await.map_err(|e| e.to_string())?;
                    return Ok(Some(db::Message { content: format!("decided {d:?}"), ..Default::default() }));
                }
                Ok(Some(db::Message { content: format!("you said: {text}"), ..Default::default() }))
            })
        });
        // A wrong code, the right one, a stranger (code and hello), the person.
        fake.updates.lock().unwrap().extend([text(1, 7, "000000"), text(2, 7, "424242"), text(3, 9, "424242"), text(4, 9, "hello"), text(5, 7, "hello"), text(6, 7, "please write notes")]);
        run(runner, state.clone());
        // Answers run side by side, so they may come in either order.
        let asked = format!("{}\nWrite notes.txt", say::MAY_I);
        let to_person = sent_to(&fake, 7, 3).await;
        assert!(to_person[0].starts_with("Linked."), "{to_person:?}");
        assert!(to_person.iter().any(|t| t == "you said: hello"), "{to_person:?}");
        assert!(to_person.iter().any(|t| *t == asked), "{to_person:?}");
        assert_eq!(load(&state).unwrap().person, Some(7));
        // The person taps Allow.
        fake.updates.lock().unwrap().push(json!({ "update_id": 7, "callback_query": { "id": "q1", "from": { "id": 7 }, "data": "ok:call-1", "message": { "chat": { "id": 7 }, "message_id": 5, "text": asked } } }));
        let to_person = sent_to(&fake, 7, 4).await;
        assert_eq!(to_person[3], "decided Allow");
        assert!(fake.sent.lock().unwrap().iter().any(|s| s["method"] == "editMessageText" && s["body"]["text"].as_str().is_some_and(|t| t.ends_with(say::ALLOWED))));
        assert!(!fake.sent.lock().unwrap().iter().any(|s| s["body"]["chat_id"] == 9), "a stranger gets nothing");
        stop();
        *TEST_API.lock().unwrap() = None;
    }

    #[test]
    fn offered_choices_become_buttons() {
        let (text, options) = split_choices("Which one?\n\n```choices\n- Tea\n- Coffee\n\nWater\n```\n");
        assert_eq!(text, "Which one?");
        assert_eq!(options, ["Tea", "Coffee", "Water"]);
        let (text, options) = split_choices("No options here.");
        assert_eq!((text.as_str(), options.len()), ("No options here.", 0));
        let (text, options) = split_choices("Cut off ```choices\nA");
        assert_eq!((text.as_str(), options.len()), ("Cut off ```choices\nA", 0));
    }

    /// Fails when a `say` text was added without running
    /// scripts/telegram_texts.py.
    #[test]
    fn the_bot_speaks_every_language_the_window_does() {
        let all = [
            say::LINKED, say::HELP, say::NEW_CHAT, say::COULD_NOT, say::STOPPED, say::TOO_BIG, say::MAY_I,
            say::ALLOW, say::DENY, say::ALLOWED, say::DENIED, say::ANSWERED, say::NOTHING, say::DIDNT_RUN,
        ];
        assert!(TEXTS.len() >= 20, "{} languages", TEXTS.len());
        for (lang, table) in TEXTS.iter() {
            for text in all {
                assert!(table.contains_key(text), "{lang} lacks {text:?}");
            }
        }
        assert_eq!(TEXTS["de"][say::NEW_CHAT], "Deine nächste Nachricht beginnt einen neuen Chat.");
    }

    #[test]
    fn long_answers_are_split_at_line_breaks() {
        let text = format!("{}\n{}", "a".repeat(30), "b".repeat(30));
        assert_eq!(pieces(&text, 40), vec!["a".repeat(30), "b".repeat(30)]);
        assert_eq!(pieces("short", 40), vec!["short"]);
        let words = "word ".repeat(20);
        assert!(pieces(&words, 23).iter().all(|p| p.chars().count() <= 23 && !p.starts_with(' ')));
        assert!(pieces("", 10).is_empty());
    }
}
