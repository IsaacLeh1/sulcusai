// SPDX-License-Identifier: AGPL-3.0-only
//! Sign in with Microsoft or Google, for mail and calendars.
//!
//! OAuth 2.0 for desktop apps: the sign-in page opens in the user's own
//! browser (never inside the app), and comes back to a one-time listener on
//! 127.0.0.1 with a code that only this run can redeem (PKCE). The app never
//! sees the password. The refresh token is kept encrypted with the account,
//! and access tokens are refreshed as they expire.
//!
//! Each provider needs an app registration (a client ID; Google also issues
//! a client "secret", which for desktop apps isn't secret). They're set at
//! build time (SULCUSAI_MS_CLIENT_ID, SULCUSAI_GOOGLE_CLIENT_ID,
//! SULCUSAI_GOOGLE_CLIENT_SECRET) or in Settings › Accounts.

use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::db;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Microsoft,
    Google,
}

/// What the sign-in is for; each gets its own token.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum For {
    Mail,
    Calendar,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Provider::Microsoft => "Microsoft",
            Provider::Google => "Google",
        }
    }

    fn auth_url(self) -> &'static str {
        match self {
            Provider::Microsoft => "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
            Provider::Google => "https://accounts.google.com/o/oauth2/v2/auth",
        }
    }

    fn token_url(self) -> &'static str {
        match self {
            Provider::Microsoft => "https://login.microsoftonline.com/common/oauth2/v2.0/token",
            Provider::Google => "https://oauth2.googleapis.com/token",
        }
    }

    pub fn scopes(self, f: For) -> &'static str {
        match (self, f) {
            (Provider::Microsoft, For::Mail) => "openid email offline_access https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send",
            (Provider::Microsoft, For::Calendar) => "openid email offline_access https://graph.microsoft.com/Calendars.ReadWrite",
            (Provider::Google, For::Mail) => "openid email https://mail.google.com/",
            (Provider::Google, For::Calendar) => "openid email https://www.googleapis.com/auth/calendar",
        }
    }

    /// Microsoft desktop apps register `http://localhost` (any port);
    /// Google's loopback flow uses the address itself.
    fn redirect(self, port: u16) -> String {
        match self {
            Provider::Microsoft => format!("http://localhost:{port}"),
            Provider::Google => format!("http://127.0.0.1:{port}"),
        }
    }
}

/// An app registration with the provider.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ClientIds {
    pub microsoft: String,
    pub google: String,
    pub google_secret: String,
}

const CLIENTS_KEY: &str = "oauth_clients";

pub fn clients(conn: &Connection) -> ClientIds {
    let saved: ClientIds = db::get(conn, CLIENTS_KEY).unwrap_or_default();
    let pick = |s: String, built: Option<&'static str>| if s.trim().is_empty() { built.unwrap_or("").to_string() } else { s.trim().to_string() };
    ClientIds {
        microsoft: pick(saved.microsoft, option_env!("SULCUSAI_MS_CLIENT_ID")),
        google: pick(saved.google, option_env!("SULCUSAI_GOOGLE_CLIENT_ID")),
        google_secret: pick(saved.google_secret, option_env!("SULCUSAI_GOOGLE_CLIENT_SECRET")),
    }
}

pub fn set_clients(conn: &Connection, ids: &ClientIds) -> Result<(), String> {
    db::set(conn, CLIENTS_KEY, ids)
}

struct Client {
    id: String,
    secret: Option<String>,
}

fn client(ids: &ClientIds, p: Provider) -> Result<Client, String> {
    let (id, secret) = match p {
        Provider::Microsoft => (ids.microsoft.clone(), None),
        Provider::Google => (ids.google.clone(), Some(ids.google_secret.clone()).filter(|s| !s.is_empty())),
    };
    if id.is_empty() {
        return Err(format!("Signing in with {} needs this copy of SulcusAI to be registered with {}. Add the app's client ID in Settings › Accounts.", p.name(), p.name()));
    }
    Ok(Client { id, secret })
}

/// What's kept (encrypted) with a mail or calendar account.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Tokens {
    pub provider: Option<Provider>,
    pub purpose: Option<For>,
    pub email: String,
    pub refresh_token: String,
    pub access_token: String,
    /// When the access token runs out (ms).
    pub expires_at: i64,
}

