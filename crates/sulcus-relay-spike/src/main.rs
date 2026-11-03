// SPDX-License-Identifier: AGPL-3.0-only
//! Spike for phase 5: a relay node and two peers on 127.0.0.1. The peers
//! meet through Circuit Relay v2, then DCUtR tries to replace the relayed
//! connection with a direct one.
//!
//! Every address is loopback and nothing resolves names, so the spike never
//! reaches past this PC. That also means it can't prove hole punching works
//! through a real router: see README.md.
//!
//!   sulcus-relay-spike demo [--quic] [--confirm] [--relay-only]   all three in one process
//!   sulcus-relay-spike rates [runs]          how often each transport goes direct
//!   sulcus-relay-spike relay [port]          TCP and QUIC on the same port number
//!   sulcus-relay-spike listen <relay multiaddr> [--confirm] [--relay-only]
//!   sulcus-relay-spike dial <relay multiaddr> [--confirm] [--relay-only]
//!
//! `--confirm` marks the peers' own listen addresses as confirmed external
//! addresses before identify reports them (what a router port mapping or a
//! hand-forwarded port would do). It makes the hole punch fail: DCUtR only
//! offers addresses it heard as candidates, and identify stays quiet about
//! an observed address that is already confirmed.
//!
//! `--relay-only` turns DCUtR off and pushes 1 MiB over the relayed
//! connection instead, to see whether an app protocol may use a relayed
//! connection at all and where the relay's per-circuit byte limit cuts in.

use std::error::Error;
use std::time::{Duration, Instant};

use futures::{AsyncReadExt, AsyncWriteExt, StreamExt};
use libp2p::core::multiaddr::{Multiaddr, Protocol};
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::{ConnectionId, NetworkBehaviour, SwarmEvent};
use libp2p::{dcutr, identify, identity, noise, ping, relay, tcp, yamux, PeerId, StreamProtocol, Swarm, SwarmBuilder};
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

type Res<T> = Result<T, Box<dyn Error + Send + Sync>>;

const PROTOCOL: &str = "/sulcus-spike/0.1.0";
const BULK: StreamProtocol = StreamProtocol::new("/sulcus-spike/bulk/0.1.0");
const BULK_BYTES: usize = 1 << 20;
const LIMIT: Duration = Duration::from_secs(30);
// Fixed seeds so separate processes can work out each other's peer ids
// without passing them around.
const RELAY_SEED: u8 = 1;
const LISTENER_SEED: u8 = 2;
const DIALER_SEED: u8 = 3;

#[derive(Clone, Copy)]
struct Opts {
    confirm: bool,
    punch: bool,
    // Reaching the relay over QUIC gives identify (and so DCUtR) QUIC
    // addresses to punch with; over TCP it gives TCP ones.
    quic: bool,
}

#[derive(NetworkBehaviour)]
struct RelayNode {
    relay: relay::Behaviour,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
}

#[derive(NetworkBehaviour)]
struct Peer {
    relay_client: relay::client::Behaviour,
    dcutr: Toggle<dcutr::Behaviour>,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
    stream: libp2p_stream::Behaviour,
}

#[derive(Debug, Default)]
struct Report {
    relayed: bool,
    dcutr: Option<Result<(), String>>,
    direct: Option<Multiaddr>,
    direct_rtt: Option<Duration>,
    took: Option<Duration>,
    bulk: Option<Result<usize, String>>,
}

enum Bulk {
    Sent(Result<usize, String>),
    Received(usize, Option<String>),
}

fn key(seed: u8) -> identity::Keypair {
    identity::Keypair::ed25519_from_bytes([seed; 32]).expect("32 bytes is an ed25519 seed")
}

fn id(seed: u8) -> PeerId {
    key(seed).public().to_peer_id()
}

fn loopback_only(a: &Multiaddr) -> bool {
    a.iter().any(|p| matches!(p, Protocol::Ip4(_) | Protocol::Ip6(_)))
        && a.iter().all(|p| match p {
            Protocol::Ip4(ip) => ip.is_loopback(),
            Protocol::Ip6(ip) => ip.is_loopback(),
            Protocol::Dns(_) | Protocol::Dns4(_) | Protocol::Dns6(_) | Protocol::Dnsaddr(_) => false,
            _ => true,
        })
}

fn identify_config(k: &identity::Keypair) -> identify::Config {
    identify::Config::new(PROTOCOL.into(), k.public())
}

