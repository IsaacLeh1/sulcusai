// SPDX-License-Identifier: AGPL-3.0-only
//! The Bug / Feedback button: what the person writes goes to the
//! developer's inbox on the project's relay (relay/worker.js, POST
//! /feedback). Only that text goes, plus a reply address and the version
//! number if they choose to add them: no name, account, chats, files or
//! anything about the PC. Crash reports they choose to send go the same way.

use std::time::Duration;

use serde_json::json;

const INBOX: &str = "https://sulcusai-relay.isaacleh1.workers.dev/feedback";

/// Sends one message to the inbox. `kind`: "bug", "idea" or "crash".
pub async fn send(kind: &str, text: &str, contact: Option<&str>, version: bool) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Write something first.".into());
    }
    if text.chars().count() > 8000 {
        return Err("That's too long to send (8,000 characters at most).".into());
    }
    // It's a user's own action, so it goes at every level, but not while
    // fully offline (unless the web is kept).
    crate::offline::check(crate::offline::Keep::Web)?;
    let b = reqwest::Client::builder().connect_timeout(Duration::from_secs(20)).timeout(Duration::from_secs(40));
    let client = crate::net::routed(b, crate::net::Route::Web)?.redirect(reqwest::redirect::Policy::none()).build().map_err(|e| e.to_string())?;
    let body = json!({
        "kind": kind,
        "text": text,
        "contact": contact.map(str::trim).filter(|c| !c.is_empty()),
        "version": version.then_some(env!("CARGO_PKG_VERSION")),
    });
    let r = client.post(INBOX).json(&body).send().await.map_err(|_| "Couldn't reach the inbox. Check the internet connection and try again.".to_string())?;
    match r.status().as_u16() {
        200 => Ok(()),
        429 => Err("Thanks, but that's a lot of messages for one hour. Try again later.".into()),
        s => Err(format!("The inbox didn't take it (error {s}). Try again later.")),
    }
}

#[tauri::command]
pub async fn send_feedback(kind: String, text: String, contact: Option<String>, version: bool) -> Result<(), String> {
    let kind = if kind == "bug" { "bug" } else { "idea" };
    send(kind, &text, contact.as_deref(), version).await
}