/// A PKCE verifier and its S256 challenge.
fn pkce() -> (String, String) {
    let verifier = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn query(pairs: &[(&str, &str)]) -> String {
    let mut u = reqwest::Url::parse("http://x/").unwrap();
    {
        let mut q = u.query_pairs_mut();
        for (k, v) in pairs {
            q.append_pair(k, v);
        }
    }
    u.query().unwrap_or("").to_string()
}

pub fn authorize_url(p: Provider, f: For, client_id: &str, redirect: &str, challenge: &str, state: &str, hint: Option<&str>) -> String {
    let mut pairs = vec![
        ("client_id", client_id),
        ("response_type", "code"),
        ("redirect_uri", redirect),
        ("scope", p.scopes(f)),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
    ];
    match p {
        Provider::Google => pairs.extend([("access_type", "offline"), ("prompt", "consent")]),
        Provider::Microsoft => pairs.push(("prompt", "select_account")),
    }
    if let Some(h) = hint.filter(|h| !h.is_empty()) {
        pairs.push(("login_hint", h));
    }
    format!("{}?{}", p.auth_url(), query(&pairs))
}

/// The signed-in address, from the ID token's claims (it came straight from
/// the provider over TLS, so its signature isn't checked here).
pub fn email_from_id_token(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?).ok()?;
    ["email", "preferred_username", "upn"].iter().find_map(|k| claims[*k].as_str().filter(|s| s.contains('@')).map(str::to_string))
}

/// The code (and state) from the browser's request to the listener.
pub fn parse_redirect(request_line: &str) -> Result<(String, String), String> {
    let target = request_line.split_whitespace().nth(1).ok_or("Bad request")?;
    let u = reqwest::Url::parse(&format!("http://x{target}")).map_err(|e| e.to_string())?;
    let get = |k: &str| u.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
    if let Some(err) = get("error") {
        let detail = get("error_description").unwrap_or_default();
        return Err(if err == "access_denied" { "Sign-in was cancelled.".to_string() } else { format!("The sign-in didn't work: {err} {detail}").trim().to_string() });
    }
    Ok((get("code").ok_or("No sign-in code came back.")?, get("state").unwrap_or_default()))
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>SulcusAI</title><body style=\"font:16px system-ui;margin:3em;color:#222\"><h2>You're signed in.</h2><p>You can close this tab and go back to SulcusAI.</p>";
const FAILED_PAGE: &str = "<!doctype html><meta charset=utf-8><title>SulcusAI</title><body style=\"font:16px system-ui;margin:3em;color:#222\"><h2>Sign-in didn't finish.</h2><p>Go back to SulcusAI and try again.</p>";

/// Waits for the browser to come back with the code.
async fn wait_for_code(listener: tokio::net::TcpListener, state: &str, timeout: Duration) -> Result<String, String> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let (mut sock, _) = tokio::time::timeout_at(deadline, listener.accept())
            .await
            .map_err(|_| "Sign-in timed out. Try again, and finish signing in within 5 minutes.".to_string())?
            .map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 8192];
        let n = tokio::time::timeout(Duration::from_secs(10), sock.read(&mut buf)).await.ok().and_then(Result::ok).unwrap_or(0);
        let text = String::from_utf8_lossy(&buf[..n]).to_string();
        let line = text.lines().next().unwrap_or("");
        // Browsers also ask for /favicon.ico; only the redirect counts.
        if !line.contains("code=") && !line.contains("error=") {
            let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            continue;
        }
        let result = parse_redirect(line).and_then(|(code, st)| if st == state { Ok(code) } else { Err("The sign-in didn't match this request. Try again.".to_string()) });
        let page = if result.is_ok() { DONE_PAGE } else { FAILED_PAGE };
        let _ = sock
            .write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}", page.len()).as_bytes())
            .await;
        return result;
    }
}

fn token_error(v: &Value, status: reqwest::StatusCode) -> String {
    let e = v["error"].as_str().unwrap_or("");
    let d = v["error_description"].as_str().unwrap_or("");
    if e == "invalid_grant" {
        return "The sign-in has expired or was revoked. Sign in again.".into();
    }
    format!("The sign-in server answered {status}: {e} {}", d.lines().next().unwrap_or("")).trim().to_string()
}

