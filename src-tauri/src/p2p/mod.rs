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
pub mod settings;
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
    let node = Node::start(NodeConfig::any_port(key.to_vec())).map_err(|e| e.to_string())?;
    invites::answer(&state, &node);
    *NODE.lock().unwrap() = Some(Arc::new(node));
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
