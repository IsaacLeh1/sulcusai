// SPDX-License-Identifier: AGPL-3.0-only
//! People across the internet with nothing to set up (no Radmin or other
//! VPN): the two apps meet in a room on SulcusAI's relay and the live link
//! runs through it, sealed exactly as on a network, so the relay passes
//! bytes it can't read. The relay also says when the other app is in the
//! room, which is what shows them as online.
//!
//! - A room comes from what only the two apps share: their link key (to
//!   talk), or the code being typed (to link). Each app shows the relay the
//!   room's token, and the room's name is where the token's hash starts, so
//!   the relay can check it and a name seen anywhere isn't enough to get in.
//! - One pipe per person, kept while both apps run and no direct connection
//!   is up; the app with the smaller id makes the call through it, as it
//!   would on a network.
//! - A code that's showing waits in its room too, so it can be typed
//!   anywhere, not only on the same network.
//! - Not while fully offline without People (`offline::Keep::LocalNetwork`).

use std::sync::LazyLock;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use super::*;

/// SulcusAI's relay, built in (its pipe needs no key: rooms are secrets).
pub const RELAY: &str = "sulcusai-relay.isaacleh1.workers.dev";
/// Bytes per message through the relay (it takes up to 1 MB).
const CHUNK: usize = 64 * 1024;
/// How long one wait in a room lasts before it's opened again.
const WAIT: Duration = Duration::from_secs(600);

/// A room on the relay: its name, and the token that gets an app in.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Room {
    pub name: String,
    token: String,
}

/// The room for a secret only the two apps have: the token (64 hex
/// characters) from the secret, the name (32) from the token's hash.
fn room(kind: &str, secret: &[u8]) -> Room {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"sulcusai pipe ");
    h.update(kind.as_bytes());
    h.update([0]);
    h.update(secret);
    let token = hex::encode(h.finalize());
    let name = hex::encode(Sha256::digest(token.as_bytes()))[..32].to_string();
    Room { name, token }
}

pub(super) fn link_room(key: &[u8]) -> Room {
    room("link", key)
}

/// The same room however the code is written (dashes, case, O for 0).
pub(super) fn code_room(code: &str) -> Room {
    room("code", clean_code(code).unwrap_or_else(|| code.to_uppercase()).as_bytes())
}

/// Allowed now: People on, the data key open, not fully offline without it.
fn allowed(state: &AppState) -> bool {
    state.work_cipher().is_ok() && !crate::offline::blocks(crate::offline::Keep::LocalNetwork) && features::is_on(&state.db.lock().unwrap(), Feature::People)
}

