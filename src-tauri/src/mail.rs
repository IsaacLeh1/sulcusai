// SPDX-License-Identifier: AGPL-3.0-only
//! Email: IMAP accounts copied into an encrypted local store (so reading and
//! searching work offline and fast), and SMTP to send.
//!
//! - Account settings, passwords included, are encrypted with the data key.
//! - Syncing needs Local AI + Web (Purpose::Web); reading the local copy
//!   doesn't.
//! - Sending always goes through the user: the Send button, or an approval
//!   card when the assistant drafts one.
//! - Plain (unencrypted) connections are only allowed to this PC, for
//!   bridges such as Proton Mail Bridge and for tests.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::TryStreamExt;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::crypto::Cipher;
use crate::features::Feature;
use crate::net::{self, Purpose};
use crate::{db, AppState, AppStateRef};

/// Messages kept per folder on the first sync; later syncs add what's new.
const FIRST_SYNC: usize = 300;
/// Bigger messages keep their headers only (attachments are left on the server).
const MAX_FULL: u32 = 2 * 1024 * 1024;
/// Body text kept per message, in characters.
const MAX_BODY: usize = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Security {
    /// TLS from the start (993 / 465).
    #[default]
    Tls,
    /// Plain, then upgraded (143 / 587).
    Starttls,
    /// No encryption: only to this PC.
    Plain,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountConfig {
    pub name: String,
    pub email: String,
    pub username: String,
    pub password: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_security: Security,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: Security,
    /// Found on the first sync (the folder marked \Sent).
    pub sent_folder: Option<String>,
    /// UIDVALIDITY per folder; when it changes, the folder is read again.
    pub validity: HashMap<String, u32>,
    /// Signed in with Microsoft or Google instead of a password.
    pub oauth: Option<crate::oauth::Tokens>,
}

/// Server settings for an account that signs in with Microsoft or Google.
pub fn oauth_config(p: crate::oauth::Provider, email: &str) -> AccountConfig {
    let (imap, smtp, smtp_port, smtp_security) = match p {
        crate::oauth::Provider::Microsoft => ("outlook.office365.com", "smtp.office365.com", 587, Security::Starttls),
        crate::oauth::Provider::Google => ("imap.gmail.com", "smtp.gmail.com", 465, Security::Tls),
    };
    AccountConfig {
        email: email.into(),
        username: email.into(),
        imap_host: imap.into(),
        imap_port: 993,
        imap_security: Security::Tls,
        smtp_host: smtp.into(),
        smtp_port,
        smtp_security,
        ..Default::default()
    }
}

/// IMAP's SASL XOAUTH2: the whole answer is sent at once.
struct XOAuth2(String);

impl async_imap::Authenticator for XOAuth2 {
    type Response = String;
    fn process(&mut self, _challenge: &[u8]) -> String {
        self.0.clone()
    }
}

/// Accounts that signed in with Microsoft or Google: a current access token,
/// saved with the account when it was refreshed.
async fn refresh(state: &AppState, c: &Cipher, id: Option<&str>, cfg: &mut AccountConfig) -> Result<(), String> {
    if let Some(t) = cfg.oauth.as_mut() {
        if crate::oauth::fresh(state, t).await? {
            if let Some(id) = id {
                store_account(&state.db.lock().unwrap(), c, id, cfg)?;
            }
        }
    }
    Ok(())
}

/// Server settings for well-known providers, and what to know about them.
pub fn preset(email: &str) -> (AccountConfig, Option<&'static str>) {
    let domain = email.rsplit('@').next().unwrap_or("").to_ascii_lowercase();
    let base = AccountConfig { email: email.into(), username: email.into(), ..Default::default() };
    let with = |imap: &str, smtp: &str, smtp_port: u16, smtp_security: Security| AccountConfig {
        imap_host: imap.into(),
        imap_port: 993,
        imap_security: Security::Tls,
        smtp_host: smtp.into(),
        smtp_port,
        smtp_security,
        ..base.clone()
    };
    match domain.as_str() {
        "gmail.com" | "googlemail.com" => (
            with("imap.gmail.com", "smtp.gmail.com", 465, Security::Tls),
            Some("Use Sign in with Google, or an app password: Google Account › Security › 2-Step Verification › App passwords."),
        ),
        "outlook.com" | "hotmail.com" | "live.com" | "msn.com" => (
            with("outlook.office365.com", "smtp-mail.outlook.com", 587, Security::Starttls),
            Some("Use Sign in with Microsoft: Microsoft has turned off password sign-in for Outlook.com mail."),
        ),
        "icloud.com" | "me.com" | "mac.com" => (
            AccountConfig { username: email.split('@').next().unwrap_or("").into(), ..with("imap.mail.me.com", "smtp.mail.me.com", 587, Security::Starttls) },
            Some("iCloud needs an app-specific password: appleid.apple.com › Sign-In and Security."),
        ),
        "yahoo.com" | "ymail.com" => (with("imap.mail.yahoo.com", "smtp.mail.yahoo.com", 465, Security::Tls), Some("Yahoo needs an app password: Account Security › Generate app password.")),
        "aol.com" => (with("imap.aol.com", "smtp.aol.com", 465, Security::Tls), Some("AOL needs an app password.")),
        "fastmail.com" | "fastmail.fm" => (with("imap.fastmail.com", "smtp.fastmail.com", 465, Security::Tls), Some("Fastmail needs an app password: Settings › Privacy & Security.")),
        "proton.me" | "protonmail.com" | "pm.me" => (
            AccountConfig {
                imap_host: "127.0.0.1".into(),
                imap_port: 1143,
                imap_security: Security::Plain,
                smtp_host: "127.0.0.1".into(),
                smtp_port: 1025,
                smtp_security: Security::Plain,
                ..base.clone()
            },
            Some("Proton Mail works through Proton Mail Bridge on this PC; use the password Bridge shows you."),
        ),
        _ => (with(&format!("imap.{domain}"), &format!("smtp.{domain}"), 465, Security::Tls), None),
    }
}

fn local_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

// ---------- messages ----------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MailData {
    pub from: String,
    pub from_name: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    pub snippet: String,
    pub body: String,
    pub message_id: String,
    pub in_reply_to: String,
    pub references: Vec<String>,
    pub attachments: Vec<String>,
    /// Too big to copy; only the headers are here.
    pub partial: bool,
    /// List-Unsubscribe: where to unsubscribe (mailto: and https: only).
    pub unsubscribe: Vec<String>,
    /// List-Unsubscribe-Post says "List-Unsubscribe=One-Click" (RFC 8058).
    pub one_click: bool,
    /// List-Id's identifier, lowercase ("news.example.com").
    pub list_id: String,
    /// List-Id's name ("Example News").
    pub list_name: String,
    /// The list headers above were read (new mail; older mail once they're
    /// fetched from the server).
    pub list_checked: bool,
    /// A calendar invitation in it (text/calendar or an .ics file with
    /// METHOD:REQUEST).
    pub invite: Option<crate::calendar::Invite>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MailItem {
    pub id: String,
    pub account_id: String,
    pub folder: String,
    pub date: i64,
    pub seen: bool,
    /// When the user marked it read with the button (listed under Marked
    /// read); cleared when it's unread again.
    pub marked_at: Option<i64>,
    /// When the user last opened it here (not marking it read, and not the AI).
    pub opened_at: Option<i64>,
    #[serde(flatten)]
    pub data: MailData,
}

/// A mailing list's headers: where to unsubscribe and which list it is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListHeaders {
    pub unsubscribe: Vec<String>,
    pub one_click: bool,
    pub list_id: String,
    pub list_name: String,
}

