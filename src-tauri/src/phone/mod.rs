// SPDX-License-Identifier: AGPL-3.0-only
//! The phone companion: a small web page this PC serves on the local
//! network, so a phone on the same Wi-Fi can chat, see tasks and notes, and
//! answer the assistant's questions, with nothing installed on the phone.
//!
//! - Off until turned on. Each phone is added with a QR code holding the
//!   page's address and a random 256-bit key after `#`, which browsers never
//!   send over the network. Phones can be removed one by one.
//! - Phone browsers can't use their built-in encryption on a plain local
//!   address, so the page carries TweetNaCl (public domain) and every
//!   request and answer is sealed with that key (XSalsa20-Poly1305). A
//!   request older than five minutes, or not newer than the last one, is
//!   refused, so recorded traffic can't be replayed.
//! - The page itself travels unencrypted: someone able to rewrite traffic on
//!   the network could alter it. The settings say to use it on a trusted
//!   home or office network.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, LazyLock, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64URL};
use base64::Engine;
use crypto_secretbox::aead::{Aead, KeyInit};
use crypto_secretbox::{Nonce, XSalsa20Poly1305};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::crypto::Cipher;
use crate::{db, notes, AppState, AppStateRef};

const KEY: &str = "phone";
const PORT: u16 = 47818;
const PAGE: &str = include_str!("page.html");
const NACL: &str = include_str!("nacl-fast.min.js");

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
    pub phones: Vec<Phone>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Phone {
    pub id: String,
    pub name: String,
    /// Sealed with the data key.
    key: String,
    pub added: i64,
    pub last_seen: Option<i64>,
    /// The newest request time seen (replays are refused).
    last_ts: i64,
}

fn settings(conn: &rusqlite::Connection) -> Settings {
    db::get(conn, KEY).unwrap_or_default()
}

struct Running {
    port: u16,
    stop: oneshot::Sender<()>,
}

static SERVER: Mutex<Option<Running>> = Mutex::new(None);
static PROBLEM: Mutex<Option<String>> = Mutex::new(None);
/// Replies being written for phone chats: chat id → text so far.
static PARTIAL: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

/// Starts or stops the server to match the settings.
pub fn apply(state: Arc<AppState>) {
    let on = settings(&state.db.lock().unwrap()).enabled;
    if let Some(r) = SERVER.lock().unwrap().take() {
        let _ = r.stop.send(());
    }
    *PROBLEM.lock().unwrap() = None;
    if !on {
        return;
    }
    let (tx, rx) = oneshot::channel();
    tauri::async_runtime::spawn(async move {
        let mut listener = None;
        for port in [PORT, 0] {
            for _ in 0..10 {
                match tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).await {
                    Ok(l) => {
                        listener = Some(l);
                        break;
                    }
                    Err(_) if port != 0 => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
                    Err(_) => break,
                }
            }
            if listener.is_some() {
                break;
            }
        }
        let Some(listener) = listener else {
            *PROBLEM.lock().unwrap() = Some("Couldn't open a port for the phone page.".into());
            return;
        };
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(PORT);
        *SERVER.lock().unwrap() = Some(Running { port, stop: tx });
        let app = Router::new().route("/", get(page)).route("/api", post(api)).with_state(state);
        let _ = axum::serve(listener, app).with_graceful_shutdown(async { rx.await.ok(); }).await;
    });
}

async fn page() -> Response {
    let html = PAGE.replace("/*NACL*/", NACL);
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
            (header::X_FRAME_OPTIONS, "DENY"),
            (header::CONTENT_SECURITY_POLICY, "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; img-src data:"),
        ],
        html,
    )
        .into_response()
}

fn seal(key: &[u8; 32], plain: &[u8]) -> String {
    let nonce: [u8; 24] = crate::sync::random_bytes();
    let ct = XSalsa20Poly1305::new(key.into()).encrypt(Nonce::from_slice(&nonce), plain).expect("encryption");
    B64.encode([nonce.as_slice(), &ct].concat())
}

