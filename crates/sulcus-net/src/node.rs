// SPDX-License-Identifier: AGPL-3.0-only
//! The node: one Swarm on QUIC and TCP (Noise, yamux), run as a task on the
//! caller's tokio runtime. It has no threads of its own, so with the node
//! stopped nothing of it is left running.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use futures::StreamExt;
use libp2p::core::multiaddr::{Multiaddr, Protocol};
use libp2p::swarm::{dial_opts::DialOpts, SwarmEvent};
use libp2p::{autonat, dcutr, identify, mdns, noise, relay, tcp, upnp, yamux, PeerId, Swarm, SwarmBuilder};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::behaviour::{Behaviour, BehaviourEvent, PROTOCOL_VERSION};
use crate::identity;

/// Long enough that a chat paused for a minute keeps its connection, short
/// enough that peers nobody uses don't hold sockets open.
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// Slow listeners on the window side must not hold the node back; missed
/// events show up as `Lagged` and the window asks for a fresh status.
const EVENT_BUFFER: usize = 256;

#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// The node key as `identity::generate` made it.
    pub key: Vec<u8>,
    /// Where to listen. Port 0 lets the OS pick, which also keeps the port
    /// from being a fixed fingerprint.
    pub listen: Vec<Multiaddr>,
    /// Find other SulcusAI PCs on the local network (mDNS).
    pub lan_discovery: bool,
    /// Ask the router to forward a port to this PC (UPnP).
    pub router_ports: bool,
    /// Relay for others who can't reach each other directly.
    pub relay_server: bool,
}

impl NodeConfig {
    /// Every interface, QUIC and TCP, ports picked by the OS.
    pub fn any_port(key: Vec<u8>) -> Self {
        Self { key, listen: vec!["/ip4/0.0.0.0/udp/0/quic-v1".parse().unwrap(), "/ip4/0.0.0.0/tcp/0".parse().unwrap()], lan_discovery: false, router_ports: false, relay_server: false }
    }

    /// 127.0.0.1 only, for tests: nothing leaves the PC.
    pub fn loopback(key: Vec<u8>) -> Self {
        Self { key, listen: vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap(), "/ip4/127.0.0.1/tcp/0".parse().unwrap()], lan_discovery: false, router_ports: false, relay_server: false }
    }
}

#[derive(Debug)]
pub enum NodeError {
    Key(identity::BadKey),
    Build(String),
    Listen(Multiaddr, String),
    Stopped,
}

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeError::Key(e) => e.fmt(f),
            NodeError::Build(e) => write!(f, "the network couldn't start: {e}"),
            NodeError::Listen(a, e) => write!(f, "couldn't listen on {a}: {e}"),
            NodeError::Stopped => f.write_str("the network is off"),
        }
    }
}

impl std::error::Error for NodeError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeEvent {
    Listening(Multiaddr),
    ListenerClosed(Multiaddr),
    Connected { peer: PeerId, addr: Multiaddr },
    /// The last connection to this peer closed.
    Disconnected { peer: PeerId },
    Identified { peer: PeerId, agent: String },
    DialFailed { peer: Option<PeerId>, error: String },
    /// A SulcusAI PC on the local network (mDNS on).
    LanPeer { peer: PeerId, addr: Multiaddr },
    LanPeerGone { peer: PeerId },
    /// The router forwards this address to the PC (UPnP on).
    ExternalAddr(Multiaddr),
    ExternalAddrGone(Multiaddr),
    /// UPnP found no router that would map a port.
    NoRouterMapping(String),
    /// A connected peer tried to reach this PC at `addr`.
    Reachability { addr: Multiaddr, reachable: bool },
    /// A relay agreed to pass connections to this PC.
    Reserved { relay: PeerId },
    /// A relayed connection became direct (or the try failed).
    Direct { peer: PeerId, ok: bool },
    /// Listening (on an address or through a relay) stopped with an error.
    ListenFailed { error: String },
}

enum Command {
    Dial(Multiaddr, oneshot::Sender<Result<(), String>>),
    Listen(Multiaddr, oneshot::Sender<Result<(), String>>),
    Stop,
}

pub struct Node;

