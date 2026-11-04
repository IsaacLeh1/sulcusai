// SPDX-License-Identifier: AGPL-3.0-only
//! People over the peer-to-peer node (design 11.2, M9), only while the
//! Network switch is on; with it off nothing here runs and People works as
//! before (direct TCP, then SulcusAI's pipe).
//!
//! - Over the link each app tells the other its node id, where it can be
//!   reached and its signed seal key (`Frame::Node`). A node id is believed
//!   for a person only from their link, which the link key already proves.
//! - Calls go up the ladder (same network, public, through relays) and carry
//!   the same sealed link frames on `/sulcus/people/1`; the pipe stays the
//!   fallback for anyone the node can't reach.
//! - While someone is away, letters wait in relay mailboxes, sealed to their
//!   seal key (`sulcus_net::seal`), so a relay sees size and timing only.

use std::sync::LazyLock;

use futures_util::{AsyncReadExt as _, AsyncWriteExt as _, StreamExt as _};
use sulcus_net::libp2p::multiaddr::Protocol;
use sulcus_net::libp2p::{identity::Keypair, Multiaddr, PeerId, StreamProtocol};
use sulcus_net::mailbox::{self as relaybox, Reply, Request};
use sulcus_net::seal::{self, SealSecret, Seen};
use sulcus_net::{ladder, NodeHandle};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

use super::link::{self, Frame};
use super::mailbox::{self, Letter};
use super::*;

pub const PROTOCOL: StreamProtocol = StreamProtocol::new("/sulcus/people/1");
const NODES: &str = "people_nodes";
const SEEN: &str = "people_seen";
const MOST_ADDRS: usize = 16;
/// Room for the letter's JSON wrapping inside a sealed letter.
const LETTER_ROOM: usize = seal::MAX_PLAIN - 1_024;
const REACH: Duration = Duration::from_secs(10);
const MAIL_EVERY: Duration = Duration::from_secs(20);

/// What a linked person's app said about its node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct NodeInfo {
    pub peer: String,
    pub addrs: Vec<String>,
    /// Their signed seal key record.
    pub seal: String,
}

/// Checks what a `Frame::Node` says: the seal key must be signed by the
/// node it names, and only addresses that parse are kept.
pub(super) fn check(peer: &str, addrs: &[String], seal_record: &str) -> Option<NodeInfo> {
    peer.parse::<PeerId>().ok()?;
    let (k, _) = seal::SealKey::open(seal_record).ok()?;
    if k.peer != peer {
        return None;
    }
    let addrs = addrs.iter().filter(|a| a.parse::<Multiaddr>().is_ok()).take(MOST_ADDRS).cloned().collect();
    Some(NodeInfo { peer: peer.to_string(), addrs, seal: seal_record.to_string() })
}

fn nodes(state: &AppState, c: &Cipher) -> HashMap<String, NodeInfo> {
    crate::p2p::invites::load_sealed(&state.db.lock().unwrap(), c, NODES)
}

/// A person told this app about their node, over the link or in a letter.
pub(super) fn heard(state: &AppState, id: &str, peer: &str, addrs: &[String], seal_record: &str) {
    let (Ok(c), Some(info)) = (state.work_cipher(), check(peer, addrs, seal_record)) else { return };
    let mut all = nodes(state, &c);
    if all.get(id) == Some(&info) {
        return;
    }
    all.insert(id.to_string(), info);
    let _ = crate::p2p::invites::save_sealed(&state.db.lock().unwrap(), &c, NODES, &all);
}

/// This app's keys for the node: its node key and its seal key.
fn keys(state: &AppState, c: &Cipher) -> Result<(Keypair, SealSecret), String> {
    let conn = state.db.lock().unwrap();
    let node = crate::p2p::keys::node_key(&conn, c)?;
    let node = sulcus_net::identity::load(&node).map_err(|e| e.to_string())?;
    Ok((node, crate::p2p::keys::seal_key(&conn, c)?))
}

/// What to tell a linked person about this app's node, while it runs.
pub(super) fn my_frame(state: &AppState) -> Option<Frame> {
    let node = crate::p2p::running()?;
    let c = state.work_cipher().ok()?;
    let (key, sk) = keys(state, &c).ok()?;
    let hide = !crate::shield::peers::rules(&crate::shield::live::current()).share_addresses;
    let mut all = node.listening();
    all.extend(node.external());
    let addrs = crate::p2p::invites::shareable(&all, true)
        .into_iter()
        .filter(|a| !hide || relayed(a))
        .map(|a| a.to_string())
        .take(MOST_ADDRS)
        .collect();
    Some(Frame::Node { peer: node.peer_id().to_string(), addrs, seal: sk.record(&key, sulcus_net::signed::now()) })
}

