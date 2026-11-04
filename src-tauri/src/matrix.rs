// SPDX-License-Identifier: AGPL-3.0-only
//! Texting the assistant from Matrix (Element and other Matrix apps). The
//! person makes an account for the assistant on any homeserver, pastes the
//! homeserver and that account's access token here, invites it to a room and
//! sends the code this PC shows; from then on only that person, in that room,
//! is answered. Works like the Telegram link (and shares its texts), with
//! typed answers in place of buttons since Matrix has none. Encrypted rooms
//! can't be read: the assistant's account has no keys, and the setup says so.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::agent::Decision;
use crate::telegram::{hear, pieces, say, split_choices, tr, Runner};
use crate::{db, AppState, AppStateRef};

const KEY: &str = "matrix";
const POLL_MS: u64 = 25_000;
/// Matrix allows 64 KB events; long answers are easier to read in pieces anyway.
const MAX_TEXT: usize = 8000;
const MAX_FILE: usize = 50 * 1024 * 1024;

#[derive(Serialize, Deserialize, Clone, Default)]
struct Link {
    homeserver: String,
    token: String,
    /// The assistant's own Matrix ID, so its own messages are skipped.
    me: String,
    /// The person it answers and their room, once the code was sent there.
    person: Option<String>,
    room: Option<String>,
    code: Option<String>,
    chat_id: Option<String>,
    tasks: bool,
    /// Where the last sync ended; None until the first one, which skips the
    /// room's history.
    since: Option<String>,
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
/// The options last offered, so a typed number picks one.
static OFFERED: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn running() -> bool {
    RUNNING.lock().unwrap().as_ref().is_some_and(|stop| !stop.load(Ordering::Relaxed))
}

fn problem(p: Option<String>) {
    *PROBLEM.lock().unwrap() = p;
}

fn client(state: &AppState) -> Result<reqwest::Client, String> {
    crate::net::service_client(state.settings().connectivity).map_err(|_| "Matrix needs web access: switch to Local AI + Web (or Cloud) for it to work.".to_string())
}

/// https only, except a server on this PC (a homeserver run here, and tests).
fn clean_homeserver(raw: &str) -> Result<String, String> {
    let raw = raw.trim().trim_end_matches('/');
    let with_scheme = if raw.contains("://") { raw.to_string() } else { format!("https://{raw}") };
    let url = reqwest::Url::parse(&with_scheme).map_err(|_| "That isn't a homeserver address. It looks like matrix.org or https://matrix.example.com.".to_string())?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if url.scheme() != "https" && !(url.scheme() == "http" && local) {
        return Err("The homeserver must use https.".into());
    }
    Ok(with_scheme)
}

/// Room IDs (`!a:b`) and user IDs (`@a:b`) go in paths and queries escaped.
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

async fn call(client: &reqwest::Client, link: &Link, method: reqwest::Method, path: &str, body: Option<Value>, wait: Duration) -> Result<Value, String> {
    let mut req = client.request(method, format!("{}{path}", link.homeserver)).bearer_auth(&link.token).timeout(wait);
    if let Some(b) = body {
        req = req.json(&b);
    }
    let r = req.send().await.map_err(|e| format!("Couldn't reach the homeserver ({}).", e.without_url()))?;
    let status = r.status();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(v);
    }
    Err(match v["errcode"].as_str() {
        Some("M_UNKNOWN_TOKEN" | "M_MISSING_TOKEN") => "The homeserver doesn't accept that access token. Copy it again from the assistant's account.".into(),
        _ => format!("The homeserver said: {}", v["error"].as_str().unwrap_or(status.as_str())),
    })
}

fn txn() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

async fn send_event(client: &reqwest::Client, link: &Link, room: &str, content: Value) {
    let path = format!("/_matrix/client/v3/rooms/{}/send/m.room.message/{}", enc(room), txn());
    let _ = call(client, link, reqwest::Method::PUT, &path, Some(content), Duration::from_secs(30)).await;
}

async fn send_text(client: &reqwest::Client, link: &Link, room: &str, text: &str) {
    for part in pieces(text, MAX_TEXT) {
        send_event(client, link, room, json!({ "msgtype": "m.text", "body": part })).await;
    }
}

