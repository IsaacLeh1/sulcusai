// SPDX-License-Identifier: AGPL-3.0-only
//! Reaching a paired phone from anywhere: this PC keeps one WebSocket open to
//! SulcusAI's relay on Cloudflare (relay/worker.js, "phone link") and answers
//! the phone through it. The PC hosts, so its phone works while it's on and
//! SulcusAI is open; the relay only passes messages on. The phone loads the
//! app's files from here, and its requests and these answers are sealed with
//! its key, so the relay can't read them.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use crate::AppState;

/// SulcusAI's relay (the one the Bug / Feedback button uses).
pub const RELAY: &str = "https://sulcusai-relay.isaacleh1.workers.dev";
/// Raw bytes in one message to the relay (it takes messages under 1 MB).
const PART: usize = 600_000;
/// A keep-alive this often (the relay answers it without waking).
const PING: Duration = Duration::from_secs(25);
/// No word from the relay for this long: the connection is gone.
const SILENT: Duration = Duration::from_secs(70);

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Status {
    pub connected: bool,
    pub problem: Option<String>,
    /// The phone's address through the relay (without the key).
    pub address: Option<String>,
}

static STATUS: LazyLock<Mutex<Status>> = LazyLock::new(Default::default);
static STOP: Mutex<Option<oneshot::Sender<()>>> = Mutex::new(None);

pub fn status() -> Status {
    STATUS.lock().unwrap().clone()
}

/// The relay's address for this PC's phone page.
pub fn page_address(relay: &str, id: &str) -> String {
    format!("{}/l/{id}/", relay.trim_end_matches('/'))
}

/// Starts or stops the link: on while the phone page is on, "Reach this phone
/// from anywhere" is on and a phone is paired.
pub fn apply(state: Arc<AppState>, relay: Option<String>, id: String, token: String) {
    if let Some(stop) = STOP.lock().unwrap().take() {
        let _ = stop.send(());
    }
    let Some(relay) = relay else {
        *STATUS.lock().unwrap() = Status::default();
        return;
    };
    if let Err(e) = crate::offline::check(crate::offline::Keep::LocalNetwork) {
        *STATUS.lock().unwrap() = Status { connected: false, problem: Some(e), address: None };
        return;
    }
    *STATUS.lock().unwrap() = Status { connected: false, problem: None, address: Some(page_address(&relay, &id)) };
    let (tx, rx) = oneshot::channel();
    *STOP.lock().unwrap() = Some(tx);
    tauri::async_runtime::spawn(run(state, relay, id, token, rx));
}

async fn run(state: Arc<AppState>, relay: String, id: String, token: String, mut stop: oneshot::Receiver<()>) {
    let mut wait = 1u64;
    loop {
        let started = Instant::now();
        let result = tokio::select! {
            _ = &mut stop => return,
            r = session(&state, &relay, &id, &token) => r,
        };
        {
            let mut s = STATUS.lock().unwrap();
            s.connected = false;
            if let Err(e) = &result {
                s.problem = Some(e.clone());
            }
        }
        if let Err(e) = &result {
            crate::applog::line("phone", &format!("The phone link dropped ({e}); trying again in {wait} s"));
        }
        // A connection that lasted a while starts the waiting over.
        if started.elapsed() > Duration::from_secs(60) {
            wait = 1;
        }
        tokio::select! {
            _ = &mut stop => return,
            _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
        }
        wait = (wait * 2).min(60);
    }
}

/// Where the relay's WebSocket for this PC is.
struct Address {
    url: String,
    host: String,
    port: u16,
    /// Over TLS: always, except a test relay on this PC itself.
    tls: bool,
}

