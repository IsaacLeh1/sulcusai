// SPDX-License-Identifier: AGPL-3.0-only
//! `/sulcus/admin/1`: the relay's owner, from their own PC, sees who's
//! waiting and decides. Only PCs on the relay's owner list are answered;
//! anyone else gets a refusal and nothing else. One request and one reply
//! per stream, length-prefixed JSON, capped.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{AsyncReadExt, AsyncWriteExt, StreamExt};
use libp2p::{PeerId, StreamProtocol};
use serde::{Deserialize, Serialize};

use crate::join::{Admissions, Pending};
use crate::node::NodeHandle;
use crate::signed;

pub const PROTOCOL: StreamProtocol = StreamProtocol::new("/sulcus/admin/1");
const MAX_FRAME: usize = 16_384;
const LIMIT: Duration = Duration::from_secs(30);

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Request {
    List,
    Decide { peer: String, approve: bool },
    Remove { peer: String, block: bool },
    NewKey { label: String },
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
#[serde(tag = "is", rename_all = "snake_case")]
pub enum Reply {
    State { pending: Vec<Pending>, members: Vec<String>, keys: Vec<(String, String, bool)> },
    Done,
    Key { id: String, secret: String },
    NotOwner,
    Unknown,
}

async fn write<S: futures::AsyncWrite + Unpin>(s: &mut S, bytes: &[u8]) -> std::io::Result<()> {
    if bytes.len() > MAX_FRAME {
        return Err(std::io::Error::other("frame too large"));
    }
    s.write_all(&(bytes.len() as u16).to_be_bytes()).await?;
    s.write_all(bytes).await?;
    s.flush().await
}

async fn read<S: futures::AsyncRead + Unpin>(s: &mut S) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 2];
    s.read_exact(&mut len).await?;
    let len = u16::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::other("frame too large"));
    }
    let mut b = vec![0u8; len];
    s.read_exact(&mut b).await?;
    Ok(b)
}

/// What the relay does with one owner request.
pub fn handle(adm: &mut Admissions, req: Request) -> Reply {
    match req {
        Request::List => Reply::State {
            pending: adm.pending.clone(),
            members: adm.members.clone(),
            keys: adm.keys.iter().map(|k| (k.id.clone(), k.label.clone(), k.disabled)).collect(),
        },
        Request::Decide { peer, approve } => {
            if adm.decide(&peer, approve) {
                Reply::Done
            } else {
                Reply::Unknown
            }
        }
        Request::Remove { peer, block } => {
            adm.remove(&peer, block);
            Reply::Done
        }
        Request::NewKey { label } => {
            let (id, secret) = adm.rotate(label.trim(), signed::now());
            Reply::Key { id, secret }
        }
    }
}

/// Serves owners on `node`; `owners` are the PeerIds allowed to manage it.
pub fn serve(node: &NodeHandle, admissions: Arc<Mutex<Admissions>>, owners: Arc<Mutex<Vec<String>>>, changed: impl Fn() + Send + 'static) {
    let Ok(mut incoming) = node.streams().accept(PROTOCOL) else { return };
    tokio::spawn(async move {
        while let Some((peer, mut s)) = incoming.next().await {
            let Ok(Ok(raw)) = tokio::time::timeout(LIMIT, read(&mut s)).await else { continue };
            let reply = if !owners.lock().unwrap().contains(&peer.to_string()) {
                Reply::NotOwner
            } else {
                match serde_json::from_slice::<Request>(&raw) {
                    Ok(req) => {
                        let r = handle(&mut admissions.lock().unwrap(), req);
                        changed();
                        r
                    }
                    Err(_) => Reply::Unknown,
                }
            };
            let _ = write(&mut s, &serde_json::to_vec(&reply).unwrap_or_default()).await;
            let _ = s.close().await;
        }
    });
}

/// The owner's side: one request to `relay` (already connected).
pub async fn ask(node: &NodeHandle, relay: PeerId, req: &Request) -> Result<Reply, String> {
    let mut s = tokio::time::timeout(LIMIT, node.streams().open_stream(relay, PROTOCOL))
        .await
        .map_err(|_| "The relay didn't answer.".to_string())?
        .map_err(|e| format!("Couldn't reach the relay: {e}"))?;
    write(&mut s, &serde_json::to_vec(req).map_err(|e| e.to_string())?).await.map_err(|e| e.to_string())?;
    let raw = tokio::time::timeout(LIMIT, read(&mut s)).await.map_err(|_| "The relay didn't answer.".to_string())?.map_err(|e| e.to_string())?;
    serde_json::from_slice(&raw).map_err(|_| "The relay's answer couldn't be read.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_requests_change_who_may_join() {
        let mut a = Admissions::default();
        let Reply::Key { id, .. } = handle(&mut a, Request::NewKey { label: "Family".into() }) else { panic!() };
        a.pending.push(Pending { peer: "ana".into(), name: "Ana".into(), kind: "pc".into(), key_label: "Family".into(), at: 1 });
        let Reply::State { pending, keys, .. } = handle(&mut a, Request::List) else { panic!() };
        assert_eq!((pending.len(), keys[0].0.clone()), (1, id));
        assert_eq!(handle(&mut a, Request::Decide { peer: "ana".into(), approve: true }), Reply::Done);
        assert!(a.is_member("ana"));
        assert_eq!(handle(&mut a, Request::Decide { peer: "nobody".into(), approve: true }), Reply::Unknown);
        assert_eq!(handle(&mut a, Request::Remove { peer: "ana".into(), block: true }), Reply::Done);
        assert!(!a.is_member("ana") && a.blocked.contains(&"ana".to_string()));
        let parsed: Request = serde_json::from_str(r#"{"do":"decide","peer":"x","approve":false}"#).unwrap();
        assert_eq!(parsed, Request::Decide { peer: "x".into(), approve: false });
    }
}
