// SPDX-License-Identifier: AGPL-3.0-only
//! Texting the assistant from Discord. The person makes an application with
//! a bot in Discord's developer portal, pastes the bot's token here, adds
//! the bot to a server of their own (Discord only lets people message bots
//! they share a server with) and sends it the code this PC shows in a direct
//! message. From then on only that account's direct messages are answered;
//! the bot ignores servers. Messages arrive over Discord's gateway (one
//! websocket), answers go back over its REST API, and approvals are typed
//! like in Matrix. Discord's servers see these messages.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::AppHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use crate::agent::Decision;
use crate::telegram::{hear, pieces, say, split_choices, tr, Runner};
use crate::{db, AppState, AppStateRef};

const KEY: &str = "discord";
const API: &str = "https://discord.com/api/v10";
const GATEWAY_HOST: &str = "gateway.discord.gg";
/// Direct messages only; their text needs no privileged intent.
const INTENTS: u64 = 1 << 12;
const MAX_TEXT: usize = 1900;
/// Discord's upload limit for bots without boosts.
const MAX_FILE: usize = 10 * 1024 * 1024;
/// Discord marks voice messages with this attachment flag.
const VOICE_FLAG: u64 = 1 << 13;

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

#[derive(Serialize, Deserialize, Clone, Default)]
struct Link {
    token: String,
    bot: String,
    bot_id: String,
    person: Option<String>,
    person_name: String,
    /// The direct-message channel with the person.
    channel: Option<String>,
    code: Option<String>,
    chat_id: Option<String>,
    tasks: bool,
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

static RUNNING: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);
static PROBLEM: Mutex<Option<String>> = Mutex::new(None);
static OFFERED: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn running() -> bool {
    RUNNING.lock().unwrap().as_ref().is_some_and(|stop| !stop.load(Ordering::Relaxed))
}

fn problem(p: Option<String>) {
    *PROBLEM.lock().unwrap() = p;
}

fn client(state: &AppState) -> Result<reqwest::Client, String> {
    crate::net::service_client(state.settings().connectivity).map_err(|_| "Discord needs web access: switch to Local AI + Web (or Cloud) for it to work.".to_string())
}

async fn call(client: &reqwest::Client, token: &str, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, String> {
    let mut req = client.request(method, format!("{}{path}", api())).header("authorization", format!("Bot {token}")).timeout(Duration::from_secs(30));
    if let Some(b) = body {
        req = req.json(&b);
    }
    let r = req.send().await.map_err(|e| format!("Couldn't reach Discord ({}).", e.without_url()))?;
    let status = r.status();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(v);
    }
    Err(match status.as_u16() {
        401 => "Discord doesn't accept that bot token. Copy it again from the developer portal (Bot › Reset Token).".into(),
        _ => format!("Discord said: {}", v["message"].as_str().unwrap_or(status.as_str())),
    })
}

async fn send_text(client: &reqwest::Client, token: &str, channel: &str, text: &str) {
    for part in pieces(text, MAX_TEXT) {
        let _ = call(client, token, reqwest::Method::POST, &format!("/channels/{channel}/messages"), Some(json!({ "content": part, "allowed_mentions": { "parse": [] } }))).await;
    }
}

async fn send_reply(client: &reqwest::Client, token: &str, channel: &str, reply: &str) {
    let (text, options) = split_choices(reply);
    if options.is_empty() {
        return send_text(client, token, channel, reply).await;
    }
    let listed: Vec<String> = options.iter().enumerate().map(|(i, o)| format!("{}. {o}", i + 1)).collect();
    send_text(client, token, channel, format!("{text}\n\n{}", listed.join("\n")).trim()).await;
    *OFFERED.lock().unwrap() = options;
}

fn picked(text: &str) -> Option<String> {
    let n: usize = text.trim().trim_end_matches('.').parse().ok()?;
    OFFERED.lock().unwrap().get(n.checked_sub(1)?).cloned()
}

/// Attachments come from Discord's own file servers only.
fn from_discord_cdn(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| u.scheme() == "https" && matches!(u.host_str(), Some("cdn.discordapp.com" | "media.discordapp.net")))
}