fn open(key: &[u8; 32], sealed: &str) -> Option<Vec<u8>> {
    let raw = B64.decode(sealed.trim()).ok()?;
    if raw.len() < 24 + 16 {
        return None;
    }
    let (nonce, ct) = raw.split_at(24);
    XSalsa20Poly1305::new(key.into()).decrypt(Nonce::from_slice(nonce), ct).ok()
}

#[derive(Deserialize)]
struct Call {
    op: String,
    #[serde(default)]
    args: Value,
    ts: i64,
}

async fn api(State(state): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Response {
    let bad = || (StatusCode::UNAUTHORIZED, "This phone isn't paired. Scan a new code from SulcusAI on your PC.").into_response();
    let Some(id) = headers.get("x-phone").and_then(|h| h.to_str().ok()).map(String::from) else { return bad() };
    // While the app is locked, nothing can be read or written.
    let Ok(c) = state.work_cipher() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "SulcusAI is locked on your PC. Unlock it there first.").into_response();
    };
    let Some(phone) = settings(&state.db.lock().unwrap()).phones.into_iter().find(|p| p.id == id) else { return bad() };
    let Some(key) = phone_key(&c, &phone) else { return bad() };
    let Some(plain) = open(&key, std::str::from_utf8(&body).unwrap_or_default()) else { return bad() };
    let Ok(call) = serde_json::from_slice::<Call>(&plain) else { return (StatusCode::BAD_REQUEST, "Unreadable request.").into_response() };
    let now = db::now_ms();
    if (now - call.ts).abs() > 5 * 60 * 1000 || call.ts <= phone.last_ts {
        return (StatusCode::CONFLICT, "This request is out of date. Check the phone's clock, then reload the page.").into_response();
    }
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        if let Some(p) = s.phones.iter_mut().find(|p| p.id == id) {
            p.last_ts = call.ts;
            p.last_seen = Some(now);
        }
        let _ = db::set(&conn, KEY, &s);
    }
    let answer = match handle(&state, &c, &phone, &call.op, &call.args).await {
        Ok(data) => json!({ "ok": true, "data": data }),
        Err(e) => json!({ "ok": false, "error": e }),
    };
    (
        [(header::CONTENT_TYPE, "text/plain"), (header::CACHE_CONTROL, "no-store")],
        seal(&key, answer.to_string().as_bytes()),
    )
        .into_response()
}

fn phone_key(c: &Cipher, p: &Phone) -> Option<[u8; 32]> {
    let hex = zeroize::Zeroizing::new(c.decrypt(&p.key).ok()?);
    hex::decode(hex.as_str()).ok()?.try_into().ok()
}

fn arg<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args[name].as_str().filter(|s| !s.is_empty()).ok_or_else(|| format!("Missing {name}."))
}