async fn token_request(client: &reqwest::Client, url: &str, form: &[(&str, &str)]) -> Result<Value, String> {
    let resp = client.post(url).form(form).send().await.map_err(|e| format!("Couldn't reach the sign-in server: {e}"))?;
    let status = resp.status();
    let v: Value = resp.json().await.map_err(|e| format!("The sign-in server sent an unreadable answer: {e}"))?;
    if !status.is_success() || v.get("access_token").is_none() {
        return Err(token_error(&v, status));
    }
    Ok(v)
}

fn tokens_from(v: &Value, p: Provider, f: For, old_refresh: &str) -> Tokens {
    Tokens {
        provider: Some(p),
        purpose: Some(f),
        email: v["id_token"].as_str().and_then(email_from_id_token).unwrap_or_default(),
        // Providers may or may not hand out a new refresh token.
        refresh_token: v["refresh_token"].as_str().unwrap_or(old_refresh).to_string(),
        access_token: v["access_token"].as_str().unwrap_or("").to_string(),
        expires_at: db::now_ms() + v["expires_in"].as_i64().unwrap_or(3600) * 1000,
    }
}

/// Opens the sign-in page in the user's own browser (sign-in pages refuse
/// embedded browsers), using their main browser if they chose one.
fn open_in_browser(conn: &Connection, url: &str) -> Result<(), String> {
    use crate::browsers::{choice, installed, MainBrowser};
    if let MainBrowser::App { name } = choice(conn) {
        if let Some(b) = installed().into_iter().find(|b| b.name == name) {
            return std::process::Command::new(&b.exe).arg(url).spawn().map(|_| ()).map_err(|e| e.to_string());
        }
    }
    crate::open_with_system(std::ffi::OsStr::new(url))
}

/// The whole sign-in: browser, code, tokens. `hint` pre-fills the address.
pub async fn sign_in(state: &crate::AppState, p: Provider, f: For, hint: Option<&str>) -> Result<Tokens, String> {
    let ids = clients(&state.db.lock().unwrap());
    let client = client(&ids, p)?;
    let http = crate::net::external_client(state.settings().connectivity, crate::net::Purpose::Web, false)?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = p.redirect(port);
    let (verifier, challenge) = pkce();
    let st = uuid::Uuid::new_v4().simple().to_string();
    let url = authorize_url(p, f, &client.id, &redirect, &challenge, &st, hint);
    open_in_browser(&state.db.lock().unwrap(), &url)?;
    state.log("network", &format!("Opened {} sign-in in the browser", p.name()));
    let code = wait_for_code(listener, &st, Duration::from_secs(300)).await?;
    let mut form = vec![
        ("client_id", client.id.as_str()),
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect.as_str()),
        ("code_verifier", verifier.as_str()),
    ];
    if let Some(s) = &client.secret {
        form.push(("client_secret", s.as_str()));
    }
    let v = token_request(&http, p.token_url(), &form).await?;
    let t = tokens_from(&v, p, f, "");
    if t.refresh_token.is_empty() {
        return Err(format!("{} didn't allow staying signed in. Try again.", p.name()));
    }
    if t.email.is_empty() {
        return Err(format!("{} didn't say which account signed in.", p.name()));
    }
    state.log("privacy", &format!("Signed in to {} as {}", p.name(), t.email));
    Ok(t)
}

/// A current access token, refreshing it when it's about to run out.
/// Returns whether the tokens changed (so the caller saves them).
pub async fn fresh(state: &crate::AppState, t: &mut Tokens) -> Result<bool, String> {
    if !t.access_token.is_empty() && t.expires_at > db::now_ms() + 120_000 {
        return Ok(false);
    }
    let p = t.provider.ok_or("This account's sign-in is incomplete. Sign in again.")?;
    let f = t.purpose.unwrap_or(For::Mail);
    let ids = clients(&state.db.lock().unwrap());
    let client = client(&ids, p)?;
    let http = crate::net::external_client(state.settings().connectivity, crate::net::Purpose::Web, false)?;
    let mut form = vec![("client_id", client.id.as_str()), ("grant_type", "refresh_token"), ("refresh_token", t.refresh_token.as_str())];
    if p == Provider::Microsoft {
        form.push(("scope", p.scopes(f)));
    }
    if let Some(s) = &client.secret {
        form.push(("client_secret", s.as_str()));
    }
    let v = token_request(&http, p.token_url(), &form).await?;
    let email = t.email.clone();
    *t = tokens_from(&v, p, f, &t.refresh_token);
    if t.email.is_empty() {
        t.email = email;
    }
    Ok(true)
}

