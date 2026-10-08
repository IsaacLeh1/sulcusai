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
            Some("Gmail needs an app password: Google Account › Security › 2-Step Verification › App passwords. Signing in with Google directly is coming."),
        ),
        "outlook.com" | "hotmail.com" | "live.com" | "msn.com" => (
            with("outlook.office365.com", "smtp-mail.outlook.com", 587, Security::Starttls),
            Some("Microsoft has turned off password sign-in for Outlook.com mail; signing in with Microsoft is coming. Work accounts may still allow it."),
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
    let snippet: String = body.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect();
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
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_string).collect();
    all.into_iter()
        .filter(|m| {
            if words.is_empty() {
                return true;
            }
            let hay = format!("{} {} {} {} {}", m.data.from, m.data.from_name, m.data.to.join(" "), m.data.subject, m.data.body).to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .take(limit)
        .collect()
}

pub fn get(conn: &Connection, c: &Cipher, id: &str) -> Option<MailItem> {
    let sql = format!("SELECT {ITEM_COLS} FROM mail_messages WHERE id = ?1");
    conn.query_row(&sql, [id], item_from_row(c)).optional().ok().flatten()
}

fn save_item(conn: &Connection, c: &Cipher, account_id: &str, folder: &str, uid: u32, date: i64, seen: bool, data: &MailData) -> Result<(), String> {
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

fn store_account(conn: &Connection, c: &Cipher, id: &str, cfg: &AccountConfig) -> Result<(), String> {
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
    let addr = format!("{}:{}", cfg.imap_host, cfg.imap_port);
    let tcp = tokio::time::timeout(Duration::from_secs(20), tokio::net::TcpStream::connect(&addr))
        .await
        .map_err(|_| format!("{addr} didn't answer."))?
        .map_err(|e| format!("Couldn't reach {addr}: {e}"))?;
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
    let mut s = imap_login(&cfg).await?;
    if cfg.sent_folder.is_none() {
        cfg.sent_folder = find_sent(&mut s).await;
    }
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
    let _ = s.logout().await;
    let conn = state.db.lock().unwrap();
    store_account(&conn, c, id, &cfg)?;
    conn.execute("UPDATE mail_accounts SET synced_at = ?2 WHERE id = ?1", params![id, db::now_ms()]).map_err(|e| e.to_string())?;
    Ok(added)
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
    let user = if cfg.username.is_empty() { cfg.email.clone() } else { cfg.username.clone() };
    let creds = Credentials::new(user, cfg.password.clone());
    let tls = || TlsParameters::new(cfg.smtp_host.clone()).map_err(|e| e.to_string());
    let builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.smtp_host).port(cfg.smtp_port).timeout(Some(Duration::from_secs(30)));
    let builder = match cfg.smtp_security {
        Security::Tls => builder.tls(Tls::Wrapper(tls()?)),
        Security::Starttls => builder.tls(Tls::Required(tls()?)),
        Security::Plain => builder.tls(Tls::None),
    };
    let mailer = builder.credentials(creds).build();
    mailer.send(msg.clone()).await.map_err(friendly)?;
    Ok(())
}

/// Sends a message and files a copy in Sent. Only ever called after the
/// user said so (the Send button, or approving the assistant's draft).
pub async fn send(state: &AppState, c: &Cipher, account_id: &str, d: &Draft) -> Result<(), String> {
    web_allowed(state)?;
    let (cfg, replying) = {
        let conn = state.db.lock().unwrap();
        let cfg = account(&conn, c, account_id).ok_or("That account no longer exists.")?;
        let replying = d.reply_to.as_deref().and_then(|id| get(&conn, c, id)).map(|m| m.data);
        (cfg, replying)
    };
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
    db::log_action(&state.db.lock().unwrap(), "email", &format!("Sent an email to {to}"));
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
            let Ok(c) = state.cipher() else { continue };
            if !ready || web_allowed(&state).is_err() {
                continue;
            }
            if let Ok(n) = sync_all(&state, &c).await {
                app.emit("mail:synced", json!({ "new": n })).ok();
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

#[tauri::command]
pub fn mail_preset(email: String) -> PresetView {
    let (config, note) = preset(email.trim());
    PresetView { config, note }
}

#[tauri::command]
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
        db::log_action(&conn, "email", &format!("Added the email account {}", cfg.email));
    }
    sync_account(&state, &c, &id).await?;
    Ok(id)
}

#[tauri::command]
pub fn remove_mail_account(state: AppStateRef, id: String) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    conn.execute("DELETE FROM mail_messages WHERE account_id = ?1", [&id]).map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM mail_accounts WHERE id = ?1", [&id]).map_err(|e| e.to_string())?;
    db::log_action(&conn, "email", "Removed an email account and its copied mail");
    Ok(())
}

#[tauri::command]
pub async fn sync_mail(state: AppStateRef<'_>) -> Result<usize, String> {
    ensure_on(&state)?;
    let c = state.cipher()?;
    sync_all(&state, &c).await
}

#[tauri::command]
pub fn mail_list(state: AppStateRef, query: Option<String>, limit: Option<usize>) -> Result<Vec<MailItem>, String> {
    ensure_on(&state)?;
    let c = state.cipher()?;
    let mut items = list(&state.db.lock().unwrap(), &c, query.as_deref().unwrap_or(""), limit.unwrap_or(200));
    // The list doesn't need whole bodies.
    for m in &mut items {
        m.data.body.clear();
    }
    Ok(items)
}

#[tauri::command]
pub fn mail_get(state: AppStateRef, id: String) -> Result<MailItem, String> {
    ensure_on(&state)?;
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let m = get(&conn, &c, &id).ok_or("That message is no longer here.")?;
    conn.execute("UPDATE mail_messages SET seen = 1 WHERE id = ?1", [&id]).ok();
    Ok(m)
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
}