fn relay_swarm(k: identity::Keypair) -> Res<Swarm<RelayNode>> {
    Ok(SwarmBuilder::with_existing_identity(k)
        .with_tokio()
        .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
        .with_quic()
        .with_behaviour(|k| RelayNode {
            relay: relay::Behaviour::new(k.public().to_peer_id(), relay::Config::default()),
            identify: identify::Behaviour::new(identify_config(k)),
            ping: ping::Behaviour::new(ping::Config::new()),
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(30)))
        .build())
}

fn peer_swarm(k: identity::Keypair, punch: bool) -> Res<Swarm<Peer>> {
    Ok(SwarmBuilder::with_existing_identity(k)
        .with_tokio()
        .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
        .with_quic()
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|k, relay_client| Peer {
            relay_client,
            dcutr: Toggle::from(punch.then(|| dcutr::Behaviour::new(k.public().to_peer_id()))),
            identify: identify::Behaviour::new(identify_config(k)),
            ping: ping::Behaviour::new(ping::Config::new().with_interval(Duration::from_secs(1))),
            stream: libp2p_stream::Behaviour::new(),
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(30)))
        .build())
}

/// The relay's TCP and QUIC addresses, once it listens on both.
type RelayAddrs = (Multiaddr, Multiaddr);

async fn run_relay(port: u16, ready: Option<oneshot::Sender<RelayAddrs>>) -> Res<()> {
    let mut swarm = relay_swarm(key(RELAY_SEED))?;
    swarm.listen_on(format!("/ip4/127.0.0.1/tcp/{port}").parse()?)?;
    swarm.listen_on(format!("/ip4/127.0.0.1/udp/{port}/quic-v1").parse()?)?;
    let me = *swarm.local_peer_id();
    let mut ready = ready;
    let (mut tcp, mut quic) = (None, None);
    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => {
                // A reservation answer must list the relay's external
                // addresses, and nothing confirms any on loopback.
                swarm.add_external_address(address.clone());
                println!("[relay] listening on {address}/p2p/{me}");
                if address.iter().any(|p| matches!(p, Protocol::QuicV1)) {
                    quic = Some(address);
                } else {
                    tcp = Some(address);
                }
                if let (Some(t), Some(q)) = (&tcp, &quic) {
                    if let Some(tx) = ready.take() {
                        let _ = tx.send((t.clone(), q.clone()));
                    }
                }
            }
            SwarmEvent::Behaviour(RelayNodeEvent::Relay(e)) => println!("[relay] {e:?}"),
            _ => {}
        }
    }
}

/// Listens on loopback, connects to the relay and waits for identify both
/// ways, so the relay has told this peer how it looks from outside.
async fn start_peer(name: &str, seed: u8, relay_addr: &Multiaddr, o: Opts) -> Res<Swarm<Peer>> {
    let mut swarm = peer_swarm(key(seed), o.punch)?;
    swarm.listen_on("/ip4/127.0.0.1/tcp/0".parse()?)?;
    swarm.listen_on("/ip4/127.0.0.1/udp/0/quic-v1".parse()?)?;
    let mut listening = 0;
    while listening < 2 {
        if let SwarmEvent::NewListenAddr { address, .. } = swarm.select_next_some().await {
            println!("[{name}] listening on {address}");
            if o.confirm {
                swarm.add_external_address(address);
            }
            listening += 1;
        }
    }
    let relay_id = id(RELAY_SEED);
    swarm.dial(relay_addr.clone().with(Protocol::P2p(relay_id)))?;
    let (mut sent, mut received) = (false, false);
    while !(sent && received) {
        match swarm.select_next_some().await {
            SwarmEvent::Behaviour(PeerEvent::Identify(identify::Event::Sent { peer_id, .. })) if peer_id == relay_id => sent = true,
            SwarmEvent::Behaviour(PeerEvent::Identify(identify::Event::Received { peer_id, info, .. })) if peer_id == relay_id => {
                println!("[{name}] the relay sees us at {}", info.observed_addr);
                received = true;
            }
            SwarmEvent::OutgoingConnectionError { error, .. } => return Err(format!("couldn't reach the relay: {error}").into()),
            _ => {}
        }
    }
    Ok(swarm)
}

