// SPDX-License-Identifier: AGPL-3.0-only
//! Language and writing: a language partner with scored practice, writing
//! help, a scam check, PC health and posting to Bluesky and Mastodon. Each
//! has its own switch, off until turned on.

pub mod health;
pub mod help;
pub mod partner;
pub mod scam;
pub mod social;

use std::collections::BTreeMap;
use std::time::Duration;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::crypto::Cipher;
use crate::{chat, db, AppStateRef};

const SWITCHES: &str = "writing_switches";
pub(crate) const PROGRESS: &str = "language_partner";
const ACCOUNTS: &str = "social_accounts";

pub const TOOLS: &[&str] = &["partner", "writing_help", "scam_check", "pc_health", "social"];

fn switches(conn: &Connection) -> BTreeMap<String, bool> {
    db::get::<BTreeMap<String, bool>>(conn, SWITCHES).unwrap_or_default()
}

pub(crate) fn require(conn: &Connection, tool: &str) -> Result<(), String> {
    if switches(conn).get(tool).copied().unwrap_or(false) {
        Ok(())
    } else {
        Err("Turn this on first in Language and writing.".into())
    }
}

pub(crate) fn load<T: for<'de> Deserialize<'de> + Default>(conn: &Connection, c: &Cipher, key: &str) -> T {
    db::get::<String>(conn, key).and_then(|s| c.decrypt(&s).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub(crate) fn save<T: Serialize>(conn: &Connection, c: &Cipher, key: &str, v: &T) -> Result<(), String> {
    db::set(conn, key, &c.encrypt(&serde_json::to_string(v).map_err(|e| e.to_string())?))
}

fn json_in(reply: &str) -> Option<Value> {
    let s = reply.find('{')?;
    let e = reply.rfind('}')?;
    serde_json::from_str(reply.get(s..=e)?).ok()
}

// ---------- switches ----------

#[tauri::command(async)]
pub fn writing_switches(state: AppStateRef) -> Result<BTreeMap<String, bool>, String> {
    let conn = state.db.lock().unwrap();
    let on = switches(&conn);
    Ok(TOOLS.iter().map(|t| (t.to_string(), on.get(*t).copied().unwrap_or(false))).collect())
}

#[tauri::command(async)]
pub fn writing_set_switch(state: AppStateRef, tool: String, on: bool) -> Result<(), String> {
    if !TOOLS.contains(&tool.as_str()) {
        return Err("Unknown tool.".into());
    }
    let conn = state.db.lock().unwrap();
    let mut s = switches(&conn);
    s.insert(tool, on);
    db::set(&conn, SWITCHES, &s)
}

// ---------- language partner ----------

#[derive(Serialize)]
pub struct Round {
    english: String,
    reference: String,
    hint: String,
    level: u8,
}

#[tauri::command]
pub async fn partner_round(state: AppStateRef<'_>, language: String, level: u8, topic: String) -> Result<Round, String> {
    state.cipher()?;
    require(&state.db.lock().unwrap(), "partner")?;
    let language = crate::translate::language_name(&language);
    let (ep, _, _hold) = crate::background_endpoint(state.inner()).await?;
    let messages = vec![json!({ "role": "user", "content": partner::round_prompt(&language, level, &topic) })];
    let reply = chat::complete(&ep, messages, json!({ "temperature": 0.8 }), 400).await?;
    let v = json_in(&reply).ok_or("The model's answer wasn't readable; try again.")?;
    let get = |k: &str| v[k].as_str().unwrap_or_default().trim().to_string();
    let (english, reference) = (get("english"), get("reference"));
    if english.is_empty() || reference.is_empty() {
        return Err("The model's answer wasn't complete; try again.".into());
    }
    Ok(Round { english, reference, hint: get("hint"), level: level.clamp(1, 6) })
}

#[derive(Serialize)]
pub struct Marked {
    score: partner::Score,
    progress: partner::Progress,
    next_level: u8,
}

#[tauri::command(async)]
pub fn partner_answer(state: AppStateRef, language: String, level: u8, reference: String, answer: String) -> Result<Marked, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    require(&conn, "partner")?;
    let score = partner::score(&reference, &answer);
    let mut all: BTreeMap<String, partner::Progress> = load(&conn, &c, PROGRESS);
    let p = all.entry(language.clone()).or_insert_with(|| partner::Progress { language, ..Default::default() });
    partner::record(p, score.score);
    let progress = p.clone();
    save(&conn, &c, PROGRESS, &all)?;
    Ok(Marked { next_level: partner::next_level(level, &progress), score, progress })
}

#[tauri::command(async)]
pub fn partner_progress(state: AppStateRef) -> Result<Vec<partner::Progress>, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    Ok(load::<BTreeMap<String, partner::Progress>>(&conn, &c, PROGRESS).into_values().collect())
}

// ---------- writing help ----------

