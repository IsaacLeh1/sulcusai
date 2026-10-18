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
}

#[derive(Debug, Clone, Serialize)]
pub struct MailItem {
    pub id: String,
    pub account_id: String,
    pub folder: String,
    pub date: i64,
    pub seen: bool,
    #[serde(flatten)]
    pub data: MailData,
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
    let data = MailData {
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
    };
    (data, m.date().map(|d| d.to_timestamp() * 1000))
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
        })
    }
}

const ITEM_COLS: &str = "id, account_id, folder, date, seen, data";

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
    pull_state(state, &mut s, id).await.ok();
    let _ = s.logout().await;
    let conn = state.db.lock().unwrap();
    store_account(&conn, c, id, &cfg)?;
    conn.execute("UPDATE mail_accounts SET synced_at = ?2 WHERE id = ?1", params![id, db::now_ms()]).map_err(|e| e.to_string())?;
    Ok(added)
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
            Change::Seen | Change::Unseen => {
                conn.execute("UPDATE mail_messages SET seen = ?2 WHERE id = ?1", params![id, (what == Change::Seen) as i64]).map_err(|e| e.to_string())?;
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
            Some(&now) if now != seen => {
                conn.execute("UPDATE mail_messages SET seen = ?2 WHERE id = ?1", params![mid, now as i64]).ok();
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

/// Sends queued changes in the background, if web access is on.
pub fn flush_soon(state: Arc<AppState>) {
    if web_allowed(&state).is_err() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        if let Ok(c) = state.work_cipher() {
            let _ = flush_pending(&state, &c).await;
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    use lettre::message::{header::ContentType, Mailbox};
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
    b.header(ContentType::TEXT_PLAIN).body(d.body.clone()).map_err(|e| e.to_string())
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
    web_allowed(state)?;
    let (mut cfg, replying) = {
        let conn = state.db.lock().unwrap();
        let cfg = account(&conn, c, account_id).ok_or("That account no longer exists.")?;
        let replying = d.reply_to.as_deref().and_then(|id| get(&conn, c, id)).map(|m| m.data);
        (cfg, replying)
    };
    refresh(state, c, Some(account_id), &mut cfg).await?;
    let msg = build(&cfg, d, replying.as_ref())?;
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
    // The list doesn't need whole bodies.
    for m in &mut items {
        m.data.body.clear();
        m.data.snippet = visible(&m.data.snippet);
    }
    Ok(items)
}

#[tauri::command(async)]
pub fn mail_get(state: AppStateRef, id: String) -> Result<MailItem, String> {
    ensure_on(&state)?;
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let m = get(&conn, &c, &id).ok_or("That message is no longer here.")?;
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
/// the mail server (now if online, else at the next sync).
#[tauri::command(async)]
pub fn mail_action(state: AppStateRef, ids: Vec<String>, action: String) -> Result<(), String> {
    ensure_on(&state)?;
    state.cipher()?;
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
