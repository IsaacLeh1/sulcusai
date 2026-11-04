// SPDX-License-Identifier: AGPL-3.0-only
//! `/sulcus/join/1`: the relay sends a fresh challenge, the newcomer answers
//! with its key id, proof, name and device kind, and the relay replies with
//! where the request stands. Small length-prefixed JSON frames, capped.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{AsyncReadExt, AsyncWriteExt, StreamExt};
use libp2p::{PeerId, StreamProtocol};
use serde::{Deserialize, Serialize};

use crate::join::{self, Admissions, Outcome};
use crate::node::NodeHandle;
use crate::signed;

pub const PROTOCOL: StreamProtocol = StreamProtocol::new("/sulcus/join/1");
const MAX_FRAME: usize = 2_048;
const LIMIT: Duration = Duration::from_secs(30);

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Request {
    pub key_id: String,
    pub answer: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
#[serde(rename_all = "snake_case")]
pub enum Reply {
    Waiting,
    Member,
    Refused(String),
    Wait(u64),
}

async fn write<S: futures::AsyncWrite + Unpin>(s: &mut S, bytes: &[u8]) -> std::io::Result<()> {
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

/// The relay side: answers join requests on `node` with `admissions`;
/// `changed` runs after each request so the caller can save and tell the owner.
pub fn serve(node: &NodeHandle, admissions: Arc<Mutex<Admissions>>, changed: impl Fn() + Send + 'static) {
    let Ok(mut incoming) = node.streams().accept(PROTOCOL) else { return };
    let me = node.peer_id().to_string();
    tokio::spawn(async move {
        while let Some((peer, mut s)) = incoming.next().await {
            let c = join::challenge();
            if write(&mut s, &c).await.is_err() {
                continue;
            }
            let Ok(Ok(raw)) = tokio::time::timeout(LIMIT, read(&mut s)).await else { continue };
            let Ok(req) = serde_json::from_slice::<Request>(&raw) else { continue };
            let answer = hex::decode(&req.answer).unwrap_or_default();
            let outcome = admissions.lock().unwrap().request(&req.key_id, &answer, &c, &me, &peer.to_string(), &req.name, &req.kind, signed::now());
            let reply = match outcome {
                Outcome::Waiting => Reply::Waiting,
                Outcome::AlreadyMember => Reply::Member,
                Outcome::Refused(why) => Reply::Refused(why.into()),
                Outcome::Wait(secs) => Reply::Wait(secs),
            };
            let _ = write(&mut s, &serde_json::to_vec(&reply).unwrap_or_default()).await;
            let _ = s.close().await;
            changed();
        }
    });
}

/// The newcomer's side: asks `relay` (already connected) to let it in.
pub async fn ask(node: &NodeHandle, relay: PeerId, key_id: &str, secret_hex: &str, name: &str, kind: &str) -> Result<Reply, String> {
    let secret = hex::decode(secret_hex.trim()).map_err(|_| "The invite key is malformed.".to_string())?;
    let mut s = tokio::time::timeout(LIMIT, node.streams().open_stream(relay, PROTOCOL))
        .await
        .map_err(|_| "The relay didn't answer.".to_string())?
        .map_err(|e| format!("Couldn't reach the relay: {e}"))?;
    let raw = tokio::time::timeout(LIMIT, read(&mut s)).await.map_err(|_| "The relay didn't answer.".to_string())?.map_err(|e| e.to_string())?;
    let c: [u8; 32] = raw.try_into().map_err(|_| "The relay sent something unexpected.".to_string())?;
    let answer = join::proof(&secret, &relay.to_string(), &node.peer_id().to_string(), &c);
    let req = Request { key_id: key_id.into(), answer: hex::encode(answer), name: name.chars().take(60).collect(), kind: kind.into() };
    write(&mut s, &serde_json::to_vec(&req).map_err(|e| e.to_string())?).await.map_err(|e| e.to_string())?;
    let raw = tokio::time::timeout(LIMIT, read(&mut s)).await.map_err(|_| "The relay didn't answer.".to_string())?.map_err(|e| e.to_string())?;
    serde_json::from_slice(&raw).map_err(|_| "The relay's answer couldn't be read.".to_string())
}