async fn send_bulk(mut control: libp2p_stream::Control, to: PeerId) -> Result<usize, String> {
    let mut s = control.open_stream(to, BULK).await.map_err(|e| format!("open: {e}"))?;
    let chunk = vec![7u8; 16 * 1024];
    let mut sent = 0;
    while sent < BULK_BYTES {
        if let Err(e) = s.write_all(&chunk).await {
            return Err(format!("cut after {sent} bytes: {e}"));
        }
        sent += chunk.len();
    }
    s.close().await.map_err(|e| format!("close after {sent} bytes: {e}"))?;
    Ok(sent)
}

fn accept_bulk(swarm: &mut Swarm<Peer>, notes: mpsc::UnboundedSender<Bulk>) -> Res<()> {
    let mut incoming = swarm.behaviour().stream.new_control().accept(BULK)?;
    tokio::spawn(async move {
        if let Some((_, mut s)) = incoming.next().await {
            let mut got = 0usize;
            let mut buf = vec![0u8; 16 * 1024];
            let problem = loop {
                match s.read(&mut buf).await {
                    Ok(0) => break None,
                    Ok(n) => {
                        got += n;
                    }
                    Err(e) => break Some(e.to_string()),
                }
            };
            let _ = notes.send(Bulk::Received(got, problem));
        }
    });
    Ok(())
}

/// Follows the connection to `other` until the mode's result is in
/// (a hole punch and a direct ping, or the bulk transfer), or time runs out.
async fn watch(name: &str, swarm: &mut Swarm<Peer>, other: PeerId, o: Opts, mut notes: mpsc::UnboundedReceiver<Bulk>, sender: Option<mpsc::UnboundedSender<Bulk>>) -> Report {
    let mut r = Report::default();
    let mut direct_conns: Vec<ConnectionId> = Vec::new();
    let mut sender = sender;
    let start = Instant::now();
    let deadline = tokio::time::sleep(LIMIT);
    tokio::pin!(deadline);
    loop {
        // When the other side's direct dial lands first, this side's DCUtR
        // may never report, though the connection is direct on both.
        let settled = r.took.is_some_and(|t| start.elapsed() > t + Duration::from_secs(2));
        let done = if o.punch { matches!(r.dcutr, Some(Err(_))) || (r.direct_rtt.is_some() && (r.dcutr.is_some() || settled)) } else { r.bulk.is_some() };
        if done {
            return r;
        }
        let ev = tokio::select! {
            _ = &mut deadline => {
                println!("[{name}] gave up after {LIMIT:?}");
                return r;
            }
            Some(note) = notes.recv() => {
                match note {
                    Bulk::Sent(res) => {
                        println!("[{name}] bulk send over the relay: {res:?} after {:?}", start.elapsed());
                        r.bulk = Some(res);
                    }
                    Bulk::Received(n, problem) => {
                        println!("[{name}] bulk received {n} bytes over the relay ({problem:?})");
                        r.bulk = Some(match problem { None => Ok(n), Some(p) => Err(format!("{n} bytes, then {p}")) });
                    }
                }
                continue;
            }
            ev = swarm.select_next_some() => ev,
        };
        match ev {
            SwarmEvent::ConnectionEstablished { peer_id, connection_id, endpoint, .. } if peer_id == other => {
                let addr = endpoint.get_remote_address().clone();
                if endpoint.is_relayed() {
                    println!("[{name}] relayed connection: {addr}");
                    r.relayed = true;
                    if let Some(tx) = sender.take() {
                        let control = swarm.behaviour().stream.new_control();
                        tokio::spawn(async move {
                            let _ = tx.send(Bulk::Sent(send_bulk(control, other).await));
                        });
                    }
                } else {
                    println!("[{name}] direct connection: {addr} ({})", if endpoint.is_dialer() { "dialed out" } else { "came in" });
                    r.direct = Some(addr);
                    direct_conns.push(connection_id);
                }
            }
            SwarmEvent::Behaviour(PeerEvent::Dcutr(dcutr::Event { remote_peer_id, result })) => {
                println!("[{name}] hole punch with {remote_peer_id}: {result:?}");
                r.dcutr = Some(result.map(|_| ()).map_err(|e| e.to_string()));
            }
            SwarmEvent::Behaviour(PeerEvent::Ping(ping::Event { peer, connection, result })) if peer == other && direct_conns.contains(&connection) => {
                if let Ok(rtt) = result {
                    if r.direct_rtt.is_none() {
                        println!("[{name}] ping over the direct connection: {rtt:?}");
                        r.direct_rtt = Some(rtt);
                        r.took = Some(start.elapsed());
                    }
                }
            }
            SwarmEvent::Behaviour(PeerEvent::RelayClient(e)) => println!("[{name}] {e:?}"),
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => println!("[{name}] a dial failed ({peer_id:?}): {error}"),
            SwarmEvent::ConnectionClosed { peer_id, endpoint, cause, .. } if peer_id == other => {
                println!("[{name}] closed {} ({cause:?})", endpoint.get_remote_address());
            }
            _ => {}
        }
    }
}