impl Node {
    /// Builds the Swarm, binds every listen address and spawns the event
    /// loop. Must run inside a tokio runtime.
    pub fn start(cfg: NodeConfig) -> Result<NodeHandle, NodeError> {
        let key = identity::load(&cfg.key).map_err(NodeError::Key)?;
        let peer_id = key.public().to_peer_id();
        let (lan, router, serve) = (cfg.lan_discovery, cfg.router_ports, cfg.relay_server);
        let mut swarm = SwarmBuilder::with_existing_identity(key)
            .with_tokio()
            .with_tcp(tcp::Config::default().nodelay(true), noise::Config::new, yamux::Config::default)
            .map_err(|e| NodeError::Build(e.to_string()))?
            .with_quic()
            .with_relay_client(noise::Config::new, yamux::Config::default)
            .map_err(|e| NodeError::Build(e.to_string()))?
            .with_behaviour(|k, rc| Behaviour::new(k, rc, lan, router, serve).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>))
            .map_err(|e| NodeError::Build(e.to_string()))?
            .with_swarm_config(|c| c.with_idle_connection_timeout(IDLE_TIMEOUT))
            .build();
        for a in cfg.listen {
            swarm.listen_on(a.clone()).map_err(|e| NodeError::Listen(a, e.to_string()))?;
        }
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let shared = Arc::new(Shared::default());
        let control = swarm.behaviour().stream.new_control();
        let task = tokio::spawn(run(swarm, cmd_rx, events.clone(), shared.clone(), serve));
        Ok(NodeHandle { peer_id, cmd: cmd_tx, events, shared, control, task: Mutex::new(Some(task)) })
    }
}

#[derive(Default)]
struct Shared {
    listening: Mutex<Vec<Multiaddr>>,
    connected: Mutex<HashSet<PeerId>>,
    /// Checked on every connection, inbound or outbound: a removed device's
    /// node id stays out even if it still knows this PC's address.
    blocked: RwLock<HashSet<PeerId>>,
    lan: Mutex<HashMap<PeerId, Multiaddr>>,
    external: Mutex<Vec<Multiaddr>>,
    /// The last reachability answers, by address.
    reachable: Mutex<HashMap<Multiaddr, bool>>,
    /// While relaying: who may use it (None: anyone, within the limits).
    relay_members: RwLock<Option<HashSet<PeerId>>>,
}

pub struct NodeHandle {
    peer_id: PeerId,
    cmd: mpsc::Sender<Command>,
    events: broadcast::Sender<NodeEvent>,
    shared: Arc<Shared>,
    control: libp2p_stream::Control,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl NodeHandle {
    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    pub fn subscribe(&self) -> broadcast::Receiver<NodeEvent> {
        self.events.subscribe()
    }

    /// The addresses bound right now (port 0 replaced by the real port).
    pub fn listening(&self) -> Vec<Multiaddr> {
        self.shared.listening.lock().unwrap().clone()
    }

    pub fn connected(&self) -> Vec<PeerId> {
        self.shared.connected.lock().unwrap().iter().copied().collect()
    }

    /// SulcusAI PCs seen on the local network (mDNS on).
    pub fn lan_peers(&self) -> Vec<(PeerId, Multiaddr)> {
        self.shared.lan.lock().unwrap().iter().map(|(p, a)| (*p, a.clone())).collect()
    }

    /// Addresses the router forwards to this PC (UPnP on).
    pub fn external(&self) -> Vec<Multiaddr> {
        self.shared.external.lock().unwrap().clone()
    }

    /// Whether peers could reach this PC, per address tried.
    pub fn reachability(&self) -> Vec<(Multiaddr, bool)> {
        self.shared.reachable.lock().unwrap().iter().map(|(a, r)| (a.clone(), *r)).collect()
    }

    /// Opens and accepts the app's own protocols.
    pub fn streams(&self) -> libp2p_stream::Control {
        self.control.clone()
    }

    /// Limits relaying to these peers. libp2p's relay has no admission hook,
    /// so anyone else is cut off as soon as a reservation or circuit of
    /// theirs is accepted.
    pub fn set_relay_members(&self, members: Option<HashSet<PeerId>>) {
        *self.shared.relay_members.write().unwrap() = members;
    }

    pub fn set_blocked(&self, peers: impl IntoIterator<Item = PeerId>) {
        *self.shared.blocked.write().unwrap() = peers.into_iter().collect();
    }

    /// Dials an address (with or without `/p2p/<id>`); resolves once the
    /// dial started, not when it connects: `Connected` or `DialFailed` follow.
    pub async fn dial(&self, addr: Multiaddr) -> Result<(), NodeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd.send(Command::Dial(addr, tx)).await.map_err(|_| NodeError::Stopped)?;
        rx.await.map_err(|_| NodeError::Stopped)?.map_err(NodeError::Build)
    }

