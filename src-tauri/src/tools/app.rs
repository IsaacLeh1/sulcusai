// SPDX-License-Identifier: AGPL-3.0-only
//! app_status, connect_account and open_page: the assistant knowing SulcusAI
//! itself, what's connected, and starting a sign-in when the user wants one.

use serde_json::{json, Value};
use tauri::Emitter;

use super::{Outcome, Preview};
use crate::crypto::Cipher;
use crate::oauth::Provider;
use crate::AppState;

pub fn app_status_params() -> Value {
    json!({ "type": "object", "properties": {} })
}

pub fn connect_account_params() -> Value {
    json!({ "type": "object", "required": ["provider", "what"], "properties": {
        "provider": { "type": "string", "enum": ["google", "microsoft"], "description": "google for Gmail and Google Calendar; microsoft for Outlook.com, Hotmail and Microsoft 365." },
        "what": { "type": "string", "enum": ["mail", "calendar", "both"] } } })
}

pub fn open_page_params() -> Value {
    json!({ "type": "object", "required": ["page"], "properties": {
        "page": { "type": "string", "enum": PAGES } } })
}

const PAGES: [&str; 13] = ["mail", "calendar", "notes", "tasks", "translate", "meetings", "studio", "memory", "scheduled", "connectors", "models", "features", "settings"];

/// What SulcusAI is and what's set up, for the system prompt (kept short:
/// small models have little room).
pub fn note(state: &AppState, c: &Cipher) -> String {
    let (mail, cal, level) = {
        let conn = state.db.lock().unwrap();
        let mail: Vec<String> = crate::mail::accounts(&conn, c).into_iter().map(|(_, a, _)| a.email).collect();
        let cal: Vec<String> = crate::calendar::accounts(&conn, c).into_iter().map(|(_, a, _)| a.name).collect();
        (mail, cal, crate::db::settings(&conn).connectivity)
    };
    let mail = if mail.is_empty() { "none connected".to_string() } else { format!("{} (you have access: search and read it with email_search and email_read)", mail.join(", ")) };
    let cal = if cal.is_empty() { "none connected (events saved on this PC still work)".to_string() } else { format!("{} (check it with calendar_events)", cal.join(", ")) };
    format!(
        "\n\n## SulcusAI\nYou are the assistant inside SulcusAI, a desktop app. Its pages: Chat, Notes, Tasks, Mail, Calendar, Translate, \
         Meetings, Studio (pictures, video, music), and Settings, Models and Features in the ⚙ menu. Internet mode: {}. \
         Email: {mail}. Calendars: {cal}. Only if the user asks to connect an email or calendar account, use connect_account \
         (Google or Microsoft sign-in; for other providers, open_page). Answer questions from what's already here first. \
         Look things up with the tool that fits: events and plans: calendar_events; email: email_search (put who it is from in the query); notes: search_notes; tasks: list_tasks; connecting Gmail, Outlook or a calendar: connect_account.",
        level_name(level)
    )
}

fn level_name(level: crate::net::Connectivity) -> &'static str {
    match level {
        crate::net::Connectivity::Offline => "Offline (email and calendar can't sync; the user switches to Local AI + Web in the status bar)",
        crate::net::Connectivity::Web => "Local AI + Web",
        _ => "Cloud",
    }
}

fn provider(args: &Value) -> Result<Provider, String> {
    match args.get("provider").and_then(Value::as_str).unwrap_or("").to_lowercase().as_str() {
        "google" | "gmail" => Ok(Provider::Google),
        "microsoft" | "outlook" | "hotmail" => Ok(Provider::Microsoft),
        other => Err(format!("Unknown provider “{other}”: use google or microsoft.")),
    }
}

fn wants(args: &Value) -> (bool, bool) {
    match args.get("what").and_then(Value::as_str).unwrap_or("both") {
        "mail" | "email" => (true, false),
        "calendar" => (false, true),
        _ => (true, true),
    }
}

/// The approval card for a sign-in.
pub fn preview(args: &Value) -> Result<Preview, String> {
    let p = provider(args)?;
    let (mail, cal) = wants(args);
    let what = match (mail, cal) {
        (true, true) => "email and calendar",
        (true, false) => "email",
        _ => "calendar",
    };
    Ok(Preview {
        title: format!("Connect {what} with {} sign-in", p.name()),
        kind: "text",
        detail: Some(format!("{}'s sign-in page opens in your browser. SulcusAI never sees your password.", p.name())),
        note: None,
    })
}

pub async fn run(name: &str, args: &Value, state: &std::sync::Arc<AppState>, c: &Cipher) -> Outcome {
    match name {
        "app_status" => {
            let conn = state.db.lock().unwrap();
            let mail: Vec<String> = crate::mail::accounts(&conn, c)
                .into_iter()
                .map(|(_, a, synced)| format!("{}{}", a.email, if synced.is_some() { "" } else { " (not synced yet)" }))
                .collect();
            let cal: Vec<String> = crate::calendar::accounts(&conn, c).into_iter().map(|(_, a, _)| format!("{} ({} calendars)", a.name, a.calendars.len())).collect();
            let messages = crate::mail::message_count(&conn);
            let features: Vec<String> = crate::features::enabled(&conn).iter().map(|f| format!("{f:?}")).collect();
            let level = crate::db::settings(&conn).connectivity;
            let text = format!(
                "Email accounts: {}\nCopied emails on this PC: {messages}\nCalendar accounts: {}\nInternet mode: {}\nFeatures on: {}",
                if mail.is_empty() { "none".into() } else { mail.join(", ") },
                if cal.is_empty() { "none (events on this PC still work)".into() } else { cal.join(", ") },
                level_name(level),
                features.join(", ")
            );
            Outcome::ok(text.clone(), "Checked what's set up", "text", Some(text))
        }
        "open_page" => {
            let page = args.get("page").and_then(Value::as_str).unwrap_or("").to_lowercase();
            if !PAGES.contains(&page.as_str()) {
                return Outcome::error("Couldn't open that page", format!("There's no page called {page}. Pages: {}.", PAGES.join(", ")));
            }
            if let Some(app) = state.app.get() {
                app.emit("app:navigate", json!({ "page": page })).ok();
            }
            Outcome::ok(format!("Opened the {page} page for the user."), format!("Opened {page}"), "text", None)
        }
        "connect_account" => {
            let p = match provider(args) {
                Ok(p) => p,
                Err(e) => return Outcome::error("Couldn't connect an account", e),
            };
            let (mail, cal) = wants(args);
            let mut done = Vec::new();
            let mut problems = Vec::new();
            if mail {
                match crate::mail::connect_oauth(state, p, None, None).await {
                    Ok((_, email)) => done.push(format!("email {email}")),
                    Err(e) => problems.push(format!("email: {e}")),
                }
            }
            if cal {
                match crate::calendar::connect_oauth(state, p, None).await {
                    Ok((_, name)) => done.push(format!("calendar {name}")),
                    Err(e) => problems.push(format!("calendar: {e}")),
                }
            }
            if done.is_empty() {
                return Outcome::error(format!("Couldn't connect with {}", p.name()), problems.join("; "));
            }
            let mut text = format!("Connected {}.", done.join(" and "));
            if !problems.is_empty() {
                text.push_str(&format!(" Not connected: {}.", problems.join("; ")));
            }
            Outcome::ok(text.clone(), format!("Connected {}", done.join(" and ")), "text", Some(text))
        }
        _ => Outcome::error("Unknown tool", name.to_string()),
    }
}
