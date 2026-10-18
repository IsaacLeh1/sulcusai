// SPDX-License-Identifier: AGPL-3.0-only
//! API keys for the sites the assistant builds. When a site needs a data
//! source that asks for a key, the model asks with request_api_key: the user
//! is told what the site would connect to and what it needs, and gives the
//! key in a popup (or says no). SulcusAI keeps it on this PC, encrypted, and
//! adds it only to that chat's sites' requests to that one host, through
//! the preview server's fetch route. The model never sees it, and it's never
//! written into the site's files.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::crypto::Cipher;
use crate::db;
use crate::AppStateRef;

const STORE: &str = "site_keys";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SiteKey {
    /// The data source's host, e.g. "finnhub.io" (its subdomains too).
    pub host: String,
    /// Its name as shown, e.g. "Finnhub".
    pub platform: String,
    /// Where the key goes: a query parameter ("token") or a header ("X-API-Key").
    #[serde(default)]
    pub param: String,
    #[serde(default)]
    pub header: String,
    pub key: String,
    /// The chats whose sites may use it.
    #[serde(default)]
    pub chats: Vec<String>,
}

impl SiteKey {
    /// The header value: "Authorization" takes "Bearer <key>".
    pub fn header_value(&self) -> String {
        if self.header.eq_ignore_ascii_case("authorization") && !self.key.contains(' ') {
            format!("Bearer {}", self.key)
        } else {
            self.key.clone()
        }
    }
}

/// A host from an address or a bare name ("https://www.Finnhub.io/api" → "finnhub.io").
pub fn host_of(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let with_scheme = if raw.contains("://") { raw.to_string() } else { format!("https://{raw}") };
    let url = reqwest::Url::parse(&with_scheme).ok()?;
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    (host.contains('.') && host.parse::<std::net::IpAddr>().is_err() && !host.ends_with(".local") && !host.ends_with(".localhost")).then_some(host)
}

/// Data sources that need no key (the ones the model is told about).
pub fn keyless(host: &str) -> bool {
    ["finance.yahoo.com", "coingecko.com", "open-meteo.com", "wikipedia.org", "frankfurter.app", "frankfurter.dev"].iter().any(|k| covers(k, host))
}

/// Whether a key saved for `key_host` goes along to `host`.
pub fn covers(key_host: &str, host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == key_host || host.ends_with(&format!(".{key_host}"))
}