impl ListHeaders {
    fn apply(self, d: &mut MailData) {
        d.unsubscribe = self.unsubscribe;
        d.one_click = self.one_click;
        d.list_id = self.list_id;
        d.list_name = self.list_name;
        d.list_checked = true;
    }
}

/// The targets of a List-Unsubscribe header (RFC 2369: `<...>` items,
/// comma-separated), keeping mailto: and https: ones. Folding whitespace
/// inside the brackets is dropped.
pub fn parse_list_unsubscribe(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find('<') {
        let Some(len) = rest[start + 1..].find('>') else { break };
        let target: String = rest[start + 1..start + 1 + len].chars().filter(|c| !c.is_whitespace()).collect();
        rest = &rest[start + 1 + len + 1..];
        let lower = target.to_ascii_lowercase();
        let usable = (lower.starts_with("mailto:") && target.len() > 7) || (lower.starts_with("https://") && target.len() > 8);
        if usable && target.len() <= 2000 && !out.contains(&target) && out.len() < 4 {
            out.push(target);
        }
    }
    out
}

/// List-Unsubscribe-Post asks for a one-click POST (RFC 8058).
pub fn is_one_click(raw: &str) -> bool {
    raw.split_whitespace().collect::<String>().eq_ignore_ascii_case("List-Unsubscribe=One-Click")
}

/// List-Id's identifier and name: `"Example News" <news.example.com>`.
pub fn parse_list_id(raw: &str) -> (String, String) {
    let raw = raw.trim();
    match (raw.rfind('<'), raw.rfind('>')) {
        (Some(a), Some(b)) if b > a => (raw[a + 1..b].trim().to_lowercase(), raw[..a].trim().trim_matches('"').trim().to_string()),
        _ if !raw.is_empty() && !raw.contains(char::is_whitespace) => (raw.to_lowercase(), String::new()),
        _ => (String::new(), String::new()),
    }
}

fn list_headers(m: &mail_parser::Message<'_>) -> ListHeaders {
    use mail_parser::HeaderName;
    let (list_id, mut list_name) = parse_list_id(m.header_raw(HeaderName::ListId).unwrap_or(""));
    // The parsed name, with encoded words ("=?UTF-8?...?=") decoded.
    if let Some(name) = m.header(HeaderName::ListId).and_then(|v| v.as_address()).and_then(|a| a.first()).and_then(|a| a.name()) {
        if !name.trim().is_empty() {
            list_name = name.trim().to_string();
        }
    }
    ListHeaders {
        unsubscribe: parse_list_unsubscribe(m.header_raw(HeaderName::ListUnsubscribe).unwrap_or("")),
        one_click: is_one_click(m.header_raw(HeaderName::ListUnsubscribePost).unwrap_or("")),
        list_id,
        list_name,
    }
}

/// The list headers from a header-only fetch (BODY.PEEK[HEADER.FIELDS (…)]).
pub fn list_headers_raw(raw: &[u8]) -> ListHeaders {
    mail_parser::MessageParser::default().parse(raw).map(|m| list_headers(&m)).unwrap_or_default()
}

/// Text without the invisible characters marketing mail pads its preview
/// with (zero-width spaces and joiners, soft hyphens, …), whitespace evened out.
pub fn visible(text: &str) -> String {
    let hidden = |c: char| matches!(c, '\u{00AD}' | '\u{034F}' | '\u{061C}' | '\u{115F}' | '\u{1160}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{3164}' | '\u{FEFF}' | '\u{FFA0}');
    text.split_whitespace().map(|w| w.chars().filter(|&c| !hidden(c)).collect::<String>()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

/// Reads a raw message.
pub fn parse(raw: &[u8], partial: bool) -> (MailData, Option<i64>) {
    use mail_parser::{Address, MessageParser, MimeHeaders};
    let Some(m) = MessageParser::default().parse(raw) else { return (MailData { partial, ..Default::default() }, None) };
    let addrs = |a: Option<&Address<'_>>| -> Vec<String> {
        a.map(|a| a.iter().filter_map(|x| x.address().map(str::to_string)).collect()).unwrap_or_default()
    };
    let first = m.from().and_then(|a| a.first());
    let body = m
        .body_text(0)
        .map(|t| t.to_string())
        .filter(|t| !t.trim().is_empty())
        .or_else(|| m.body_html(0).map(|h| crate::web::html_to_text(&h).1))
        .unwrap_or_default();
    let body: String = body.replace("\r\n", "\n").chars().take(MAX_BODY).collect();
    let snippet: String = visible(&body).chars().take(160).collect();
    let list = |v: &mail_parser::HeaderValue<'_>| -> Vec<String> {
        v.as_text_list().map(|l| l.into_iter().map(|s| s.to_string()).collect()).unwrap_or_default()
    };
    let mut data = MailData {
        from: first.and_then(|a| a.address()).unwrap_or("").to_string(),
        from_name: first.and_then(|a| a.name()).unwrap_or("").to_string(),
        to: addrs(m.to()),
        cc: addrs(m.cc()),
        subject: m.subject().unwrap_or("").to_string(),
        snippet,
        body,
        message_id: m.message_id().unwrap_or("").to_string(),
        in_reply_to: list(m.in_reply_to()).into_iter().next().unwrap_or_default(),
        references: list(m.references()),
        attachments: m.attachments().filter_map(|a| a.attachment_name().map(str::to_string)).collect(),
        partial,
        invite: invite_of(&m),
        ..Default::default()
    };
    list_headers(&m).apply(&mut data);
    (data, m.date().map(|d| d.to_timestamp() * 1000))
}

/// Calendar parts bigger than this aren't read.
const MAX_ICS_READ: usize = 1024 * 1024;

/// The invitation in a message: its text/calendar part (or an attached
/// .ics) when that's a request (METHOD:REQUEST), with the raw text kept
/// up to `calendar::MAX_ICS`.
fn invite_of(m: &mail_parser::Message<'_>) -> Option<crate::calendar::Invite> {
    use mail_parser::MimeHeaders;
    for part in &m.parts {
        let ct = part.content_type();
        let is = |t: &str, s: &str| ct.is_some_and(|c| c.ctype().eq_ignore_ascii_case(t) && c.subtype().is_some_and(|x| x.eq_ignore_ascii_case(s)));
        let ics_file = part.attachment_name().is_some_and(|n| n.to_ascii_lowercase().ends_with(".ics"));
        if !(is("text", "calendar") || is("application", "ics") || ics_file) || part.len() > MAX_ICS_READ {
            continue;
        }
        let Some(text) = part.text_contents() else { continue };
        let Some(mut inv) = crate::calendar::parse_invite(text) else { continue };
        if inv.method.is_empty() {
            inv.method = ct.and_then(|c| c.attribute("method")).unwrap_or("").trim().to_ascii_uppercase();
        }
        if inv.method != "REQUEST" {
            continue;
        }
        if text.len() <= crate::calendar::MAX_ICS {
            inv.ics = text.to_string();
        }
        return Some(inv);
    }
    None
}

