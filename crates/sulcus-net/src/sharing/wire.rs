// SPDX-License-Identifier: AGPL-3.0-only
//! Sharing over the node's streams: share pieces from many sources, model
//! cards from a friend, virtual LAN packets and remote-control input. Each
//! side decides who may use it; nothing is served unless the app calls
//! `serve_*`, which it only does with that item's switch on.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{AsyncReadExt, AsyncWriteExt, StreamExt};
use libp2p::{PeerId, StreamProtocol};
use serde::{Deserialize, Serialize};

use super::remote::{Host, State};
use super::share::{self, Link, Manifest, Swarm};
use crate::NodeHandle;

pub const SHARE: StreamProtocol = StreamProtocol::new("/sulcus/share/1");
pub const CARDS: StreamProtocol = StreamProtocol::new("/sulcus/cards/1");
pub const VLAN: StreamProtocol = StreamProtocol::new("/sulcus/vlan/1");
pub const REMOTE: StreamProtocol = StreamProtocol::new("/sulcus/remote/1");

const LIMIT: Duration = Duration::from_secs(20);
const MAX_JSON: usize = 4 << 20;
const MAX_PACKET: usize = 65_535;
/// A screen picture (JPEG) sent to the controller.
const MAX_FRAME: usize = 8 << 20;
const FRAME_EVERY: Duration = Duration::from_millis(200);

async fn write<S: futures::AsyncWrite + Unpin>(s: &mut S, bytes: &[u8]) -> std::io::Result<()> {
    s.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    s.write_all(bytes).await?;
    s.flush().await
}

async fn read<S: futures::AsyncRead + Unpin>(s: &mut S, cap: usize) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    if len > cap {
        return Err(std::io::Error::other("frame too large"));
    }
    let mut b = vec![0u8; len];
    s.read_exact(&mut b).await?;
    Ok(b)
}

async fn open(node: &NodeHandle, peer: PeerId, p: StreamProtocol) -> Result<libp2p::Stream, String> {
    tokio::time::timeout(LIMIT, node.streams().open_stream(peer, p))
        .await
        .map_err(|_| "They didn't answer.".to_string())?
        .map_err(|e| format!("Couldn't reach them: {e}"))
}

// ---------- share pieces ----------

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "ask", rename_all = "snake_case")]
pub enum ShareAsk {
    Manifest { id: String },
    Have { id: String },
    Piece { id: String, index: u32 },
}

/// What this PC hosts. Pieces stay sealed: the host never needs the key.
pub trait Pieces: Send + Sync + 'static {
    fn manifest(&self, id: &str) -> Option<Manifest>;
    fn have(&self, id: &str) -> Vec<u32>;
    fn piece(&self, id: &str, index: u32) -> Option<Vec<u8>>;
}

pub fn serve_share(node: &NodeHandle, host: Arc<dyn Pieces>, may_use: impl Fn(&PeerId) -> bool + Send + Sync + 'static) {
    let Ok(mut incoming) = node.streams().accept(SHARE) else { return };
    let may_use = Arc::new(may_use);
    tokio::spawn(async move {
        while let Some((peer, mut s)) = incoming.next().await {
            let (host, may_use) = (host.clone(), may_use.clone());
            tokio::spawn(async move {
                let Ok(Ok(raw)) = tokio::time::timeout(LIMIT, read(&mut s, 4096)).await else { return };
                let reply = match serde_json::from_slice::<ShareAsk>(&raw) {
                    _ if !may_use(&peer) => Vec::new(),
                    Ok(ShareAsk::Manifest { id }) => host.manifest(&id).and_then(|m| serde_json::to_vec(&m).ok()).unwrap_or_default(),
                    Ok(ShareAsk::Have { id }) => serde_json::to_vec(&host.have(&id)).unwrap_or_default(),
                    Ok(ShareAsk::Piece { id, index }) => host.piece(&id, index).unwrap_or_default(),
                    Err(_) => Vec::new(),
                };
                let _ = write(&mut s, &reply).await;
                let _ = s.close().await;
            });
        }
    });
}