async fn handle(state: &Arc<AppState>, c: &Cipher, phone: &Phone, op: &str, args: &Value) -> Result<Value, String> {
    match op {
        "hello" => {
            let conn = state.db.lock().unwrap();
            let name = db::profile(&conn, c).name;
            Ok(json!({ "pc": std::env::var("COMPUTERNAME").unwrap_or_default(), "name": name, "phone": phone.name }))
        }
        "chats" => {
            let running: Vec<String> = state.generations.lock().unwrap().keys().cloned().collect();
            let chats = db::list_chats(&state.db.lock().unwrap(), c);
            Ok(json!(chats
                .into_iter()
                .filter(|ch| !ch.incognito)
                .take(60)
                .map(|ch| json!({ "id": ch.id, "title": ch.title, "updated_at": ch.updated_at, "running": running.contains(&ch.id) }))
                .collect::<Vec<_>>()))
        }
        "messages" => {
            let chat_id = arg(args, "chat_id")?;
            let all = db::messages(&state.db.lock().unwrap(), c, chat_id);
            let kept: Vec<_> = all.iter().filter(|m| (m.role == "user" || m.role == "assistant") && !m.content.trim().is_empty()).collect();
            let shown: Vec<Value> = kept[kept.len().saturating_sub(60)..]
                .iter()
                .map(|m| json!({ "id": m.id, "role": m.role, "content": m.content, "at": m.created_at }))
                .collect();
            let running = state.generations.lock().unwrap().contains_key(chat_id);
            let partial = PARTIAL.lock().unwrap().get(chat_id).cloned();
            let approvals = state.approvals.pending(chat_id);
            Ok(json!({ "messages": shown, "running": running, "partial": partial, "approvals": approvals }))
        }
        "send" => {
            let text = arg(args, "text")?.trim().to_string();
            if text.chars().count() > 20_000 {
                return Err("That message is too long for the phone page.".into());
            }
            let chat_id = match args["chat_id"].as_str().filter(|s| !s.is_empty()) {
                Some(id) => id.to_string(),
                None => {
                    let conn = state.db.lock().unwrap();
                    let model = db::settings(&conn).default_model;
                    db::create_chat(&conn, c, model)?.id
                }
            };
            let app = state.app.get().ok_or("The app isn't ready yet.")?.clone();
            let (s, id) = (state.clone(), chat_id.clone());
            PARTIAL.lock().unwrap().insert(id.clone(), String::new());
            let tee_id = id.clone();
            let tee: crate::Tee = Arc::new(move |event: &str, payload: &Value| {
                if payload["chat_id"].as_str() != Some(tee_id.as_str()) {
                    return;
                }
                let mut p = PARTIAL.lock().unwrap();
                match event {
                    "chat:delta" => {
                        if let Some(t) = payload["content"].as_str() {
                            p.entry(tee_id.clone()).or_default().push_str(t);
                        }
                    }
                    "chat:start" | "agent:step" => {
                        p.insert(tee_id.clone(), String::new());
                    }
                    _ => {}
                }
            });
            tauri::async_runtime::spawn(async move {
                let r = crate::run_turn(&app, &s, id.clone(), text, Vec::new(), Some(tee)).await;
                PARTIAL.lock().unwrap().remove(&id);
                if let Err(e) = r {
                    s.log("chat", &format!("A message from the phone didn't get an answer: {e}"));
                }
            });
            state.log("chat", &format!("A message came in from {}", phone.name));
            Ok(json!({ "chat_id": chat_id }))
        }
        "stop" => {
            let chat_id = arg(args, "chat_id")?;
            if let Some(flag) = state.generations.lock().unwrap().get(chat_id) {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            Ok(Value::Null)
        }
        "approve" => {
            let call_id = arg(args, "call_id")?;
            let d = crate::agent::Decision::parse(arg(args, "decision")?).ok_or("Unknown answer.")?;
            if !state.approvals.answer(call_id, d) {
                return Err("That question was already answered.".into());
            }
            state.log("agent", &format!("Answered the assistant's question from {}", phone.name));
            Ok(Value::Null)
        }
        "tasks" => {
            let tasks = notes::list_tasks(&state.db.lock().unwrap(), c);
            Ok(json!(tasks.into_iter().filter(|t| t.done_at.is_none()).take(100).collect::<Vec<_>>()))
        }
        "add_task" => {
            let (title, due) = split_due(arg(args, "title")?.trim(), chrono::Local::now().date_naive());
            let mut t = notes::new_task(&title);
            if let Some((date, time)) = due {
                t.due = notes::to_ms(date, time);
                t.due_has_time = time.is_some();
            }
            let saved = notes::save_task(&state.db.lock().unwrap(), c, t)?;
            Ok(json!(saved))
        }
        "done_task" => {
            let id = arg(args, "id")?;
            let conn = state.db.lock().unwrap();
            let mut t = notes::get_task(&conn, c, id).ok_or("That task is gone.")?;
            t.done_at = Some(db::now_ms());
            notes::save_task(&conn, c, t)?;
            Ok(Value::Null)
        }
        "notes" => {
            let list = notes::list_notes(&state.db.lock().unwrap(), c);
            Ok(json!(list
                .into_iter()
                .take(100)
                .map(|n| json!({ "id": n.id, "title": n.data.title, "body": n.data.body.chars().take(4000).collect::<String>(), "pinned": n.pinned }))
                .collect::<Vec<_>>()))
        }
        _ => Err("Unknown request.".into()),
    }
}

type Due = Option<(chrono::NaiveDate, Option<chrono::NaiveTime>)>;

/// "Call Sam tomorrow 3pm" → ("Call Sam", tomorrow at 3 pm): the longest
/// run of last words (up to four) that reads as a date.
fn split_due(text: &str, today: chrono::NaiveDate) -> (String, Due) {
    let words: Vec<&str> = text.split_whitespace().collect();
    for n in (1..=words.len().saturating_sub(1).min(4)).rev() {
        let (head, tail) = words.split_at(words.len() - n);
        if let Some(d) = notes::parse_due(&tail.join(" "), today) {
            return (head.join(" "), Some(d));
        }
    }
    (text.to_string(), None)
}

/// This PC's address on the local network (looks up the route; sends nothing).
fn local_ip() -> Option<IpAddr> {
    let s = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    s.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    s.local_addr().ok().map(|a| a.ip()).filter(|ip| !ip.is_unspecified() && !ip.is_loopback())
}

#[derive(Serialize)]
pub struct PhoneView {
    enabled: bool,
    running: bool,
    address: Option<String>,
    problem: Option<String>,
    phones: Vec<PhoneRow>,
}

#[derive(Serialize)]
pub struct PhoneRow {
    id: String,
    name: String,
    added: i64,
    last_seen: Option<i64>,
}

fn view(state: &AppState) -> PhoneView {
    let s = settings(&state.db.lock().unwrap());
    let port = SERVER.lock().unwrap().as_ref().map(|r| r.port);
    PhoneView {
        enabled: s.enabled,
        running: port.is_some(),
        address: port.and_then(|p| local_ip().map(|ip| format!("http://{}", SocketAddr::new(ip, p)))),
        problem: PROBLEM.lock().unwrap().clone(),
        phones: s.phones.iter().map(|p| PhoneRow { id: p.id.clone(), name: p.name.clone(), added: p.added, last_seen: p.last_seen }).collect(),
    }
}

#[tauri::command]
pub fn phone_view(state: AppStateRef) -> PhoneView {
    view(&state)
}

#[tauri::command]
pub async fn set_phone(state: AppStateRef<'_>, enabled: bool) -> Result<PhoneView, String> {
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        s.enabled = enabled;
        db::set(&conn, KEY, &s)?;
    }
    state.log("settings", if enabled { "Turned on the phone page" } else { "Turned off the phone page" });
    apply(state.inner().clone());
    // Give the server a moment to take its port.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    Ok(view(&state))
}