#[tauri::command(async)]
pub fn writing_check(state: AppStateRef, text: String) -> Result<help::Report, String> {
    require(&state.db.lock().unwrap(), "writing_help")?;
    Ok(help::check(&text))
}

#[tauri::command(async)]
pub fn writing_fix(state: AppStateRef, text: String) -> Result<String, String> {
    require(&state.db.lock().unwrap(), "writing_help")?;
    Ok(help::apply_fixes(&text, &help::check(&text).issues))
}

#[tauri::command]
pub async fn writing_rewrite(state: AppStateRef<'_>, text: String, tone: String) -> Result<String, String> {
    state.cipher()?;
    require(&state.db.lock().unwrap(), "writing_help")?;
    let system = help::rewrite_prompt(&tone)?;
    let (ep, _, _hold) = crate::background_endpoint(state.inner()).await?;
    let messages = vec![json!({ "role": "system", "content": system }), json!({ "role": "user", "content": text })];
    let max = (text.len() as u32 / 2).clamp(256, 4000);
    Ok(chat::complete(&ep, messages, json!({ "temperature": 0.4 }), max).await?.trim().to_string())
}

// ---------- scam check and PC health ----------

#[tauri::command(async)]
pub fn scam_check(state: AppStateRef, text: String, sender: Option<String>) -> Result<scam::Verdict, String> {
    require(&state.db.lock().unwrap(), "scam_check")?;
    Ok(scam::check(&text, sender.as_deref().filter(|s| !s.trim().is_empty())))
}

#[tauri::command(async)]
pub fn pc_health(state: AppStateRef) -> Result<health::Health, String> {
    require(&state.db.lock().unwrap(), "pc_health")?;
    Ok(health::judge(health::take()))
}

// ---------- social posting ----------

#[tauri::command(async)]
pub fn social_accounts(state: AppStateRef) -> Result<Vec<social::Shown>, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    Ok(load::<Vec<social::Account>>(&conn, &c, ACCOUNTS).iter().map(social::Account::shown).collect())
}

#[tauri::command(async)]
pub fn social_add(state: AppStateRef, account: social::Account) -> Result<(), String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    require(&conn, "social")?;
    let a = social::validate(account)?;
    let mut all: Vec<social::Account> = load(&conn, &c, ACCOUNTS);
    all.retain(|x| x.shown() != a.shown());
    all.push(a);
    save(&conn, &c, ACCOUNTS, &all)
}

#[tauri::command(async)]
pub fn social_remove(state: AppStateRef, index: usize) -> Result<(), String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut all: Vec<social::Account> = load(&conn, &c, ACCOUNTS);
    if index < all.len() {
        all.remove(index);
    }
    save(&conn, &c, ACCOUNTS, &all)
}

/// Whether a tool's switch is on, for offering its chat tools.
pub(crate) fn is_on(conn: &Connection, tool: &str) -> bool {
    require(conn, tool).is_ok()
}

/// Called only from the Post button after the person has seen the text.
#[tauri::command]
pub async fn social_post(state: AppStateRef<'_>, index: usize, text: String) -> Result<String, String> {
    let c = state.cipher()?;
    let account = {
        let conn = state.db.lock().unwrap();
        require(&conn, "social")?;
        load::<Vec<social::Account>>(&conn, &c, ACCOUNTS).into_iter().nth(index).ok_or("That account isn't there any more.")?
    };
    let b = reqwest::Client::builder()
        .user_agent(concat!("SulcusAI/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(60));
    let client = crate::net::routed(b, crate::net::Route::Web)?.build().map_err(|e| e.to_string())?;
    let link = social::post(&client, &account, &text).await?;
    let shown = account.shown();
    state.log("social", &format!("Posted to {} ({})", shown.kind, shown.name));
    Ok(link)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        conn
    }

    #[test]
    fn every_tool_starts_off() {
        let c = conn();
        for t in TOOLS {
            assert!(require(&c, t).is_err());
        }
        let mut s = switches(&c);
        s.insert("scam_check".into(), true);
        db::set(&c, SWITCHES, &s).unwrap();
        assert!(require(&c, "scam_check").is_ok());
        assert!(require(&c, "social").is_err());
    }

    #[test]
    fn accounts_are_stored_encrypted() {
        let (c, k) = (conn(), Cipher::for_test());
        let a = vec![social::Account::Mastodon { instance: "https://m.example".into(), token: "secrettokensecrettoken".into() }];
        save(&c, &k, ACCOUNTS, &a).unwrap();
        let raw: String = db::get(&c, ACCOUNTS).unwrap();
        assert!(!raw.contains("secrettoken"));
        assert_eq!(load::<Vec<social::Account>>(&c, &k, ACCOUNTS), a);
    }

    #[test]
    fn model_json_is_found_in_chatter() {
        assert_eq!(json_in("Sure! {\"english\": \"Hi\"} done").unwrap()["english"], "Hi");
        assert!(json_in("no json").is_none());
    }
}
