// SPDX-License-Identifier: AGPL-3.0-only
//! The composed libp2p behaviour. Later milestones add the relay client,
//! DCUtR, AutoNAT, Kademlia, mDNS and UPnP here, each behind a toggle.

use std::time::Duration;

use libp2p::identity::Keypair;
use libp2p::swarm::NetworkBehaviour;
use libp2p::{identify, ping};

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
}

impl Behaviour {
    pub fn new(key: &Keypair) -> Self {
        Self {
            identify: identify::Behaviour::new(
                identify::Config::new(PROTOCOL_VERSION.into(), key.public())
                    .with_agent_version(format!("sulcus-net/{}", env!("CARGO_PKG_VERSION"))),
            ),
            ping: ping::Behaviour::new(ping::Config::new().with_interval(PING_EVERY)),
            stream: libp2p_stream::Behaviour::new(),
        }
    }
}