    /// Becomes reachable through a relay: `relay` is the relay's address
    /// ending in `/p2p/<relay id>`. `Reserved` follows when it agrees.
    pub async fn listen_via(&self, relay: Multiaddr) -> Result<(), NodeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd.send(Command::Listen(relay.with(Protocol::P2pCircuit), tx)).await.map_err(|_| NodeError::Stopped)?;
        rx.await.map_err(|_| NodeError::Stopped)?.map_err(NodeError::Build)
    }

    /// Closes every connection and listener and waits for the task to end,
    /// so the ports are free when this returns.
    pub async fn stop(&self) {
        let _ = self.cmd.send(Command::Stop).await;
        let task = self.task.lock().unwrap().take();
        if let Some(t) = task {
            let _ = t.await;
        }
    }
}

impl Drop for NodeHandle {
    fn drop(&mut self) {
        if let Some(t) = self.task.lock().unwrap().take() {
            t.abort();
        }
    }
}

/// The peer an address leads to (its last `/p2p/`, which for an address
/// through a relay is the peer, not the relay), and the address without it.
fn split_peer(a: &Multiaddr) -> (Option<PeerId>, Multiaddr) {
    let mut parts: Vec<Protocol> = a.iter().collect();
    match parts.last() {
        Some(Protocol::P2p(id)) => {
            let id = *id;
            parts.pop();
            (Some(id), parts.into_iter().collect())
        }
        _ => (None, a.clone()),
    }
}