async fn download(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    if !from_discord_cdn(url) {
        return Err("That file isn't on Discord's servers.".into());
    }
    let r = client.get(url).timeout(Duration::from_secs(120)).send().await.map_err(|e| e.without_url().to_string())?;
    let bytes = r.bytes().await.map_err(|e| e.without_url().to_string())?;
    if bytes.len() > 50 * 1024 * 1024 {
        return Err("That file is too big.".into());
    }
    Ok(bytes.to_vec())
}

pub fn start(app: AppHandle, state: Arc<AppState>) {
    run(
        Arc::new(move |state, chat_id, text, images| {
            let app = app.clone();
            Box::pin(async move { crate::run_turn(&app, &state, chat_id, text, images, None).await })
        }),
        state,
    );
}

fn run(runner: Runner, state: Arc<AppState>) {
    if load(&state).is_none() && state.cipher().is_ok() {
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    if let Some(old) = RUNNING.lock().unwrap().replace(stop.clone()) {
        old.store(true, Ordering::Relaxed);
    }
    tauri::async_runtime::spawn(async move {
        let mut wait = 2;
        let mut change = crate::quiet::change_watcher();
        while !stop.load(Ordering::Relaxed) {
            crate::quiet::awake().await;
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
            match gateway(&runner, &state, &client, &link.token, &stop).await {
                Ok(()) => wait = 2,
                Err(e) => {
                    problem(Some(e));
                    tokio::time::sleep(Duration::from_secs(wait)).await;
                    wait = (wait * 2).min(300);
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

fn identify(token: &str) -> Value {
    json!({ "op": 2, "d": { "token": token, "intents": INTENTS, "properties": { "os": std::env::consts::OS, "browser": "SulcusAI", "device": "SulcusAI" } } })
}

fn heartbeat(seq: Option<u64>) -> Value {
    json!({ "op": 1, "d": seq })
}

/// One gateway session, until Discord or the network ends it. Each new
/// session starts afresh (no resume), so messages sent while it was down
/// aren't answered; Discord keeps them in the chat.
async fn gateway(runner: &Runner, state: &Arc<AppState>, client: &reqwest::Client, token: &str, stop: &AtomicBool) -> Result<(), String> {
    let tcp = crate::net::connect(GATEWAY_HOST, 443, crate::net::Route::Mail).await?;
    let name = tokio_rustls::rustls::pki_types::ServerName::try_from(GATEWAY_HOST.to_string()).map_err(|e| e.to_string())?;
    let tls = tokio_rustls::TlsConnector::from(crate::mail::tls_config()).connect(name, tcp).await.map_err(|e| format!("Secure connection to Discord failed: {e}"))?;
    let mut request = format!("wss://{GATEWAY_HOST}/?v=10&encoding=json").into_client_request().map_err(|e| e.to_string())?;
    request.headers_mut().insert("user-agent", concat!("SulcusAI/", env!("CARGO_PKG_VERSION")).parse().map_err(|_| "bad agent")?);
    let (mut ws, _) = tokio::time::timeout(Duration::from_secs(20), tokio_tungstenite::client_async(request, tls))
        .await
        .map_err(|_| "Discord's gateway didn't answer.".to_string())?
        .map_err(|e| format!("Couldn't open Discord's gateway: {e}"))?;
    let mut seq: Option<u64> = None;
    let mut beat = tokio::time::interval(Duration::from_secs(41));
    let mut acked = true;
    let mut ready = false;
    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = ws.close(None).await;
            return Ok(());
        }
        tokio::select! {
            _ = beat.tick(), if ready => {
                // No answer to the last beat: the connection is gone.
                if !acked {
                    return Err("Discord stopped answering.".into());
                }
                acked = false;
                ws.send(Message::text(heartbeat(seq).to_string())).await.map_err(|e| e.to_string())?;
            }
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            got = ws.next() => {
                let frame = match got {
                    Some(Ok(Message::Text(t))) => t,
                    Some(Ok(Message::Close(c))) => {
                        let code = c.as_ref().map(|c| u16::from(c.code)).unwrap_or(0);
                        return Err(match code {
                            4004 => "Discord doesn't accept that bot token. Copy it again from the developer portal.".into(),
                            4014 => "Discord refused the bot's permissions. Check the bot's settings in the developer portal.".into(),
                            _ => format!("Discord closed the connection ({code})."),
                        });
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return Err(e.to_string()),
                    None => return Err("Discord closed the connection.".into()),
                };
                let v: Value = serde_json::from_str(&frame).unwrap_or(Value::Null);
                if let Some(s) = v["s"].as_u64() {
                    seq = Some(s);
                }
                match v["op"].as_u64() {
                    Some(10) => {
                        let every = v["d"]["heartbeat_interval"].as_u64().unwrap_or(41_250).max(5_000);
                        beat = tokio::time::interval_at(tokio::time::Instant::now() + Duration::from_millis(every / 2), Duration::from_millis(every));
                        ready = true;
                        ws.send(Message::text(identify(token).to_string())).await.map_err(|e| e.to_string())?;
                    }
                    Some(11) => acked = true,
                    Some(1) => ws.send(Message::text(heartbeat(seq).to_string())).await.map_err(|e| e.to_string())?,
                    Some(7) => return Ok(()),
                    Some(9) => {
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        return Ok(());
                    }
                    Some(0) => {
                        if v["t"] == "READY" {
                            problem(None);
                        }
                        if v["t"] == "MESSAGE_CREATE" {
                            if let Some(link) = load(state) {
                                handle(runner, state, client, link, &v["d"]).await;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn handle(runner: &Runner, state: &Arc<AppState>, client: &reqwest::Client, mut link: Link, m: &Value) {
    // Servers are ignored: only direct messages from people.
    if !m["guild_id"].is_null() || m["author"]["bot"].as_bool() == Some(true) {
        return;
    }
    let from = m["author"]["id"].as_str().unwrap_or("").to_string();
    let channel = m["channel_id"].as_str().unwrap_or("").to_string();
    if from.is_empty() || from == link.bot_id || channel.is_empty() {
        return;
    }
    let text = m["content"].as_str().unwrap_or("").trim().to_string();
    let Some(person) = link.person.clone() else {
        if link.code.as_deref().is_some_and(|c| text == c) {
            link.person = Some(from);
            link.person_name = m["author"]["global_name"].as_str().or(m["author"]["username"].as_str()).unwrap_or("").to_string();
            link.channel = Some(channel.clone());
            link.code = None;
            let _ = save(state, &link);
            state.log("discord", "A Discord account was linked to the bot");
            send_text(client, &link.token, &channel, &tr(state, say::LINKED)).await;
        }
        return;
    };
    if from != person {
        return;
    }
    if link.channel.as_deref() != Some(channel.as_str()) {
        link.channel = Some(channel.clone());
        let _ = save(state, &link);
    }
    if let Some(chat) = &link.chat_id {
        if let Some(p) = state.approvals.pending(chat).first() {
            let said = text.to_lowercase();
            let allow = said == tr(state, say::ALLOW).to_lowercase() || said == "yes" || said == "allow";
            let deny = said == tr(state, say::DENY).to_lowercase() || said == "no" || said == "deny";
            if allow || deny {
                let took = state.approvals.answer(&p.call_id, if allow { Decision::Allow } else { Decision::Deny });
                let reply = if !took { say::ANSWERED } else if allow { say::ALLOWED } else { say::DENIED };
                return send_text(client, &link.token, &channel, &tr(state, reply)).await;
            }
        }
    }
    match text.as_str() {
        "/start" | "/help" => return send_text(client, &link.token, &channel, &tr(state, say::HELP)).await,
        "/new" => {
            link.chat_id = None;
            let _ = save(state, &link);
            return send_text(client, &link.token, &channel, &tr(state, say::NEW_CHAT)).await;
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
    let text = picked(&text).unwrap_or(text);
    let (runner, state, client, m) = (runner.clone(), state.clone(), client.clone(), m.clone());
    tauri::async_runtime::spawn(async move {
        if let Err(e) = answer(&runner, &state, &client, link.clone(), &channel, &m, text).await {
            send_text(&client, &link.token, &channel, &format!("{} {e}", tr(&state, say::COULD_NOT))).await;
        }
    });
}

fn chat_for(state: &AppState, link: &mut Link) -> Result<String, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    if let Some(id) = link.chat_id.as_ref().filter(|id| db::chat(&conn, &c, id).is_some()) {
        return Ok(id.clone());
    }
    let model = db::settings(&conn).default_model;
    let chat = db::create_chat(&conn, &c, model)?;
    db::set_chat_title(&conn, &c, &chat.id, "From Discord")?;
    drop(conn);
    link.chat_id = Some(chat.id.clone());
    save(state, link)?;
    Ok(chat.id)
}

async fn answer(runner: &Runner, state: &Arc<AppState>, client: &reqwest::Client, mut link: Link, channel: &str, m: &Value, mut text: String) -> Result<(), String> {
    let chat_id = chat_for(state, &mut link)?;
    let c = state.cipher()?;
    let mut images = Vec::new();
    for a in m["attachments"].as_array().into_iter().flatten() {
        let kind = a["content_type"].as_str().unwrap_or("");
        let url = a["url"].as_str().unwrap_or("");
        if kind.starts_with("image/") {
            let bytes = download(client, url).await?;
            images.push(crate::media::add(state, &c, bytes, "image", kind, "paste", "", None, Some(chat_id.clone()), true)?.id);
        } else if kind.starts_with("audio/") || a["flags"].as_u64().is_some_and(|f| f & VOICE_FLAG != 0) {
            let heard = hear(state, &download(client, url).await?).await?;
            text = if text.is_empty() { heard } else { format!("{text}\n\n{heard}") };
        }
    }
    if text.is_empty() && images.is_empty() {
        return Err("only text, pictures and voice messages can be read here".into());
    }
    let started = db::now_ms();
    let done = Arc::new(AtomicBool::new(false));
    tauri::async_runtime::spawn(typing(client.clone(), link.token.clone(), channel.to_string(), done.clone()));
    tauri::async_runtime::spawn(ask_approvals(state.clone(), client.clone(), link.token.clone(), channel.to_string(), chat_id.clone(), done.clone()));
    let result = runner(state.clone(), chat_id.clone(), text, images).await;
    done.store(true, Ordering::Relaxed);
    let reply = match result {
        Ok(Some(msg)) => msg.content,
        Ok(None) => tr(state, say::STOPPED),
        Err(e) => return Err(e),
    };
    let made: Vec<crate::media::MediaItem> = db::messages(&state.db.lock().unwrap(), &c, &chat_id)
        .into_iter()
        .filter(|msg| msg.role == "tool" && msg.created_at >= started)
        .filter_map(|msg| msg.meta.and_then(|v| serde_json::from_value::<Vec<crate::media::MediaItem>>(v["media"].clone()).ok()))
        .flatten()
        .collect();
    for item in made {
        if let Ok(bytes) = crate::media::store::read(&state.paths, &c, &item.id) {
            send_file(client, &link.token, channel, &item, bytes, &tr(state, say::TOO_BIG)).await;
        }
    }
    send_reply(client, &link.token, channel, &reply).await;
    state.log("discord", "Answered a message from Discord");
    Ok(())
}

async fn send_file(client: &reqwest::Client, token: &str, channel: &str, item: &crate::media::MediaItem, bytes: Vec<u8>, too_big: &str) {
    if bytes.len() > MAX_FILE {
        return send_text(client, token, channel, too_big).await;
    }
    let ext = item.mime.split('/').nth(1).unwrap_or("bin").replace("mpeg", "mp3");
    let Ok(part) = reqwest::multipart::Part::bytes(bytes).file_name(format!("{}.{ext}", item.kind)).mime_str(&item.mime) else { return };
    let form = reqwest::multipart::Form::new().text("payload_json", json!({ "attachments": [{ "id": 0 }] }).to_string()).part("files[0]", part);
    let _ = client
        .post(format!("{}/channels/{channel}/messages", api()))
        .header("authorization", format!("Bot {token}"))
        .multipart(form)
        .timeout(Duration::from_secs(300))
        .send()
        .await;
}

async fn typing(client: reqwest::Client, token: String, channel: String, done: Arc<AtomicBool>) {
    while !done.load(Ordering::Relaxed) {
        let _ = call(&client, &token, reqwest::Method::POST, &format!("/channels/{channel}/typing"), None).await;
        tokio::time::sleep(Duration::from_secs(8)).await;
    }
}

async fn ask_approvals(state: Arc<AppState>, client: reqwest::Client, token: String, channel: String, chat_id: String, done: Arc<AtomicBool>) {
    let mut asked = HashSet::new();
    while !done.load(Ordering::Relaxed) {
        for p in state.approvals.pending(&chat_id) {
            if !asked.insert(p.call_id.clone()) {
                continue;
            }
            let text = format!("{}\n{}\n({} / {})", tr(&state, say::MAY_I), p.preview.title, tr(&state, say::ALLOW), tr(&state, say::DENY));
            send_text(&client, &token, &channel, &text).await;
        }
        tokio::time::sleep(Duration::from_millis(700)).await;
    }
}

pub fn task_done(state: &Arc<AppState>, name: &str, outcome: Result<&str, &str>) {
    let Some(link) = load(state).filter(|l| l.tasks && l.person.is_some()) else { return };
    let Some(channel) = link.channel.clone() else { return };
    let Ok(client) = client(state) else { return };
    let Ok(c) = state.cipher() else { return };
    let text = match outcome {
        Ok(chat_id) => {
            let last = db::messages(&state.db.lock().unwrap(), &c, chat_id).into_iter().rev().find(|m| m.role == "assistant" && !m.content.trim().is_empty());
            format!("{name}\n\n{}", last.map(|m| m.content).unwrap_or_else(|| tr(state, say::NOTHING)))
        }
        Err(e) => format!("{name}\n{} {e}", tr(state, say::DIDNT_RUN)),
    };
    tauri::async_runtime::spawn(async move { send_text(&client, &link.token, &channel, &text).await });
}

#[derive(Serialize)]
pub struct DiscordView {
    connected: bool,
    bot: Option<String>,
    /// Adds the bot to one of the person's servers, so they can message it.
    invite: Option<String>,
    linked_to: Option<String>,
    code: Option<String>,
    tasks: bool,
    problem: Option<String>,
}

fn view(state: &AppState) -> DiscordView {
    let link = load(state);
    DiscordView {
        connected: link.is_some(),
        bot: link.as_ref().map(|l| l.bot.clone()),
        invite: link.as_ref().filter(|l| !l.bot_id.is_empty()).map(|l| format!("https://discord.com/oauth2/authorize?client_id={}&scope=bot&permissions=0", l.bot_id)),
        linked_to: link.as_ref().filter(|l| l.person.is_some()).map(|l| l.person_name.clone()),
        code: link.as_ref().and_then(|l| l.code.clone()),
        tasks: link.as_ref().is_some_and(|l| l.tasks),
        problem: link.is_some().then(|| PROBLEM.lock().unwrap().clone()).flatten(),
    }
}

#[tauri::command(async)]
pub fn discord_view(state: AppStateRef) -> Result<DiscordView, String> {
    state.cipher()?;
    Ok(view(&state))
}

#[tauri::command]
pub async fn discord_connect(app: AppHandle, state: AppStateRef<'_>, token: String) -> Result<DiscordView, String> {
    let token = token.trim().trim_start_matches("Bot ").to_string();
    if token.len() < 50 || !token.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) {
        return Err("That doesn't look like a bot token. The developer portal shows one under Bot › Reset Token.".into());
    }
    let me = call(&client(&state)?, &token, reqwest::Method::GET, "/users/@me", None).await?;
    let link = Link {
        token,
        bot: me["username"].as_str().unwrap_or("").to_string(),
        bot_id: me["id"].as_str().unwrap_or("").to_string(),
        code: Some(format!("{:06}", uuid::Uuid::new_v4().as_u128() % 1_000_000)),
        tasks: true,
        ..Default::default()
    };
    save(&state, &link)?;
    state.log("discord", &format!("Connected the Discord bot {}", link.bot));
    problem(None);
    start(app, state.inner().clone());
    Ok(view(&state))
}

#[tauri::command(async)]
pub fn discord_set_tasks(state: AppStateRef, on: bool) -> Result<DiscordView, String> {
    let mut link = load(&state).ok_or("Discord isn't connected.")?;
    link.tasks = on;
    save(&state, &link)?;
    Ok(view(&state))
}

#[tauri::command(async)]
pub fn discord_disconnect(state: AppStateRef) -> Result<(), String> {
    state.cipher()?;
    stop();
    state.db.lock().unwrap().execute("DELETE FROM settings WHERE key = ?1", [KEY]).map_err(|e| e.to_string())?;
    problem(None);
    state.log("discord", "Disconnected the Discord bot");
    Ok(())
}

#[cfg(test)]
#[path = "discord_tests.rs"]
mod tests;
