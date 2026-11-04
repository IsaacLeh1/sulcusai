// SPDX-License-Identifier: AGPL-3.0-only
//! Posting to Bluesky and Mastodon from the person's own accounts. Each post
//! is shown in full and sent only after the person presses Post; the model
//! can draft but never send. Sign-in details stay on this PC, encrypted.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Account {
    /// An app password made in Bluesky's settings, never the main password.
    Bluesky { handle: String, app_password: String, #[serde(default = "default_pds")] server: String },
    /// An access token from the instance's Development page (write:statuses).
    Mastodon { instance: String, token: String },
}

fn default_pds() -> String {
    "https://bsky.social".into()
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Shown {
    pub kind: &'static str,
    pub name: String,
    pub limit: usize,
}

impl Account {
    pub fn shown(&self) -> Shown {
        match self {
            Account::Bluesky { handle, .. } => Shown { kind: "bluesky", name: handle.clone(), limit: 300 },
            Account::Mastodon { instance, .. } => Shown { kind: "mastodon", name: instance.clone(), limit: 500 },
        }
    }
}

/// Only public https servers: a token sent to a local or plain-http address
/// could be read on the way.
pub fn check_server(raw: &str) -> Result<String, String> {
    let raw = raw.trim().trim_end_matches('/');
    let with = if raw.contains("://") { raw.to_string() } else { format!("https://{raw}") };
    let url = reqwest::Url::parse(&with).map_err(|_| "That server address doesn't look right.".to_string())?;
    if url.scheme() != "https" {
        return Err("The server must use https.".into());
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if host.parse::<std::net::IpAddr>().is_ok() || !host.contains('.') || host.ends_with(".local") || host.ends_with(".localhost") || host == "localhost" {
        return Err("Use the server's public name, like mastodon.social.".into());
    }
    Ok(format!("https://{host}"))
}

pub fn validate(a: Account) -> Result<Account, String> {
    match a {
        Account::Bluesky { handle, app_password, server } => {
            let handle = handle.trim().trim_start_matches('@').to_ascii_lowercase();
            if handle.is_empty() || handle.contains(char::is_whitespace) {
                return Err("Type the Bluesky handle, like name.bsky.social.".into());
            }
            let p = app_password.trim().to_string();
            // App passwords look like xxxx-xxxx-xxxx-xxxx; a main password is refused.
            let ok = p.len() == 19 && p.split('-').count() == 4 && p.split('-').all(|g| g.len() == 4 && g.chars().all(|c| c.is_ascii_alphanumeric()));
            if !ok {
                return Err("Use an app password (Settings, Privacy and security, App passwords), not the account password.".into());
            }
            Ok(Account::Bluesky { handle, app_password: p, server: check_server(&server)? })
        }
        Account::Mastodon { instance, token } => {
            let token = token.trim().to_string();
            if token.len() < 20 || token.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err("Paste the access token from the instance's Development page.".into());
            }
            Ok(Account::Mastodon { instance: check_server(&instance)?, token })
        }
    }
}

/// Text length the way the sites count it closely enough (characters).
pub fn check_text(text: &str, limit: usize) -> Result<String, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("The post is empty.".into());
    }
    let n = t.chars().count();
    if n > limit {
        return Err(format!("The post is {n} characters; the limit is {limit}."));
    }
    Ok(t.to_string())
}

pub fn bluesky_record(did: &str, text: &str, now: &str) -> Value {
    json!({ "repo": did, "collection": "app.bsky.feed.post",
            "record": { "$type": "app.bsky.feed.post", "text": text, "createdAt": now } })
}

/// Sends the post; returns a link to it when the site gives one.
pub async fn post(client: &reqwest::Client, a: &Account, text: &str) -> Result<String, String> {
    let text = check_text(text, a.shown().limit)?;
    let fail = |e: reqwest::Error| format!("Couldn't reach the server: {e}");
    match a {
        Account::Bluesky { handle, app_password, server } => {
            let s: Value = client
                .post(format!("{server}/xrpc/com.atproto.server.createSession"))
                .json(&json!({ "identifier": handle, "password": app_password }))
                .send().await.map_err(fail)?
                .error_for_status().map_err(|e| format!("Bluesky refused the sign-in ({}).", e.status().map_or(0, |s| s.as_u16())))?
                .json().await.map_err(fail)?;
            let (jwt, did) = (s["accessJwt"].as_str().unwrap_or_default(), s["did"].as_str().unwrap_or_default());
            if jwt.is_empty() || did.is_empty() {
                return Err("Bluesky's answer had no session.".into());
            }
            let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            let r: Value = client
                .post(format!("{server}/xrpc/com.atproto.repo.createRecord"))
                .bearer_auth(jwt)
                .json(&bluesky_record(did, &text, &now))
                .send().await.map_err(fail)?
                .error_for_status().map_err(|e| format!("Bluesky didn't take the post ({}).", e.status().map_or(0, |s| s.as_u16())))?
                .json().await.map_err(fail)?;
            let rkey = r["uri"].as_str().and_then(|u| u.rsplit('/').next()).unwrap_or_default();
            Ok(format!("https://bsky.app/profile/{handle}/post/{rkey}"))
        }
        Account::Mastodon { instance, token } => {
            let r: Value = client
                .post(format!("{instance}/api/v1/statuses"))
                .bearer_auth(token)
                .form(&[("status", text.as_str())])
                .send().await.map_err(fail)?
                .error_for_status().map_err(|e| format!("Mastodon didn't take the post ({}).", e.status().map_or(0, |s| s.as_u16())))?
                .json().await.map_err(fail)?;
            Ok(r["url"].as_str().unwrap_or_default().to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_must_be_public_https() {
        assert_eq!(check_server("mastodon.social/").unwrap(), "https://mastodon.social");
        assert!(check_server("http://mastodon.social").is_err());
        assert!(check_server("https://192.168.1.4").is_err());
        assert!(check_server("https://localhost").is_err());
        assert!(check_server("https://pi.local").is_err());
    }

    #[test]
    fn bluesky_needs_an_app_password() {
        let a = |p: &str| validate(Account::Bluesky { handle: "@Sam.bsky.social".into(), app_password: p.into(), server: default_pds() });
        assert!(a("hunter2").is_err());
        match a("abcd-efgh-ijkl-mnop").unwrap() {
            Account::Bluesky { handle, .. } => assert_eq!(handle, "sam.bsky.social"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn mastodon_token_shape() {
        assert!(validate(Account::Mastodon { instance: "mastodon.social".into(), token: "short".into() }).is_err());
        assert!(validate(Account::Mastodon { instance: "mastodon.social".into(), token: "a".repeat(43) }).is_ok());
    }

    #[test]
    fn text_limits() {
        assert!(check_text("   ", 300).is_err());
        assert!(check_text(&"é".repeat(300), 300).is_ok());
        assert!(check_text(&"x".repeat(301), 300).is_err());
    }

    #[test]
    fn bluesky_record_shape() {
        let r = bluesky_record("did:plc:x", "hi", "2026-11-04T00:00:00.000Z");
        assert_eq!(r["record"]["$type"], "app.bsky.feed.post");
        assert_eq!(r["repo"], "did:plc:x");
    }

    #[test]
    fn accounts_round_trip_without_secrets_in_shown() {
        let a = Account::Mastodon { instance: "https://m.example".into(), token: "t".repeat(30) };
        let back: Account = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(back, a);
        assert!(!serde_json::to_string(&a.shown()).unwrap().contains("ttt"));
    }
}
