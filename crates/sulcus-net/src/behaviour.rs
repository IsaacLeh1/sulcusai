// SPDX-License-Identifier: AGPL-3.0-only
//! The composed libp2p behaviour. Finding PCs on the local network (mDNS)
//! and asking the router to open a port (UPnP) are each behind their own
//! switch and absent when off; the reachability check (AutoNAT v2) only
//! asks peers this PC is already connected to, never a public server.

use std::time::Duration;

use libp2p::identity::Keypair;
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::NetworkBehaviour;
use libp2p::{autonat, identify, mdns, ping, upnp};

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
}

impl Behaviour {
    pub fn new(key: &Keypair, lan_discovery: bool, router_ports: bool) -> std::io::Result<Self> {
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
        })
    }
}