#[derive(Serialize)]
pub struct NewPhone {
    /// What the QR code holds: the page's address with the key after `#`.
    link: String,
    /// The same as an SVG picture.
    qr: String,
    view: PhoneView,
}

/// Adds a phone: a fresh key, shown once as a QR code.
#[tauri::command]
pub fn add_phone(state: AppStateRef, name: String) -> Result<NewPhone, String> {
    let c = state.cipher()?;
    let v = view(&state);
    let base = v.address.ok_or("Turn on the phone page first, and make sure this PC is on a network.")?;
    let key: [u8; 32] = crate::sync::random_bytes();
    let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
    let name = match name.trim() {
        "" => "Phone".to_string(),
        n => n.chars().take(40).collect(),
    };
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        s.phones.push(Phone { id: id.clone(), name: name.clone(), key: c.encrypt(&zeroize::Zeroizing::new(hex::encode(key))), added: db::now_ms(), last_seen: None, last_ts: 0 });
        db::set(&conn, KEY, &s)?;
    }
    state.log("settings", &format!("Added {name} to the phone page"));
    let link = format!("{base}/#p={id}.{}", B64URL.encode(key));
    let qr = qrcode::QrCode::with_error_correction_level(link.as_bytes(), qrcode::EcLevel::M)
        .map_err(|e| e.to_string())?
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .build();
    Ok(NewPhone { link, qr, view: view(&state) })
}