async fn run(mut swarm: Swarm<Behaviour>, mut cmds: mpsc::Receiver<Command>, events: broadcast::Sender<NodeEvent>, shared: Arc<Shared>, relaying: bool) {
    let send = |e: NodeEvent| {
        let _ = events.send(e);
    };
    loop {
        tokio::select! {
            cmd = cmds.recv() => match cmd {
                Some(Command::Dial(addr, reply)) => {
                    let (peer, rest) = split_peer(&addr);
                    if peer.is_some_and(|p| shared.blocked.read().unwrap().contains(&p)) {
                        let _ = reply.send(Err("that device was removed".into()));
                        continue;
                    }
                    let opts = match peer {
                        Some(p) => DialOpts::peer_id(p).addresses(vec![rest]).build(),
                        None => DialOpts::unknown_peer_id().address(addr).build(),
                    };
                    let _ = reply.send(swarm.dial(opts).map_err(|e| e.to_string()));
                }
                Some(Command::Listen(addr, reply)) => {
                    let _ = reply.send(swarm.listen_on(addr).map(|_| ()).map_err(|e| e.to_string()));
                }
                Some(Command::Stop) | None => break,
            },
            ev = swarm.select_next_some() => match ev {
                SwarmEvent::NewListenAddr { address, .. } => {
                    // A relay answers reservations with its own addresses, and
                    // only once it has some it stands by; relaying is chosen
                    // for a PC others can reach, so its listen addresses are.
                    if relaying && !address.iter().any(|p| matches!(p, Protocol::P2pCircuit)) {
                        swarm.add_external_address(address.clone());
                    }
                    shared.listening.lock().unwrap().push(address.clone());
                    send(NodeEvent::Listening(address));
                }
                SwarmEvent::ExpiredListenAddr { address, .. } => {
                    shared.listening.lock().unwrap().retain(|a| a != &address);
                    send(NodeEvent::ListenerClosed(address));
                }
                SwarmEvent::ConnectionEstablished { peer_id, endpoint, num_established, .. } => {
                    if shared.blocked.read().unwrap().contains(&peer_id) {
                        let _ = swarm.disconnect_peer_id(peer_id);
                        continue;
                    }
                    if num_established.get() == 1 {
                        shared.connected.lock().unwrap().insert(peer_id);
                        send(NodeEvent::Connected { peer: peer_id, addr: endpoint.get_remote_address().clone() });
                    }
                }
                SwarmEvent::ConnectionClosed { peer_id, num_established: 0, .. } => {
                    if shared.connected.lock().unwrap().remove(&peer_id) {
                        send(NodeEvent::Disconnected { peer: peer_id });
                    }
                }
                SwarmEvent::ListenerError { error, .. } => {
                    send(NodeEvent::ListenFailed { error: error.to_string() });
                }
                SwarmEvent::ListenerClosed { reason: Err(error), .. } => {
                    send(NodeEvent::ListenFailed { error: error.to_string() });
                }
                SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                    send(NodeEvent::DialFailed { peer: peer_id, error: error.to_string() });
                }
                SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received { peer_id, info, .. })) => {
                    // Not a SulcusAI node: nothing to say to it.
                    if info.protocol_version != PROTOCOL_VERSION {
                        let _ = swarm.disconnect_peer_id(peer_id);
                        continue;
                    }
                    send(NodeEvent::Identified { peer: peer_id, agent: info.agent_version });
                }
                SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(found))) => {
                    for (peer, addr) in found {
                        if peer == *swarm.local_peer_id() || shared.blocked.read().unwrap().contains(&peer) {
                            continue;
                        }
                        if shared.lan.lock().unwrap().insert(peer, addr.clone()).is_none() {
                            send(NodeEvent::LanPeer { peer, addr });
                        }
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Expired(gone))) => {
                    for (peer, _) in gone {
                        if shared.lan.lock().unwrap().remove(&peer).is_some() {
                            send(NodeEvent::LanPeerGone { peer });
                        }
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Upnp(upnp::Event::NewExternalAddr { external_addr, .. })) => {
                    shared.external.lock().unwrap().push(external_addr.clone());
                    send(NodeEvent::ExternalAddr(external_addr));
                }
                SwarmEvent::Behaviour(BehaviourEvent::Upnp(upnp::Event::ExpiredExternalAddr { external_addr, .. })) => {
                    shared.external.lock().unwrap().retain(|a| a != &external_addr);
                    send(NodeEvent::ExternalAddrGone(external_addr));
                }
                SwarmEvent::Behaviour(BehaviourEvent::Upnp(upnp::Event::GatewayNotFound)) => {
                    send(NodeEvent::NoRouterMapping("No router on this network answered a port request (UPnP may be off on it).".into()));
                }
                SwarmEvent::Behaviour(BehaviourEvent::Upnp(upnp::Event::NonRoutableGateway)) => {
                    send(NodeEvent::NoRouterMapping("The router answered, but it isn't on the internet itself (another router or the provider's network is in front of it).".into()));
                }
                SwarmEvent::Behaviour(BehaviourEvent::RelayClient(relay::client::Event::ReservationReqAccepted { relay_peer_id, .. })) => {
                    send(NodeEvent::Reserved { relay: relay_peer_id });
                }
                SwarmEvent::Behaviour(BehaviourEvent::RelayServer(relay::Event::ReservationReqAccepted { src_peer_id, .. } | relay::Event::CircuitReqAccepted { src_peer_id, .. })) => {
                    let allowed = shared.relay_members.read().unwrap().as_ref().is_none_or(|m| m.contains(&src_peer_id));
                    if !allowed {
                        let _ = swarm.disconnect_peer_id(src_peer_id);
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Dcutr(dcutr::Event { remote_peer_id, result })) => {
                    send(NodeEvent::Direct { peer: remote_peer_id, ok: result.is_ok() });
                }
                SwarmEvent::Behaviour(BehaviourEvent::Autonat(autonat::v2::client::Event { tested_addr, result, .. })) => {
                    let reachable = result.is_ok();
                    shared.reachable.lock().unwrap().insert(tested_addr.clone(), reachable);
                    send(NodeEvent::Reachability { addr: tested_addr, reachable });
                }
                _ => {}
            },
        }
    }
    shared.listening.lock().unwrap().clear();
    shared.connected.lock().unwrap().clear();
    shared.lan.lock().unwrap().clear();
    shared.external.lock().unwrap().clear();
}