/// Options become numbered lines; replying with the number (or the words) picks one.
async fn send_reply(client: &reqwest::Client, link: &Link, room: &str, reply: &str) {
    let (text, options) = split_choices(reply);
    if options.is_empty() {
        return send_text(client, link, room, reply).await;
    }
    let listed: Vec<String> = options.iter().enumerate().map(|(i, o)| format!("{}. {o}", i + 1)).collect();
    send_text(client, link, room, &format!("{text}\n\n{}", listed.join("\n")).trim().to_string()).await;
    *OFFERED.lock().unwrap() = options;
}

/// A typed answer to the last numbered options, as the option's words.
fn picked(text: &str) -> Option<String> {
    let n: usize = text.trim().trim_end_matches('.').parse().ok()?;
    OFFERED.lock().unwrap().get(n.checked_sub(1)?).cloned()
}

/// `mxc://server/id` to its download address (authenticated media first).
fn media_paths(mxc: &str) -> Option<[String; 2]> {
    let (server, id) = mxc.strip_prefix("mxc://")?.split_once('/')?;
    let safe = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:".contains(c));
    if !safe(server) || !safe(id) {
        return None;
    }
    Some([format!("/_matrix/client/v1/media/download/{server}/{id}"), format!("/_matrix/media/v3/download/{server}/{id}")])
}