fn circuit(relay_addr: &Multiaddr) -> Multiaddr {
    relay_addr.clone().with(Protocol::P2p(id(RELAY_SEED))).with(Protocol::P2pCircuit)
}

async fn run_listener(relay_addr: Multiaddr, o: Opts, ready: Option<oneshot::Sender<()>>) -> Res<(Report, Swarm<Peer>)> {
    let mut swarm = timeout(LIMIT, start_peer("listener", LISTENER_SEED, &relay_addr, o)).await??;
    let (tx, rx) = mpsc::unbounded_channel();
    accept_bulk(&mut swarm, tx)?;
    swarm.listen_on(circuit(&relay_addr))?;
    let reserved = timeout(LIMIT, async {
        loop {
            if let SwarmEvent::Behaviour(PeerEvent::RelayClient(e @ relay::client::Event::ReservationReqAccepted { .. })) = swarm.select_next_some().await {
                println!("[listener] {e:?}");
                return;
            }
        }
    });
    reserved.await?;
    println!("[listener] reachable at {}", circuit(&relay_addr).with(Protocol::P2p(id(LISTENER_SEED))));
    if let Some(tx) = ready {
        let _ = tx.send(());
    }
    let r = watch("listener", &mut swarm, id(DIALER_SEED), o, rx, None).await;
    Ok((r, swarm))
}

async fn run_dialer(relay_addr: Multiaddr, o: Opts) -> Res<(Report, Swarm<Peer>)> {
    let mut swarm = timeout(LIMIT, start_peer("dialer", DIALER_SEED, &relay_addr, o)).await??;
    swarm.dial(circuit(&relay_addr).with(Protocol::P2p(id(LISTENER_SEED))))?;
    let (tx, rx) = mpsc::unbounded_channel();
    let sender = (!o.punch).then_some(tx);
    let r = watch("dialer", &mut swarm, id(LISTENER_SEED), o, rx, sender).await;
    Ok((r, swarm))
}

async fn demo(o: Opts) -> Res<(Report, Report)> {
    let (tx, rx) = oneshot::channel();
    let relay = tokio::spawn(async move {
        if let Err(e) = run_relay(0, Some(tx)).await {
            eprintln!("[relay] stopped: {e}");
        }
    });
    let (tcp, quic) = timeout(LIMIT, rx).await??;
    let relay_addr = if o.quic { quic } else { tcp };
    let (ltx, lrx) = oneshot::channel();
    let listener = tokio::spawn(run_listener(relay_addr.clone(), o, Some(ltx)));
    timeout(LIMIT, lrx).await??;
    // Both swarms stay alive until both sides are done: a side that drops
    // its swarm early closes the other's fresh direct connection.
    let (d, _dialer_swarm) = run_dialer(relay_addr, o).await?;
    let (l, _listener_swarm) = timeout(LIMIT + Duration::from_secs(5), listener).await???;
    relay.abort();
    Ok((l, d))
}

fn summary(name: &str, r: &Report) {
    let punch = match &r.dcutr {
        Some(Ok(())) => "worked".to_string(),
        Some(Err(e)) => format!("failed: {e}"),
        None => "no result".to_string(),
    };
    let direct = r.direct.as_ref().map_or("none".to_string(), |a| a.to_string());
    println!("{name}: relayed {}, hole punch {punch}, direct {direct}, rtt {:?}, direct after {:?}, bulk {:?}", r.relayed, r.direct_rtt, r.took, r.bulk);
}

fn relay_arg(a: Option<String>) -> Res<Multiaddr> {
    let addr: Multiaddr = a.ok_or("give the relay's address, e.g. /ip4/127.0.0.1/tcp/4801")?.parse()?;
    if !loopback_only(&addr) {
        return Err("this spike only talks to 127.0.0.1".into());
    }
    Ok(addr)
}