fn relayed(a: &Multiaddr) -> bool {
    a.iter().any(|p| matches!(p, Protocol::P2pCircuit))
}

/// The relays someone can be reached through: the part of each circuit
/// address before `/p2p-circuit`.
pub(super) fn relays_in(addrs: &[String]) -> Vec<Multiaddr> {
    let mut out: Vec<Multiaddr> = Vec::new();
    for a in addrs.iter().filter_map(|a| a.parse::<Multiaddr>().ok()) {
        let mut relay = Multiaddr::empty();
        let mut circuit = false;
        for p in a.iter() {
            if matches!(p, Protocol::P2pCircuit) {
                circuit = true;
                break;
            }
            relay.push(p);
        }
        if circuit && matches!(relay.iter().last(), Some(Protocol::P2p(_))) && !out.contains(&relay) {
            out.push(relay);
        }
    }
    out
}

fn with_peer(mut a: Multiaddr, peer: PeerId) -> Multiaddr {
    if a.iter().last() != Some(Protocol::P2p(peer)) {
        a.push(Protocol::P2p(peer));
    }
    a
}

fn peer_of(a: &Multiaddr) -> Option<PeerId> {
    match a.iter().last() {
        Some(Protocol::P2p(p)) => Some(p),
        _ => None,
    }
}

/// Connects to `peer` (if not already), best address first.
pub(super) async fn reach(node: &NodeHandle, peer: PeerId, addrs: &[Multiaddr], relays_only: bool) -> Result<(), String> {
    if node.connected().contains(&peer) {
        return Ok(());
    }
    for a in ladder::order(addrs, relays_only) {
        if node.dial(with_peer(a, peer)).await.is_err() {
            continue;
        }
        let deadline = Instant::now() + REACH;
        while Instant::now() < deadline {
            if node.connected().contains(&peer) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }
    Err("Couldn't reach them over the network.".into())
}

/// A node stream as a tokio stream, so the link code runs on it unchanged.
/// Ends both ways when either side ends.
pub(super) fn bridge<S: futures_util::AsyncRead + futures_util::AsyncWrite + Send + 'static>(s: S) -> DuplexStream {
    let (mine, theirs) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let (mut sr, mut sw) = s.split();
        let (mut tr, mut tw) = tokio::io::split(theirs);
        let inbound = async {
            let mut b = vec![0u8; 16 * 1024];
            while let Ok(n) = sr.read(&mut b).await {
                if n == 0 || tw.write_all(&b[..n]).await.is_err() {
                    break;
                }
            }
            let _ = tw.shutdown().await;
        };
        let outbound = async {
            let mut b = vec![0u8; 16 * 1024];
            while let Ok(n) = tr.read(&mut b).await {
                if n == 0 || sw.write_all(&b[..n]).await.is_err() || sw.flush().await.is_err() {
                    break;
                }
            }
            let _ = sw.close().await;
        };
        tokio::select! {
            _ = inbound => {}
            _ = outbound => {}
        }
    });
    mine
}

/// Takes People calls on the node; started with the node.
pub fn serve(state: &Arc<AppState>, node: &NodeHandle) {
    let Ok(mut incoming) = node.streams().accept(PROTOCOL) else { return };
    let state = state.clone();
    tokio::spawn(async move {
        while let Some((peer, s)) = incoming.next().await {
            let state = state.clone();
            tokio::spawn(async move {
                let mut d = bridge(s);
                let Ok(open) = recv_plain::<Open>(&mut d).await else { return };
                let Ok(c) = state.work_cipher() else { return };
                // Only the node this person named over their link may call as them.
                if nodes(&state, &c).get(&open.id).map(|n| n.peer.clone()) != Some(peer.to_string()) {
                    return;
                }
                let _ = link::answer(&state, d, &open, None).await;
            });
        }
    });
}

#[derive(Default)]
struct Tries {
    call: Option<Instant>,
    fails: u32,
}

static TRIES: LazyLock<Mutex<HashMap<String, Tries>>> = LazyLock::new(Default::default);
static MAILED: Mutex<Option<Instant>> = Mutex::new(None);