/// The relay's WebSocket address for this PC.
fn host_url(relay: &str, id: &str) -> Result<Address, String> {
    let base = tauri::Url::parse(relay.trim_end_matches('/')).map_err(|_| "The relay address isn't valid.".to_string())?;
    let host = base.host_str().ok_or("The relay address has no host.")?.to_string();
    let local = matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]");
    let tls = match base.scheme() {
        "https" => true,
        "http" if local => false,
        _ => return Err("The relay address must start with https://.".into()),
    };
    let port = base.port().unwrap_or(if tls { 443 } else { 80 });
    let path = base.path().trim_end_matches('/');
    let authority = if base.port().is_none() { host.clone() } else { format!("{host}:{port}") };
    let scheme = if tls { "wss" } else { "ws" };
    Ok(Address { url: format!("{scheme}://{authority}{path}/v1/link/{id}/host"), host, port, tls })
}

/// A stream to the relay: TLS, or plain for a test relay on this PC.
trait Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Io for T {}

/// One connection to the relay, until it closes.
async fn session(state: &Arc<AppState>, relay: &str, id: &str, token: &str) -> Result<(), String> {
    let Address { url, host, port, tls } = host_url(relay, id)?;
    let tcp = tokio::time::timeout(Duration::from_secs(20), tokio::net::TcpStream::connect((host.trim_matches(['[', ']']), port)))
        .await
        .map_err(|_| "The relay didn't answer.".to_string())?
        .map_err(|e| format!("Couldn't reach the relay: {e}"))?;
    let stream: Box<dyn Io> = if tls {
        let name = tokio_rustls::rustls::pki_types::ServerName::try_from(host.clone()).map_err(|e| e.to_string())?;
        Box::new(tokio_rustls::TlsConnector::from(crate::mail::tls_config()).connect(name, tcp).await.map_err(|e| format!("Secure connection to the relay failed: {e}"))?)
    } else {
        Box::new(tcp)
    };
    let mut request = url.into_client_request().map_err(|e| e.to_string())?;
    let headers = request.headers_mut();
    headers.insert("authorization", format!("Bearer {token}").parse().map_err(|_| "bad token")?);
    headers.insert("user-agent", concat!("SulcusAI/", env!("CARGO_PKG_VERSION")).parse().map_err(|_| "bad agent")?);
    let (ws, _) = tokio::time::timeout(Duration::from_secs(20), tokio_tungstenite::client_async(request, stream))
        .await
        .map_err(|_| "The relay didn't answer.".to_string())?
        .map_err(|e| match e {
            tokio_tungstenite::tungstenite::Error::Http(r) if r.status() == 401 => "The relay refused this PC's link key. Turn \"Reach this phone from anywhere\" off and on again to make a new link.".to_string(),
            e => format!("The relay refused the connection: {e}"),
        })?;
    {
        let mut s = STATUS.lock().unwrap();
        s.connected = true;
        s.problem = None;
    }
    crate::applog::line("phone", "The phone link is connected");
    let (mut sink, mut stream) = ws.split();
    let (out, mut outgoing) = mpsc::unbounded_channel::<String>();
    // Requests from the phone arrive in parts: rid -> what came so far.
    let mut parts: HashMap<u64, String> = HashMap::new();
    let mut ping = tokio::time::interval(PING);
    let mut heard = Instant::now();
    loop {
        tokio::select! {
            _ = ping.tick() => {
                if heard.elapsed() > SILENT {
                    return Err("the relay stopped answering".into());
                }
                sink.send(Message::Text("ping".into())).await.map_err(|e| e.to_string())?;
            }
            Some(frame) = outgoing.recv() => {
                sink.send(Message::Text(frame.into())).await.map_err(|e| e.to_string())?;
            }
            msg = stream.next() => {
                heard = Instant::now();
                let text = match msg {
                    None => return Err("the relay closed the connection".into()),
                    Some(Err(e)) => return Err(e.to_string()),
                    Some(Ok(Message::Text(t))) => t.to_string(),
                    Some(Ok(Message::Close(_))) => return Err("the relay closed the connection".into()),
                    Some(Ok(_)) => continue,
                };
                if text == "pong" {
                    continue;
                }
                let Ok(frame) = serde_json::from_str::<Value>(&text) else { continue };
                let Some(rid) = frame["rid"].as_u64() else { continue };
                match frame["t"].as_str() {
                    Some("get") => {
                        let (state, out, path) = (state.clone(), out.clone(), frame["path"].as_str().unwrap_or("/").to_string());
                        tokio::spawn(async move {
                            let (status, headers, body) = super::serve_file(&state, &path);
                            for f in frames(rid, status, headers, &body) {
                                let _ = out.send(f);
                            }
                        });
                    }
                    Some("api") => {
                        let piece = parts.entry(rid).or_default();
                        piece.push_str(frame["data"].as_str().unwrap_or_default());
                        if piece.len() > 40_000_000 {
                            parts.remove(&rid);
                            continue;
                        }
                        if frame["more"].as_bool() == Some(true) {
                            continue;
                        }
                        let body = parts.remove(&rid).unwrap_or_default();
                        let phone = frame["phone"].as_str().unwrap_or_default().to_string();
                        let (state, out) = (state.clone(), out.clone());
                        tokio::spawn(async move {
                            let (status, text) = super::answer(&state, Some(&phone), &body).await;
                            let headers = json!({ "content-type": "text/plain; charset=utf-8", "cache-control": "no-store" });
                            for f in frames(rid, status.as_u16(), headers, text.as_bytes()) {
                                let _ = out.send(f);
                            }
                        });
                    }
                    _ => {}
                }
            }
        }
    }
}

