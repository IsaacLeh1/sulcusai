// SPDX-License-Identifier: AGPL-3.0-only
//! The whole app on a paired phone: the phone shows the same window and its
//! commands run here, as if this PC's window had asked (with the same
//! permission checks), and the events it listens to are kept for it.
//!
//! - `invoke`: one command. A few stay on the PC (`DENIED`): keys and
//!   passwords shown, phones added or removed, security settings, and things
//!   that only open on this PC's screen. One that takes longer than `QUICK`
//!   (a chat reply, a translation) gives a ticket, and the phone asks for its
//!   result until it's done, so no request waits long on the relay.
//! - Events: the phone says which it listens to; each is kept for it until
//!   it asks (a long-poll), the newest `KEEP` at most.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponse, InvokeResponseBody};
use tauri::webview::InvokeRequest;
use tauri::{AppHandle, Listener, Manager};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// A command that takes longer than this answers with a ticket.
const QUICK: Duration = Duration::from_secs(15);
/// How long one ask for events or a result waits (under the relay's 35 s).
pub const WAIT: Duration = Duration::from_secs(20);
/// Events kept for a phone that hasn't asked yet.
const KEEP: usize = 800;
/// A finished result nobody came for is let go after this.
const TICKET_TTL: Duration = Duration::from_secs(600);
/// The biggest file sent to a phone in one answer.
const MAX_RAW: usize = 60 * 1024 * 1024;

/// Commands a phone can't run, and why.
const DENIED: &[(&str, &str)] = &[
    // Phones themselves.
    ("add_phone", "Add phones on the PC."),
    ("remove_phone", "Remove phones on the PC."),
    ("set_phone", "Change the phone settings on the PC."),
    ("set_phone_while_locked", "Change the phone settings on the PC."),
    ("set_phone_link", "Change the phone settings on the PC."),
    // Locks, passwords and keys.
    ("unlock", "Unlock SulcusAI on the PC."),
    ("unlock_with_hello", "Unlock SulcusAI on the PC."),
    ("enable_lock", "Change the lock on the PC."),
    ("disable_lock", "Change the lock on the PC."),
    ("change_pin", "Change the PIN on the PC."),
    ("reset_pin", "Change the PIN on the PC."),
    ("set_auto_lock", "Change the lock on the PC."),
    ("set_keep_working", "Change the lock on the PC."),
    ("set_hello", "Change Windows Hello on the PC."),
    ("set_advanced_pin", "Change the PIN on the PC."),
    ("api_server_view", "The local API key shows on the PC only."),
    ("new_api_key", "Make a new API key on the PC."),
    ("set_api_server", "Change the local API server on the PC."),
    ("site_keys", "Keys for sites show on the PC only."),
    ("site_key_give", "Give keys for sites on the PC."),
    ("oauth_clients", "Sign-in settings show on the PC only."),
    ("set_oauth_clients", "Change sign-in settings on the PC."),
    ("set_relay", "Change the sync relay on the PC."),
    ("set_dev_mode", "Change developer mode on the PC."),
    ("set_desktop_access", "Change what the assistant may do on the desktop on the PC."),
    ("set_fully_offline", "Change Fully offline on the PC (it would cut this phone off)."),
    ("restore_backup", "Restore a backup on the PC."),
    ("make_backup", "Make a backup on the PC."),
    ("install_plugin", "Add plugins on the PC."),
    ("people_allow_firewall", "Change the firewall on the PC."),
    // Pairing other devices.
    ("pair_device", "Pair PCs on the PC."),
    ("start_pairing", "Pair PCs on the PC."),
    ("remove_device", "Remove PCs on the PC."),
    // Sign-ins that open the PC's browser.
    ("add_mail_account_oauth", "Sign in on the PC: it opens the PC's browser."),
    ("add_calendar_account_oauth", "Sign in on the PC: it opens the PC's browser."),
    // Only on the PC's screen.
    ("choose_folder", "Choose folders on the PC."),
    ("open_app_folder", "That opens on the PC."),
    ("show_in_folder", "That opens on the PC."),
    ("show_extension_folder", "That opens on the PC."),
    ("open_document", "That opens on the PC."),
    ("open_browser", "That opens on the PC."),
    ("open_web", "That opens on the PC."),
    ("preview_open_external", "That opens on the PC."),
    ("quick_clipboard", "That works on the PC only."),
    ("quick_screenshot", "That works on the PC only."),
    ("quick_hide", "That works on the PC only."),
    ("quick_resize", "That works on the PC only."),
    ("quick_open_in_app", "That works on the PC only."),
];

/// Why a phone can't run `cmd` (None: it can).
pub fn denied(cmd: &str) -> Option<&'static str> {
    if cmd.starts_with("plugin:") {
        return Some("That works on the PC only.");
    }
    DENIED.iter().find(|(c, _)| *c == cmd).map(|(_, why)| *why)
}