fn item_from_row(c: &Cipher) -> impl Fn(&rusqlite::Row) -> rusqlite::Result<MailItem> + '_ {
    move |r| {
        Ok(MailItem {
            id: r.get(0)?,
            account_id: r.get(1)?,
            folder: r.get(2)?,
            date: r.get(3)?,
            seen: r.get::<_, i64>(4)? != 0,
            data: c.decrypt(&r.get::<_, String>(5)?).ok().and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default(),
            marked_at: r.get(6)?,
            opened_at: r.get(7)?,
        })
    }
}

const ITEM_COLS: &str = "id, account_id, folder, date, seen, data, marked_at, opened_at";

pub fn list(conn: &Connection, c: &Cipher, query: &str, limit: usize) -> Vec<MailItem> {
    let sql = format!("SELECT {ITEM_COLS} FROM mail_messages ORDER BY date DESC");
    let Ok(mut stmt) = conn.prepare(&sql) else { return Vec::new() };
    let all = stmt.query_map([], item_from_row(c)).map(|rows| rows.filter_map(Result::ok).collect::<Vec<_>>()).unwrap_or_default();
    let terms = search_terms(query);
    let hits = |m: &MailItem| terms.iter().filter(|t| t.matches(&m.data)).count();
    if all.iter().any(|m| hits(m) == terms.len()) {
        return all.into_iter().filter(|m| hits(m) == terms.len()).take(limit).collect();
    }
    // Nothing has every word: the mail with the most of them, newest first among equals.
    let mut some: Vec<(usize, MailItem)> = all.into_iter().map(|m| (hits(&m), m)).filter(|(n, _)| *n > 0).collect();
    some.sort_by(|a, b| b.0.cmp(&a.0));
    some.into_iter().map(|(_, m)| m).take(limit).collect()
}

/// One word of a mail search. Gmail-style `from:`, `to:` and `subject:`
/// narrow it to that part (models often write searches that way).
#[derive(Debug, PartialEq)]
enum Term {
    Any(String),
    From(String),
    To(String),
    Subject(String),
}

impl Term {
    fn matches(&self, d: &MailData) -> bool {
        match self {
            Term::Any(w) => format!("{} {} {} {} {}", d.from, d.from_name, d.to.join(" "), d.subject, d.body).to_lowercase().contains(w.as_str()),
            Term::From(w) => format!("{} {}", d.from, d.from_name).to_lowercase().contains(w.as_str()),
            Term::To(w) => d.to.join(" ").to_lowercase().contains(w.as_str()),
            Term::Subject(w) => d.subject.to_lowercase().contains(w.as_str()),
        }
    }
}

/// Words that say what to look for rather than being in it ("my most recent
/// email from Google"): left out of a search.
const FILLER: &[&str] = &[
    "my", "the", "a", "an", "me", "i", "any", "all", "about", "from", "most", "recent", "latest", "newest", "last", "new", "email", "emails",
    "e-mail", "mail", "mails", "message", "messages", "inbox", "did", "got", "get", "show", "find", "summarize",
];

