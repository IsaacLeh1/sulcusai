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

// ---------- changes made here, sent to the mail server ----------

/// Something done to messages in SulcusAI that the mail server should know:
/// read state, archiving (out of the inbox) and moving to the trash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Seen,
    Unseen,
    Archive,
    Trash,
}

impl Change {
    pub fn parse(s: &str) -> Option<Change> {
        Some(match s {
            "seen" => Change::Seen,
            "unseen" => Change::Unseen,
            "archive" => Change::Archive,
            "trash" => Change::Trash,
            _ => return None,
        })
    }
    fn code(self) -> &'static str {
        match self {
            Change::Seen => "seen",
            Change::Unseen => "unseen",
            Change::Archive => "archive",
            Change::Trash => "trash",
        }
    }
}

/// Where archived mail is kept on this PC (it isn't synced from the server).
pub const ARCHIVE: &str = "Archive";

/// Applies a change on this PC now and queues it for the server (sent by
/// `flush_pending` right away when online, else at the next sync).
pub fn change(conn: &Connection, ids: &[String], what: Change) -> Result<(), String> {
    for id in ids {
        let row: Option<(String, String, u32)> = conn
            .query_row("SELECT account_id, folder, uid FROM mail_messages WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((account, folder, uid)) = row else { continue };
        match what {
            Change::Seen => {
                conn.execute("UPDATE mail_messages SET seen = 1 WHERE id = ?1", [id]).map_err(|e| e.to_string())?;
            }
            // Unread again: out of Marked read, back in the inbox.
            Change::Unseen => {
                conn.execute("UPDATE mail_messages SET seen = 0, marked_at = NULL WHERE id = ?1", [id]).map_err(|e| e.to_string())?;
            }
            Change::Archive => {
                if folder == ARCHIVE {
                    continue;
                }
                // Kept here under Archive (All mail shows it); if that clashes, the copy goes.
                if conn.execute("UPDATE mail_messages SET folder = ?2, seen = 1 WHERE id = ?1", params![id, ARCHIVE]).is_err() {
                    conn.execute("DELETE FROM mail_messages WHERE id = ?1", [id]).map_err(|e| e.to_string())?;
                }
            }
            Change::Trash => {
                conn.execute("DELETE FROM mail_triage WHERE id = ?1", [id]).ok();
                conn.execute("DELETE FROM mail_messages WHERE id = ?1", [id]).map_err(|e| e.to_string())?;
            }
        }
        // A copy kept under Archive has no place on the server we track.
        if folder != ARCHIVE {
            conn.execute(
                "INSERT INTO mail_pending (account_id, folder, uid, op, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![account, folder, uid, what.code(), db::now_ms()],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// The mark-read button: read here and on the server, and listed under
/// Marked read (newest mark first) instead of the inbox, until it's unread
/// again.
pub fn mark_read(conn: &Connection, ids: &[String]) -> Result<(), String> {
    change(conn, ids, Change::Seen)?;
    let now = db::now_ms();
    for (i, id) in ids.iter().enumerate() {
        conn.execute("UPDATE mail_messages SET marked_at = ?2 WHERE id = ?1", params![id, now + i as i64]).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// A special folder by its IMAP SPECIAL-USE flag, else by a usual name.
async fn find_special(s: &mut Session, want: &str) -> Option<String> {
    use async_imap::types::NameAttribute;
    let names: Vec<_> = s.list(None, Some("*")).await.ok()?.try_collect().await.ok()?;
    let flagged = |n: &async_imap::types::Name| {
        n.attributes().iter().any(|a| match want {
            "all" => matches!(a, NameAttribute::All),
            "archive" => matches!(a, NameAttribute::Archive),
            "trash" => matches!(a, NameAttribute::Trash),
            _ => false,
        })
    };
    let usual: &[&str] = match want {
        "all" => &["[gmail]/all mail", "[google mail]/all mail", "all mail"],
        "archive" => &["archive", "archives"],
        "trash" => &["trash", "deleted items", "deleted messages", "[gmail]/trash", "[gmail]/bin", "[google mail]/trash", "[google mail]/bin"],
        _ => &[],
    };
    names
        .iter()
        .find(|n| flagged(n))
        .or_else(|| names.iter().find(|n| usual.contains(&n.name().to_ascii_lowercase().as_str())))
        .map(|n| n.name().to_string())
}

/// Moves messages; servers without MOVE get copy + delete-from-here.
async fn move_to(s: &mut Session, set: &str, target: &str) -> Result<(), String> {
    if s.uid_mv(set, target).await.is_ok() {
        return Ok(());
    }
    s.uid_copy(set, target).await.map_err(|e| e.to_string())?;
    let _: Vec<_> = s.uid_store(set, "+FLAGS.SILENT (\\Deleted)").await.map_err(|e| e.to_string())?.try_collect().await.map_err(|e| e.to_string())?;
    let _: Vec<_> = s.uid_expunge(set).await.map_err(|e| e.to_string())?.try_collect().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Sends one account's queued changes over an open session.
async fn push_pending(state: &AppState, s: &mut Session, id: &str, cfg: &AccountConfig) -> Result<(), String> {
    let rows: Vec<(i64, String, u32, String, i64)> = {
        let conn = state.db.lock().unwrap();
        let mut stmt = conn.prepare("SELECT rowid, folder, uid, op, created_at FROM mail_pending WHERE account_id = ?1 ORDER BY rowid").map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        rows
    };
    if rows.is_empty() {
        return Ok(());
    }
    // Gmail archives by leaving the inbox for All Mail.
    let gmail = cfg.imap_host.to_ascii_lowercase().contains("gmail") || cfg.imap_host.to_ascii_lowercase().contains("googlemail");
    // Where archived and trashed mail goes, looked up before any folder is open.
    let archive = if rows.iter().any(|r| r.3 == "archive") { find_special(s, if gmail { "all" } else { "archive" }).await } else { None };
    let trash = if rows.iter().any(|r| r.3 == "trash") { find_special(s, "trash").await } else { None };
    // Runs of the same folder and change, in the order they were made.
    let mut i = 0;
    while i < rows.len() {
        let (folder, op) = (rows[i].1.clone(), rows[i].3.clone());
        let mut j = i;
        while j < rows.len() && rows[j].1 == folder && rows[j].3 == op {
            j += 1;
        }
        let group = &rows[i..j];
        let set = group.iter().map(|r| r.2.to_string()).collect::<Vec<_>>().join(",");
        let result: Result<(), String> = async {
            s.select(&folder).await.map_err(|e| format!("Couldn't open {folder}: {e}"))?;
            match op.as_str() {
                "seen" | "unseen" => {
                    let sign = if op == "seen" { "+" } else { "-" };
                    let _: Vec<_> = s.uid_store(&set, format!("{sign}FLAGS.SILENT (\\Seen)")).await.map_err(|e| e.to_string())?.try_collect().await.map_err(|e| e.to_string())?;
                }
                "archive" => {
                    if let Some(target) = archive.as_deref().filter(|t| *t != folder) {
                        move_to(s, &set, target).await?;
                    }
                }
                // Never deleted outright: without a trash folder it stays put.
                "trash" => {
                    if let Some(target) = trash.as_deref().filter(|t| *t != folder) {
                        move_to(s, &set, target).await?;
                    }
                }
                _ => {}
            }
            Ok(())
        }
        .await;
        let conn = state.db.lock().unwrap();
        if let Err(e) = &result {
            db::log_action(&conn, "email", &format!("Couldn't update the mail server ({op} in {folder}, will try again): {e}"));
        }
        let week_ago = db::now_ms() - 7 * 86_400_000;
        for r in group {
            // Done, or failing for a week (the message is gone): drop it.
            if result.is_ok() || r.4 < week_ago {
                conn.execute("DELETE FROM mail_pending WHERE rowid = ?1", [r.0]).ok();
            }
        }
        drop(conn);
        i = j;
    }
    Ok(())
}

/// Read state and inbox membership of mail already here, as the server has
/// them now (read on the phone, archived on the web…).
async fn pull_state(state: &AppState, s: &mut Session, id: &str) -> Result<(), String> {
    let local: Vec<(String, u32, bool)> = {
        let conn = state.db.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, uid, seen FROM mail_messages WHERE account_id = ?1 AND folder = 'INBOX' ORDER BY uid DESC LIMIT 500")
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0))).map_err(|e| e.to_string())?.filter_map(Result::ok).collect();
        rows
    };
    if local.is_empty() {
        return Ok(());
    }
    s.examine("INBOX").await.map_err(|e| e.to_string())?;
    let mut flags: HashMap<u32, bool> = HashMap::new();
    for chunk in local.chunks(200) {
        let set = chunk.iter().map(|(_, u, _)| u.to_string()).collect::<Vec<_>>().join(",");
        let fetched: Vec<_> = s.uid_fetch(&set, "(UID FLAGS)").await.map_err(|e| e.to_string())?.try_collect().await.map_err(|e| e.to_string())?;
        for f in fetched {
            if let Some(uid) = f.uid {
                flags.insert(uid, f.flags().any(|fl| matches!(fl, async_imap::types::Flag::Seen)));
            }
        }
    }
    let conn = state.db.lock().unwrap();
    let waiting: std::collections::HashSet<u32> = conn
        .prepare("SELECT uid FROM mail_pending WHERE account_id = ?1 AND folder = 'INBOX'")
        .and_then(|mut st| st.query_map([id], |r| r.get(0)).map(|rows| rows.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    for (mid, uid, seen) in local {
        if waiting.contains(&uid) {
            continue;
        }
        match flags.get(&uid) {
            // Unread again there: out of Marked read here too.
            Some(&now) if now != seen => {
                conn.execute("UPDATE mail_messages SET seen = ?2, marked_at = CASE WHEN ?2 = 0 THEN NULL ELSE marked_at END WHERE id = ?1", params![mid, now as i64]).ok();
            }
            Some(_) => {}
            // Gone from the inbox there (archived or deleted elsewhere).
            None => {
                if conn.execute("UPDATE mail_messages SET folder = ?2 WHERE id = ?1", params![mid, ARCHIVE]).is_err() {
                    conn.execute("DELETE FROM mail_messages WHERE id = ?1", [&mid]).ok();
                }
            }
        }
    }
    Ok(())
}

/// Sends queued changes for every account that has some.
pub async fn flush_pending(state: &AppState, c: &Cipher) -> Result<(), String> {
    web_allowed(state)?;
    let ids: Vec<String> = {
        let conn = state.db.lock().unwrap();
        let mut stmt = conn.prepare("SELECT DISTINCT account_id FROM mail_pending").map_err(|e| e.to_string())?;
        let ids = stmt.query_map([], |r| r.get(0)).map_err(|e| e.to_string())?.filter_map(Result::ok).collect();
        ids
    };
    for id in ids {
        let Some(mut cfg) = account(&state.db.lock().unwrap(), c, &id) else {
            state.db.lock().unwrap().execute("DELETE FROM mail_pending WHERE account_id = ?1", [&id]).ok();
            continue;
        };
        refresh(state, c, Some(&id), &mut cfg).await?;
        let mut s = imap_login(&cfg).await?;
        let r = push_pending(state, &mut s, &id, &cfg).await;
        let _ = s.logout().await;
        r?;
    }
    Ok(())
}

/// Changes made within this long of each other go to the server together
/// (marking read one message after another, quickly).
const FLUSH_QUIET_MS: i64 = 1500;
static FLUSH_ASKED: AtomicU64 = AtomicU64::new(0);
static FLUSH_LAST: AtomicI64 = AtomicI64::new(0);
static FLUSHING: AtomicBool = AtomicBool::new(false);

/// Sends queued changes in the background, if web access is on: once the
/// changes pause for a moment, and one sign-in at a time.
pub fn flush_soon(state: Arc<AppState>) {
    if web_allowed(&state).is_err() {
        return;
    }
    FLUSH_LAST.store(db::now_ms(), Ordering::SeqCst);
    FLUSH_ASKED.fetch_add(1, Ordering::SeqCst);
    if FLUSHING.swap(true, Ordering::SeqCst) {
        // The one running sends this too.
        return;
    }
    tauri::async_runtime::spawn(async move {
        loop {
            loop {
                let quiet = db::now_ms() - FLUSH_LAST.load(Ordering::SeqCst);
                if quiet >= FLUSH_QUIET_MS {
                    break;
                }
                tokio::time::sleep(Duration::from_millis((FLUSH_QUIET_MS - quiet) as u64)).await;
            }
            let asked = FLUSH_ASKED.load(Ordering::SeqCst);
            if let Ok(c) = state.work_cipher() {
                let _ = flush_pending(&state, &c).await;
            }
            // More changes came while sending: send those too.
            if FLUSH_ASKED.load(Ordering::SeqCst) != asked {
                continue;
            }
            FLUSHING.store(false, Ordering::SeqCst);
            // One asked just now saw this running and left it to this one.
            if FLUSH_ASKED.load(Ordering::SeqCst) == asked || FLUSHING.swap(true, Ordering::SeqCst) {
                break;
            }
        }
    });
}

pub async fn sync_all(state: &AppState, c: &Cipher) -> Result<usize, String> {
    let ids: Vec<String> = accounts(&state.db.lock().unwrap(), c).into_iter().map(|(id, _, _)| id).collect();
    let mut total = 0;
    let mut errors = Vec::new();
    for id in ids {
        match sync_account(state, c, &id).await {
            Ok(n) => total += n,
            Err(e) => errors.push(e),
        }
    }
    if total == 0 && !errors.is_empty() {
        return Err(errors.join(" "));
    }
    Ok(total)
}

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

pub fn build(cfg: &AccountConfig, d: &Draft, replying: Option<&MailData>) -> Result<lettre::Message, String> {
    build_with(cfg, d, replying, None)
}

/// `build`, with a calendar reply (METHOD:REPLY) next to the text when
/// `calendar` is given, as mail apps send invitation answers.
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

/// Every 10 minutes while Email is on and the level allows it.
pub fn start_sync(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<Arc<AppState>>().inner().clone();
        loop {
            tokio::time::sleep(Duration::from_secs(600)).await;
            let ready = {
                let conn = state.db.lock().unwrap();
                crate::features::is_on(&conn, Feature::Email) && count(&conn) > 0
            };
            let Ok(c) = state.work_cipher() else { continue };
            if !ready || web_allowed(&state).is_err() {
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
    Draft {
        to: vec![inv.organizer.clone()],
        cc: Vec::new(),
        subject: format!("{word}: {}", inv.title),
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
mod tests {
    use super::*;

    const RAW: &[u8] = b"From: Ana Ruiz <ana@example.com>\r\nTo: me@example.org, team@example.org\r\nSubject: Budget review on Friday\r\nMessage-ID: <abc123@example.com>\r\nDate: Tue, 6 Oct 2026 09:30:00 -0600\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHi,\r\n\r\nCan we move the budget review to Friday at 2pm?\r\n\r\nAna\r\n";

    #[test]
    fn changes_apply_here_and_queue_for_the_server() {
        struct Plain;
        impl crate::crypto::Protector for Plain {
            fn protect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
                Ok(d.to_vec())
            }
            fn unprotect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
                Ok(d.to_vec())
            }
        }
        let dir = std::env::temp_dir().join(format!("sulcusai-mail-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let c = crate::crypto::Vault::open(&dir.join("keys.json"), Box::new(Plain)).unwrap().cipher().unwrap();
        let conn = db::open(&dir.join("t.db")).unwrap();
        store_account(&conn, &c, "acc", &AccountConfig { email: "me@example.org".into(), ..Default::default() }).unwrap();
        for uid in 1..=3 {
            save_item(&conn, &c, "acc", "INBOX", uid, 1_000 * uid as i64, false, &MailData { subject: format!("m{uid}"), ..Default::default() }).unwrap();
        }
        let id = |subject: &str| list(&conn, &c, "", 10).into_iter().find(|m| m.data.subject == subject).unwrap().id;
        let (m1, m2, m3) = (id("m1"), id("m2"), id("m3"));
        change(&conn, &[m1.clone()], Change::Seen).unwrap();
        change(&conn, &[m2.clone()], Change::Archive).unwrap();
        change(&conn, &[m3.clone()], Change::Trash).unwrap();
        assert!(get(&conn, &c, &m1).unwrap().seen);
        assert_eq!(get(&conn, &c, &m2).unwrap().folder, ARCHIVE, "archived mail stays here, out of the inbox");
        assert!(get(&conn, &c, &m3).is_none(), "trashed mail leaves this PC");
        let queued: Vec<(String, u32, String)> = conn
            .prepare("SELECT folder, uid, op FROM mail_pending ORDER BY rowid")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(queued, vec![("INBOX".into(), 1, "seen".into()), ("INBOX".into(), 2, "archive".into()), ("INBOX".into(), 3, "trash".into())]);
        // Trashed from the archive: nothing more to tell the server.
        change(&conn, &[m2.clone()], Change::Trash).unwrap();
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM mail_pending", [], |r| r.get::<_, i64>(0)).unwrap(), 3);
    }

    #[test]
    fn mark_read_lists_it_under_marked_read_until_unread_again() {
        struct Plain;
        impl crate::crypto::Protector for Plain {
            fn protect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
                Ok(d.to_vec())
            }
            fn unprotect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
                Ok(d.to_vec())
            }
        }
        let dir = std::env::temp_dir().join(format!("sulcusai-mail-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let c = crate::crypto::Vault::open(&dir.join("keys.json"), Box::new(Plain)).unwrap().cipher().unwrap();
        let conn = db::open(&dir.join("t.db")).unwrap();
        store_account(&conn, &c, "acc", &AccountConfig { email: "me@example.org".into(), ..Default::default() }).unwrap();
        for uid in 1..=3 {
            save_item(&conn, &c, "acc", "INBOX", uid, 1_000 * uid as i64, false, &MailData { subject: format!("m{uid}"), ..Default::default() }).unwrap();
        }
        let id = |subject: &str| list(&conn, &c, "", 10).into_iter().find(|m| m.data.subject == subject).unwrap().id;
        let (m1, m2, m3) = (id("m1"), id("m2"), id("m3"));
        assert!(get(&conn, &c, &m1).unwrap().marked_at.is_none());
        mark_read(&conn, &[m1.clone(), m2.clone()]).unwrap();
        let (a, b) = (get(&conn, &c, &m1).unwrap(), get(&conn, &c, &m2).unwrap());
        assert!(a.seen && b.seen);
        assert!(a.marked_at.is_some() && b.marked_at > a.marked_at, "marked in order");
        // Opening a message reads it without listing it under Marked read.
        change(&conn, std::slice::from_ref(&m3), Change::Seen).unwrap();
        assert!(get(&conn, &c, &m3).unwrap().marked_at.is_none());
        // Mark unread (or the AI's undo): back in the inbox.
        change(&conn, std::slice::from_ref(&m1), Change::Unseen).unwrap();
        let a = get(&conn, &c, &m1).unwrap();
        assert!(!a.seen && a.marked_at.is_none());
        let ops: Vec<String> = conn.prepare("SELECT op FROM mail_pending ORDER BY rowid").unwrap().query_map([], |r| r.get(0)).unwrap().filter_map(Result::ok).collect();
        assert_eq!(ops, ["seen", "seen", "seen", "unseen"], "every mark goes to the server");
    }

    #[test]
    fn list_headers_say_how_to_unsubscribe() {
        let raw = b"From: The Paper <news@paper.example>\r\nTo: me@example.org\r\nSubject: Morning\r\nList-Id: \"The Morning\" <morning.paper.example>\r\nList-Unsubscribe: <mailto:leave@paper.example?subject=unsubscribe%20me>,\r\n <https://paper.example/u?id=1\r\n 23>, <http://plain.example/u>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\nContent-Type: text/plain\r\n\r\nNews.\r\n";
        let (m, _) = parse(raw, false);
        assert_eq!(m.unsubscribe, vec!["mailto:leave@paper.example?subject=unsubscribe%20me", "https://paper.example/u?id=123"], "plain http is left out; folding is dropped");
        assert!(m.one_click && m.list_checked);
        assert_eq!((m.list_id.as_str(), m.list_name.as_str()), ("morning.paper.example", "The Morning"));
        // Personal mail: none of it, but read.
        let (p, _) = parse(RAW, false);
        assert!(p.unsubscribe.is_empty() && !p.one_click && p.list_id.is_empty() && p.list_checked);
        // Older mail: the headers fetched on their own.
        let fields = b"List-Unsubscribe: <https://shop.example/out>\r\nList-Id: <deals.shop.example>\r\n\r\n";
        let h = list_headers_raw(fields);
        assert_eq!((h.unsubscribe, h.one_click, h.list_id.as_str(), h.list_name.as_str()), (vec!["https://shop.example/out".to_string()], false, "deals.shop.example", ""));
        assert_eq!(list_headers_raw(b"\r\n"), ListHeaders::default());
        assert!(is_one_click(" list-unsubscribe=one-click\r\n"));
        assert!(!is_one_click("List-Unsubscribe=Later"));
        assert_eq!(parse_list_id("news.example.com"), ("news.example.com".to_string(), String::new()));
    }

    #[test]
    fn invisible_padding_is_dropped() {
        let preview = "Learn the signs. \u{034F} \u{200C}\u{00AD} \u{034F} \u{FEFF}Save  energy";
        assert_eq!(visible(preview), "Learn the signs. Save energy");
    }

    #[test]
    fn searches_understand_from_to_and_subject() {
        let (m, _) = parse(RAW, false);
        let hit = |q: &str| search_terms(q).iter().all(|t| t.matches(&m));
        assert!(hit("budget"));
        assert!(hit("from:Ana"));
        assert!(hit("from:\"ana@example.com\" subject:review"));
        assert!(hit("to:team"));
        assert!(!hit("from:team"));
        assert!(!hit("subject:ana"));
        assert!(hit(""));
        assert_eq!(search_terms("from: x"), vec![Term::Any("x".into())]);
        // How models search: the whole request.
        assert!(hit("my most recent email from Ana"));
        assert_eq!(search_terms("Summarize my latest emails"), vec![]);
    }

    #[test]
    fn messages_are_read() {
        let (m, date) = parse(RAW, false);
        assert_eq!(m.from, "ana@example.com");
        assert_eq!(m.from_name, "Ana Ruiz");
        assert_eq!(m.to, vec!["me@example.org", "team@example.org"]);
        assert_eq!(m.subject, "Budget review on Friday");
        assert_eq!(m.message_id, "abc123@example.com");
        assert!(m.body.contains("Friday at 2pm") && !m.body.contains('\r'));
        assert!(m.snippet.starts_with("Hi, Can we move"));
        assert_eq!(date, Some(1_791_300_600_000));
    }

    #[test]
    fn html_only_mail_becomes_text() {
        let raw = b"From: a@b.c\r\nSubject: x\r\nContent-Type: text/html\r\n\r\n<html><body><p>Your <b>order</b> shipped.</p><script>x()</script></body></html>";
        let (m, _) = parse(raw, false);
        assert!(m.body.contains("order") && m.body.contains("shipped") && !m.body.contains("x()"), "{}", m.body);
    }

    #[test]
    fn providers_are_filled_in() {
        let (g, note) = preset("me@gmail.com");
        assert_eq!((g.imap_host.as_str(), g.imap_port, g.smtp_port), ("imap.gmail.com", 993, 465));
        assert!(note.unwrap().contains("app password"));
        let (o, _) = preset("x@company.org");
        assert_eq!(o.imap_host, "imap.company.org");
        let (i, _) = preset("kim@icloud.com");
        assert_eq!(i.username, "kim");
    }

    #[test]
    fn replies_thread_and_bad_addresses_are_caught() {
        let cfg = AccountConfig { name: "Me".into(), email: "me@example.org".into(), ..Default::default() };
        let (orig, _) = parse(RAW, false);
        let d = Draft { to: vec!["ana@example.com".into()], subject: "Re: Budget review on Friday".into(), body: "Friday works.".into(), ..Default::default() };
        let msg = String::from_utf8(build(&cfg, &d, Some(&orig)).unwrap().formatted()).unwrap();
        assert!(msg.contains("In-Reply-To: <abc123@example.com>") && msg.contains("Friday works."), "{msg}");
        let bad = Draft { to: vec!["not an address".into()], ..d.clone() };
        assert!(build(&cfg, &bad, None).is_err());
        assert!(build(&cfg, &Draft { to: vec![], ..d }, None).is_err());
        assert_eq!(addresses("a@x.com, Bo <b@y.com>; "), vec!["a@x.com", "Bo <b@y.com>"]);
    }

    fn invite_mail(method: &str, how: &str) -> Vec<u8> {
        let ics = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:{method}\r\nBEGIN:VEVENT\r\nUID:inv-1@example.com\r\nSEQUENCE:0\r\nDTSTART:20261023T210000Z\r\nDTEND:20261023T220000Z\r\nSUMMARY:Lunch\r\nORGANIZER;CN=Ana Ruiz:mailto:ana@example.com\r\nATTENDEE;PARTSTAT=NEEDS-ACTION;CN=Me:mailto:me@example.org\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let part = match how {
            // Inline, as Gmail and Outlook send it.
            "inline" => format!("Content-Type: text/calendar; charset=utf-8; method={method}\r\n\r\n{ics}"),
            // Only as a file.
            _ => format!("Content-Type: application/octet-stream; name=invite.ics\r\nContent-Disposition: attachment; filename=invite.ics\r\n\r\n{ics}"),
        };
        format!(
            "From: Ana Ruiz <ana@example.com>\r\nTo: me@example.org\r\nSubject: Invitation: Lunch\r\nMessage-ID: <inv-mail@example.com>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n--b1\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nYou're invited to lunch.\r\n--b1\r\n{part}\r\n--b1--\r\n"
        )
        .into_bytes()
    }

    #[test]
    fn invitations_are_kept_with_the_message() {
        for how in ["inline", "file"] {
            let (m, _) = parse(&invite_mail("REQUEST", how), false);
            let inv = m.invite.unwrap_or_else(|| panic!("{how}: no invitation"));
            assert_eq!((inv.method.as_str(), inv.uid.as_str(), inv.title.as_str(), inv.organizer.as_str()), ("REQUEST", "inv-1@example.com", "Lunch", "ana@example.com"), "{how}");
            assert!(inv.ics.contains("UID:inv-1@example.com"), "{how}: the raw text is kept");
            assert!(m.body.contains("invited to lunch"), "{how}: {}", m.body);
        }
        // A cancellation or a reply isn't an invitation to answer.
        for method in ["CANCEL", "REPLY"] {
            assert!(parse(&invite_mail(method, "inline"), false).0.invite.is_none(), "{method}");
        }
        assert!(parse(RAW, false).0.invite.is_none());
        // Over the size cap: the event is kept, the raw text isn't.
        let big = String::from_utf8(invite_mail("REQUEST", "inline")).unwrap().replace("SUMMARY:Lunch", &format!("SUMMARY:Lunch\r\nDESCRIPTION:{}", "x".repeat(crate::calendar::MAX_ICS)));
        let inv = parse(big.as_bytes(), false).0.invite.unwrap();
        assert!(inv.ics.is_empty() && inv.title == "Lunch");
    }

    #[test]
    fn invitation_answers_go_to_the_organizer_with_a_calendar_reply() {
        let (m, _) = parse(&invite_mail("REQUEST", "inline"), false);
        let inv = m.invite.clone().unwrap();
        assert_eq!(partstat("accept"), Some("ACCEPTED"));
        assert_eq!(partstat("maybe"), Some("TENTATIVE"));
        assert_eq!(partstat("decline"), Some("DECLINED"));
        assert_eq!(partstat("later"), None);
        assert_eq!(invited_as(&inv, "ME@example.org"), "me@example.org", "the address on the invitation");
        assert_eq!(invited_as(&inv, "other@example.org"), "other@example.org");
        let d = invite_reply_draft(&inv, "TENTATIVE", "Isaac", "local-id");
        assert_eq!((d.to.clone(), d.subject.as_str(), d.body.as_str()), (vec!["ana@example.com".to_string()], "Tentative: Lunch", "Isaac has tentatively accepted this invitation."));
        let cfg = AccountConfig { name: "Isaac".into(), email: "me@example.org".into(), ..Default::default() };
        let ics = crate::calendar::reply_ics(&inv, "me@example.org", "Isaac", "TENTATIVE", 1_000);
        let msg = String::from_utf8(build_with(&cfg, &d, Some(&m), Some(&ics)).unwrap().formatted()).unwrap();
        assert!(msg.contains("multipart/alternative") && msg.contains("text/calendar") && msg.contains("method=REPLY"), "{msg}");
        assert!(msg.contains("In-Reply-To: <inv-mail@example.com>"), "{msg}");
        // Plain mail stays plain.
        let plain = String::from_utf8(build(&cfg, &d, None).unwrap().formatted()).unwrap();
        assert!(!plain.contains("text/calendar"));
    }

    #[test]
    fn a_thread_is_the_mail_it_answers_and_answers_to_it() {
        struct Plain;
        impl crate::crypto::Protector for Plain {
            fn protect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
                Ok(d.to_vec())
            }
            fn unprotect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
                Ok(d.to_vec())
            }
        }
        let dir = std::env::temp_dir().join(format!("sulcusai-mail-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let c = crate::crypto::Vault::open(&dir.join("keys.json"), Box::new(Plain)).unwrap().cipher().unwrap();
        let conn = db::open(&dir.join("t.db")).unwrap();
        store_account(&conn, &c, "acc", &AccountConfig { email: "me@example.org".into(), ..Default::default() }).unwrap();
        let msg = |id: &str, irt: &str, refs: &[&str], subject: &str| MailData {
            message_id: id.into(),
            in_reply_to: irt.into(),
            references: refs.iter().map(|r| r.to_string()).collect(),
            subject: subject.into(),
            ..Default::default()
        };
        save_item(&conn, &c, "acc", "INBOX", 1, 100, true, &msg("a@x", "", &[], "Trip")).unwrap();
        save_item(&conn, &c, "acc", "Sent", 1, 200, true, &msg("b@x", "a@x", &["a@x"], "Re: Trip")).unwrap();
        save_item(&conn, &c, "acc", "INBOX", 2, 300, false, &msg("c@x", "b@x", &["a@x", "b@x"], "Re: Trip")).unwrap();
        save_item(&conn, &c, "acc", "INBOX", 3, 250, true, &msg("z@x", "", &[], "Other")).unwrap();
        let all = list(&conn, &c, "", 10);
        let last = all.iter().find(|m| m.data.message_id == "c@x").unwrap();
        let subjects: Vec<String> = thread(&conn, &c, last).into_iter().map(|m| m.data.message_id).collect();
        assert_eq!(subjects, vec!["a@x", "b@x"], "oldest first, the unrelated one left out");
        assert!(thread(&conn, &c, all.iter().find(|m| m.data.message_id == "z@x").unwrap()).is_empty());
    }

    #[tokio::test]
    async fn plain_connections_only_to_this_pc() {
        let cfg = AccountConfig { imap_host: "mail.example.com".into(), imap_port: 143, imap_security: Security::Plain, ..Default::default() };
        assert!(imap_login(&cfg).await.unwrap_err().contains("only allowed to this PC"));
    }
}

/// Against local test servers (pymap on 11430, an aiosmtpd script on 10250):
/// `cargo test --lib mail_round_trip -- --ignored --nocapture`
#[cfg(test)]
mod live {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn mail_round_trip() {
        let cfg = AccountConfig {
            name: "Demo".into(),
            email: "demo@example.org".into(),
            username: "demouser".into(),
            password: "demopass".into(),
            imap_host: "127.0.0.1".into(),
            imap_port: 11430,
            imap_security: Security::Plain,
            smtp_host: "127.0.0.1".into(),
            smtp_port: 10250,
            smtp_security: Security::Plain,
            ..Default::default()
        };
        let bad = AccountConfig { password: "wrong".into(), ..cfg.clone() };
        let e = imap_login(&bad).await.unwrap_err();
        println!("wrong password: {e}");
        assert!(e.contains("didn't accept"), "{e}");

        let mut s = imap_login(&cfg).await.unwrap();
        let sent = find_sent(&mut s).await;
        println!("sent folder: {sent:?}");
        let (inbox, validity, _) = sync_folder(&mut s, "INBOX", 0, None).await.unwrap();
        println!("inbox: {} messages, validity {validity:?}", inbox.len());
        for m in inbox.iter().take(4) {
            println!("  {} | {} | {} | seen {}", m.data.from, m.data.subject, m.data.snippet.chars().take(50).collect::<String>(), m.seen);
        }
        assert!(!inbox.is_empty());
        let max = inbox.iter().map(|m| m.uid).max().unwrap();
        let (again, _, _) = sync_folder(&mut s, "INBOX", max, validity).await.unwrap();
        assert!(again.is_empty(), "nothing new on the second sync");

        let d = Draft { to: vec!["ana@example.com".into()], subject: "Re: hello".into(), body: "Sounds good.\nThanks!".into(), ..Default::default() };
        let msg = build(&cfg, &d, Some(&inbox[0].data)).unwrap();
        smtp_send(&cfg, &msg).await.unwrap();
        println!("sent over SMTP");
        let sent = sent.expect("demo data has a Sent folder");
        let before = sync_folder(&mut s, &sent, 0, None).await.unwrap().0.len();
        s.append(&sent, Some("(\\Seen)"), None, msg.formatted()).await.unwrap();
        let after = sync_folder(&mut s, &sent, 0, None).await.unwrap().0;
        println!("sent folder: {before} → {}", after.len());
        assert_eq!(after.len(), before + 1);
        assert!(after.iter().any(|m| m.data.subject == "Re: hello" && m.data.body.contains("Sounds good")));
        s.logout().await.unwrap();
    }

    /// Read, archive and trash done here reach the server, and what the
    /// server changed (read on a phone, archived on the web) comes back.
    /// Needs pymap on 11430, as above (start it fresh: it changes the demo data).
    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn mail_changes_reach_the_server_and_back() {
        let cfg = AccountConfig {
            name: "Demo".into(),
            email: "demo@example.org".into(),
            username: "demouser".into(),
            password: "demopass".into(),
            imap_host: "127.0.0.1".into(),
            imap_port: 11430,
            imap_security: Security::Plain,
            ..Default::default()
        };
        let state = crate::e2e::scratch_state();
        let c = state.cipher().unwrap();
        {
            let conn = state.db.lock().unwrap();
            db::update_settings(&conn, |s| s.connectivity = crate::net::Connectivity::Web).unwrap();
            store_account(&conn, &c, "acc", &cfg).unwrap();
        }
        // Somewhere to archive to and a trash, as Outlook has them (pymap's
        // own Trash is read-only).
        let mut s = imap_login(&cfg).await.unwrap();
        let _ = s.create("Archive").await;
        let _ = s.delete("Trash").await;
        let _ = s.create("Deleted Items").await;
        for n in 0..2 {
            let raw = format!("From: Bo <bo@example.com>\r\nTo: demo@example.org\r\nSubject: Extra {n}\r\nDate: Tue, 13 Oct 2026 09:30:00 -0600\r\n\r\nHello {n}.\r\n");
            s.append("INBOX", None, None, raw.as_bytes()).await.unwrap();
        }
        s.logout().await.unwrap();

        sync_account(&state, &c, "acc").await.unwrap();
        let inbox: Vec<MailItem> = list(&state.db.lock().unwrap(), &c, "", 100).into_iter().filter(|m| m.folder == "INBOX").collect();
        println!("inbox here: {}", inbox.len());
        assert!(inbox.len() >= 5, "demo data has enough mail");
        let uid_of = |id: &str| state.db.lock().unwrap().query_row("SELECT uid FROM mail_messages WHERE id = ?1", [id], |r| r.get::<_, u32>(0)).unwrap();
        let (read, archived, trashed, phone, web) = (&inbox[0], &inbox[1], &inbox[2], &inbox[3], &inbox[4]);
        let (read_uid, archived_uid, trashed_uid, phone_uid, web_uid) = (uid_of(&read.id), uid_of(&archived.id), uid_of(&trashed.id), uid_of(&phone.id), uid_of(&web.id));
        {
            let conn = state.db.lock().unwrap();
            change(&conn, &[read.id.clone()], if read.seen { Change::Unseen } else { Change::Seen }).unwrap();
            change(&conn, &[archived.id.clone()], Change::Archive).unwrap();
            change(&conn, &[trashed.id.clone()], Change::Trash).unwrap();
        }
        flush_pending(&state, &c).await.unwrap();
        for a in db::actions(&state.db.lock().unwrap(), &c, 5) {
            println!("log: {}", a.summary);
        }
        assert_eq!(state.db.lock().unwrap().query_row("SELECT COUNT(*) FROM mail_pending", [], |r| r.get::<_, i64>(0)).unwrap(), 0, "all sent");

        // What the server has now.
        let mut s = imap_login(&cfg).await.unwrap();
        s.select("INBOX").await.unwrap();
        let left: Vec<u32> = s.uid_search("ALL").await.unwrap().into_iter().collect();
        assert!(!left.contains(&archived_uid) && !left.contains(&trashed_uid), "archived and trashed mail left the inbox");
        let fetched: Vec<_> = s.uid_fetch(read_uid.to_string(), "(UID FLAGS)").await.unwrap().try_collect().await.unwrap();
        let seen_there = fetched[0].flags().any(|f| matches!(f, async_imap::types::Flag::Seen));
        assert_eq!(seen_there, !read.seen, "read state reached the server");
        for folder in ["Archive", "Deleted Items"] {
            let mb = s.examine(folder).await.unwrap();
            println!("{folder} on the server: {} messages", mb.exists);
            assert_eq!(mb.exists, 1);
        }
        // Meanwhile, on a phone: one read, one archived.
        s.select("INBOX").await.unwrap();
        let _: Vec<_> = s.uid_store(phone_uid.to_string(), if phone.seen { "-FLAGS (\\Seen)" } else { "+FLAGS (\\Seen)" }).await.unwrap().try_collect().await.unwrap();
        move_to(&mut s, &web_uid.to_string(), "Archive").await.unwrap();
        s.logout().await.unwrap();

        sync_account(&state, &c, "acc").await.unwrap();
        let conn = state.db.lock().unwrap();
        assert_eq!(get(&conn, &c, &phone.id).unwrap().seen, !phone.seen, "read state came back");
        assert_eq!(get(&conn, &c, &web.id).unwrap().folder, ARCHIVE, "archived elsewhere: out of the inbox here");
        assert!(get(&conn, &c, &trashed.id).is_none());
    }
}