/// An answer as relay messages: the status and headers first, the body in
/// parts (`more` until the last).
fn frames(rid: u64, status: u16, headers: Value, body: &[u8]) -> Vec<String> {
    if body.is_empty() {
        return vec![json!({ "rid": rid, "status": status, "headers": headers }).to_string()];
    }
    let chunks: Vec<&[u8]> = body.chunks(PART).collect();
    let last = chunks.len() - 1;
    chunks
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut f = json!({ "rid": rid, "data": B64.encode(c), "more": i < last });
            if i == 0 {
                f["status"] = json!(status);
                f["headers"] = headers.clone();
            }
            f.to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_answers_go_in_parts_under_a_megabyte() {
        let body = vec![7u8; PART * 2 + 10];
        let f = frames(9, 200, json!({ "content-type": "x" }), &body);
        assert_eq!(f.len(), 3);
        let first: Value = serde_json::from_str(&f[0]).unwrap();
        assert_eq!(first["status"], 200);
        assert_eq!(first["more"], true);
        let last: Value = serde_json::from_str(&f[2]).unwrap();
        assert_eq!(last["more"], false);
        assert!(last.get("status").is_none());
        assert!(f.iter().all(|x| x.len() < 1_000_000));
        let joined: Vec<u8> = f.iter().flat_map(|x| B64.decode(serde_json::from_str::<Value>(x).unwrap()["data"].as_str().unwrap()).unwrap()).collect();
        assert_eq!(joined, body);
        assert_eq!(frames(1, 404, json!({}), b"").len(), 1);
    }

    #[test]
    fn the_relay_address_becomes_its_websocket() {
        let a = host_url("https://sulcusai-relay.example.workers.dev/", "ab").unwrap();
        assert_eq!(a.url, "wss://sulcusai-relay.example.workers.dev/v1/link/ab/host");
        assert_eq!((a.host.as_str(), a.port, a.tls), ("sulcusai-relay.example.workers.dev", 443, true));
        assert!(host_url("http://plain.example", "ab").is_err(), "plain http only to this PC");
        let local = host_url("http://127.0.0.1:8787", "ab").unwrap();
        assert_eq!((local.url.as_str(), local.port, local.tls), ("ws://127.0.0.1:8787/v1/link/ab/host", 8787, false));
        assert_eq!(page_address("https://r.example/", "ab"), "https://r.example/l/ab/");
    }
}
