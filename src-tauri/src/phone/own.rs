// SPDX-License-Identifier: AGPL-3.0-only
//! The phone through the person's own relay (sulcus-relay) instead of
//! SulcusAI's on Cloudflare. The relay serves the page over its own HTTPS
//! name and passes each request here over `/sulcus/phone/1`; the answers are
//! the same ones the Cloudflare link gives, still sealed with the phone's key.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use sulcus_net::libp2p::multiaddr::Protocol;
use sulcus_net::libp2p::{Multiaddr, PeerId};
use sulcus_net::phone_wire::{self, Answer, Request};
use sulcus_net::NodeHandle;

use crate::AppState;

/// The relay that may ask, and for which link (none: the switch is off).
static OWN: Mutex<Option<(PeerId, String)>> = Mutex::new(None);

/// The relay's address, to reach it and claim the link.
static ADDRESS: Mutex<String> = Mutex::new(String::new());

pub fn set(who: Option<(PeerId, String)>, address: &str) {
    *ADDRESS.lock().unwrap() = address.trim().to_string();
    *OWN.lock().unwrap() = who;
    claim_soon();
}

/// Tells the relay this PC answers for the link, once Network is on and the
/// relay is reached; the relay sends nothing here before that.
pub fn claim_soon() {
    let (Some((relay, link)), Some(node)) = (OWN.lock().unwrap().clone(), crate::p2p::running()) else { return };
    let Ok(addr) = ADDRESS.lock().unwrap().parse::<Multiaddr>() else { return };
    tauri::async_runtime::spawn(async move {
        if !node.connected().contains(&relay) {
            let mut events = node.subscribe();
            if node.dial(addr).await.is_err() {
                return;
            }
            let reached = tokio::time::timeout(std::time::Duration::from_secs(30), async {
                loop {
                    match events.recv().await {
                        Ok(sulcus_net::NodeEvent::Identified { peer, .. }) if peer == relay => return true,
                        Err(_) => return false,
                        _ => {}
                    }
                }
            })
            .await;
            if reached != Ok(true) {
                crate::applog::line("phone", "Your own relay didn't answer; the phone can't reach this PC through it yet");
                return;
            }
        }
        if let Err(e) = phone_wire::claim(&node, relay, &link).await {
            crate::applog::line("phone", &format!("Your own relay didn't take the phone link: {e}"));
        }
    });
}

/// The relay's id from its address, which must end in `/p2p/<id>`.
pub fn relay_id(address: &str) -> Result<PeerId, String> {
    let addr: Multiaddr = address.trim().parse().map_err(|_| "That isn't a relay address.".to_string())?;
    match addr.iter().last() {
        Some(Protocol::P2p(id)) => Ok(id),
        _ => Err("The relay's address needs its id at the end (/p2p/...).".into()),
    }
}

/// Where the phone opens the page: the relay's own HTTPS name.
pub fn page_base(page: &str) -> Result<String, String> {
    let url = tauri::Url::parse(page.trim()).map_err(|_| "The relay's web address isn't valid.".to_string())?;
    if url.scheme() != "https" || url.host_str().is_none() {
        return Err("The relay's web address must start with https:// (phones need a valid certificate).".into());
    }
    Ok(page.trim().trim_end_matches('/').to_string())
}

fn headers_of(v: &Value) -> Vec<(String, String)> {
    v.as_object().map_or(Vec::new(), |m| m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string())).collect())
}

/// Called for each new node; answers only while the switch names a relay.
pub fn serve(state: &Arc<AppState>, node: &NodeHandle) {
    let state = state.clone();
    phone_wire::serve(node, || OWN.lock().unwrap().clone(), move |req| {
        let state = state.clone();
        async move {
            match req {
                Request::Get { path, .. } => {
                    let (status, headers, body) = super::serve_file(&state, &path);
                    Answer { status, headers: headers_of(&headers), body }
                }
                Request::Api { phone, body, .. } => {
                    let (status, text) = super::answer(&state, Some(&phone), &body).await;
                    let headers = vec![("content-type".into(), "text/plain; charset=utf-8".into()), ("cache-control".into(), "no-store".into())];
                    Answer { status: status.as_u16(), headers, body: text.into_bytes() }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_own_relay_needs_its_id_and_https() {
        assert!(relay_id("/ip4/1.2.3.4/tcp/4001").is_err());
        assert!(relay_id("/ip4/1.2.3.4/tcp/4001/p2p/12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN").is_ok());
        assert!(page_base("http://relay.example").is_err());
        assert!(page_base("nonsense").is_err());
        assert_eq!(page_base(" https://relay.example/ ").unwrap(), "https://relay.example");
        assert_eq!(headers_of(&serde_json::json!({ "content-type": "a" })), vec![("content-type".to_string(), "a".to_string())]);
    }
}