/// Query parameter and header names a model may name for a key.
pub fn param_ok(name: &str) -> bool {
    !name.is_empty() && name.len() <= 40 && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

pub fn header_ok(name: &str) -> bool {
    const NEVER: &[&str] = &["host", "cookie", "content-length", "transfer-encoding", "connection", "origin", "referer", "user-agent"];
    !name.is_empty() && name.len() <= 40 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && !NEVER.contains(&name.to_ascii_lowercase().as_str())
}

pub fn all(conn: &Connection, c: &Cipher) -> Vec<SiteKey> {
    db::get::<String>(conn, STORE).and_then(|s| c.decrypt(&s).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save(conn: &Connection, c: &Cipher, keys: &[SiteKey]) -> Result<(), String> {
    db::set(conn, STORE, &c.encrypt(&serde_json::to_string(keys).map_err(|e| e.to_string())?))
}

/// The key a chat's site may add to a request for `host`.
pub fn for_request(conn: &Connection, c: &Cipher, chat_id: &str, host: &str) -> Option<SiteKey> {
    all(conn, c).into_iter().find(|k| covers(&k.host, host) && k.chats.iter().any(|id| id == chat_id))
}

/// A key saved for `host` already (by any chat), to offer again.
pub fn saved_for(conn: &Connection, c: &Cipher, host: &str) -> Option<SiteKey> {
    all(conn, c).into_iter().find(|k| k.host == host)
}

/// Saves what the user approved: a new key (or the one saved before, when
/// `key` is None) for this chat's sites.
pub fn approve(conn: &Connection, c: &Cipher, chat_id: &str, mut given: SiteKey, key: Option<String>) -> Result<(), String> {
    let mut keys = all(conn, c);
    match keys.iter_mut().find(|k| k.host == given.host) {
        Some(k) => {
            if let Some(key) = key {
                k.key = key;
            }
            k.platform = given.platform;
            k.param = given.param;
            k.header = given.header;
            if !k.chats.iter().any(|id| id == chat_id) {
                k.chats.push(chat_id.to_string());
            }
        }
        None => {
            given.key = key.ok_or("No key was given.")?;
            given.chats = vec![chat_id.to_string()];
            keys.push(given);
        }
    }
    save(conn, c, &keys)
}

/// What the popup shows for a request.
#[derive(Debug, Clone, Serialize)]
pub struct Ask {
    pub platform: String,
    pub host: String,
    pub reason: String,
    /// Where to get a key (https only).
    pub signup: Option<String>,
    /// A key for this host is saved already.
    pub saved: bool,
}

/// Requests waiting in a popup (by the request's id), with the key once typed.
static ASKS: LazyLock<Mutex<HashMap<String, (Ask, Option<String>)>>> = LazyLock::new(Default::default);

pub fn offer(call_id: &str, ask: Ask) {
    ASKS.lock().unwrap().insert(call_id.to_string(), (ask, None));
}

/// The key the user typed for a request (None: they kept the saved one, or gave none).
pub fn taken(call_id: &str) -> Option<String> {
    ASKS.lock().unwrap().remove(call_id).and_then(|(_, k)| k)
}

// ---------- commands ----------

#[tauri::command(async)]
pub fn site_key_ask(state: AppStateRef, call_id: String) -> Result<Ask, String> {
    state.cipher()?;
    ASKS.lock().unwrap().get(&call_id).map(|(a, _)| a.clone()).ok_or_else(|| "That request has ended.".into())
}

/// The key the user typed in the popup, before they allow it.
#[tauri::command(async)]
pub fn site_key_give(state: AppStateRef, call_id: String, key: String) -> Result<(), String> {
    state.cipher()?;
    let key = key.trim().to_string();
    if key.is_empty() || key.len() > 400 || key.chars().any(char::is_control) {
        return Err("Paste the key as it is shown on the site, on one line.".into());
    }
    match ASKS.lock().unwrap().get_mut(&call_id) {
        Some(entry) => {
            entry.1 = Some(key);
            Ok(())
        }
        None => Err("That request has ended.".into()),
    }
}

/// A saved key, as Settings lists it (never the key itself).
#[derive(Debug, Clone, Serialize)]
pub struct Saved {
    pub host: String,
    pub platform: String,
    pub chats: usize,
}

#[tauri::command(async)]
pub fn site_keys(state: AppStateRef) -> Result<Vec<Saved>, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    Ok(all(&conn, &c).into_iter().map(|k| Saved { host: k.host, platform: k.platform, chats: k.chats.len() }).collect())
}

#[tauri::command(async)]
pub fn site_key_forget(state: AppStateRef, host: String) -> Result<(), String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut keys = all(&conn, &c);
    keys.retain(|k| k.host != host);
    save(&conn, &c, &keys)?;
    db::log_action(&conn, "privacy", &format!("Removed the saved API key for {host}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_come_from_addresses_or_names() {
        assert_eq!(host_of("https://www.Finnhub.io/api/v1/").as_deref(), Some("finnhub.io"));
        assert_eq!(host_of("api.polygon.io").as_deref(), Some("api.polygon.io"));
        assert_eq!(host_of("http://localhost:3000"), None);
        assert_eq!(host_of("http://192.168.1.4/api"), None);
        assert_eq!(host_of("printer.local"), None);
    }

    #[test]
    fn a_key_goes_only_to_its_own_host() {
        assert!(covers("finnhub.io", "finnhub.io"));
        assert!(covers("finnhub.io", "api.finnhub.io"));
        assert!(!covers("finnhub.io", "evilfinnhub.io"));
        assert!(!covers("finnhub.io", "finnhub.io.evil.com"));
        assert!(keyless("query1.finance.yahoo.com") && keyless("api.coingecko.com") && !keyless("finnhub.io"));
    }

    #[test]
    fn key_places_are_checked() {
        assert!(param_ok("token") && param_ok("api_key") && param_ok("apiKey"));
        assert!(!param_ok("") && !param_ok("a b") && !param_ok("x&y=1"));
        assert!(header_ok("X-Finnhub-Token") && header_ok("Authorization"));
        assert!(!header_ok("Cookie") && !header_ok("Host") && !header_ok("X Bad"));
        let k = SiteKey { host: "x.io".into(), platform: "X".into(), param: String::new(), header: "Authorization".into(), key: "abc".into(), chats: vec![] };
        assert_eq!(k.header_value(), "Bearer abc");
    }

    #[test]
    fn approving_saves_once_per_host_and_remembers_the_chats() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        let c = Cipher::for_test();
        let k = SiteKey { host: "finnhub.io".into(), platform: "Finnhub".into(), param: "token".into(), header: String::new(), key: String::new(), chats: vec![] };
        assert!(approve(&conn, &c, "chat1", k.clone(), None).is_err(), "a first approval needs a key");
        approve(&conn, &c, "chat1", k.clone(), Some("secret1".into())).unwrap();
        assert_eq!(for_request(&conn, &c, "chat1", "finnhub.io").map(|k| k.key).as_deref(), Some("secret1"));
        assert!(for_request(&conn, &c, "chat2", "finnhub.io").is_none(), "another chat hasn't been allowed it");
        // Another chat reuses the saved key once its user allows it.
        approve(&conn, &c, "chat2", k, None).unwrap();
        assert_eq!(for_request(&conn, &c, "chat2", "api.finnhub.io").map(|k| k.key).as_deref(), Some("secret1"));
        assert_eq!(all(&conn, &c).len(), 1);
        // Stored encrypted.
        let raw: String = db::get(&conn, STORE).unwrap();
        assert!(!raw.contains("secret1"));
    }
}