#[tokio::main]
async fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let o = Opts { confirm: flag("--confirm"), punch: !flag("--relay-only"), quic: flag("--quic") };
    let mut rest = args.iter().filter(|a| !a.starts_with("--")).cloned();
    match rest.next().as_deref() {
        Some("relay") => run_relay(rest.next().map(|p| p.parse()).transpose()?.unwrap_or(0), None).await,
        Some("listen") => {
            let (r, _swarm) = run_listener(relay_arg(rest.next())?, o, None).await?;
            summary("listener", &r);
            Ok(())
        }
        Some("dial") => {
            let (r, _swarm) = run_dialer(relay_arg(rest.next())?, o).await?;
            summary("dialer", &r);
            Ok(())
        }
        Some("rates") => {
            let n: usize = rest.next().map(|s| s.parse()).transpose()?.unwrap_or(50);
            for quic in [false, true] {
                let (mut direct, mut punched, mut took) = (0, 0, Vec::new());
                for _ in 0..n {
                    let (l, d) = demo(Opts { quic, ..o }).await?;
                    if l.direct_rtt.is_some() && d.direct_rtt.is_some() {
                        direct += 1;
                        took.extend(d.took);
                    }
                    if matches!(l.dcutr, Some(Ok(()))) || matches!(d.dcutr, Some(Ok(()))) {
                        punched += 1;
                    }
                }
                took.sort();
                let median = took.get(took.len() / 2).copied();
                println!("RATES {}: direct on both sides {direct}/{n}, a hole punch reported {punched}/{n}, median time to direct {median:?}", if quic { "quic" } else { "tcp" });
            }
            Ok(())
        }
        Some("demo") | None => {
            let started = Instant::now();
            let (l, d) = demo(o).await?;
            summary("listener", &l);
            summary("dialer", &d);
            println!("whole demo: {:?}", started.elapsed());
            let ok = if o.punch { d.direct_rtt.is_some() } else { d.bulk.is_some() };
            if !ok {
                std::process::exit(1);
            }
            Ok(())
        }
        Some(other) => Err(format!("unknown mode {other}").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_addresses_pass() {
        assert!(loopback_only(&"/ip4/127.0.0.1/tcp/4801".parse().unwrap()));
        assert!(loopback_only(&"/ip6/::1/udp/4801/quic-v1".parse().unwrap()));
        assert!(!loopback_only(&"/ip4/192.168.1.4/tcp/4801".parse().unwrap()));
        assert!(!loopback_only(&"/dns4/localhost/tcp/4801".parse().unwrap()));
        assert!(!loopback_only(&"/p2p-circuit".parse().unwrap()));
    }

    async fn goes_direct(quic: bool) {
        let (l, d) = demo(Opts { confirm: false, punch: true, quic }).await.unwrap();
        assert!(l.relayed && d.relayed, "{l:?} {d:?}");
        assert!(matches!(d.dcutr, Some(Ok(()))) || matches!(l.dcutr, Some(Ok(()))), "{l:?} {d:?}");
        assert!(d.direct_rtt.is_some() && l.direct_rtt.is_some(), "{l:?} {d:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn peers_meet_through_the_relay_then_go_direct_over_quic() {
        goes_direct(true).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "Windows sometimes refuses the TCP simultaneous open (os error 10048); `rates` measures how often"]
    async fn peers_meet_through_the_relay_then_go_direct_over_tcp() {
        goes_direct(false).await;
    }

    // Kept as a test so a libp2p upgrade that changes this shows up.
    #[tokio::test(flavor = "multi_thread")]
    async fn confirmed_addresses_leave_dcutr_nothing_to_offer() {
        let (l, d) = demo(Opts { confirm: true, punch: true, quic: false }).await.unwrap();
        assert!(l.relayed && d.relayed, "{l:?} {d:?}");
        assert!(d.dcutr.as_ref().is_some_and(|r| r.is_err()), "{d:?}");
        assert!(d.direct.is_none(), "{d:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn relayed_streams_work_until_the_circuit_byte_limit() {
        let (l, d) = demo(Opts { confirm: false, punch: false, quic: false }).await.unwrap();
        assert!(matches!(d.bulk, Some(Err(_))), "{d:?}");
        // The receiver sees a clean end of stream even though the relay cut
        // it, so real protocols must carry their own lengths.
        let got = l.bulk.unwrap().unwrap();
        assert!(got > 0 && got <= 128 * 1024, "{got}");
    }
}