/// Waits in `room` as `side` until the other side is there too (up to
/// `wait`), then the pipe as a stream: what's written goes to them, what
/// they write comes out. It ends when either leaves.
pub(super) async fn meet(room: &Room, side: &str, wait: Duration) -> Result<DuplexStream, String> {
    let tcp = timeout(Duration::from_secs(20), TcpStream::connect((RELAY, 443)))
        .await
        .map_err(|_| "SulcusAI's relay didn't answer.".to_string())?
        .map_err(|e| format!("Couldn't reach SulcusAI's relay: {e}"))?;
    let name = tokio_rustls::rustls::pki_types::ServerName::try_from(RELAY.to_string()).map_err(|e| e.to_string())?;
    let tls = tokio_rustls::TlsConnector::from(crate::mail::tls_config()).connect(name, tcp).await.map_err(|e| format!("Secure connection to the relay failed: {e}"))?;
    let mut request = format!("wss://{RELAY}/v1/pipe/{}/{side}", room.name).into_client_request().map_err(|e| e.to_string())?;
    request.headers_mut().insert("user-agent", concat!("SulcusAI/", env!("CARGO_PKG_VERSION")).parse().map_err(|_| "bad agent")?);
    request.headers_mut().insert("authorization", format!("Bearer {}", room.token).parse().map_err(|_| "bad token")?);
    let (ws, _) = timeout(Duration::from_secs(20), tokio_tungstenite::client_async(request, tls))
        .await
        .map_err(|_| "SulcusAI's relay didn't answer.".to_string())?
        .map_err(|e| format!("The relay refused the connection: {e}"))?;
    let (mut sink, mut src) = ws.split();
    // Until the other side comes; a ping now and then keeps the wait open.
    let until = Instant::now() + wait;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            let _ = sink.close().await;
            return Err("They aren't there.".into());
        }
        match timeout(left.min(Duration::from_secs(25)), src.next()).await {
            Err(_) => sink.send(Message::Text("ping".into())).await.map_err(|e| e.to_string())?,
            Ok(Some(Ok(Message::Text(t)))) if t.as_str().contains("\"on\":true") => break,
            Ok(Some(Ok(_))) => {}
            Ok(_) => return Err("The relay closed the connection.".into()),
        }
    }
    let (app, pipe) = tokio::io::duplex(CHUNK * 4);
    let (mut from_app, mut to_app) = tokio::io::split(pipe);
    // The relay to the app, until they leave.
    spawn(async move {
        while let Some(m) = src.next().await {
            match m {
                Ok(Message::Binary(b)) => {
                    if to_app.write_all(&b).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Text(t)) if t.as_str().contains("\"on\":false") => break,
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => {}
            }
        }
        let _ = to_app.shutdown().await;
    });
    // The app to the relay, with a ping when it's quiet.
    spawn(async move {
        let mut buf = vec![0u8; CHUNK];
        loop {
            tokio::select! {
                n = from_app.read(&mut buf) => match n {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if sink.send(Message::Binary(buf[..n].to_vec().into())).await.is_err() {
                            break;
                        }
                    }
                },
                _ = tokio::time::sleep(Duration::from_secs(25)) => {
                    if sink.send(Message::Text("ping".into())).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = sink.close().await;
    });
    Ok(app)
}

/// People with a pipe being kept, and codes waiting in their rooms.
static KEPT: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
/// Connected to them through the pipe (not directly).
static THROUGH: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

/// Connected to them through SulcusAI's relay.
pub(super) fn through(id: &str) -> bool {
    THROUGH.lock().unwrap().contains(id) && link::connected(id)
}

/// Keeps a pipe to everyone linked (while no direct connection is up), and
/// a code that's showing waiting in its room. Called with People's tick.
pub(super) fn tick(state: &Arc<AppState>) {
    if !allowed(state) {
        return;
    }
    let Ok(c) = state.work_cipher() else { return };
    let b = book(&state.db.lock().unwrap());
    for p in b.contacts {
        let key = format!("person:{}", p.id);
        if !KEPT.lock().unwrap().insert(key.clone()) {
            continue;
        }
        let Ok(secret) = link_key(&c, &p) else {
            KEPT.lock().unwrap().remove(&key);
            continue;
        };
        let (s, id) = (state.clone(), p.id.clone());
        spawn(async move {
            keep_person(&s, &id, link_room(&secret)).await;
            KEPT.lock().unwrap().remove(&key);
        });
    }
    for i in b.invites.iter().filter(|i| i.open()) {
        let key = format!("code:{}", i.id);
        let Ok(code) = c.decrypt(&i.code) else { continue };
        if !KEPT.lock().unwrap().insert(key.clone()) {
            continue;
        }
        let (s, invite) = (state.clone(), i.id.clone());
        spawn(async move {
            wait_with_code(&s, &invite, &code).await;
            KEPT.lock().unwrap().remove(&key);
        });
    }
}

/// One person's pipe: met in their room, called (smaller id) or answered,
/// and met again when it ends, for as long as they're linked.
async fn keep_person(state: &Arc<AppState>, id: &str, room: Room) {
    let me = device_id(&state.paths.data);
    let side = if me.as_str() < id { "a" } else { "b" };
    let mut fails = 0u32;
    while allowed(state) && contact(state, id).is_some() {
        // A direct connection is better: the pipe waits while there is one.
        if link::connected(id) {
            tokio::time::sleep(Duration::from_secs(10)).await;
            continue;
        }
        match meet(&room, side, WAIT).await {
            Ok(mut stream) => {
                fails = 0;
                THROUGH.lock().unwrap().insert(id.to_string());
                if side == "a" {
                    if let Some(busy) = Busy::take(&format!("person:{id}")) {
                        let _ = link::call_on(state, id, stream, None, None, busy).await;
                    }
                } else if let Ok(open) = recv_plain::<Open>(&mut stream).await {
                    if open.id == id && link::answer(state, stream, &open, None).await.is_ok() {
                        // The link runs on its own: wait for it to start, then to end.
                        for _ in 0..20 {
                            if link::connected(id) {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(500)).await;
                        }
                        while link::connected(id) {
                            tokio::time::sleep(Duration::from_secs(2)).await;
                        }
                    }
                }
                THROUGH.lock().unwrap().remove(id);
                changed(state);
            }
            // Not there yet (a wait ran out): straight back in. The relay
            // out of reach: longer between tries, up to two minutes.
            Err(e) if e == "They aren't there." => {}
            Err(_) => {
                fails = fails.saturating_add(1);
                tokio::time::sleep(Duration::from_secs(5 * (1 << fails.min(5))).min(Duration::from_secs(120))).await;
            }
        }
    }
}

/// A code that's showing, waiting in its room for whoever types it
/// anywhere, as long as it takes links.
async fn wait_with_code(state: &Arc<AppState>, invite: &str, code: &str) {
    let room = code_room(code);
    let mut fails = 0u32;
    let open_now = |s: &AppState| book(&s.db.lock().unwrap()).invites.iter().any(|i| i.id == invite && i.open());
    while allowed(state) && open_now(state) {
        match meet(&room, "host", WAIT).await {
            Ok(mut stream) => {
                fails = 0;
                if let Ok(open) = recv_plain::<Open>(&mut stream).await {
                    if let (Some(theirs), Some(LINK)) = (open.pair.clone(), open.want.as_deref()) {
                        let _ = codes::pair_answer(state, stream, &open, &theirs).await;
                    }
                }
            }
            Err(e) if e == "They aren't there." => {}
            Err(_) => {
                fails = fails.saturating_add(1);
                tokio::time::sleep(Duration::from_secs(5 * (1 << fails.min(5))).min(Duration::from_secs(120))).await;
            }
        }
    }
}

/// Typing a code with no one nearby showing it: through the relay, to the
/// app whose code it is, wherever it is.
pub(super) async fn link_by_code(state: &Arc<AppState>, code: &str, step: &(dyn Fn(&str) + Send + Sync)) -> Result<String, String> {
    if !allowed(state) {
        return Err("SulcusAI is fully offline without People, so it can't reach anyone over the internet.".into());
    }
    step("reach");
    let stream = meet(&code_room(code), "guest", Duration::from_secs(20))
        .await
        .map_err(|_| "No one with that code is waiting. Ask them to open People → Connect on their PC with the code showing (SulcusAI open), then try again.".to_string())?;
    codes::pair_on(state, code, stream, None, step).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooms_come_from_the_secret_only() {
        use sha2::{Digest, Sha256};
        let a = link_room(b"the link key");
        assert_eq!((a.name.len(), a.token.len()), (32, 64));
        assert!(a.name.chars().chain(a.token.chars()).all(|c| c.is_ascii_hexdigit()));
        // What the relay checks: the name is where the token's hash starts.
        assert_eq!(a.name, hex::encode(Sha256::digest(a.token.as_bytes()))[..32]);
        assert_eq!(a, link_room(b"the link key"), "both apps find the same room");
        assert_ne!(a, link_room(b"another key"));
        assert_eq!(code_room("k7q2-9xmb"), code_room("K7Q29XMB"));
        assert_ne!(code_room("K7Q29XMB"), link_room(b"K7Q29XMB"), "codes and links never share a room");
    }

    /// Two apps meet in a room on the real relay (relay/worker.js, deployed)
    /// and bytes go both ways; the room's name with the wrong token gets nowhere.
    #[tokio::test]
    #[ignore]
    async fn two_apps_meet_through_the_real_relay() {
        let room = link_room(uuid::Uuid::new_v4().as_bytes());
        let (r1, r2) = (room.clone(), room.clone());
        let a = tokio::spawn(async move { meet(&r1, "a", Duration::from_secs(30)).await });
        let mut b = meet(&r2, "b", Duration::from_secs(30)).await.expect("b met a");
        let mut a = a.await.unwrap().expect("a met b");
        a.write_all(b"hello from a").await.unwrap();
        let mut got = [0u8; 12];
        timeout(Duration::from_secs(10), b.read_exact(&mut got)).await.unwrap().unwrap();
        assert_eq!(&got, b"hello from a");
        b.write_all(b"hi a").await.unwrap();
        let mut back = [0u8; 4];
        timeout(Duration::from_secs(10), a.read_exact(&mut back)).await.unwrap().unwrap();
        assert_eq!(&back, b"hi a");
        // Someone who saw the room's name but hasn't the secret.
        let wrong = Room { name: room.name.clone(), token: "0".repeat(64) };
        assert!(meet(&wrong, "a", Duration::from_secs(5)).await.is_err(), "the relay let a wrong token in");
    }
}