async fn share_ask(node: &NodeHandle, peer: PeerId, ask: &ShareAsk, cap: usize) -> Result<Vec<u8>, String> {
    let mut s = open(node, peer, SHARE).await?;
    write(&mut s, &serde_json::to_vec(ask).map_err(|e| e.to_string())?).await.map_err(|e| e.to_string())?;
    let r = tokio::time::timeout(LIMIT, read(&mut s, cap)).await.map_err(|_| "They didn't answer.".to_string())?.map_err(|e| e.to_string())?;
    if r.is_empty() {
        return Err("They aren't sharing that.".into());
    }
    Ok(r)
}

/// Downloads a share from every source that has pieces of it, checking each
/// piece; `progress` gets (done, total) after each piece.
pub async fn fetch(node: &NodeHandle, link: &Link, sources: &[PeerId], mut progress: impl FnMut(u32, u32)) -> Result<(Manifest, Vec<u8>), String> {
    let key = link.key_bytes().map_err(|_| "This link is damaged.".to_string())?;
    let mut manifest = None;
    for p in sources {
        if let Ok(raw) = share_ask(node, *p, &ShareAsk::Manifest { id: link.id.clone() }, MAX_JSON).await {
            if let Ok(m) = serde_json::from_slice::<Manifest>(&raw) {
                if link.check(&m).is_ok() {
                    manifest = Some(m);
                    break;
                }
            }
        }
    }
    let m = manifest.ok_or("No one sharing this link could be reached.")?;
    let total = m.pieces.len() as u32;
    let mut swarm = Swarm::new(total);
    for p in sources {
        if let Ok(raw) = share_ask(node, *p, &ShareAsk::Have { id: link.id.clone() }, MAX_JSON).await {
            if let Ok(have) = serde_json::from_slice::<Vec<u32>>(&raw) {
                swarm.add_source(&p.to_string(), have);
            }
        }
    }
    let mut pieces: Vec<Option<Vec<u8>>> = vec![None; total as usize];
    let cap = m.piece_size as usize + 64;
    while !swarm.finished() {
        let batch = swarm.next();
        if batch.is_empty() {
            return Err("The people sharing this don't have all of it.".into());
        }
        let asks = batch.into_iter().map(|(peer, i)| {
            let id = link.id.clone();
            async move {
                let got = match peer.parse::<PeerId>() {
                    Ok(pid) => share_ask(node, pid, &ShareAsk::Piece { id, index: i }, cap).await.ok(),
                    Err(_) => None,
                };
                (peer, i, got)
            }
        });
        for (peer, i, got) in futures::future::join_all(asks).await {
            match got.and_then(|b| share::open_piece(&m, &key, i, &b).ok()) {
                Some(plain) => {
                    pieces[i as usize] = Some(plain);
                    swarm.arrived(&peer, i, true);
                    let (d, t) = swarm.progress();
                    progress(d, t);
                }
                None => swarm.arrived(&peer, i, false),
            }
        }
    }
    let data: Vec<u8> = pieces.into_iter().flatten().flatten().collect();
    if data.len() as u64 != m.size {
        return Err("The download is incomplete.".into());
    }
    Ok((m, data))
}

// ---------- model cards ----------

/// Gives a friend the signed card texts this PC chose to pass on.
pub fn serve_cards(node: &NodeHandle, cards: impl Fn(&PeerId) -> Option<Vec<String>> + Send + 'static) {
    let Ok(mut incoming) = node.streams().accept(CARDS) else { return };
    tokio::spawn(async move {
        while let Some((peer, mut s)) = incoming.next().await {
            let body = cards(&peer).and_then(|c| serde_json::to_vec(&c).ok()).unwrap_or_default();
            let _ = write(&mut s, &body).await;
            let _ = s.close().await;
        }
    });
}

