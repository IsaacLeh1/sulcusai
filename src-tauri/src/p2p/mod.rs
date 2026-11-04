// SPDX-License-Identifier: AGPL-3.0-only
//! The peer-to-peer node in the app: started and stopped from the Network
//! switch, and nothing else. With the switch off (the default) no task,
//! thread or socket of the node exists.
//!
//! Listening ports are always picked by the OS (port 0), so they are random
//! at every start whatever the shield's random-port switch says; a fixed
//! port for router forwarding comes with the router-port switch.

pub mod invites;
pub mod keys;
pub mod relays;
pub mod settings;
pub mod remote_io;
pub mod sharing;
pub mod vlan_adapter;
#[cfg(test)]
mod tests;

use std::sync::{Arc, Mutex};

use serde::Serialize;
use sulcus_net::libp2p;
use sulcus_net::{Node, NodeConfig, NodeHandle};

use crate::offline::{self, Keep};
use crate::{AppState, AppStateRef};

pub use settings::P2pSettings;

static NODE: Mutex<Option<Arc<NodeHandle>>> = Mutex::new(None);

/// Whether the node may run now: the switch is on and fully offline (if on)
/// keeps the local network. Under that level the node should also keep to
/// LAN addresses; until the address book exists it dials only what the
/// person typed.
pub fn allowed(s: &P2pSettings, offline_blocks_lan: bool) -> bool {
    s.network && !offline_blocks_lan
}

/// Starts or stops the node to match the settings. Needs the data key for
/// the node key, so a locked vault leaves the node off.
pub async fn apply(state: Arc<AppState>) -> Result<(), String> {
    let s = settings::load(&state.db.lock().unwrap());
    if !allowed(&s, offline::blocks(Keep::LocalNetwork)) {
        stop().await;
        return Ok(());
    }
    if NODE.lock().unwrap().is_some() {
        return Ok(());
    }
    let cipher = state.vault.lock().unwrap().cipher().ok_or("Unlock SulcusAI first.")?;
    let key = keys::node_key(&state.db.lock().unwrap(), &cipher)?;
    let serve = s.relay_for_others && !crate::consent::needs(&state.db.lock().unwrap(), "relay");
    let cfg = NodeConfig { lan_discovery: s.lan_discovery, router_ports: s.router_ports, relay_server: serve, ..NodeConfig::any_port(key.to_vec()) };
    let node = Arc::new(Node::start(cfg).map_err(|e| e.to_string())?);
    invites::answer(&state, &node);
    crate::sync::people::net::serve(&state, &node);
    sharing::serve(&state, &node);
    *NODE.lock().unwrap() = Some(node.clone());
    // Reachable through the saved relays: connect, then ask each for a reservation.
    for r in s.relays.iter().filter_map(|r| r.parse::<libp2p::Multiaddr>().ok()) {
        let n = node.clone();
        tokio::spawn(async move {
            let Some(libp2p::multiaddr::Protocol::P2p(relay)) = r.iter().last() else { return };
            let mut events = n.subscribe();
            if n.dial(r.clone()).await.is_err() {
                return;
            }
            // A reservation asked for before the relay is identified is dropped.
            let identified = tokio::time::timeout(std::time::Duration::from_secs(30), async {
                loop {
                    match events.recv().await {
                        Ok(sulcus_net::NodeEvent::Identified { peer, .. }) if peer == relay => return true,
                        Ok(_) => {}
                        Err(_) => return false,
                    }
                }
            })
            .await
            .unwrap_or(false);
            if identified {
                let _ = n.listen_via(r).await;
            }
        });
    }
    Ok(())
}

pub async fn stop() {
    let node = NODE.lock().unwrap().take();
    if let Some(n) = node {
        n.stop().await;
    }
}

pub fn running() -> Option<Arc<NodeHandle>> {
    NODE.lock().unwrap().clone()
}

