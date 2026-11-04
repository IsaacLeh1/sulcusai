// SPDX-License-Identifier: AGPL-3.0-only
//! Local or cloud, per message, set per assistant or project (off unless
//! turned on there): messages with personal details, private words or
//! files stay on a model on this PC; long or hard ones may go to the cloud
//! model picked. The chat shows which way each message went and why.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::crypto::Cipher;
use crate::db;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Route {
    pub on: bool,
    /// A model on this PC for private messages (empty: the chat's own, if local).
    pub local_model: Option<String>,
    /// The cloud model for long or hard messages (empty: never the cloud).
    pub cloud_model: Option<String>,
    /// Characters past which a message counts as long (0: 6,000).
    pub long_chars: usize,
    /// Words that mark a message as private, on top of the built-in ones.
    pub private_words: Vec<String>,
    /// Words that mark a message as hard, on top of the built-in ones.
    pub hard_words: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Why {
    Private,
    Files,
    Long,
    Hard,
}

impl Why {
    pub fn word(self) -> &'static str {
        match self {
            Why::Private => "private",
            Why::Files => "files",
            Why::Long => "long",
            Why::Hard => "hard",
        }
    }
}

const PRIVATE: &[&str] = &["confidential", "salary", "diagnosis", "medical", "password", "passport", "social security", "bank account", "lawsuit", "payroll", "patient", "do not share", "internal only", "nda"];
const HARD: &[&str] = &["prove", "step by step", "analyze", "analyse", "refactor", "architecture", "debug", "optimize", "derive", "in depth", "research"];
const DOCS: &[&str] = &[".pdf", ".docx", ".doc", ".xlsx", ".xls", ".csv", ".pptx", ".txt", ".md", ".json", ".eml", ".msg"];

fn key(scope: &str, id: &str) -> Result<String, String> {
    match scope {
        "assistant" | "project" => Ok(format!("cloud_route:{scope}:{id}")),
        _ => Err("Routes are set per assistant or project.".into()),
    }
}

pub fn get(conn: &Connection, scope: &str, id: &str) -> Route {
    key(scope, id).ok().and_then(|k| db::get(conn, &k)).unwrap_or_default()
}

/// The route for a chat: its assistant's if that one's on, else its project's.
pub fn for_chat(conn: &Connection, c: &Cipher, chat_id: &str, project: Option<&str>) -> Option<Route> {
    let by_assistant = crate::assistants::of_chat(conn, c, chat_id).map(|a| get(conn, "assistant", &a.id)).filter(|r| r.on);
    by_assistant.or_else(|| project.map(|p| get(conn, "project", p)).filter(|r| r.on))
}

fn has_word(text: &str, words: &[String], builtin: &[&str]) -> bool {
    let t = text.to_lowercase();
    let whole = |w: &str| {
        let w = w.trim().to_lowercase();
        !w.is_empty() && t.match_indices(&w).any(|(i, _)| {
            let before = t[..i].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
            let after = t[i + w.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric());
            before && after
        })
    };
    builtin.iter().any(|w| whole(w)) || words.iter().any(|w| whole(w))
}

/// A path or a document's file name in the message: someone's own files.
fn mentions_files(text: &str) -> bool {
    let t = text.to_lowercase();
    let path = t.contains(":\\") || t.contains("\\\\") || t.split_whitespace().any(|w| w.starts_with("~/") || w.starts_with("/home/") || w.starts_with("/users/"));
    path || t.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '(' | ')' | ',')).any(|w| DOCS.iter().any(|d| w.len() > d.len() && w.trim_end_matches(['.', '?', '!', ':']).ends_with(d)))
}

/// Which model answers this message, and why, if the route changes it.
/// Private beats long or hard: a long message with a salary in it stays here.
pub fn decide(r: &Route, text: &str, cloud_allowed: bool) -> Option<(Option<String>, Why)> {
    if !r.on {
        return None;
    }
    let mut redactor = super::Redactor::default();
    let personal = redactor.redact(text) != text;
    let why = if personal || has_word(text, &r.private_words, PRIVATE) {
        Some(Why::Private)
    } else if mentions_files(text) {
        Some(Why::Files)
    } else if text.chars().count() > if r.long_chars == 0 { 6_000 } else { r.long_chars } {
        Some(Why::Long)
    } else if has_word(text, &r.hard_words, HARD) {
        Some(Why::Hard)
    } else {
        None
    }?;
    match why {
        Why::Private | Why::Files => Some((r.local_model.clone().filter(|m| !super::is_cloud(m)), why)),
        Why::Long | Why::Hard => r.cloud_model.clone().filter(|m| cloud_allowed && super::is_cloud(m)).map(|m| (Some(m), why)),
    }
}