#[tauri::command]
pub fn remove_phone(state: AppStateRef, id: String) -> Result<PhoneView, String> {
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        s.phones.retain(|p| p.id != id);
        db::set(&conn, KEY, &s)?;
    }
    state.log("settings", "Removed a phone from the phone page");
    Ok(view(&state))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A phone (Node with the page's TweetNaCl) against the real server.
    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn e2e_phone_page() {
        let state = crate::e2e::temp_state(None);
        let c = state.cipher().unwrap();
        let key: [u8; 32] = crate::sync::random_bytes();
        {
            let conn = state.db.lock().unwrap();
            let p = Phone { id: "ph1".into(), name: "Test phone".into(), key: c.encrypt(&hex::encode(key)), added: 1, last_seen: None, last_ts: 0 };
            db::set(&conn, KEY, &Settings { enabled: true, phones: vec![p] }).unwrap();
            notes::save_note(&conn, &c, None, notes::NoteBody { title: "Groceries".into(), body: "eggs, milk".into(), folder: String::new(), tags: vec![] }).unwrap();
        }
        apply(state.clone());
        let mut port = None;
        for _ in 0..50 {
            port = SERVER.lock().unwrap().as_ref().map(|r| r.port);
            if port.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let port = port.expect("server up");
        // The page is served with TweetNaCl inlined.
        let page = reqwest::get(format!("http://127.0.0.1:{port}/")).await.unwrap().text().await.unwrap();
        assert!(page.contains("nacl.secretbox") && page.contains("secretbox.open") && !page.contains("/*NACL*/"));

        let dir = std::env::temp_dir().join(format!("sulcus-phone-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("nacl.js"), NACL).unwrap();
        let script = r#"
const nacl = require("./nacl.js");
const [url, phone, keyB64] = process.argv.slice(2);
const key = Uint8Array.from(Buffer.from(keyB64, "base64"));
let last = 0;
async function raw(op, args, ts, k = key) {
  const nonce = nacl.randomBytes(24);
  const box = nacl.secretbox(new TextEncoder().encode(JSON.stringify({ op, args, ts })), nonce, k);
  const r = await fetch(url + "/api", { method: "POST", headers: { "X-Phone": phone }, body: Buffer.concat([Buffer.from(nonce), Buffer.from(box)]).toString("base64") });
  const text = await r.text();
  if (!r.ok) return { status: r.status, text };
  const b = Buffer.from(text, "base64");
  const plain = nacl.secretbox.open(new Uint8Array(b.subarray(24)), new Uint8Array(b.subarray(0, 24)), k);
  return JSON.parse(new TextDecoder().decode(plain));
}
const call = (op, args = {}) => raw(op, args, (last = Math.max(Date.now(), last + 1)));
(async () => {
  const out = {};
  out.hello = await call("hello");
  out.added = await call("add_task", { title: "Call Sam tomorrow" });
  out.tasks = await call("tasks");
  out.done = await call("done_task", { id: out.added.data.id });
  out.after = await call("tasks");
  out.notes = await call("notes");
  out.chats = await call("chats");
  out.unknown = await call("approve", { call_id: "nope", decision: "allow" });
  out.replay = await raw("hello", {}, last);
  out.wrongKey = await raw("hello", {}, Date.now() + 5, new Uint8Array(32));
  console.log(JSON.stringify(out));
})();
"#;
        std::fs::write(dir.join("client.js"), script).unwrap();
        let out = tokio::process::Command::new("node")
            .current_dir(&dir)
            .args(["client.js", &format!("http://127.0.0.1:{port}"), "ph1", &B64.encode(key)])
            .output()
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        eprintln!("{text}\n{}", String::from_utf8_lossy(&out.stderr));
        let v: Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(v["hello"]["ok"], true);
        assert_eq!(v["hello"]["data"]["phone"], "Test phone");
        assert_eq!(v["added"]["data"]["title"], "Call Sam");
        assert!(v["added"]["data"]["due"].is_number(), "\"tomorrow\" became a due date");
        assert_eq!(v["tasks"]["data"].as_array().unwrap().len(), 1);
        assert_eq!(v["after"]["data"].as_array().unwrap().len(), 0, "done tasks drop off");
        assert_eq!(v["notes"]["data"][0]["title"], "Groceries");
        assert_eq!(v["unknown"]["ok"], false);
        assert_eq!(v["replay"]["status"], 409);
        assert_eq!(v["wrongKey"]["status"], 401);
        assert!(settings(&state.db.lock().unwrap()).phones[0].last_seen.is_some());
        std::fs::remove_dir_all(&dir).ok();
        SERVER.lock().unwrap().take().map(|r| r.stop.send(()));
    }

    /// Serves the page with sample data for two minutes, to look at it in a
    /// browser: prints the link.
    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn demo_phone_page() {
        let state = crate::e2e::temp_state(None);
        let c = state.cipher().unwrap();
        let key: [u8; 32] = crate::sync::random_bytes();
        {
            let conn = state.db.lock().unwrap();
            let p = Phone { id: "demo".into(), name: "Demo phone".into(), key: c.encrypt(&hex::encode(key)), added: 1, last_seen: None, last_ts: 0 };
            db::set(&conn, KEY, &Settings { enabled: true, phones: vec![p] }).unwrap();
            notes::save_note(&conn, &c, None, notes::NoteBody { title: "Groceries".into(), body: "eggs, milk, **coffee**".into(), folder: String::new(), tags: vec![] }).unwrap();
            notes::save_task(&conn, &c, notes::new_task("Call Sam")).unwrap();
            let chat = db::create_chat(&conn, &c, None).unwrap();
            db::set_chat_title(&conn, &c, &chat.id, "Trip to Italy").unwrap();
            for (role, text) in [("user", "Plan three days in Rome"), ("assistant", "**Day 1:** the Colosseum and the Forum.

**Day 2:** the Vatican.

```
Day 3: Trastevere
```")] {
                db::add_message(&conn, &c, &db::Message { id: uuid::Uuid::new_v4().to_string(), chat_id: chat.id.clone(), role: role.into(), content: text.into(), created_at: db::now_ms(), ..Default::default() }).unwrap();
            }
        }
        apply(state.clone());
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let port = SERVER.lock().unwrap().as_ref().map(|r| r.port).unwrap();
        println!("LINK http://127.0.0.1:{port}/#p=demo.{}", B64URL.encode(key));
        tokio::time::sleep(std::time::Duration::from_secs(150)).await;
    }

    #[test]
    fn quick_tasks_keep_the_date_apart() {
        let tue = chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap();
        let (t, d) = split_due("Call Sam tomorrow 3pm", tue);
        assert_eq!(t, "Call Sam");
        let (date, time) = d.unwrap();
        assert_eq!(date, chrono::NaiveDate::from_ymd_opt(2026, 10, 7).unwrap());
        assert!(time.is_some());
        assert_eq!(split_due("Buy milk", tue), ("Buy milk".to_string(), None));
        assert_eq!(split_due("tomorrow", tue).0, "tomorrow", "a lone date stays the title");
    }

    #[test]
    fn sealing_matches_tweetnacl_secretbox() {
        // nacl.secretbox(utf8("hello"), nonce = 24 × 1, key = 32 × 2), from tweetnacl-js 1.0.3.
        let key = [2u8; 32];
        let nonce = [1u8; 24];
        let ct = XSalsa20Poly1305::new((&key).into()).encrypt(Nonce::from_slice(&nonce), b"hello".as_slice()).unwrap();
        let sealed = B64.encode([nonce.as_slice(), &ct].concat());
        assert_eq!(sealed, "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBcofRSI69/FovMXOb2VNxu0qyaEg4");
        assert_eq!(open(&key, &sealed).unwrap(), b"hello");
        assert!(open(&[3u8; 32], &sealed).is_none());
        let again = seal(&key, b"hi there");
        assert_eq!(open(&key, &again).unwrap(), b"hi there");
    }
}