/// Calls linked people over the node and exchanges letters through relay
/// mailboxes. Runs from People's tick, before the pipe, and only while the
/// node runs.
pub(super) fn tick(state: &Arc<AppState>) {
    let Some(node) = crate::p2p::running() else { return };
    let Ok(c) = state.work_cipher() else { return };
    let me = device_id(&state.paths.data);
    let relays_only = !crate::shield::peers::rules(&crate::shield::live::current()).dial_direct;
    let all = nodes(state, &c);
    for (id, info) in all.iter() {
        // The smaller id calls, as on the pipe, so two calls don't cross.
        if link::connected(id) || contact(state, id).is_none() || me > *id {
            continue;
        }
        {
            let mut tries = TRIES.lock().unwrap();
            let t = tries.entry(id.clone()).or_default();
            let wait = if t.fails == 0 { Duration::ZERO } else { (Duration::from_secs(10) * (1 << (t.fails - 1).min(4))).min(Duration::from_secs(120)) };
            if t.call.is_some_and(|at| at.elapsed() < wait) {
                continue;
            }
            t.call = Some(Instant::now());
        }
        let (s, n, id, info) = (state.clone(), node.clone(), id.clone(), info.clone());
        spawn(async move {
            let ok = call(&s, &n, &id, &info, relays_only).await.is_ok();
            let mut tries = TRIES.lock().unwrap();
            let t = tries.entry(id).or_default();
            t.fails = if ok { 0 } else { t.fails.saturating_add(1) };
        });
    }
    {
        let mut m = MAILED.lock().unwrap();
        if m.is_some_and(|t| t.elapsed() < MAIL_EVERY) {
            return;
        }
        *m = Some(Instant::now());
    }
    let s = state.clone();
    spawn(async move {
        if let Err(e) = mail(&s, &node, all).await {
            crate::applog::line("people", &e);
        }
    });
}

async fn call(state: &Arc<AppState>, node: &NodeHandle, id: &str, info: &NodeInfo, relays_only: bool) -> Result<(), String> {
    let Some(busy) = Busy::take(&format!("person:{id}")) else { return Err(link::BUSY.into()) };
    let peer: PeerId = info.peer.parse().map_err(|_| UNEXPECTED.to_string())?;
    let addrs: Vec<Multiaddr> = info.addrs.iter().filter_map(|a| a.parse().ok()).collect();
    reach(node, peer, &addrs, relays_only).await?;
    let s = timeout(REACH, node.streams().open_stream(peer, PROTOCOL)).await.map_err(|_| "They didn't answer.".to_string())?.map_err(|e| e.to_string())?;
    link::call_on(state, id, bridge(s), None, None, busy).await
}

fn my_relays(state: &AppState) -> Vec<Multiaddr> {
    crate::p2p::settings::load(&state.db.lock().unwrap()).relays.iter().filter_map(|r| r.parse().ok()).collect()
}

/// Seals frames for someone; the letter's id is random.
pub(super) fn seal_frames(key: &Keypair, sk: &SealSecret, their_seal: &str, frames: Vec<Frame>) -> Result<(String, Vec<u8>), String> {
    let id = hex::encode(wire::random::<16>());
    let body = serde_json::to_vec(&Letter { frames }).map_err(|e| e.to_string())?;
    let sealed = seal::seal(key, sk, their_seal, &id, &body, sulcus_net::signed::now()).map_err(|e| e.to_string())?;
    Ok((id, sealed))
}

/// Opens a letter taken from a mailbox into who sent it and its frames.
pub(super) fn open_frames(me: &PeerId, sk: &SealSecret, letter: &relaybox::Letter, seen: &mut Seen) -> Result<(PeerId, Vec<Frame>), String> {
    let o = seal::open(me, sk, letter, seen, sulcus_net::signed::now()).map_err(|e| e.to_string())?;
    let l: Letter = serde_json::from_slice(&o.body).map_err(|_| UNEXPECTED.to_string())?;
    Ok((o.from, l.frames))
}

/// Leaves a letter on the first relay that keeps it.
pub(super) async fn put(node: &NodeHandle, relays: &[Multiaddr], to: &PeerId, id: &str, body: Vec<u8>) -> Result<(), String> {
    let mut why = "No relay to leave letters on.".to_string();
    for r in relays {
        let Some(relay) = peer_of(r) else { continue };
        if let Err(e) = reach(node, relay, std::slice::from_ref(r), false).await {
            why = e;
            continue;
        }
        match relaybox::ask(node, relay, &Request::Put { to: to.to_string(), id: id.to_string(), body: body.clone() }).await {
            Ok(Reply::Kept) => return Ok(()),
            Ok(Reply::Refused { why: w }) => why = w,
            Ok(_) => {}
            Err(e) => why = e,
        }
    }
    Err(why)
}