pub async fn fetch_cards(node: &NodeHandle, peer: PeerId) -> Result<Vec<String>, String> {
    let mut s = open(node, peer, CARDS).await?;
    let raw = tokio::time::timeout(LIMIT, read(&mut s, MAX_JSON)).await.map_err(|_| "They didn't answer.".to_string())?.map_err(|e| e.to_string())?;
    if raw.is_empty() {
        return Err("They aren't passing on model cards.".into());
    }
    serde_json::from_slice(&raw).map_err(|_| "Their cards couldn't be read.".to_string())
}

// ---------- virtual LAN ----------

#[derive(Debug, Serialize, Deserialize)]
struct Frame {
    net: String,
    #[serde(with = "b64")]
    packet: Vec<u8>,
}

mod b64 {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        STANDARD.decode(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

/// Takes packets for virtual LANs; `deliver` gets (network, sender, packet)
/// and does its own membership and address checks (`Network::route`).
/// One long-lived stream per member carries many packets.
pub fn serve_vlan(node: &NodeHandle, deliver: impl Fn(&str, &PeerId, Vec<u8>) + Send + Sync + 'static) {
    let Ok(mut incoming) = node.streams().accept(VLAN) else { return };
    let deliver = Arc::new(deliver);
    tokio::spawn(async move {
        while let Some((peer, mut s)) = incoming.next().await {
            let deliver = deliver.clone();
            tokio::spawn(async move {
                while let Ok(raw) = read(&mut s, MAX_PACKET * 2).await {
                    let Ok(f) = serde_json::from_slice::<Frame>(&raw) else { break };
                    if f.packet.len() <= MAX_PACKET {
                        deliver(&f.net, &peer, f.packet);
                    }
                }
            });
        }
    });
}

/// A sending link to one member; keeps its stream open between packets.
pub struct VlanLink {
    s: libp2p::Stream,
    net: String,
}

impl VlanLink {
    pub async fn open(node: &NodeHandle, peer: PeerId, net: &str) -> Result<Self, String> {
        Ok(VlanLink { s: open(node, peer, VLAN).await?, net: net.to_string() })
    }

    pub async fn send(&mut self, packet: &[u8]) -> Result<(), String> {
        if packet.len() > MAX_PACKET {
            return Err("packet too large".into());
        }
        let f = serde_json::to_vec(&Frame { net: self.net.clone(), packet: packet.to_vec() }).map_err(|e| e.to_string())?;
        write(&mut self.s, &f).await.map_err(|e| e.to_string())
    }
}

// ---------- remote control ----------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Input {
    Move { x: f32, y: f32 },
    Click { button: u8, down: bool },
    Key { code: String, down: bool },
    Scroll { dy: f32 },
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "is", rename_all = "snake_case")]
pub enum RemoteReply {
    Live,
    Refused { why: String },
    Ended,
}

#[derive(Debug, Serialize, Deserialize)]
struct Hello {
    code: String,
}

