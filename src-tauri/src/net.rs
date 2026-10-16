// SPDX-License-Identifier: AGPL-3.0-only
//! Connectivity levels. Every request that leaves the PC asks this module first.

use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use crate::privacy::Route;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Connectivity {
    /// Nothing leaves the PC (default).
    #[default]
    Offline,
    /// Web access for search, browsing and account sync; AI stays local.
    Web,
    /// Cloud AI providers and cloud sync are allowed too.
    Cloud,
}

// Cloud is used once cloud providers (phase 6) land.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Engine or model download the user started. Allowed at every level:
    /// it only contacts the file host and carries no user data.
    ModelDownload,
    /// Web search and reading pages. Besides the levels and the chat's
    /// globe, the "Web search" feature allows it in every chat while the
    /// app stays Offline (callers pass that as `chat_web`).
    Search,
    /// The browser, email/calendar sync.
    Web,
    /// Cloud AI providers, cloud sync relay.
    Cloud,
}

/// `chat_web` is the per-chat globe toggle, which turns web on for one chat
/// while the app stays Offline.
pub fn allowed(level: Connectivity, purpose: Purpose, chat_web: bool) -> bool {
    // Fully offline: only what the person kept.
    let kept = match purpose {
        Purpose::ModelDownload => !crate::offline::blocks(crate::offline::Keep::Downloads),
        Purpose::Search | Purpose::Web => !crate::offline::blocks(crate::offline::Keep::Web),
        Purpose::Cloud => true,
    };
    kept && match purpose {
        Purpose::ModelDownload => true,
        Purpose::Search | Purpose::Web => level != Connectivity::Offline || chat_web,
        Purpose::Cloud => level == Connectivity::Cloud,
    }
}

pub fn blocked_message(purpose: Purpose) -> &'static str {
    match purpose {
        Purpose::ModelDownload => "",
        Purpose::Search => "Web search is off. Turn on web for this chat, switch to Local AI + Web, or turn on Web search in Features.",
        Purpose::Web => "This needs web access, which is off. Turn on web for this chat or switch to Local AI + Web.",
        Purpose::Cloud => "This needs a cloud provider, which is off. Switch to the Cloud level in Settings to use it.",
    }
}

/// Which private-connection switch covers this purpose (none for cloud).
fn route(purpose: Purpose) -> Option<Route> {
    match purpose {
        Purpose::Search => Some(Route::Web),
        Purpose::Web => Some(Route::Mail),
        Purpose::ModelDownload => Some(Route::Models),
        Purpose::Cloud => None,
    }
}

/// Sends a client the private way when the user turned that on for `route`.
pub fn routed(b: reqwest::ClientBuilder, route: Route) -> Result<reqwest::ClientBuilder, String> {
    crate::privacy::check(route)?;
    Ok(match crate::privacy::proxy_for(route) {
        Some(p) => b.proxy(reqwest::Proxy::all(&p).map_err(|e| e.to_string())?),
        None => b,
    })
}

/// Whether an address may be sent keys or private data: https, or plain
/// http only to this PC itself (exactly `localhost`, or a loopback address).
/// "http://localhost.example.com" is somewhere else entirely.
pub fn private_ok(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url) else { return false };
    let Some(host) = u.host_str() else { return false };
    match u.scheme() {
        "https" => true,
        "http" => host.eq_ignore_ascii_case("localhost") || host.trim_matches(['[', ']']).parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()),
        _ => false,
    }
}