async fn download(client: &reqwest::Client, link: &Link, mxc: &str) -> Result<Vec<u8>, String> {
    let paths = media_paths(mxc).ok_or("The file's address couldn't be read.")?;
    for path in paths {
        let Ok(r) = client.get(format!("{}{path}", link.homeserver)).bearer_auth(&link.token).timeout(Duration::from_secs(120)).send().await else { continue };
        if !r.status().is_success() {
            continue;
        }
        if r.content_length().is_some_and(|n| n as usize > MAX_FILE) {
            return Err("That file is too big.".into());
        }
        let bytes = r.bytes().await.map_err(|e| e.without_url().to_string())?;
        if bytes.len() > MAX_FILE {
            return Err("That file is too big.".into());
        }
        return Ok(bytes.to_vec());
    }
    Err("The homeserver didn't give the file.".into())
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
            let filter = json!({ "room": { "timeline": { "limit": 20, "types": ["m.room.message", "m.room.encrypted"] }, "state": { "types": [] }, "ephemeral": { "types": [] } }, "presence": { "types": [] }, "account_data": { "types": [] } });
            let mut path = format!("/_matrix/client/v3/sync?filter={}", enc(&filter.to_string()));
            match &link.since {
                Some(since) => path.push_str(&format!("&timeout={POLL_MS}&since={}", enc(since))),
                None => path.push_str("&timeout=0"),
            }
            let got = call(&client, &link, reqwest::Method::GET, &path, None, Duration::from_millis(POLL_MS + 20_000)).await;
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match got {
                Ok(v) => {
                    problem(None);
                    wait = 2;
                    let first = link.since.is_none();
                    let Some(mut link) = load(&state) else { break };
                    link.since = v["next_batch"].as_str().map(str::to_string).or(link.since);
                    let _ = save(&state, &link);
                    if first {
                        continue;
                    }
                    take_invites(&state, &client, &link, &v).await;
                    for (room, r) in v["rooms"]["join"].as_object().into_iter().flatten() {
                        for e in r["timeline"]["events"].as_array().into_iter().flatten() {
                            let Some(link) = load(&state) else { break };
                            handle(&runner, &state, &client, link, room, e).await;
                        }
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

/// Joins a room it's invited to while waiting for the code, and afterwards
/// only rooms the linked person invites it to.
async fn take_invites(state: &AppState, client: &reqwest::Client, link: &Link, v: &Value) {
    for (room, r) in v["rooms"]["invite"].as_object().into_iter().flatten() {
        let inviter = r["invite_state"]["events"].as_array().into_iter().flatten().find(|e| e["type"] == "m.room.member" && e["state_key"] == link.me.as_str()).and_then(|e| e["sender"].as_str());
        let allowed = match &link.person {
            None => link.code.is_some(),
            Some(p) => inviter == Some(p.as_str()),
        };
        let path = if allowed { format!("/_matrix/client/v3/join/{}", enc(room)) } else { format!("/_matrix/client/v3/rooms/{}/leave", enc(room)) };
        let _ = call(client, link, reqwest::Method::POST, &path, Some(json!({})), Duration::from_secs(30)).await;
        if allowed {
            state.log("matrix", "Joined a Matrix room the assistant was invited to");
        }
    }
}

async fn handle(runner: &Runner, state: &Arc<AppState>, client: &reqwest::Client, mut link: Link, room: &str, e: &Value) {
    let sender = e["sender"].as_str().unwrap_or("");
    if sender == link.me || sender.is_empty() {
        return;
    }
    let content = &e["content"];
    let text = content["body"].as_str().unwrap_or("").trim().to_string();
    let Some(person) = link.person.clone() else {
        if e["type"] == "m.room.message" && link.code.as_deref().is_some_and(|c| text == c) {
            link.person = Some(sender.to_string());
            link.room = Some(room.to_string());
            link.code = None;
            let _ = save(state, &link);
            state.log("matrix", "A Matrix account was linked to the assistant");
            send_text(client, &link, room, &tr(state, say::LINKED)).await;
        }
        return;
    };
    if sender != person || link.room.as_deref() != Some(room) {
        return;
    }
    if e["type"] == "m.room.encrypted" {
        send_text(client, &link, room, &format!("{} This room is encrypted, so the assistant can't read it. Make a room with encryption off and invite it there.", tr(state, say::COULD_NOT))).await;
        return;
    }
    // An answer to a waiting approval: the words of Allow or Deny, or yes/no.
    if let Some(chat) = &link.chat_id {
        let waiting = state.approvals.pending(chat);
        if let Some(p) = waiting.first() {
            let said = text.to_lowercase();
            let allow = said == tr(state, say::ALLOW).to_lowercase() || said == "yes" || said == "allow";
            let deny = said == tr(state, say::DENY).to_lowercase() || said == "no" || said == "deny";
            if allow || deny {
                let took = state.approvals.answer(&p.call_id, if allow { Decision::Allow } else { Decision::Deny });
                let reply = if !took { say::ANSWERED } else if allow { say::ALLOWED } else { say::DENIED };
                send_text(client, &link, room, &tr(state, reply)).await;
                return;
            }
        }
    }
    match text.as_str() {
        "/start" | "/help" => return send_text(client, &link, room, &tr(state, say::HELP)).await,
        "/new" => {
            link.chat_id = None;
            let _ = save(state, &link);
            return send_text(client, &link, room, &tr(state, say::NEW_CHAT)).await;
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
    let (runner, state, client, e, room) = (runner.clone(), state.clone(), client.clone(), e.clone(), room.to_string());
    tauri::async_runtime::spawn(async move {
        if let Err(err) = answer(&runner, &state, &client, link.clone(), &room, &e, text).await {
            send_text(&client, &link, &room, &format!("{} {err}", tr(&state, say::COULD_NOT))).await;
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
    db::set_chat_title(&conn, &c, &chat.id, "From Matrix")?;
    drop(conn);
    link.chat_id = Some(chat.id.clone());
    save(state, link)?;
    Ok(chat.id)
}

async fn answer(runner: &Runner, state: &Arc<AppState>, client: &reqwest::Client, mut link: Link, room: &str, e: &Value, mut text: String) -> Result<(), String> {
    let chat_id = chat_for(state, &mut link)?;
    let c = state.cipher()?;
    let content = &e["content"];
    let mut images = Vec::new();
    match content["msgtype"].as_str() {
        Some("m.image") => {
            let bytes = download(client, &link, content["url"].as_str().unwrap_or("")).await?;
            let mime = content["info"]["mimetype"].as_str().unwrap_or("image/jpeg");
            images.push(crate::media::add(state, &c, bytes, "image", mime, "paste", "", None, Some(chat_id.clone()), true)?.id);
            // The body of a picture is its file name, not words to answer.
            text.clear();
        }
        Some("m.audio") => text = hear(state, &download(client, &link, content["url"].as_str().unwrap_or("")).await?).await?,
        Some("m.text" | "m.notice" | "m.emote") | None => {}
        Some(_) => return Err("only text, pictures and voice messages can be read here".into()),
    }
    if text.is_empty() && images.is_empty() {
        return Err("only text, pictures and voice messages can be read here".into());
    }
    let started = db::now_ms();
    let done = Arc::new(AtomicBool::new(false));
    tauri::async_runtime::spawn(typing(client.clone(), link.clone(), room.to_string(), done.clone()));
    tauri::async_runtime::spawn(ask_approvals(state.clone(), client.clone(), link.clone(), room.to_string(), chat_id.clone(), done.clone()));
    let result = runner(state.clone(), chat_id.clone(), text, images).await;
    done.store(true, Ordering::Relaxed);
    let reply = match result {
        Ok(Some(msg)) => msg.content,
        Ok(None) => tr(state, say::STOPPED),
        Err(err) => return Err(err),
    };
    let made: Vec<crate::media::MediaItem> = db::messages(&state.db.lock().unwrap(), &c, &chat_id)
        .into_iter()
        .filter(|msg| msg.role == "tool" && msg.created_at >= started)
        .filter_map(|msg| msg.meta.and_then(|v| serde_json::from_value::<Vec<crate::media::MediaItem>>(v["media"].clone()).ok()))
        .flatten()
        .collect();
    for item in made {
        if let Ok(bytes) = crate::media::store::read(&state.paths, &c, &item.id) {
            send_file(client, &link, room, &item, bytes, &tr(state, say::TOO_BIG)).await;
        }
    }
    send_reply(client, &link, room, &reply).await;
    state.log("matrix", "Answered a message from Matrix");
    Ok(())
}

async fn send_file(client: &reqwest::Client, link: &Link, room: &str, item: &crate::media::MediaItem, bytes: Vec<u8>, too_big: &str) {
    if bytes.len() > MAX_FILE {
        return send_text(client, link, room, too_big).await;
    }
    let ext = item.mime.split('/').nth(1).unwrap_or("bin").replace("mpeg", "mp3");
    let name = format!("{}.{ext}", item.kind);
    let size = bytes.len();
    let up = client
        .post(format!("{}/_matrix/media/v3/upload?filename={}", link.homeserver, enc(&name)))
        .bearer_auth(&link.token)
        .header(reqwest::header::CONTENT_TYPE, item.mime.as_str())
        .body(bytes)
        .timeout(Duration::from_secs(300))
        .send()
        .await;
    let Ok(up) = up else { return };
    let Ok(v) = up.json::<Value>().await else { return };
    let Some(uri) = v["content_uri"].as_str() else { return };
    let msgtype = match item.kind.as_str() {
        "image" => "m.image",
        "audio" => "m.audio",
        "video" => "m.video",
        _ => "m.file",
    };
    send_event(client, link, room, json!({ "msgtype": msgtype, "body": name, "url": uri, "info": { "mimetype": item.mime, "size": size } })).await;
}

async fn typing(client: reqwest::Client, link: Link, room: String, done: Arc<AtomicBool>) {
    let path = format!("/_matrix/client/v3/rooms/{}/typing/{}", enc(&room), enc(&link.me));
    while !done.load(Ordering::Relaxed) {
        let _ = call(&client, &link, reqwest::Method::PUT, &path, Some(json!({ "typing": true, "timeout": 6000 })), Duration::from_secs(10)).await;
        tokio::time::sleep(Duration::from_millis(5000)).await;
    }
    let _ = call(&client, &link, reqwest::Method::PUT, &path, Some(json!({ "typing": false })), Duration::from_secs(10)).await;
}

async fn ask_approvals(state: Arc<AppState>, client: reqwest::Client, link: Link, room: String, chat_id: String, done: Arc<AtomicBool>) {
    let mut asked = HashSet::new();
    while !done.load(Ordering::Relaxed) {
        for p in state.approvals.pending(&chat_id) {
            if !asked.insert(p.call_id.clone()) {
                continue;
            }
            let text = format!("{}\n{}\n({} / {})", tr(&state, say::MAY_I), p.preview.title, tr(&state, say::ALLOW), tr(&state, say::DENY));
            send_text(&client, &link, &room, &text).await;
        }
        tokio::time::sleep(Duration::from_millis(700)).await;
    }
}

pub fn task_done(state: &Arc<AppState>, name: &str, outcome: Result<&str, &str>) {
    let Some(link) = load(state).filter(|l| l.tasks && l.person.is_some()) else { return };
    let Some(room) = link.room.clone() else { return };
    let Ok(client) = client(state) else { return };
    let Ok(c) = state.cipher() else { return };
    let text = match outcome {
        Ok(chat_id) => {
            let last = db::messages(&state.db.lock().unwrap(), &c, chat_id).into_iter().rev().find(|m| m.role == "assistant" && !m.content.trim().is_empty());
            format!("{name}\n\n{}", last.map(|m| m.content).unwrap_or_else(|| tr(state, say::NOTHING)))
        }
        Err(e) => format!("{name}\n{} {e}", tr(state, say::DIDNT_RUN)),
    };
    tauri::async_runtime::spawn(async move { send_text(&client, &link, &room, &text).await });
}

#[derive(Serialize)]
pub struct MatrixView {
    connected: bool,
    account: Option<String>,
    linked_to: Option<String>,
    code: Option<String>,
    tasks: bool,
    problem: Option<String>,
}

fn view(state: &AppState) -> MatrixView {
    let link = load(state);
    MatrixView {
        connected: link.is_some(),
        account: link.as_ref().map(|l| l.me.clone()),
        linked_to: link.as_ref().and_then(|l| l.person.clone()),
        code: link.as_ref().and_then(|l| l.code.clone()),
        tasks: link.as_ref().is_some_and(|l| l.tasks),
        problem: link.is_some().then(|| PROBLEM.lock().unwrap().clone()).flatten(),
    }
}

#[tauri::command(async)]
pub fn matrix_view(state: AppStateRef) -> Result<MatrixView, String> {
    state.cipher()?;
    Ok(view(&state))
}

/// Checks the token with the homeserver, then waits for the code.
#[tauri::command]
pub async fn matrix_connect(app: AppHandle, state: AppStateRef<'_>, homeserver: String, token: String) -> Result<MatrixView, String> {
    let homeserver = clean_homeserver(&homeserver)?;
    let token = token.trim().to_string();
    if token.is_empty() || token.chars().any(char::is_whitespace) {
        return Err("Paste the assistant account's access token (in Element: Settings › Help & About › Access token).".into());
    }
    let mut link = Link { homeserver, token, tasks: true, ..Default::default() };
    let me = call(&client(&state)?, &link, reqwest::Method::GET, "/_matrix/client/v3/account/whoami", None, Duration::from_secs(30)).await?;
    link.me = me["user_id"].as_str().unwrap_or("").to_string();
    link.code = Some(format!("{:06}", uuid::Uuid::new_v4().as_u128() % 1_000_000));
    save(&state, &link)?;
    state.log("matrix", &format!("Connected the Matrix account {}", link.me));
    problem(None);
    start(app, state.inner().clone());
    Ok(view(&state))
}

#[tauri::command(async)]
pub fn matrix_set_tasks(state: AppStateRef, on: bool) -> Result<MatrixView, String> {
    let mut link = load(&state).ok_or("Matrix isn't connected.")?;
    link.tasks = on;
    save(&state, &link)?;
    Ok(view(&state))
}

#[tauri::command(async)]
pub fn matrix_disconnect(state: AppStateRef) -> Result<(), String> {
    state.cipher()?;
    stop();
    state.db.lock().unwrap().execute("DELETE FROM settings WHERE key = ?1", [KEY]).map_err(|e| e.to_string())?;
    problem(None);
    state.log("matrix", "Disconnected the Matrix account");
    Ok(())
}

#[cfg(test)]
#[path = "matrix_tests.rs"]
mod tests;
