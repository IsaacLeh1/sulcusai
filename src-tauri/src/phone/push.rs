// SPDX-License-Identifier: AGPL-3.0-only
//! Notifications on paired phones (webpush.rs does the sealing): what the
//! phone page asks for (the PC's public key, subscribing, unsubscribing)
//! and the sending, for Social messages and calls, finished work and
//! reminders.
//!
//! - Off until a phone turns notifications on from its own page; each phone
//!   chooses for itself. Only paired phones subscribe: the request arrives
//!   sealed with the phone's key like every other.
//! - What a notification says is sealed for that phone's browser; the push
//!   service (Apple, Google, Mozilla) relays bytes it can't read.
//! - "Reply" on a notification opens the page at that conversation with
//!   the box ready (`url` carries `reply=`); the answer goes through the
//!   phone's normal sealed request, never through the push service.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::webpush::{self, BrowserSubscription, HttpTransport, Notification, PushStore, SendOptions};
use crate::webpush_p256::P256;
use crate::{db, AppState};

const KEY: &str = "phone_push";
/// A contact the push services can reach about this sender (RFC 8292).
const SUBJECT: &str = "https://github.com/IsaacLeh1/sulcusai";

fn load(conn: &rusqlite::Connection) -> PushStore {
    db::get::<String>(conn, KEY).map(|t| PushStore::from_json(&t)).unwrap_or_default()
}

fn save(conn: &rusqlite::Connection, s: &PushStore) {
    let _ = db::set(conn, KEY, &s.to_json());
}

/// The phone page's push requests, by command name. `None`: not one of them.
pub fn handle(state: &AppState, phone: &str, cmd: &str, args: &Value) -> Option<Result<Value, String>> {
    let r = match cmd {
        "push_public_key" => (|| {
            let c = state.cipher()?;
            let conn = state.db.lock().unwrap();
            let mut s = load(&conn);
            let v = s.vapid(&P256, &c, SUBJECT)?;
            save(&conn, &s);
            Ok(json!(v.public_b64()))
        })(),
        "push_subscribe" => (|| {
            let sub: BrowserSubscription = serde_json::from_value(args["subscription"].clone()).map_err(|_| "That isn't a push subscription.".to_string())?;
            let conn = state.db.lock().unwrap();
            let mut s = load(&conn);
            s.subscribe(phone, &sub, db::now_ms())?;
            save(&conn, &s);
            db::log_action(&conn, "phone", "A phone turned on notifications");
            Ok(Value::Null)
        })(),
        "push_unsubscribe" => {
            let conn = state.db.lock().unwrap();
            let mut s = load(&conn);
            s.unsubscribe(phone, args["endpoint"].as_str().unwrap_or_default());
            save(&conn, &s);
            Ok(Value::Null)
        }
        _ => return None,
    };
    Some(r)
}

/// For a phone that was removed.
pub fn forget(state: &AppState, phone: &str) {
    let conn = state.db.lock().unwrap();
    let mut s = load(&conn);
    s.forget_phone(phone);
    save(&conn, &s);
}

/// What kind of notification, for how urgently the push service delivers it.
#[derive(Clone, Copy)]
pub enum Kind {
    /// A call or someone naming the user: delivered at once.
    Urgent,
    /// A message, finished work, a reminder.
    Normal,
}

/// Sends to every phone that turned notifications on. Quiet when none did,
/// when the app is fully offline, or when privacy mode keeps the web shut.
pub fn send(state: &Arc<AppState>, kind: Kind, note: Notification) {
    if crate::offline::check(crate::offline::Keep::LocalNetwork).is_err() {
        return;
    }
    let targets = {
        let conn = state.db.lock().unwrap();
        let mut s = load(&conn);
        let t = s.targets(None, db::now_ms());
        save(&conn, &s);
        t
    };
    if targets.is_empty() {
        return;
    }
    let state = state.clone();
    tauri::async_runtime::spawn(async move {
        let Ok(c) = state.cipher() else { return };
        let vapid = {
            let conn = state.db.lock().unwrap();
            let mut s = load(&conn);
            let v = s.vapid(&P256, &c, SUBJECT);
            save(&conn, &s);
            match v {
                Ok(v) => v,
                Err(_) => return,
            }
        };
        let Ok(client) = crate::net::external_client(state.settings().connectivity, crate::net::Purpose::Web, false) else { return };
        let opts = match kind {
            Kind::Urgent => SendOptions { ttl_secs: 60, urgency: "high" },
            Kind::Normal => SendOptions::default(),
        };
        let mut bytes = note.to_bytes();
        if bytes.len() > webpush::MAX_PAYLOAD {
            let mut short = note.clone();
            short.body = short.body.chars().take(200).collect();
            bytes = short.to_bytes();
        }
        let now = db::now_ms() / 1000;
        let done = webpush::deliver(&P256, &HttpTransport(client), &vapid, &targets, &bytes, &opts, now).await;
        let conn = state.db.lock().unwrap();
        let mut s = load(&conn);
        if s.record(&done) > 0 {
            save(&conn, &s);
        }
    });
}

/// A Social message for the phone: tapping opens the channel, Reply opens
/// it with the box ready.
pub fn social_message(state: &Arc<AppState>, group: &str, channel: &str, from: &str, text: &str, mentions_me: bool) {
    let body: String = text.chars().take(300).collect();
    send(
        state,
        if mentions_me { Kind::Urgent } else { Kind::Normal },
        Notification { title: from.to_string(), body, url: Some(format!("./#/social/{group}/{channel}?reply=1")), tag: Some(format!("social-{group}-{channel}")) },
    );
}

/// Someone is calling, or a voice channel the user is in got busy.
pub fn social_call(state: &Arc<AppState>, group: &str, channel: &str, who: &str) {
    send(state, Kind::Urgent, Notification { title: format!("{who} is calling"), body: "Tap to answer on this phone.".into(), url: Some(format!("./#/calls/{group}/{channel}")), tag: Some(format!("call-{group}")) });
}