/// The SASL XOAUTH2 string for IMAP and SMTP.
pub fn xoauth2(user: &str, token: &str) -> String {
    format!("user={user}\x01auth=Bearer {token}\x01\x01")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_authorize_address_asks_for_a_pkce_code() {
        let (verifier, challenge) = pkce();
        assert!(verifier.len() >= 43 && verifier.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_eq!(challenge, URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())));
        let u = reqwest::Url::parse(&authorize_url(Provider::Google, For::Calendar, "cid", "http://127.0.0.1:5000", &challenge, "st", Some("me@gmail.com"))).unwrap();
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["access_type"], "offline");
        assert_eq!(q["login_hint"], "me@gmail.com");
        assert!(q["scope"].contains("auth/calendar"));
        let m = authorize_url(Provider::Microsoft, For::Mail, "cid", &Provider::Microsoft.redirect(5000), &challenge, "st", None);
        assert!(m.starts_with("https://login.microsoftonline.com/common/"));
        assert!(m.contains("redirect_uri=http%3A%2F%2Flocalhost%3A5000") && m.contains("IMAP.AccessAsUser.All"));
    }

    #[test]
    fn redirects_are_read() {
        assert_eq!(parse_redirect("GET /?code=abc%2F1&state=xyz HTTP/1.1").unwrap(), ("abc/1".to_string(), "xyz".to_string()));
        assert_eq!(parse_redirect("GET /?error=access_denied&state=xyz HTTP/1.1").unwrap_err(), "Sign-in was cancelled.");
    }

    #[test]
    fn the_address_comes_from_the_id_token() {
        let payload = URL_SAFE_NO_PAD.encode(br#"{"email":"ana@example.com","name":"Ana"}"#);
        assert_eq!(email_from_id_token(&format!("h.{payload}.s")).as_deref(), Some("ana@example.com"));
        let ms = URL_SAFE_NO_PAD.encode(br#"{"preferred_username":"bo@outlook.com"}"#);
        assert_eq!(email_from_id_token(&format!("h.{ms}.s")).as_deref(), Some("bo@outlook.com"));
        assert_eq!(email_from_id_token("garbage"), None);
    }

    #[test]
    fn xoauth2_is_formatted_for_sasl() {
        assert_eq!(xoauth2("a@b.c", "tok"), "user=a@b.c\u{1}auth=Bearer tok\u{1}\u{1}");
    }

    #[test]
    fn missing_registrations_say_what_to_do() {
        let e = client(&ClientIds::default(), Provider::Microsoft).err().unwrap();
        assert!(e.contains("Settings › Accounts"));
        let g = client(&ClientIds { google: "g".into(), google_secret: "s".into(), ..Default::default() }, Provider::Google).ok().unwrap();
        assert_eq!(g.secret.as_deref(), Some("s"));
    }

    /// The listener and the token exchange, against a stand-in provider.
    #[tokio::test]
    async fn the_code_comes_back_through_the_listener() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let waiting = tokio::spawn(async move { wait_for_code(listener, "s1", Duration::from_secs(5)).await });
        // A favicon request first, then the redirect.
        let mut a = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        a.write_all(b"GET /favicon.ico HTTP/1.1\r\n\r\n").await.unwrap();
        let mut sink = Vec::new();
        a.read_to_end(&mut sink).await.unwrap();
        let mut b = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        b.write_all(b"GET /?code=C0DE&state=s1 HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();
        let mut page = String::new();
        b.read_to_string(&mut page).await.unwrap();
        assert!(page.contains("You're signed in"));
        assert_eq!(waiting.await.unwrap().unwrap(), "C0DE");

        let fake = tokens_from(&serde_json::json!({ "access_token": "a", "expires_in": 60 }), Provider::Google, For::Mail, "keep-me");
        assert_eq!(fake.refresh_token, "keep-me", "an old refresh token is kept when none comes back");
    }
}