/// What an error from a command says.
fn error_text(v: Value) -> String {
    match v {
        Value::String(s) => s,
        Value::Null => "That didn't work.".into(),
        other => other.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| other.to_string()),
    }
}

/// Runs `cmd` the way the PC's window would.
async fn dispatch(app: AppHandle, cmd: String, args: Value) -> Result<Value, String> {
    let window = app.get_webview_window("main").ok_or("SulcusAI's window isn't open on the PC.")?;
    let webview: &tauri::Webview = window.as_ref();
    let webview = webview.clone();
    let url = webview.url().map_err(|e| e.to_string())?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let request = InvokeRequest {
        cmd,
        callback: CallbackFn(0),
        error: CallbackFn(1),
        url,
        body: InvokeBody::Json(if args.is_null() { json!({}) } else { args }),
        headers: Default::default(),
        invoke_key: app.invoke_key().to_string(),
    };
    webview.on_message(
        request,
        Box::new(move |_webview, _cmd, response, _ok, _err| {
            let _ = tx.send(response);
        }),
    );
    match rx.await.map_err(|_| "The command didn't answer.".to_string())? {
        InvokeResponse::Ok(InvokeResponseBody::Json(s)) => Ok(serde_json::from_str(&s).unwrap_or(Value::Null)),
        InvokeResponse::Ok(InvokeResponseBody::Raw(bytes)) => {
            if bytes.len() > MAX_RAW {
                return Err("That file is too big to open on the phone.".into());
            }
            Ok(json!({ "__raw": B64.encode(bytes) }))
        }
        InvokeResponse::Err(e) => Err(error_text(e.0)),
    }
}

/// The phone's own rules for a command it may run: approvals are answered
/// one call at a time, and a system folder is allowed on the PC.
fn phone_rules(cmd: &str, args: &Value) -> Result<(), String> {
    if cmd == "answer_approval" {
        match args["decision"].as_str() {
            Some("deny") => {}
            Some("allow") => crate::workspace::phone_may_allow(args["callId"].as_str().or(args["call_id"].as_str()).unwrap_or_default())?,
            _ => return Err("From the phone, answer Allow or Don't allow; Allow for this chat works on the PC.".into()),
        }
    }
    Ok(())
}

struct Ticket {
    phone: String,
    task: JoinHandle<Result<Value, String>>,
    made: Instant,
}

static TICKETS: LazyLock<Mutex<HashMap<String, Ticket>>> = LazyLock::new(Default::default);

/// What the phone gets back: the answer, or a ticket to ask for it again.
fn settled(r: Result<Value, String>) -> Result<Value, String> {
    r.map(|data| json!({ "done": true, "data": data }))
}

/// Runs a command for a phone.
pub async fn invoke(app: AppHandle, phone: &str, cmd: &str, args: Value) -> Result<Value, String> {
    if let Some(why) = denied(cmd) {
        return Err(why.into());
    }
    phone_rules(cmd, &args)?;
    let mut task = tokio::spawn(dispatch(app, cmd.to_string(), args));
    match tokio::time::timeout(QUICK, &mut task).await {
        Ok(joined) => settled(joined.map_err(|e| e.to_string())?),
        Err(_) => {
            let ticket = hex::encode(crate::sync::random_bytes::<12>());
            let mut all = TICKETS.lock().unwrap();
            all.retain(|_, t| t.made.elapsed() < TICKET_TTL);
            all.insert(ticket.clone(), Ticket { phone: phone.to_string(), task, made: Instant::now() });
            Ok(json!({ "done": false, "ticket": ticket }))
        }
    }
}

/// The result of a command that gave a ticket, waiting up to `WAIT` for it.
pub async fn result(phone: &str, ticket: &str) -> Result<Value, String> {
    let taken = {
        let mut all = TICKETS.lock().unwrap();
        match all.get(ticket) {
            Some(t) if t.phone == phone => all.remove(ticket),
            _ => None,
        }
    };
    let Some(mut t) = taken else { return Err("That request is gone (the PC may have restarted).".into()) };
    match tokio::time::timeout(WAIT, &mut t.task).await {
        Ok(joined) => settled(joined.map_err(|e| e.to_string())?),
        Err(_) => {
            TICKETS.lock().unwrap().insert(ticket.to_string(), t);
            Ok(json!({ "done": false, "ticket": ticket }))
        }
    }
}

struct Queue {
    names: HashSet<String>,
    events: VecDeque<(u64, String, Value)>,
}

static QUEUES: LazyLock<Mutex<HashMap<String, Queue>>> = LazyLock::new(Default::default);
/// Event names this app already listens to for phones.
static HOOKED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
static NEXT: AtomicU64 = AtomicU64::new(1);
static NEWS: LazyLock<Notify> = LazyLock::new(Notify::new);

/// An event name the app uses ("chat:delta", "install:progress").
fn good_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-' | '/'))
}

