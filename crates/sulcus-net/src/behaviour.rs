// SPDX-License-Identifier: AGPL-3.0-only
//! The composed libp2p behaviour. Finding PCs on the local network (mDNS)
//! and asking the router to open a port (UPnP) are each behind their own
//! switch and absent when off; the reachability check (AutoNAT v2) only
//! asks peers this PC is already connected to, never a public server.

use std::time::Duration;

use libp2p::identity::Keypair;
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::NetworkBehaviour;
use libp2p::{autonat, dcutr, identify, kad, mdns, ping, relay, upnp};

/// SulcusAI's own DHT, apart from the public IPFS one.
pub const KAD_PROTOCOL: libp2p::StreamProtocol = libp2p::StreamProtocol::new("/sulcus/kad/1");

/// Sent in identify; peers on another major version are told apart early.
pub const PROTOCOL_VERSION: &str = "/sulcus/1";

/// Often enough to notice a dead connection, rare enough to cost nothing
/// idle; 25 s also keeps a NAT's UDP mapping open for QUIC.
pub const PING_EVERY: Duration = Duration::from_secs(25);

#[derive(NetworkBehaviour)]
pub struct Behaviour {
    pub identify: identify::Behaviour,
    pub ping: ping::Behaviour,
    pub stream: libp2p_stream::Behaviour,
    pub mdns: Toggle<mdns::tokio::Behaviour>,
    pub upnp: Toggle<upnp::tokio::Behaviour>,
    pub autonat: autonat::v2::client::Behaviour,
    pub autonat_server: autonat::v2::server::Behaviour,
    /// Reaching peers through a relay, and trying for a direct path after.
    pub relay_client: relay::client::Behaviour,
    pub dcutr: dcutr::Behaviour,
    /// Being a relay for others ("Help this group connect?"), off unless asked.
    pub relay_server: Toggle<relay::Behaviour>,
    /// Finding peers by id through other SulcusAI PCs (Kademlia), off
    /// unless asked; answering others' lookups only when helping strangers.
    pub kad: Toggle<kad::Behaviour<kad::store::MemoryStore>>,
}

/// A PC relaying for others carries little: a few reservations, short
/// circuits, so helping costs its owner almost nothing.
pub fn relay_limits() -> relay::Config {
    relay::Config {
        max_reservations: 32,
        max_reservations_per_peer: 2,
        max_circuits: 16,
        max_circuits_per_peer: 4,
        max_circuit_duration: Duration::from_secs(10 * 60),
        max_circuit_bytes: 64 * 1024 * 1024,
        ..Default::default()
    }
}

impl Behaviour {
    pub fn new(key: &Keypair, relay_client: relay::client::Behaviour, lan_discovery: bool, router_ports: bool, relay_server: bool, dht: bool, dht_server: bool) -> std::io::Result<Self> {
        let me = key.public().to_peer_id();
        let mdns = if lan_discovery { Some(mdns::tokio::Behaviour::new(mdns::Config::default(), key.public().to_peer_id())?) } else { None };
        Ok(Self {
            identify: identify::Behaviour::new(
                identify::Config::new(PROTOCOL_VERSION.into(), key.public())
                    .with_agent_version(format!("sulcus-net/{}", env!("CARGO_PKG_VERSION"))),
            ),
            ping: ping::Behaviour::new(ping::Config::new().with_interval(PING_EVERY)),
            stream: libp2p_stream::Behaviour::new(),
            mdns: mdns.into(),
            upnp: router_ports.then(upnp::tokio::Behaviour::default).into(),
            autonat: autonat::v2::client::Behaviour::default(),
            autonat_server: autonat::v2::server::Behaviour::default(),
            relay_client,
            dcutr: dcutr::Behaviour::new(me),
            relay_server: relay_server.then(|| relay::Behaviour::new(me, relay_limits())).into(),
            kad: dht
                .then(|| {
                    let mut k = kad::Behaviour::with_config(me, kad::store::MemoryStore::new(me), kad::Config::new(KAD_PROTOCOL));
                    k.set_mode(Some(if dht_server { kad::Mode::Server } else { kad::Mode::Client }));
                    k
                })
                .into(),
        })
    }
}