fn search_terms(query: &str) -> Vec<Term> {
    query
        .to_lowercase()
        .split_whitespace()
        .filter(|w| !FILLER.contains(&w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-')))
        .filter_map(|w| {
            let (kind, w) = match w.split_once(':') {
                Some((k @ ("from" | "to" | "subject"), rest)) => (k, rest),
                _ => ("", w),
            };
            let w = w.trim_matches(|c| c == '"' || c == '\'').to_string();
            if w.is_empty() {
                return None;
            }
            Some(match kind {
                "from" => Term::From(w),
                "to" => Term::To(w),
                "subject" => Term::Subject(w),
                _ => Term::Any(w),
            })
        })
        .collect()
}

pub fn get(conn: &Connection, c: &Cipher, id: &str) -> Option<MailItem> {
    let sql = format!("SELECT {ITEM_COLS} FROM mail_messages WHERE id = ?1");
    conn.query_row(&sql, [id], item_from_row(c)).optional().ok().flatten()
}

/// Message-IDs the user's sent mail answers (its In-Reply-To and References).
pub fn replied_ids(conn: &Connection, c: &Cipher) -> std::collections::HashSet<String> {
    let sql = format!("SELECT {ITEM_COLS} FROM mail_messages WHERE folder NOT IN ('INBOX', '{ARCHIVE}')");
    let Ok(mut stmt) = conn.prepare(&sql) else { return Default::default() };
    let sent: Vec<MailItem> = stmt.query_map([], item_from_row(c)).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default();
    sent.into_iter()
        .flat_map(|m| std::iter::once(m.data.in_reply_to).chain(m.data.references))
        .map(|id| id.trim().trim_matches(['<', '>']).to_string())
        .filter(|id| !id.is_empty())
        .collect()
}

/// Messages kept for a thread's context, at most.
const THREAD_MAX: usize = 6;

fn bare_id(id: &str) -> String {
    id.trim().trim_matches(['<', '>']).to_string()
}

/// The rest of a message's conversation here (any folder, sent mail too),
/// oldest first: messages its References and In-Reply-To name, and others
/// answering the same ones. The last THREAD_MAX.
pub fn thread(conn: &Connection, c: &Cipher, m: &MailItem) -> Vec<MailItem> {
    let ids: std::collections::HashSet<String> = std::iter::once(&m.data.message_id)
        .chain(std::iter::once(&m.data.in_reply_to))
        .chain(m.data.references.iter())
        .map(|i| bare_id(i))
        .filter(|i| !i.is_empty())
        .collect();
    if ids.is_empty() {
        return Vec::new();
    }
    let mut found: Vec<MailItem> = list(conn, c, "", usize::MAX)
        .into_iter()
        .filter(|x| x.id != m.id)
        .filter(|x| {
            let d = &x.data;
            let own = bare_id(&d.message_id);
            // The same message in two folders (Gmail's Sent and Inbox) counts once, as this one.
            (own.is_empty() || own != bare_id(&m.data.message_id))
                && (ids.contains(&own) || ids.contains(&bare_id(&d.in_reply_to)) || d.references.iter().any(|r| ids.contains(&bare_id(r))))
        })
        .collect();
    found.sort_by_key(|x| x.date);
    found.dedup_by(|a, b| !a.data.message_id.is_empty() && a.data.message_id == b.data.message_id);
    let skip = found.len().saturating_sub(THREAD_MAX);
    found.into_iter().skip(skip).collect()
}

/// Changes a message's stored (encrypted) data.
pub(crate) fn update_data(conn: &Connection, c: &Cipher, id: &str, f: impl FnOnce(&mut MailData)) -> Result<(), String> {
    let mut m = get(conn, c, id).ok_or("That message is no longer here.")?;
    f(&mut m.data);
    let enc = c.encrypt(&serde_json::to_string(&m.data).map_err(|e| e.to_string())?);
    conn.execute("UPDATE mail_messages SET data = ?2 WHERE id = ?1", params![id, enc]).map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn save_item(conn: &Connection, c: &Cipher, account_id: &str, folder: &str, uid: u32, date: i64, seen: bool, data: &MailData) -> Result<(), String> {
    conn.execute(
        "INSERT OR IGNORE INTO mail_messages (id, account_id, folder, uid, date, seen, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            uuid::Uuid::new_v4().to_string(),
            account_id,
            folder,
            uid,
            date,
            seen as i64,
            c.encrypt(&serde_json::to_string(data).map_err(|e| e.to_string())?)
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------- accounts ----------

#[derive(Debug, Clone, Serialize)]
pub struct AccountView {
    pub id: String,
    pub name: String,
    pub email: String,
    pub imap_host: String,
    pub smtp_host: String,
    pub synced_at: Option<i64>,
}

pub fn account(conn: &Connection, c: &Cipher, id: &str) -> Option<AccountConfig> {
    let data: String = conn.query_row("SELECT data FROM mail_accounts WHERE id = ?1", [id], |r| r.get(0)).optional().ok().flatten()?;
    c.decrypt(&data).ok().and_then(|j| serde_json::from_str(&j).ok())
}

pub(crate) fn store_account(conn: &Connection, c: &Cipher, id: &str, cfg: &AccountConfig) -> Result<(), String> {
    let data = c.encrypt(&serde_json::to_string(cfg).map_err(|e| e.to_string())?);
    conn.execute(
        "INSERT INTO mail_accounts (id, data, created_at) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET data = excluded.data",
        params![id, data, db::now_ms()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn accounts(conn: &Connection, c: &Cipher) -> Vec<(String, AccountConfig, Option<i64>)> {
    let Ok(mut stmt) = conn.prepare("SELECT id, data, synced_at FROM mail_accounts ORDER BY created_at") else { return Vec::new() };
    stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<i64>>(2)?)))
        .map(|rows| {
            rows.filter_map(Result::ok)
                .filter_map(|(id, data, at)| Some((id, serde_json::from_str(&c.decrypt(&data).ok()?).ok()?, at)))
                .collect()
        })
        .unwrap_or_default()
}

pub fn message_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM mail_messages", [], |r| r.get(0)).unwrap_or(0)
}

pub fn count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM mail_accounts", [], |r| r.get(0)).unwrap_or(0)
}

// ---------- connections ----------

pub trait Io: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug> Io for T {}

type Session = async_imap::Session<Box<dyn Io>>;

pub fn tls_config() -> Arc<tokio_rustls::rustls::ClientConfig> {
    use tokio_rustls::rustls;
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    Arc::new(
        rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("TLS versions")
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

async fn tls_wrap<S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug + 'static>(host: &str, s: S) -> Result<Box<dyn Io>, String> {
    let name = tokio_rustls::rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
    let tls = tokio_rustls::TlsConnector::from(tls_config()).connect(name, s).await.map_err(|e| format!("Secure connection to {host} failed: {e}"))?;
    Ok(Box::new(tls))
}

fn friendly(e: impl std::fmt::Display) -> String {
    let t = e.to_string();
    let l = t.to_lowercase();
    if l.contains("authenticat") || l.contains("login") || l.contains("credentials") || l.contains("invalid") && l.contains("password") {
        format!("The server didn't accept the sign-in. Check the user name and (app) password. ({t})")
    } else {
        t
    }
}

async fn imap_login(cfg: &AccountConfig) -> Result<Session, String> {
    if cfg.imap_security == Security::Plain && !local_host(&cfg.imap_host) {
        return Err("Unencrypted connections are only allowed to this PC.".into());
    }
    // Directly, or the private way when that's on for mail.
    let tcp = net::connect(&cfg.imap_host, cfg.imap_port, net::Route::Mail).await?;
    let stream: Box<dyn Io> = match cfg.imap_security {
        Security::Tls => tls_wrap(&cfg.imap_host, tcp).await?,
        Security::Plain => Box::new(tcp),
        Security::Starttls => {
            let mut client = async_imap::Client::new(tcp);
            client.read_response().await.map_err(|e| e.to_string())?;
            client.run_command_and_check_ok("STARTTLS", None).await.map_err(|e| e.to_string())?;
            tls_wrap(&cfg.imap_host, client.into_inner()).await?
        }
    };
    let mut client = async_imap::Client::new(stream);
    if cfg.imap_security != Security::Starttls {
        client.read_response().await.map_err(|e| e.to_string())?.ok_or("The mail server closed the connection.")?;
    }
    let user = if cfg.username.is_empty() { &cfg.email } else { &cfg.username };
    if let Some(t) = &cfg.oauth {
        return tokio::time::timeout(Duration::from_secs(30), client.authenticate("XOAUTH2", XOAuth2(crate::oauth::xoauth2(user, &t.access_token))))
            .await
            .map_err(|_| "Signing in took too long.".to_string())?
            .map_err(|(e, _)| format!("The mail server didn't accept the sign-in; try signing in again. ({e})"));
    }
    tokio::time::timeout(Duration::from_secs(30), client.login(user, &cfg.password))
        .await
        .map_err(|_| "Signing in took too long.".to_string())?
        .map_err(|(e, _)| friendly(e))
}

/// The folder marked \Sent, if the server marks one.
async fn find_sent(s: &mut Session) -> Option<String> {
    use async_imap::types::NameAttribute;
    let names: Vec<_> = s.list(None, Some("*")).await.ok()?.try_collect().await.ok()?;
    names
        .iter()
        .find(|n| n.attributes().iter().any(|a| matches!(a, NameAttribute::Sent)))
        .or_else(|| names.iter().find(|n| matches!(n.name().to_ascii_lowercase().as_str(), "sent" | "sent items" | "sent messages" | "[gmail]/sent mail")))
        .map(|n| n.name().to_string())
}

struct Fetched {
    uid: u32,
    date: i64,
    seen: bool,
    data: MailData,
}

/// New messages in one folder since the last sync.
async fn sync_folder(s: &mut Session, folder: &str, known_max: u32, validity_was: Option<u32>) -> Result<(Vec<Fetched>, Option<u32>, bool), String> {
    let mb = s.examine(folder).await.map_err(|e| format!("Couldn't open {folder}: {e}"))?;
    let reset = validity_was.is_some() && mb.uid_validity != validity_was;
    let floor = if reset { 0 } else { known_max };
    if mb.exists == 0 {
        return Ok((Vec::new(), mb.uid_validity, reset));
    }
    let mut uids: Vec<u32> = s.uid_search("ALL").await.map_err(|e| e.to_string())?.into_iter().filter(|u| *u > floor).collect();
    uids.sort_unstable();
    if uids.len() > FIRST_SYNC {
        uids.drain(..uids.len() - FIRST_SYNC);
    }
    let mut out = Vec::new();
    for chunk in uids.chunks(40) {
        let set = chunk.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        let sizes: Vec<_> = s.uid_fetch(&set, "(UID RFC822.SIZE)").await.map_err(|e| e.to_string())?.try_collect().await.map_err(|e| e.to_string())?;
        let (small, big): (Vec<_>, Vec<_>) = sizes.iter().filter_map(|f| Some((f.uid?, f.size.unwrap_or(0)))).partition(|(_, size)| *size <= MAX_FULL);
        for (list, query, partial) in [(small, "(UID FLAGS INTERNALDATE BODY.PEEK[])", false), (big, "(UID FLAGS INTERNALDATE BODY.PEEK[HEADER])", true)] {
            if list.is_empty() {
                continue;
            }
            let set = list.iter().map(|(u, _)| u.to_string()).collect::<Vec<_>>().join(",");
            let fetched: Vec<_> = s.uid_fetch(&set, query).await.map_err(|e| e.to_string())?.try_collect().await.map_err(|e| e.to_string())?;
            for f in fetched {
                let Some(uid) = f.uid else { continue };
                let raw = if partial { f.header() } else { f.body() };
                let Some(raw) = raw else { continue };
                let (data, sent_at) = parse(raw, partial);
                let seen = f.flags().any(|fl| matches!(fl, async_imap::types::Flag::Seen));
                let date = sent_at.or_else(|| f.internal_date().map(|d| d.timestamp_millis())).unwrap_or_else(db::now_ms);
                out.push(Fetched { uid, date, seen, data });
            }
        }
    }
    Ok((out, mb.uid_validity, reset))
}

pub fn web_allowed(state: &AppState) -> Result<(), String> {
    let level = state.settings().connectivity;
    if net::allowed(level, Purpose::Web, false) {
        Ok(())
    } else {
        Err("Email needs Local AI + Web to sync and send. Switch it from the status bar; reading what's already here works Offline.".into())
    }
}

/// Copies new mail from one account. Returns how many messages arrived.
pub async fn sync_account(state: &AppState, c: &Cipher, id: &str) -> Result<usize, String> {
    web_allowed(state)?;
    let mut cfg = account(&state.db.lock().unwrap(), c, id).ok_or("That account no longer exists.")?;
    refresh(state, c, Some(id), &mut cfg).await?;
    let mut s = imap_login(&cfg).await?;
    if cfg.sent_folder.is_none() {
        cfg.sent_folder = find_sent(&mut s).await;
    }
    // What was changed here goes up first, so the server's state below includes it.
    push_pending(state, &mut s, id, &cfg).await.ok();
    let mut folders = vec!["INBOX".to_string()];
    folders.extend(cfg.sent_folder.clone());
    let mut added = 0;
    for folder in folders {
        let known: u32 = state
            .db
            .lock()
            .unwrap()
            .query_row("SELECT COALESCE(MAX(uid), 0) FROM mail_messages WHERE account_id = ?1 AND folder = ?2", params![id, folder], |r| r.get(0))
            .unwrap_or(0);
        let (items, validity, reset) = sync_folder(&mut s, &folder, known, cfg.validity.get(&folder).copied()).await?;
        let conn = state.db.lock().unwrap();
        if reset {
            conn.execute("DELETE FROM mail_messages WHERE account_id = ?1 AND folder = ?2", params![id, folder]).map_err(|e| e.to_string())?;
        }
        for m in &items {
            save_item(&conn, c, id, &folder, m.uid, m.date, m.seen, &m.data)?;
        }
        if folder == "INBOX" {
            added += items.len();
        }
        if let Some(v) = validity {
            cfg.validity.insert(folder.clone(), v);
        }
    }
    read_list_headers(state, c, &mut s, id).await.ok();
    pull_state(state, &mut s, id).await.ok();
    let _ = s.logout().await;
    let conn = state.db.lock().unwrap();
    store_account(&conn, c, id, &cfg)?;
    conn.execute("UPDATE mail_accounts SET synced_at = ?2 WHERE id = ?1", params![id, db::now_ms()]).map_err(|e| e.to_string())?;
    if added > 0 && crate::personal::enabled(&conn) {
        crate::personal::cmds::scan_mail(&conn, c, added.min(500)).ok();
    }
    Ok(added)
}

/// Inbox mail looked at when the list headers are read for older mail.
const LIST_BACKFILL: usize = 1000;

fn lists_read_key(id: &str) -> String {
    format!("mail_lists_read:{id}")
}

/// Once per account: the mailing-list headers (where to unsubscribe, which
/// list) for inbox mail copied before they were kept, fetched on their own.
async fn read_list_headers(state: &AppState, c: &Cipher, s: &mut Session, id: &str) -> Result<(), String> {
    let missing: Vec<(String, u32, MailData)> = {
        let conn = state.db.lock().unwrap();
        if db::get::<bool>(&conn, &lists_read_key(id)).unwrap_or(false) {
            return Ok(());
        }
        let mut stmt = conn.prepare("SELECT id, uid, data FROM mail_messages WHERE account_id = ?1 AND folder = 'INBOX' ORDER BY uid DESC LIMIT ?2").map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![id, LIST_BACKFILL as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?, r.get::<_, String>(2)?)))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .filter_map(|(mid, uid, data)| Some((mid, uid, serde_json::from_str::<MailData>(&c.decrypt(&data).ok()?).ok()?)))
            .filter(|(_, _, d)| !d.list_checked)
            .collect();
        rows
    };
    if !missing.is_empty() {
        s.examine("INBOX").await.map_err(|e| e.to_string())?;
        for chunk in missing.chunks(200) {
            let set = chunk.iter().map(|(_, u, _)| u.to_string()).collect::<Vec<_>>().join(",");
            let fetched: Vec<_> = s
                .uid_fetch(&set, "(UID BODY.PEEK[HEADER.FIELDS (LIST-UNSUBSCRIBE LIST-UNSUBSCRIBE-POST LIST-ID)])")
                .await
                .map_err(|e| e.to_string())?
                .try_collect()
                .await
                .map_err(|e| e.to_string())?;
            let found: HashMap<u32, ListHeaders> = fetched.iter().filter_map(|f| Some((f.uid?, list_headers_raw(f.header()?)))).collect();
            let conn = state.db.lock().unwrap();
            for (mid, uid, data) in chunk {
                let mut data = data.clone();
                // Gone from the server: nothing to read, and nothing to ask again.
                found.get(uid).cloned().unwrap_or_default().apply(&mut data);
                let enc = c.encrypt(&serde_json::to_string(&data).map_err(|e| e.to_string())?);
                conn.execute("UPDATE mail_messages SET data = ?2 WHERE id = ?1", params![mid, enc]).map_err(|e| e.to_string())?;
            }
        }
    }
    db::set(&state.db.lock().unwrap(), &lists_read_key(id), &true)
}

mod changes;
pub use changes::*;

// ---------- sending ----------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Draft {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    pub body: String,
    /// The message this answers (its local id).
    pub reply_to: Option<String>,
}

/// Splits "a@x.com, Bo <b@y.com>; c@z" into addresses.
pub fn addresses(s: &str) -> Vec<String> {
    s.split([',', ';']).map(str::trim).filter(|a| !a.is_empty()).map(str::to_string).collect()
}

/// A message to send (a reply threads under `replying`), with a calendar
/// reply (METHOD:REPLY) next to the text when `calendar` is given, as mail
/// apps send invitation answers.
pub fn build_with(cfg: &AccountConfig, d: &Draft, replying: Option<&MailData>, calendar: Option<&str>) -> Result<lettre::Message, String> {
    use lettre::message::{header::ContentType, Mailbox, MultiPart, SinglePart};
    if d.to.is_empty() {
        return Err("Add at least one recipient.".into());
    }
    let from: Mailbox = if cfg.name.is_empty() { cfg.email.parse() } else { format!("{} <{}>", cfg.name, cfg.email).parse() }
        .map_err(|e| format!("Your address doesn't look right: {e}"))?;
    let mut b = lettre::Message::builder().from(from).subject(d.subject.clone());
    for a in &d.to {
        b = b.to(a.parse().map_err(|_| format!("“{a}” isn't an email address."))?);
    }
    for a in &d.cc {
        b = b.cc(a.parse().map_err(|_| format!("“{a}” isn't an email address."))?);
    }
    if let Some(r) = replying.filter(|r| !r.message_id.is_empty()) {
        let id = format!("<{}>", r.message_id);
        b = b.in_reply_to(id.clone());
        let mut refs: Vec<String> = r.references.iter().map(|x| format!("<{x}>")).collect();
        refs.push(id);
        b = b.references(refs.join(" "));
    }
    match calendar {
        None => b.header(ContentType::TEXT_PLAIN).body(d.body.clone()),
        Some(ics) => {
            let kind = ContentType::parse("text/calendar; charset=utf-8; method=REPLY").map_err(|e| e.to_string())?;
            b.multipart(MultiPart::alternative().singlepart(SinglePart::plain(d.body.clone())).singlepart(SinglePart::builder().header(kind).body(ics.to_string())))
        }
    }
    .map_err(|e| e.to_string())
}

async fn smtp_send(cfg: &AccountConfig, msg: &lettre::Message) -> Result<(), String> {
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::transport::smtp::client::{Tls, TlsParameters};
    use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
    if cfg.smtp_security == Security::Plain && !local_host(&cfg.smtp_host) {
        return Err("Unencrypted connections are only allowed to this PC.".into());
    }
    use lettre::transport::smtp::authentication::Mechanism;
    let user = if cfg.username.is_empty() { cfg.email.clone() } else { cfg.username.clone() };
    let (creds, mechanisms) = match &cfg.oauth {
        Some(t) => (Credentials::new(user, t.access_token.clone()), vec![Mechanism::Xoauth2]),
        None => (Credentials::new(user, cfg.password.clone()), vec![Mechanism::Plain, Mechanism::Login]),
    };
    let tls = || TlsParameters::new(cfg.smtp_host.clone()).map_err(|e| e.to_string());
    if crate::privacy::proxy_for(net::Route::Mail).is_some() {
        return smtp_send_private(cfg, msg, creds, mechanisms, tls).await;
    }
    let builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.smtp_host).port(cfg.smtp_port).timeout(Some(Duration::from_secs(30)));
    let builder = match cfg.smtp_security {
        Security::Tls => builder.tls(Tls::Wrapper(tls()?)),
        Security::Starttls => builder.tls(Tls::Required(tls()?)),
        Security::Plain => builder.tls(Tls::None),
    };
    let mailer = builder.credentials(creds).authentication(mechanisms).build();
    mailer.send(msg.clone()).await.map_err(friendly)?;
    Ok(())
}

