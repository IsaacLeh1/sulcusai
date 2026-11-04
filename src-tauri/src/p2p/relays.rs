// SPDX-License-Identifier: AGPL-3.0-only
//! Joining someone's relay with the invite key they gave, and managing a
//! relay this PC owns. Both talk to the relay over its own protocols; a
//! joined relay is added to the relays this PC stays reachable through once
//! the owner approves.

use serde::Serialize;
use sulcus_net::admin_wire::{self, Reply as AdminReply, Request as AdminRequest};
use sulcus_net::join_wire::{self, Reply};
use sulcus_net::libp2p::multiaddr::Protocol;
use sulcus_net::libp2p::{Multiaddr, PeerId};
use sulcus_net::NodeEvent;

use tauri::WebviewWindow;

use crate::AppStateRef;

fn relay_of(address: &str) -> Result<(Multiaddr, PeerId), String> {
    let addr: Multiaddr = address.trim().parse().map_err(|_| "That isn't a relay address.".to_string())?;
    match addr.iter().last() {
        Some(Protocol::P2p(id)) => Ok((addr, id)),
        _ => Err("The relay's address needs its id at the end (/p2p/...).".into()),
    }
}

/// Connected and identified, so the relay's protocols can be used.
async fn reach(node: &sulcus_net::NodeHandle, addr: Multiaddr, relay: PeerId) -> Result<(), String> {
    if node.connected().contains(&relay) {
        return Ok(());
    }
    let mut events = node.subscribe();
    node.dial(addr).await.map_err(|e| e.to_string())?;
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            match events.recv().await {
                Ok(NodeEvent::Identified { peer, .. }) if peer == relay => return Ok(()),
                Ok(NodeEvent::DialFailed { peer: Some(p), error }) if p == relay => return Err(error),
                Ok(_) => {}
                Err(_) => return Err("the network stopped".to_string()),
            }
        }
    })
    .await
    .map_err(|_| "The relay didn't answer.".to_string())?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinView {
    /// "waiting", "member", "refused" or "wait".
    pub state: String,
    pub detail: String,
}

#[tauri::command]
pub async fn p2p_join_relay(state: AppStateRef<'_>, address: String, key_id: String, secret: String) -> Result<JoinView, String> {
    let node = super::running().ok_or("Turn on Network first.")?;
    let (addr, relay) = relay_of(&address)?;
    reach(&node, addr.clone(), relay).await?;
    let name = state.vault.lock().unwrap().cipher().map(|c| crate::db::profile(&state.db.lock().unwrap(), &c).name).unwrap_or_default();
    let reply = join_wire::ask(&node, relay, key_id.trim(), secret.trim(), if name.trim().is_empty() { "SulcusAI" } else { &name }, "pc").await?;
    let view = match reply {
        Reply::Waiting => JoinView { state: "waiting".into(), detail: "Asked. The relay's owner has to approve this PC; try again later to see.".into() },
        Reply::Member => {
            // Approved: stay reachable through it from now on.
            let _ = super::p2p_set_relay(state, addr.to_string(), false).await;
            JoinView { state: "member".into(), detail: "You're a member of this relay. This PC is now reachable through it.".into() }
        }
        Reply::Refused(why) => JoinView { state: "refused".into(), detail: format!("Refused: {why}.") },
        Reply::Wait(secs) => JoinView { state: "wait".into(), detail: format!("Too many wrong tries. Wait {secs} s.") },
    };
    Ok(view)
}

/// Letting someone in or making a key that admits people.
fn lets_people_in(req: &AdminRequest) -> bool {
    matches!(req, AdminRequest::Decide { approve: true, .. } | AdminRequest::NewKey { .. })
}

/// With the app lock on, letting people in takes the owner's PIN or Windows
/// Hello, so someone at an unlocked PC can't open the relay to strangers.
async fn owner_present(window: &WebviewWindow, state: &AppStateRef<'_>, pin: Option<String>) -> Result<(), String> {
    let (lock, hello_on) = {
        let v = state.vault.lock().unwrap();
        (v.lock_enabled(), v.hello_enabled())
    };
    if !lock {
        return Ok(());
    }
    if let Some(pin) = pin.filter(|p| !p.is_empty()) {
        let wait = state.vault.lock().unwrap().penalty();
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        let checked = crate::security::blocking(state.inner(), move |s| s.vault.lock().unwrap().check_pin(&pin)).await;
        if checked.is_err() {
            state.log("security", "Wrong PIN entered for a relay decision");
        }
        return checked;
    }
    if hello_on {
        let hwnd = crate::security::hwnd_of(window)?;
        let ok = tauri::async_runtime::spawn_blocking(move || crate::hello::verify(hwnd, "Let someone use your relay"))
            .await
            .map_err(|e| e.to_string())??;
        return if ok { Ok(()) } else { Err("Windows Hello didn't verify you.".into()) };
    }
    Err(PIN_NEEDED.into())
}

/// The window shows a PIN box on this error.
pub const PIN_NEEDED: &str = "Enter your PIN to let people use your relay.";

/// For a relay this PC owns: who's waiting, who's in, and the decisions.
#[tauri::command]
pub async fn p2p_relay_admin(window: WebviewWindow, state: AppStateRef<'_>, address: String, request: AdminRequest, pin: Option<String>) -> Result<AdminReply, String> {
    if lets_people_in(&request) {
        owner_present(&window, &state, pin).await?;
        state.log("network", "Let someone use an owned relay");
    }
    let node = super::running().ok_or("Turn on Network first.")?;
    let (addr, relay) = relay_of(&address)?;
    reach(&node, addr, relay).await?;
    match admin_wire::ask(&node, relay, &request).await? {
        AdminReply::NotOwner => Err("This PC isn't one of that relay's owners. On the relay, run: sulcus-relay owner <this PC's network id>".into()),
        other => Ok(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relay_address_needs_its_id() {
        assert!(relay_of("/ip4/1.2.3.4/tcp/4001").is_err());
        assert!(relay_of("nonsense").is_err());
        let (_, id) = relay_of("/ip4/1.2.3.4/tcp/4001/p2p/12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN").unwrap();
        assert_eq!(id.to_string(), "12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN");
    }

    #[test]
    fn only_letting_people_in_needs_the_owner() {
        assert!(lets_people_in(&AdminRequest::Decide { peer: "p".into(), approve: true }));
        assert!(lets_people_in(&AdminRequest::NewKey { label: "Family".into() }));
        assert!(!lets_people_in(&AdminRequest::Decide { peer: "p".into(), approve: false }));
        assert!(!lets_people_in(&AdminRequest::Remove { peer: "p".into(), block: true }));
        assert!(!lets_people_in(&AdminRequest::List));
    }
}