/// Starts keeping `name` events for a phone.
pub fn listen(app: &AppHandle, phone: &str, name: &str) -> Result<(), String> {
    if !good_name(name) {
        return Err("Unknown event.".into());
    }
    QUEUES.lock().unwrap().entry(phone.to_string()).or_insert_with(|| Queue { names: HashSet::new(), events: VecDeque::new() }).names.insert(name.to_string());
    if HOOKED.lock().unwrap().insert(name.to_string()) {
        let event = name.to_string();
        app.listen_any(name, move |e| push(&event, e.payload()));
    }
    Ok(())
}

pub fn unlisten(phone: &str, name: &str) {
    if let Some(q) = QUEUES.lock().unwrap().get_mut(phone) {
        q.names.remove(name);
    }
}

/// Lets go of everything kept for a phone (it signed out or was removed).
pub fn forget(phone: &str) {
    QUEUES.lock().unwrap().remove(phone);
    TICKETS.lock().unwrap().retain(|_, t| t.phone != phone);
}

fn push(name: &str, payload: &str) {
    let value: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
    let seq = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut any = false;
    for q in QUEUES.lock().unwrap().values_mut() {
        if q.names.contains(name) {
            q.events.push_back((seq, name.to_string(), value.clone()));
            while q.events.len() > KEEP {
                q.events.pop_front();
            }
            any = true;
        }
    }
    if any {
        NEWS.notify_waiters();
    }
}

/// The phone's events after `after`, waiting up to `wait` for one. `cursor`
/// is what to ask after next time; after a restart here it starts again.
pub async fn events(phone: &str, after: u64, wait: Duration) -> Value {
    let deadline = Instant::now() + wait;
    loop {
        let news = NEWS.notified();
        tokio::pin!(news);
        news.as_mut().enable();
        let now_seq = NEXT.load(Ordering::Relaxed) - 1;
        let after = if after > now_seq { 0 } else { after };
        // How many events it listens to: none after a restart here, so the
        // phone says them again.
        let (found, listening): (Vec<Value>, usize) = {
            let mut all = QUEUES.lock().unwrap();
            let q = all.entry(phone.to_string()).or_insert_with(|| Queue { names: HashSet::new(), events: VecDeque::new() });
            q.events.retain(|(seq, _, _)| *seq > after);
            (q.events.iter().map(|(seq, name, payload)| json!({ "seq": seq, "event": name, "payload": payload })).collect(), q.names.len())
        };
        if !found.is_empty() {
            let cursor = found.last().and_then(|e| e["seq"].as_u64()).unwrap_or(now_seq);
            return json!({ "events": found, "cursor": cursor, "listening": listening });
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if listening == 0 || left.is_zero() || tokio::time::timeout(left, news).await.is_err() {
            return json!({ "events": [], "cursor": NEXT.load(Ordering::Relaxed) - 1, "listening": listening });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_phones_and_the_pcs_own_screen_stay_on_the_pc() {
        for cmd in ["add_phone", "unlock", "new_api_key", "site_keys", "plugin:dialog|open", "quick_screenshot", "set_fully_offline"] {
            assert!(denied(cmd).is_some(), "{cmd}");
        }
        for cmd in ["list_chats", "send_message", "media_start", "set_default_model", "media_file", "stop_generation"] {
            assert!(denied(cmd).is_none(), "{cmd}");
        }
    }

    #[test]
    fn a_phone_answers_one_call_at_a_time() {
        assert!(phone_rules("answer_approval", &json!({ "callId": "x", "decision": "always" })).is_err());
        assert!(phone_rules("answer_approval", &json!({ "callId": "x", "decision": "deny" })).is_ok());
        assert!(phone_rules("answer_approval", &json!({ "callId": "x", "decision": "allow" })).is_ok());
        assert!(phone_rules("list_chats", &json!({})).is_ok());
    }

    #[tokio::test]
    async fn events_wait_for_news_and_keep_their_order() {
        let phone = "test-events-phone";
        QUEUES.lock().unwrap().insert(phone.into(), Queue { names: ["x:one".to_string()].into(), events: VecDeque::new() });
        let empty = events(phone, 0, Duration::from_millis(50)).await;
        assert_eq!(empty["events"].as_array().unwrap().len(), 0);
        let cursor = empty["cursor"].as_u64().unwrap();
        let waiter = tokio::spawn(async move { events(phone, cursor, Duration::from_secs(5)).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        push("x:one", r#"{"n":1}"#);
        push("x:other", r#"{"n":2}"#);
        push("x:one", r#"{"n":3}"#);
        let got = waiter.await.unwrap();
        let list = got["events"].as_array().unwrap();
        assert!(!list.is_empty());
        assert_eq!(list[0]["payload"]["n"], 1);
        let rest = events(phone, got["cursor"].as_u64().unwrap(), Duration::from_millis(50)).await;
        let all: Vec<i64> = list.iter().chain(rest["events"].as_array().unwrap()).map(|e| e["payload"]["n"].as_i64().unwrap()).collect();
        assert_eq!(all, vec![1, 3], "only what it listens to, in order");
        forget(phone);
    }
}