/// The controlled side: a request shows the prompt via `asked` (the host
/// state changes are seen by the app through `host`), and input reaches
/// `apply` only while the session is live for that peer.
pub fn serve_remote(
    node: &NodeHandle,
    host: Arc<Mutex<Host>>,
    now_ms: impl Fn() -> u64 + Send + Sync + 'static,
    asked: impl Fn(&PeerId, &str) + Send + Sync + 'static,
    apply: impl Fn(Input) + Send + Sync + 'static,
    frame: impl Fn() -> Option<Vec<u8>> + Send + Sync + 'static,
) {
    let Ok(mut incoming) = node.streams().accept(REMOTE) else { return };
    let (now_ms, asked, apply, frame) = (Arc::new(now_ms), Arc::new(asked), Arc::new(apply), Arc::new(frame));
    tokio::spawn(async move {
        while let Some((peer, mut s)) = incoming.next().await {
            let (host, now_ms, asked, apply, frame) = (host.clone(), now_ms.clone(), asked.clone(), apply.clone(), frame.clone());
            tokio::spawn(async move {
                let Ok(Ok(raw)) = tokio::time::timeout(LIMIT, read(&mut s, 1024)).await else { return };
                let Ok(hello) = serde_json::from_slice::<Hello>(&raw) else { return };
                let who = peer.to_string();
                let first = host.lock().unwrap().request(&who, &hello.code, now_ms());
                if let Err(e) = first {
                    let _ = write(&mut s, &serde_json::to_vec(&RemoteReply::Refused { why: format!("{e:?}") }).unwrap()).await;
                    return;
                }
                asked(&peer, &hello.code);
                loop {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    let state = {
                        let mut h = host.lock().unwrap();
                        h.tick(now_ms());
                        h.state().clone()
                    };
                    match state {
                        State::Live { controller, .. } if controller == who => break,
                        State::Asking { controller, .. } if controller == who => continue,
                        _ => {
                            let _ = write(&mut s, &serde_json::to_vec(&RemoteReply::Refused { why: "declined".into() }).unwrap()).await;
                            return;
                        }
                    }
                }
                if write(&mut s, &serde_json::to_vec(&RemoteReply::Live).unwrap()).await.is_err() {
                    host.lock().unwrap().stop();
                    return;
                }
                let (mut rd, mut wr) = s.split();
                let (h2, w2, f2) = (host.clone(), who.clone(), frame.clone());
                tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(FRAME_EVERY).await;
                        if !h2.lock().unwrap().may_input(&w2) {
                            break;
                        }
                        let f = f2.clone();
                        let Ok(Some(pic)) = tokio::task::spawn_blocking(move || f()).await else { continue };
                        if pic.len() > MAX_FRAME || write(&mut wr, &pic).await.is_err() {
                            break;
                        }
                    }
                    let _ = wr.close().await;
                });
                while let Ok(raw) = read(&mut rd, 1024).await {
                    if !host.lock().unwrap().may_input(&who) {
                        break;
                    }
                    if let Ok(i) = serde_json::from_slice::<Input>(&raw) {
                        apply(i);
                    }
                }
                let still_live = host.lock().unwrap().may_input(&who);
                if still_live {
                    host.lock().unwrap().stop();
                }
            });
        }
    });
}

/// The controlling side: asks with the shown code and waits for Accept.
/// `frames` gets the other screen as pictures until the session ends.
pub struct RemoteLink {
    wr: futures::io::WriteHalf<libp2p::Stream>,
    pub frames: tokio::sync::mpsc::Receiver<Vec<u8>>,
}

impl RemoteLink {
    pub async fn ask(node: &NodeHandle, peer: PeerId, code: &str) -> Result<Self, String> {
        let mut s = open(node, peer, REMOTE).await?;
        write(&mut s, &serde_json::to_vec(&Hello { code: code.into() }).unwrap()).await.map_err(|e| e.to_string())?;
        let raw = tokio::time::timeout(Duration::from_secs(15), read(&mut s, 1024)).await.map_err(|_| "No answer.".to_string())?.map_err(|e| e.to_string())?;
        match serde_json::from_slice::<RemoteReply>(&raw) {
            Ok(RemoteReply::Live) => {
                let (mut rd, wr) = s.split();
                let (tx, frames) = tokio::sync::mpsc::channel(4);
                tokio::spawn(async move {
                    while let Ok(f) = read(&mut rd, MAX_FRAME).await {
                        if tx.send(f).await.is_err() {
                            break;
                        }
                    }
                });
                Ok(RemoteLink { wr, frames })
            }
            Ok(RemoteReply::Refused { why }) => Err(why),
            _ => Err("They ended it.".into()),
        }
    }

    pub async fn send(&mut self, i: &Input) -> Result<(), String> {
        write(&mut self.wr, &serde_json::to_vec(i).unwrap()).await.map_err(|e| e.to_string())
    }
}