/// Called once at startup; does nothing while the switch is off.
pub fn start(state: Arc<AppState>) {
    if !settings::load(&state.db.lock().unwrap()).network {
        return;
    }
    tauri::async_runtime::spawn(async move {
        if let Err(e) = apply(state).await {
            crate::applog::line("network", &e);
        }
    });
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct P2pStatus {
    pub settings: P2pSettings,
    pub running: bool,
    pub node_id: Option<String>,
    pub listening: Vec<String>,
    pub connected: Vec<String>,
    /// PCs seen on the local network (LAN discovery on).
    pub lan_peers: Vec<String>,
    /// Addresses the router forwards here (router ports on).
    pub external: Vec<String>,
    /// Whether connected peers could reach this PC, per address tried.
    pub reachable: Vec<(String, bool)>,
}

#[tauri::command]
pub fn p2p_status(state: AppStateRef) -> P2pStatus {
    let settings = settings::load(&state.db.lock().unwrap());
    let node = running();
    P2pStatus {
        settings,
        running: node.is_some(),
        node_id: node.as_ref().map(|n| n.peer_id().to_string()),
        listening: node.as_ref().map_or(vec![], |n| n.listening().iter().map(|a| a.to_string()).collect()),
        connected: node.as_ref().map_or(vec![], |n| n.connected().iter().map(|p| p.to_string()).collect()),
        lan_peers: node.as_ref().map_or(vec![], |n| n.lan_peers().iter().map(|(p, _)| p.to_string()).collect()),
        external: node.as_ref().map_or(vec![], |n| n.external().iter().map(|a| a.to_string()).collect()),
        reachable: node.as_ref().map_or(vec![], |n| n.reachability().into_iter().map(|(a, r)| (a.to_string(), r)).collect()),
    }
}

#[tauri::command]
pub async fn p2p_set_network(state: AppStateRef<'_>, on: bool) -> Result<P2pStatus, String> {
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings::load(&conn);
        s.network = on;
        settings::save(&conn, &s)?;
    }
    apply(state.inner().clone()).await?;
    Ok(p2p_status(state))
}

/// LAN discovery, router ports and relaying for others; the node restarts
/// to take them. Relaying needs its notice accepted first.
#[tauri::command]
pub async fn p2p_set_options(state: AppStateRef<'_>, lan_discovery: bool, router_ports: bool, relay_for_others: Option<bool>) -> Result<P2pStatus, String> {
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings::load(&conn);
        s.lan_discovery = lan_discovery;
        s.router_ports = router_ports;
        if let Some(on) = relay_for_others {
            if on {
                crate::consent::require(&conn, "relay")?;
            }
            s.relay_for_others = on;
        }
        settings::save(&conn, &s)?;
    }
    stop().await;
    apply(state.inner().clone()).await?;
    Ok(p2p_status(state))
}

/// Adds (or with `remove`, drops) a relay this PC stays reachable through.
#[tauri::command]
pub async fn p2p_set_relay(state: AppStateRef<'_>, address: String, remove: bool) -> Result<P2pStatus, String> {
    let addr: libp2p::Multiaddr = address.trim().parse().map_err(|_| "That isn't a relay address.".to_string())?;
    if !matches!(addr.iter().last(), Some(libp2p::multiaddr::Protocol::P2p(_))) {
        return Err("The relay's address needs its id at the end (/p2p/...).".into());
    }
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings::load(&conn);
        s.relays.retain(|r| r != &addr.to_string());
        if !remove {
            if s.relays.len() >= 4 {
                return Err("Up to 4 relays.".into());
            }
            s.relays.push(addr.to_string());
        }
        settings::save(&conn, &s)?;
    }
    stop().await;
    apply(state.inner().clone()).await?;
    Ok(p2p_status(state))
}

/// Connects to a node by its full address (`/ip4/.../udp/.../quic-v1/p2p/...`).
#[tauri::command]
pub async fn p2p_dial(address: String) -> Result<(), String> {
    let node = running().ok_or("Turn on Network first.")?;
    let addr: libp2p::Multiaddr = address.trim().parse().map_err(|_| "That isn't a node address.".to_string())?;
    // Without the node id, Noise would accept whoever answers at that address.
    if !addr.iter().any(|p| matches!(p, libp2p::multiaddr::Protocol::P2p(_))) {
        return Err("The address needs the node id at the end (/p2p/...).".into());
    }
    node.dial(addr).await.map_err(|e| e.to_string())
}