/// A connection made through a proxy, in the form the SMTP library takes.
#[derive(Debug)]
struct Through<S>(S);

impl<S: AsyncRead + Unpin> AsyncRead for Through<S> {
    fn poll_read(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &mut tokio::io::ReadBuf<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Through<S> {
    fn poll_write(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &[u8]) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.0).poll_write(cx, buf)
    }
    fn poll_flush(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_flush(cx)
    }
    fn poll_shutdown(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send + Sync + std::fmt::Debug> lettre::transport::smtp::client::AsyncTokioStream for Through<S> {
    fn peer_addr(&self) -> std::io::Result<std::net::SocketAddr> {
        Err(std::io::Error::other("connected through a proxy"))
    }
}

/// Sending through the private connection: the SMTP conversation over a
/// connection made through the proxy (TLS from the start, or STARTTLS).
async fn smtp_send_private(
    cfg: &AccountConfig,
    msg: &lettre::Message,
    creds: lettre::transport::smtp::authentication::Credentials,
    mechanisms: Vec<lettre::transport::smtp::authentication::Mechanism>,
    tls: impl Fn() -> Result<lettre::transport::smtp::client::TlsParameters, String>,
) -> Result<(), String> {
    use lettre::transport::smtp::client::AsyncSmtpConnection;
    use lettre::transport::smtp::extension::ClientId;
    let hello = ClientId::Domain("localhost".into());
    let raw = net::connect(&cfg.smtp_host, cfg.smtp_port, net::Route::Mail).await?;
    let mut conn = match cfg.smtp_security {
        Security::Tls => {
            let name = tokio_rustls::rustls::pki_types::ServerName::try_from(cfg.smtp_host.clone()).map_err(|e| e.to_string())?;
            let tls_stream = tokio_rustls::TlsConnector::from(tls_config()).connect(name, raw).await.map_err(|e| format!("Secure connection to {} failed: {e}", cfg.smtp_host))?;
            AsyncSmtpConnection::connect_with_transport(Box::new(Through(tls_stream)), &hello).await.map_err(friendly)?
        }
        Security::Starttls => {
            let mut c = AsyncSmtpConnection::connect_with_transport(Box::new(Through(raw)), &hello).await.map_err(friendly)?;
            c.starttls(tls()?, &hello).await.map_err(friendly)?;
            c
        }
        Security::Plain => AsyncSmtpConnection::connect_with_transport(Box::new(Through(raw)), &hello).await.map_err(friendly)?,
    };
    conn.auth(&mechanisms, &creds).await.map_err(friendly)?;
    let envelope = msg.envelope().clone();
    conn.send(&envelope, &msg.formatted()).await.map_err(friendly)?;
    conn.quit().await.ok();
    Ok(())
}

/// Sends a message and files a copy in Sent. Only ever called after the
/// user said so (the Send button, or approving the assistant's draft).
pub async fn send(state: &AppState, c: &Cipher, account_id: &str, d: &Draft) -> Result<(), String> {
    send_with(state, c, account_id, d, None).await
}

/// `send`, with a calendar reply part (an invitation's answer).
pub async fn send_with(state: &AppState, c: &Cipher, account_id: &str, d: &Draft, calendar: Option<&str>) -> Result<(), String> {
    web_allowed(state)?;
    let (mut cfg, replying) = {
        let conn = state.db.lock().unwrap();
        let cfg = account(&conn, c, account_id).ok_or("That account no longer exists.")?;
        let replying = d.reply_to.as_deref().and_then(|id| get(&conn, c, id)).map(|m| m.data);
        (cfg, replying)
    };
    refresh(state, c, Some(account_id), &mut cfg).await?;
    let msg = build_with(&cfg, d, replying.as_ref(), calendar)?;
    smtp_send(&cfg, &msg).await?;
    // Gmail and Outlook file sent mail themselves; others need a copy.
    let host = cfg.smtp_host.to_lowercase();
    if !host.contains("gmail") && !host.contains("outlook") && !host.contains("office365") {
        if let Some(sent) = &cfg.sent_folder {
            if let Ok(mut s) = imap_login(&cfg).await {
                let _ = s.append(sent, Some("(\\Seen)"), None, msg.formatted()).await;
                let _ = s.logout().await;
            }
        }
    }
    let to = d.to.join(", ");
    db::log_action_enc(&state.db.lock().unwrap(), c, "email", &format!("Sent an email to {to}"));
    Ok(())
}

// ---------- background ----------

/// Every 10 minutes while Email is on with an account and the level allows
/// it; otherwise asleep until something changes.
pub fn start_sync(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<Arc<AppState>>().inner().clone();
        let mut change = crate::quiet::change_watcher();
        // As if ready, so the first look comes one interval after start.
        let mut ready = true;
        loop {
            if ready {
                crate::quiet::nap(Duration::from_secs(600)).await;
            } else {
                crate::quiet::until_changed(&mut change, crate::quiet::BACKSTOP).await;
                change.borrow_and_update();
            }
            ready = {
                let conn = state.db.lock().unwrap();
                crate::features::is_on(&conn, Feature::Email) && count(&conn) > 0
            };
            let Ok(c) = state.work_cipher() else {
                ready = false;
                continue;
            };
            if !ready || web_allowed(&state).is_err() {
                ready = false;
                continue;
            }
            if let Ok(n) = sync_all(&state, &c).await {
                app.emit("mail:synced", json!({ "new": n })).ok();
                if n > 0 {
                    crate::triage::after_sync(&state, &app).await;
                }
            }
        }
    });
}