/// The model for this message in a chat with a route, given the chat's own
/// model: None when the route leaves it alone.
pub fn pick(conn: &Connection, c: &Cipher, chat_id: &str, project: Option<&str>, chat_model: Option<&str>, text: &str) -> Option<(String, Why)> {
    let r = for_chat(conn, c, chat_id, project)?;
    let cloud_allowed = !crate::policy::get().local_only && db::settings(conn).connectivity == crate::net::Connectivity::Cloud;
    let (model, why) = decide(&r, text, cloud_allowed)?;
    let model = match model {
        Some(m) => m,
        // Private, no local model set: the chat's own, if it's on this PC, else the default.
        None => chat_model.filter(|m| !super::is_cloud(m)).map(str::to_string).or_else(|| db::settings(conn).default_model.filter(|m| !super::is_cloud(m)))?,
    };
    (Some(model.as_str()) != chat_model || why == Why::Private || why == Why::Files).then_some((model, why))
}

#[tauri::command(async)]
pub fn cloud_route(state: crate::AppStateRef, scope: String, id: String) -> Result<Route, String> {
    key(&scope, &id)?;
    Ok(get(&state.db.lock().unwrap(), &scope, &id))
}

#[tauri::command(async)]
pub fn set_cloud_route(state: crate::AppStateRef, scope: String, id: String, route: Route) -> Result<Route, String> {
    let k = key(&scope, &id)?;
    if route.local_model.as_deref().is_some_and(super::is_cloud) {
        return Err("Pick a model on this PC for private messages.".into());
    }
    if route.cloud_model.as_deref().is_some_and(|m| !super::is_cloud(m)) {
        return Err("Pick a cloud model for long or hard messages.".into());
    }
    db::set(&state.db.lock().unwrap(), &k, &route)?;
    state.log("settings", &format!("{} local and cloud routing for a {scope}", if route.on { "Turned on" } else { "Changed" }));
    Ok(route)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route() -> Route {
        Route { on: true, local_model: Some("qwen3-8b".into()), cloud_model: Some("cloud:azure:gpt-4o".into()), long_chars: 200, ..Default::default() }
    }

    #[test]
    fn private_stays_here_and_hard_may_go_out() {
        let r = route();
        assert_eq!(decide(&r, "Email jo@corp.example about the plan", true), Some((Some("qwen3-8b".into()), Why::Private)));
        assert_eq!(decide(&r, "Summarize the confidential board notes", true).unwrap().1, Why::Private);
        assert_eq!(decide(&r, "What does C:\\Users\\pat\\q3.xlsx say?", true).unwrap().1, Why::Files);
        assert_eq!(decide(&r, "Read budget.pdf and list the totals.", true).unwrap().1, Why::Files);
        assert_eq!(decide(&r, "Refactor this parser for speed", true), Some((Some("cloud:azure:gpt-4o".into()), Why::Hard)));
        assert_eq!(decide(&r, &"word ".repeat(60), true).unwrap().1, Why::Long);
        assert_eq!(decide(&r, "hi there", true), None);
        assert_eq!(decide(&r, "Refactor this parser", false), None, "no cloud allowed: left alone");
        let long_private = format!("{} my salary is high", "word ".repeat(60));
        assert_eq!(decide(&r, &long_private, true).unwrap().1, Why::Private, "private beats long");
        assert_eq!(decide(&Route { on: false, ..route() }, "Refactor it", true), None, "off by default");
        assert!(!Route::default().on);
    }

    #[test]
    fn words_match_whole() {
        assert!(!has_word("the ndash and patients", &[], &["nda"]));
        assert!(has_word("under NDA, please", &[], &["nda"]));
        assert!(has_word("project bluebird update", &["Bluebird".into()], &[]));
    }
}