/// Everything waiting for this node on `relays`.
pub(super) async fn take(node: &NodeHandle, relays: &[Multiaddr]) -> Vec<relaybox::Letter> {
    let mut out = Vec::new();
    for r in relays {
        let Some(relay) = peer_of(r) else { continue };
        if reach(node, relay, std::slice::from_ref(r), false).await.is_err() {
            continue;
        }
        if let Ok(Reply::Letters { letters }) = relaybox::ask(node, relay, &Request::Take).await {
            out.extend(letters);
        }
    }
    out
}

/// Sends what's waiting for everyone away (to their relays, then this
/// user's), then reads what was left on this user's relays.
async fn mail(state: &Arc<AppState>, node: &NodeHandle, all: HashMap<String, NodeInfo>) -> Result<(), String> {
    let c = state.work_cipher()?;
    let (key, sk) = keys(state, &c)?;
    let mine = my_relays(state);
    for (id, info) in all.iter() {
        if link::connected(id) {
            continue;
        }
        let Some(p) = contact(state, id) else { continue };
        let Ok(to) = info.peer.parse::<PeerId>() else { continue };
        let mut relays = relays_in(&info.addrs);
        relays.extend(mine.iter().filter(|r| !relays.contains(r)).cloned().collect::<Vec<_>>());
        if relays.is_empty() {
            continue;
        }
        let frames = mailbox::gather(state, &c, &p, LETTER_ROOM)?;
        if frames.is_empty() {
            continue;
        }
        let mut frames = frames;
        frames.insert(0, Frame::Hello { name: my_name(state), port: 0, parts: true });
        let mut pending = mailbox::letters_up_to(frames, LETTER_ROOM).into_iter();
        while let Some(letter) = pending.next() {
            let sent = match seal_frames(&key, &sk, &info.seal, letter.clone()) {
                Ok((lid, body)) => put(node, &relays, &to, &lid, body).await,
                Err(e) => Err(e),
            };
            if sent.is_err() {
                mailbox::put_back(id, letter.into_iter().chain(pending.flatten()).collect());
                break;
            }
        }
    }
    let letters = take(node, &mine).await;
    if letters.is_empty() {
        return Ok(());
    }
    let mut seen: Seen = crate::p2p::invites::load_sealed(&state.db.lock().unwrap(), &c, SEEN);
    let by_peer: HashMap<String, String> = all.iter().map(|(id, n)| (n.peer.clone(), id.clone())).collect();
    for l in letters {
        let Ok((from, frames)) = open_frames(&node.peer_id(), &sk, &l, &mut seen) else { continue };
        let Some(p) = by_peer.get(&from.to_string()).and_then(|id| contact(state, id)) else { continue };
        for f in frames.into_iter().filter(mailbox::relayable) {
            link::dispatch(state, &p, f);
        }
    }
    crate::p2p::invites::save_sealed(&state.db.lock().unwrap(), &c, SEEN, &seen)?;
    changed(state);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use sulcus_net::{Node, NodeConfig};

    fn who() -> (Vec<u8>, Keypair, SealSecret) {
        let bytes = sulcus_net::identity::generate();
        let kp = sulcus_net::identity::load(&bytes).unwrap();
        (bytes, kp, SealSecret::generate())
    }

    #[test]
    fn a_node_frame_is_believed_only_when_its_seal_key_is_signed_by_that_node() {
        let ((_, a, a_sk), (_, b, _)) = (who(), who());
        let a_id = a.public().to_peer_id().to_string();
        let rec = a_sk.record(&a, 1);
        let ok = check(&a_id, &["/ip4/10.0.0.2/tcp/4001".into(), "not an address".into()], &rec).unwrap();
        assert_eq!(ok.addrs.len(), 1);
        assert!(check(&b.public().to_peer_id().to_string(), &[], &rec).is_none(), "someone else's seal key");
        assert!(check("nope", &[], &rec).is_none());
    }

    #[test]
    fn relays_come_from_circuit_addresses() {
        let r = PeerId::random();
        let me = PeerId::random();
        let addrs = vec![format!("/ip4/1.2.3.4/tcp/4001/p2p/{r}/p2p-circuit/p2p/{me}"), format!("/ip4/1.2.3.4/udp/4001/quic-v1/p2p/{r}/p2p-circuit/p2p/{me}"), "/ip4/10.0.0.2/tcp/9".into()];
        let got = relays_in(&addrs);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|a| peer_of(a) == Some(r)));
    }

    #[test]
    fn frames_sealed_for_someone_open_for_them_once() {
        let ((_, a, a_sk), (_, b, b_sk)) = (who(), who());
        let (id, body) = seal_frames(&a, &a_sk, &b_sk.record(&b, sulcus_net::signed::now()), vec![Frame::Read { ids: vec!["m1".into()] }]).unwrap();
        let l = relaybox::Letter { from: a.public().to_peer_id().to_string(), id, body, at: 0 };
        let mut seen = Seen::default();
        let (from, frames) = open_frames(&b.public().to_peer_id(), &b_sk, &l, &mut seen).unwrap();
        assert_eq!(from, a.public().to_peer_id());
        assert!(matches!(&frames[..], [Frame::Read { ids }] if ids == &vec!["m1".to_string()]));
        assert!(open_frames(&b.public().to_peer_id(), &b_sk, &l, &mut seen).is_err(), "a replay");
    }

    async fn tcp_addr(n: &NodeHandle) -> Multiaddr {
        for _ in 0..100 {
            if let Some(a) = n.listening().into_iter().find(|a| a.iter().any(|p| matches!(p, Protocol::Tcp(_)))) {
                return a;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("not listening");
    }

    /// Two apps' nodes on loopback: a People stream both ways through the
    /// bridge, and a sealed letter left on a relay while one is away.
    #[tokio::test(flavor = "multi_thread")]
    async fn people_streams_and_sealed_letters_between_two_nodes() {
        let relay = Node::start(NodeConfig { relay_server: true, ..NodeConfig::loopback(sulcus_net::identity::generate()) }).unwrap();
        relaybox::serve(&relay, Arc::new(StdMutex::new(relaybox::Store::default())), |_| true, || {});
        let relay_addr = with_peer(tcp_addr(&relay).await, relay.peer_id());
        let ((a_bytes, a_kp, a_sk), (b_bytes, b_kp, b_sk)) = (who(), who());
        let a = Node::start(NodeConfig::loopback(a_bytes)).unwrap();
        let b = Node::start(NodeConfig::loopback(b_bytes.clone())).unwrap();

        // B answers People streams by echoing, through the bridge.
        let mut incoming = b.streams().accept(PROTOCOL).unwrap();
        tokio::spawn(async move {
            while let Some((_, s)) = incoming.next().await {
                tokio::spawn(async move {
                    let mut d = bridge(s);
                    let mut buf = [0u8; 5];
                    if d.read_exact(&mut buf).await.is_ok() {
                        let _ = d.write_all(&buf).await;
                    }
                });
            }
        });
        let b_addr = tcp_addr(&b).await;
        reach(&a, b.peer_id(), &[b_addr], false).await.unwrap();
        let mut d = bridge(a.streams().open_stream(b.peer_id(), PROTOCOL).await.unwrap());
        d.write_all(b"hello").await.unwrap();
        let mut back = [0u8; 5];
        timeout(Duration::from_secs(10), d.read_exact(&mut back)).await.unwrap().unwrap();
        assert_eq!(&back, b"hello");
        b.stop().await;

        let now = sulcus_net::signed::now();
        let (id, body) = seal_frames(&a_kp, &a_sk, &b_sk.record(&b_kp, now), vec![Frame::Read { ids: vec!["m9".into()] }]).unwrap();
        put(&a, std::slice::from_ref(&relay_addr), &b_kp.public().to_peer_id(), &id, body).await.unwrap();
        a.stop().await;
        let b = Node::start(NodeConfig::loopback(b_bytes)).unwrap();
        let letters = take(&b, std::slice::from_ref(&relay_addr)).await;
        assert_eq!(letters.len(), 1);
        let (from, frames) = open_frames(&b.peer_id(), &b_sk, &letters[0], &mut Seen::default()).unwrap();
        assert_eq!(from, a_kp.public().to_peer_id());
        assert!(matches!(&frames[..], [Frame::Read { .. }]));
        b.stop().await;
        relay.stop().await;
    }
}