// ---------- commands ----------

fn ensure_on(state: &AppState) -> Result<(), String> {
    crate::features::require(state, Feature::Email)
}

#[derive(Serialize)]
pub struct PresetView {
    config: AccountConfig,
    note: Option<&'static str>,
}

#[tauri::command(async)]
pub fn mail_preset(email: String) -> PresetView {
    let (config, note) = preset(email.trim());
    PresetView { config, note }
}

#[tauri::command(async)]
pub fn mail_accounts(state: AppStateRef) -> Result<Vec<AccountView>, String> {
    let c = state.cipher()?;
    Ok(accounts(&state.db.lock().unwrap(), &c)
        .into_iter()
        .map(|(id, a, synced_at)| AccountView { id, name: a.name, email: a.email, imap_host: a.imap_host, smtp_host: a.smtp_host, synced_at })
        .collect())
}

/// Checks the sign-in, saves the account and copies its recent mail.
#[tauri::command]
pub async fn add_mail_account(state: AppStateRef<'_>, config: AccountConfig) -> Result<String, String> {
    ensure_on(&state)?;
    let c = state.cipher()?;
    web_allowed(&state)?;
    let mut cfg = config;
    cfg.email = cfg.email.trim().to_string();
    if cfg.username.trim().is_empty() {
        cfg.username = cfg.email.clone();
    }
    let mut s = imap_login(&cfg).await?;
    cfg.sent_folder = find_sent(&mut s).await;
    let _ = s.logout().await;
    let id = uuid::Uuid::new_v4().to_string();
    {
        let conn = state.db.lock().unwrap();
        store_account(&conn, &c, &id, &cfg)?;
        db::log_action_enc(&conn, &c, "email", &format!("Added the email account {}", cfg.email));
    }
    sync_account(&state, &c, &id).await?;
    Ok(id)
}