/// Client for requests that leave the PC; refuses when the level forbids it.
pub fn external_client(level: Connectivity, purpose: Purpose, chat_web: bool) -> Result<reqwest::Client, String> {
    match purpose {
        Purpose::ModelDownload => crate::offline::check(crate::offline::Keep::Downloads)?,
        Purpose::Search | Purpose::Web => crate::offline::check(crate::offline::Keep::Web)?,
        Purpose::Cloud => {}
    }
    if !allowed(level, purpose, chat_web) {
        return Err(blocked_message(purpose).to_string());
    }
    let mut b = reqwest::Client::builder()
        .user_agent(concat!("SulcusAI/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(if route(purpose).and_then(crate::privacy::proxy_for).is_some() { 90 } else { 20 }));
    // Cloud providers and relays get keys: never follow them to another
    // host, or down to plain http (a key header would go along).
    if purpose == Purpose::Cloud {
        b = b.redirect(reqwest::redirect::Policy::custom(|a| {
            let same_host = a.previous().last().is_some_and(|p| p.host_str() == a.url().host_str());
            if a.previous().len() < 5 && same_host && private_ok(a.url().as_str()) {
                a.follow()
            } else {
                a.stop()
            }
        }));
    }
    let b = match route(purpose) {
        Some(r) => routed(b, r)?,
        None => b,
    };
    b.build().map_err(|e| e.to_string())
}

pub trait Stream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + Sync + std::fmt::Debug {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + Sync + std::fmt::Debug> Stream for T {}

/// A TCP connection (mail servers), the private way when that's on for `route`.
pub async fn connect(host: &str, port: u16, route: Route) -> Result<Box<dyn Stream>, String> {
    crate::privacy::check(route)?;
    connect_through(host, port, crate::privacy::proxy_for(route).as_deref()).await
}

/// A TCP connection directly, or through a SOCKS5 or HTTP (CONNECT) proxy.
pub async fn connect_through(host: &str, port: u16, proxy: Option<&str>) -> Result<Box<dyn Stream>, String> {
    let addr = format!("{host}:{port}");
    let Some(p) = proxy else {
        let tcp = tokio::time::timeout(Duration::from_secs(20), tokio::net::TcpStream::connect(&addr))
            .await
            .map_err(|_| format!("{addr} didn't answer."))?
            .map_err(|e| format!("Couldn't reach {addr}: {e}"))?;
        return Ok(Box::new(tcp));
    };
    let url = reqwest::Url::parse(p).map_err(|e| e.to_string())?;
    let proxy = format!("{}:{}", url.host_str().unwrap_or("127.0.0.1"), url.port_or_known_default().unwrap_or(1080));
    let user = percent_decode(url.username());
    let pass = percent_decode(url.password().unwrap_or(""));
    let fut = async {
        if url.scheme().starts_with("socks5") {
            let s = if user.is_empty() {
                tokio_socks::tcp::Socks5Stream::connect(proxy.as_str(), (host, port)).await
            } else {
                tokio_socks::tcp::Socks5Stream::connect_with_password(proxy.as_str(), (host, port), &user, &pass).await
            };
            s.map(|s| Box::new(s) as Box<dyn Stream>).map_err(|e| e.to_string())
        } else {
            http_connect(&proxy, &addr, &user, &pass).await.map(|s| Box::new(s) as Box<dyn Stream>)
        }
    };
    tokio::time::timeout(Duration::from_secs(120), fut)
        .await
        .map_err(|_| format!("{addr} didn't answer through the private connection."))?
        .map_err(|e| format!("Couldn't reach {addr} through the private connection: {e}"))
}

/// A user name or password as typed (URLs keep them %-encoded).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match (b[i], b.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok())) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A tunnel through an HTTP proxy (CONNECT), for mail through a proxy that
/// only speaks HTTP.
async fn http_connect(proxy: &str, addr: &str, user: &str, pass: &str) -> Result<tokio::net::TcpStream, String> {
    use base64::Engine;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(proxy).await.map_err(|e| format!("The proxy at {proxy} didn't answer: {e}"))?;
    let mut req = format!("CONNECT {addr} HTTP/1.1\r\nHost: {addr}\r\n");
    if !user.is_empty() {
        req += &format!("Proxy-Authorization: Basic {}\r\n", base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}")));
    }
    req += "\r\n";
    s.write_all(req.as_bytes()).await.map_err(|e| e.to_string())?;
    // The reply's head, one byte at a time so nothing after it is read.
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 8192 || s.read(&mut b).await.map_err(|e| e.to_string())? == 0 {
            return Err("the proxy closed the connection".into());
        }
        head.push(b[0]);
    }
    let status = String::from_utf8_lossy(&head).lines().next().unwrap_or("").to_string();
    if status.split_whitespace().nth(1) != Some("200") {
        return Err(format!("the proxy said {status}"));
    }
    Ok(s)
}

/// Client for the app's own engines on 127.0.0.1. Ignores system proxies,
/// which would otherwise intercept localhost traffic.
pub fn local_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("local HTTP client")
}

#[cfg(test)]
mod tests {
    use super::*;
    use Connectivity::*;

    /// Keys only go over https, or to this PC itself.
    #[test]
    fn only_this_pc_may_be_plain_http() {
        for ok in ["https://api.openai.com/v1", "http://localhost:11434/v1", "http://LOCALHOST/v1", "http://127.0.0.1:1234", "http://127.5.0.1", "http://[::1]:8080/v1"] {
            assert!(private_ok(ok), "{ok}");
        }
        for bad in [
            "http://localhost.attacker.example/v1",
            "http://127.0.0.1.attacker.example",
            "http://localhost@evil.example/",
            "http://192.168.1.5:11434",
            "http://api.openai.com/v1",
            "ftp://localhost",
            "localhost:11434",
            "",
        ] {
            assert!(!private_ok(bad), "{bad}");
        }
    }

    #[test]
    fn offline_blocks_web_and_cloud() {
        assert!(!allowed(Offline, Purpose::Web, false));
        assert!(!allowed(Offline, Purpose::Cloud, false));
        assert!(allowed(Offline, Purpose::ModelDownload, false));
    }

    #[test]
    fn chat_globe_enables_web_but_never_cloud() {
        assert!(allowed(Offline, Purpose::Web, true));
        assert!(!allowed(Offline, Purpose::Cloud, true));
        assert!(!allowed(Web, Purpose::Cloud, true));
    }

    #[test]
    fn search_follows_web_and_never_unlocks_cloud() {
        assert!(!allowed(Offline, Purpose::Search, false));
        // The Web search feature or the globe: search while Offline.
        assert!(allowed(Offline, Purpose::Search, true));
        assert!(allowed(Web, Purpose::Search, false));
        assert!(!allowed(Offline, Purpose::Cloud, true));
    }

    #[test]
    fn levels_are_cumulative() {
        assert!(allowed(Web, Purpose::Web, false));
        assert!(allowed(Cloud, Purpose::Web, false));
        assert!(allowed(Cloud, Purpose::Cloud, false));
    }

    #[test]
    fn external_client_refuses_when_blocked() {
        assert!(external_client(Offline, Purpose::Web, false).is_err());
        assert!(external_client(Offline, Purpose::ModelDownload, false).is_ok());
    }
}