/// Adds an Outlook.com, Microsoft 365 or Gmail account by signing in with
/// Microsoft or Google in the browser (no password).
#[tauri::command]
pub async fn add_mail_account_oauth(state: AppStateRef<'_>, provider: crate::oauth::Provider, name: Option<String>, hint: Option<String>) -> Result<String, String> {
    connect_oauth(&state, provider, name, hint).await.map(|(id, _)| id)
}

/// Signs in with Microsoft or Google, adds the account and copies its mail.
/// Returns the account's id and address. The assistant uses this too.
pub async fn connect_oauth(state: &AppState, provider: crate::oauth::Provider, name: Option<String>, hint: Option<String>) -> Result<(String, String), String> {
    ensure_on(state)?;
    let c = state.cipher()?;
    web_allowed(state)?;
    let tokens = crate::oauth::sign_in(state, provider, crate::oauth::For::Mail, hint.as_deref()).await?;
    let mut cfg = oauth_config(provider, &tokens.email);
    cfg.name = name.unwrap_or_default().trim().to_string();
    cfg.oauth = Some(tokens);
    let mut s = imap_login(&cfg).await?;
    cfg.sent_folder = find_sent(&mut s).await;
    let _ = s.logout().await;
    let id = uuid::Uuid::new_v4().to_string();
    {
        let conn = state.db.lock().unwrap();
        store_account(&conn, &c, &id, &cfg)?;
        db::log_action_enc(&conn, &c, "email", &format!("Added the email account {} (signed in with {})", cfg.email, provider.name()));
    }
    sync_account(state, &c, &id).await?;
    Ok((id, cfg.email))
}

#[tauri::command(async)]
pub fn remove_mail_account(state: AppStateRef, id: String) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    conn.execute("DELETE FROM mail_messages WHERE account_id = ?1", [&id]).map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM mail_accounts WHERE id = ?1", [&id]).map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM settings WHERE key = ?1", [lists_read_key(&id)]).ok();
    db::log_action(&conn, "email", "Removed an email account and its copied mail");
    Ok(())
}

#[tauri::command]
pub async fn sync_mail(app: AppHandle, state: AppStateRef<'_>) -> Result<usize, String> {
    ensure_on(&state)?;
    let c = state.cipher()?;
    let n = sync_all(&state, &c).await?;
    if n > 0 {
        let (state, app) = (state.inner().clone(), app.clone());
        tauri::async_runtime::spawn(async move { crate::triage::after_sync(&state, &app).await });
    }
    Ok(n)
}

#[tauri::command(async)]
pub fn mail_list(state: AppStateRef, query: Option<String>, limit: Option<usize>) -> Result<Vec<MailItem>, String> {
    ensure_on(&state)?;
    let c = state.cipher()?;
    let mut items = list(&state.db.lock().unwrap(), &c, query.as_deref().unwrap_or(""), limit.unwrap_or(200));
    // The list doesn't need whole bodies (or invitations' raw text).
    for m in &mut items {
        m.data.body.clear();
        m.data.snippet = visible(&m.data.snippet);
        if let Some(i) = m.data.invite.as_mut() {
            i.ics.clear();
            i.notes.clear();
        }
    }
    Ok(items)
}

/// Opens a message: the user is reading it (noted in `opened_at`, which
/// Subscriptions uses to spot lists never read), and it's read.
#[tauri::command(async)]
pub fn mail_get(state: AppStateRef, id: String) -> Result<MailItem, String> {
    ensure_on(&state)?;
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut m = get(&conn, &c, &id).ok_or("That message is no longer here.")?;
    let now = db::now_ms();
    crate::subscriptions::opens_since(&conn);
    conn.execute("UPDATE mail_messages SET opened_at = ?2 WHERE id = ?1", params![id, now]).map_err(|e| e.to_string())?;
    m.opened_at = Some(now);
    if !m.seen {
        change(&conn, std::slice::from_ref(&id), Change::Seen)?;
        drop(conn);
        flush_soon(state.inner().clone());
    }
    Ok(m)
}

/// Marks a message read or unread, here and on the mail server.
#[tauri::command(async)]
pub fn mail_set_seen(state: AppStateRef, id: String, seen: bool) -> Result<(), String> {
    mail_action(state, vec![id], if seen { "seen".into() } else { "unseen".into() })
}

/// Read, unread, archive or trash for some messages: here at once, and on
/// the mail server (now if online, else at the next sync). "mark_read" is
/// the mark-read button (read, and listed under Marked read).
#[tauri::command(async)]
pub fn mail_action(state: AppStateRef, ids: Vec<String>, action: String) -> Result<(), String> {
    ensure_on(&state)?;
    state.cipher()?;
    if action == "mark_read" {
        mark_read(&state.db.lock().unwrap(), &ids)?;
        flush_soon(state.inner().clone());
        return Ok(());
    }
    let what = Change::parse(&action).ok_or("Unknown mail action.")?;
    {
        let conn = state.db.lock().unwrap();
        change(&conn, &ids, what)?;
        if matches!(what, Change::Archive | Change::Trash) {
            let n = ids.len();
            db::log_action(&conn, "email", &format!("{} {n} email{}", if what == Change::Archive { "Archived" } else { "Moved to the trash" }, if n == 1 { "" } else { "s" }));
        }
    }
    flush_soon(state.inner().clone());
    Ok(())
}

#[tauri::command]
pub async fn send_mail(state: AppStateRef<'_>, account_id: String, draft: Draft) -> Result<(), String> {
    ensure_on(&state)?;
    let c = state.cipher()?;
    send(&state, &c, &account_id, &draft).await
}

// ---------- invitations ----------

/// ACCEPTED, TENTATIVE or DECLINED, from the window's word for it.
pub fn partstat(answer: &str) -> Option<&'static str> {
    Some(match answer.to_ascii_lowercase().as_str() {
        "accept" | "accepted" | "yes" => "ACCEPTED",
        "tentative" | "maybe" => "TENTATIVE",
        "decline" | "declined" | "no" => "DECLINED",
        _ => return None,
    })
}

/// The address the user was invited at: one of the invitation's attendees
/// that is this account's address, else the account's address.
pub fn invited_as(inv: &crate::calendar::Invite, account_email: &str) -> String {
    inv.attendees.iter().find(|a| a.email.eq_ignore_ascii_case(account_email.trim())).map(|a| a.email.clone()).unwrap_or_else(|| account_email.trim().to_string())
}

/// The email that carries an invitation's answer to its organizer.
pub fn invite_reply_draft(inv: &crate::calendar::Invite, partstat: &str, name: &str, reply_to: &str) -> Draft {
    let (word, says) = match partstat {
        "ACCEPTED" => ("Accepted", "accepted"),
        "TENTATIVE" => ("Tentative", "tentatively accepted"),
        _ => ("Declined", "declined"),
    };
    let who = if name.trim().is_empty() { "The invitee".to_string() } else { name.trim().to_string() };
    // The title comes from someone else's invitation: one line, so it can't add header lines.
    let title: String = inv.title.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ").chars().take(200).collect();
    Draft {
        to: vec![inv.organizer.clone()],
        cc: Vec::new(),
        subject: format!("{word}: {title}"),
        body: format!("{who} has {says} this invitation."),
        reply_to: Some(reply_to.to_string()),
    }
}

/// An invitation's event already on a calendar here (same UID), if any.
#[tauri::command(async)]
pub fn mail_invite_status(state: AppStateRef, id: String) -> Result<Option<crate::calendar::Event>, String> {
    ensure_on(&state)?;
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let m = get(&conn, &c, &id).ok_or("That message is no longer here.")?;
    let inv = m.data.invite.ok_or("This email has no invitation in it.")?;
    Ok(crate::calendar::find_by_uid(&conn, &c, &inv.uid))
}

#[derive(Debug, Clone, Serialize)]
pub struct InviteOutcome {
    pub answer: crate::calendar::InviteAnswer,
    /// The event on the calendar: added now, or found there.
    pub event: Option<crate::calendar::Event>,
    /// It was on the calendar already (the mail service may have put it there).
    pub already: bool,
    /// The answer couldn't be sent (everything else was done).
    pub send_error: String,
}

/// Answers an invitation (the user clicked Accept, Maybe or Decline):
/// Accept and Maybe put the event on `calendar` (a connected calendar's
/// address, or this PC when empty) unless it's on a calendar already, by
/// UID; with `send_reply` (the user's tick) the answer goes to the
/// organizer as an iTIP reply. Nothing is sent without that tick.
#[tauri::command]
pub async fn mail_invite_respond(state: AppStateRef<'_>, id: String, answer: String, calendar: Option<String>, send_reply: bool) -> Result<InviteOutcome, String> {
    use crate::calendar;
    ensure_on(&state)?;
    let c = state.cipher()?;
    let partstat = partstat(&answer).ok_or("Unknown answer.")?;
    let (m, cfg, profile_name) = {
        let conn = state.db.lock().unwrap();
        let m = get(&conn, &c, &id).ok_or("That message is no longer here.")?;
        let cfg = account(&conn, &c, &m.account_id).ok_or("That account no longer exists.")?;
        (m, cfg, db::profile(&conn, &c).name)
    };
    let inv = m.data.invite.clone().ok_or("This email has no invitation in it.")?;
    if send_reply {
        web_allowed(&state)?;
        if !inv.organizer.contains('@') {
            return Err("The invitation doesn't say who sent it, so there's no one to send your answer to.".into());
        }
    }
    let target = calendar.filter(|h| !h.trim().is_empty());
    if partstat != "DECLINED" {
        crate::features::require(&state, Feature::Calendar)?;
        // The calendar's latest first: Gmail and Outlook often add invitations themselves.
        if let Some(cal) = &target {
            calendar::refresh_owner(&state, &c, cal).await.ok();
        }
    }
    let mut event = calendar::find_by_uid(&state.db.lock().unwrap(), &c, &inv.uid);
    // Added by an earlier answer here (Outlook gives it its own UID after a sync).
    let added_before = inv.answer.as_ref().is_some_and(|a| a.event_id.is_some() && a.partstat != "DECLINED");
    let mut already = event.is_some() || added_before;
    if partstat != "DECLINED" && !already {
        let n = calendar::NewEvent {
            title: if inv.title.trim().is_empty() { m.data.subject.clone() } else { inv.title.clone() },
            start: inv.start,
            end: inv.end,
            all_day: inv.all_day,
            location: inv.location.clone(),
            notes: inv.notes.clone(),
            // No attendees: adding it here must not invite anyone.
            attendees: Vec::new(),
            calendar: target.clone(),
        };
        match calendar::create_as(&state, &c, n, Some(inv.uid.clone())).await {
            Ok(e) => event = Some(e),
            Err(e) if e == calendar::ALREADY_ON_CALENDAR => already = true,
            Err(e) => return Err(e),
        }
    }
    let mut send_error = String::new();
    let mut sent = false;
    if send_reply {
        let name = if cfg.name.trim().is_empty() { profile_name } else { cfg.name.clone() };
        let ics = calendar::reply_ics(&inv, &invited_as(&inv, &cfg.email), &name, partstat, db::now_ms());
        match send_with(&state, &c, &m.account_id, &invite_reply_draft(&inv, partstat, &name, &m.id), Some(&ics)).await {
            Ok(()) => sent = true,
            Err(e) => send_error = e,
        }
    }
    let event_id = event.as_ref().map(|e| e.id.clone()).or_else(|| inv.answer.as_ref().and_then(|a| a.event_id.clone()));
    let answer = calendar::InviteAnswer { partstat: partstat.into(), at: db::now_ms(), sent, event_id };
    let conn = state.db.lock().unwrap();
    let saved = answer.clone();
    update_data(&conn, &c, &m.id, move |d| {
        if let Some(i) = d.invite.as_mut() {
            i.answer = Some(saved);
        }
    })?;
    let what = match partstat {
        "ACCEPTED" => "Accepted",
        "TENTATIVE" => "Answered maybe to",
        _ => "Declined",
    };
    db::log_action_enc(&conn, &c, "email", &format!("{what} the invitation “{}”{}", inv.title, if sent { " and sent the answer to the organizer" } else { "" }));
    Ok(InviteOutcome { answer, event, already, send_error })
}

#[cfg(test)]
mod tests;

/// Against local test servers (pymap on 11430, an aiosmtpd script on 10250):
/// `cargo test --lib mail_round_trip -- --ignored --nocapture`
#[cfg(test)]
mod live;
