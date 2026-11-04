// SPDX-License-Identifier: AGPL-3.0-only
//! Using an invite: the newcomer dials the inviter at the invite's
//! addresses and presents its secret over `/sulcus/invite/1`; the inviter
//! checks it against its book (hashed, uses left, expiry) and both learn
//! each other's name and signed address record. One small frame each way,
//! length-prefixed and capped.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{AsyncReadExt, AsyncWriteExt, StreamExt};
use libp2p::{PeerId, StreamProtocol};
use serde::{Deserialize, Serialize};

use crate::invite::{Book, Checked, Kind};
use crate::node::{NodeEvent, NodeHandle};
use crate::records::AddressRecord;
use crate::signed;

pub const PROTOCOL: StreamProtocol = StreamProtocol::new("/sulcus/invite/1");
const MAX_FRAME: usize = 8_192;
const LIMIT: Duration = Duration::from_secs(30);

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Hello {
    pub secret: String,
    pub name: String,
    /// The newcomer's signed address record, text form.
    pub record: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Answer {
    pub ok: bool,
    pub name: String,
    pub record: Option<String>,
}

async fn write_frame<S: futures::AsyncWrite + Unpin, T: Serialize>(s: &mut S, v: &T) -> std::io::Result<()> {
    let body = serde_json::to_vec(v).map_err(std::io::Error::other)?;
    if body.len() > MAX_FRAME {
        return Err(std::io::Error::other("frame too large"));
    }
    s.write_all(&(body.len() as u16).to_be_bytes()).await?;
    s.write_all(&body).await?;
    s.flush().await
}

async fn read_frame<S: futures::AsyncRead + Unpin, T: for<'de> Deserialize<'de>>(s: &mut S) -> std::io::Result<T> {
    let mut len = [0u8; 2];
    s.read_exact(&mut len).await?;
    let len = u16::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::other("frame too large"));
    }
    let mut body = vec![0u8; len];
    s.read_exact(&mut body).await?;
    parse_frame(&body)
}

/// A frame's body, read strictly (the part fuzzed).
pub fn parse_frame<T: for<'de> Deserialize<'de>>(body: &[u8]) -> std::io::Result<T> {
    if body.len() > MAX_FRAME {
        return Err(std::io::Error::other("frame too large"));
    }
    serde_json::from_slice(body).map_err(std::io::Error::other)
}

/// Someone who used one of this PC's invites.
#[derive(Debug, Clone, PartialEq)]
pub struct Joined {
    pub peer: PeerId,
    pub name: String,
    pub kind: Kind,
    pub record: Option<AddressRecord>,
}

/// Answers invites on this node until it stops; each newcomer whose secret
/// redeems is passed to `joined`.
pub fn answer_invites(node: &NodeHandle, book: Arc<Mutex<Book>>, my_name: String, my_record: Option<String>, joined: impl Fn(Joined) + Send + 'static) {
    let Ok(mut incoming) = node.streams().accept(PROTOCOL) else { return };
    tokio::spawn(async move {
        while let Some((peer, mut s)) = incoming.next().await {
            let hello: Hello = match tokio::time::timeout(LIMIT, read_frame(&mut s)).await {
                Ok(Ok(h)) => h,
                _ => continue,
            };
            let kind = book.lock().unwrap().redeem(&hello.secret, signed::now());
            let answer = Answer { ok: kind.is_some(), name: if kind.is_some() { my_name.clone() } else { String::new() }, record: kind.and(my_record.clone()) };
            let _ = write_frame(&mut s, &answer).await;
            let _ = s.close().await;
            if let Some(kind) = kind {
                let record = hello.record.as_deref().and_then(|r| AddressRecord::open(r).ok()).filter(|r| r.peer == peer.to_string());
                joined(Joined { peer, name: hello.name.chars().take(60).collect(), kind, record });
            }
        }
    });
}

/// Uses an invite: dials the inviter and presents the secret. Returns the
/// inviter's name and record on success.
pub async fn use_invite(node: &NodeHandle, invite: &Checked, my_name: &str, my_record: Option<String>) -> Result<(String, Option<AddressRecord>), String> {
    let mut events = node.subscribe();
    if !node.connected().contains(&invite.from) {
        let mut started = false;
        for a in &invite.addrs {
            let target = a.clone().with_p2p(invite.from).unwrap_or_else(|a| a);
            if node.dial(target).await.is_ok() {
                started = true;
            }
        }
        if !started {
            return Err("None of the invite's addresses could be tried.".into());
        }
        let peer = invite.from;
        tokio::time::timeout(LIMIT, async {
            while !node.connected().contains(&peer) {
                if let Ok(NodeEvent::Connected { peer: p, .. }) = events.recv().await {
                    if p == peer {
                        return;
                    }
                }
            }
        })
        .await
        .map_err(|_| "The inviter's PC didn't answer. It may be off, or on a network that needs a relay.".to_string())?;
    }
    let mut s = tokio::time::timeout(LIMIT, node.streams().open_stream(invite.from, PROTOCOL))
        .await
        .map_err(|_| "The inviter's PC didn't answer.".to_string())?
        .map_err(|e| format!("Couldn't reach the inviter's PC: {e}"))?;
    write_frame(&mut s, &Hello { secret: invite.invite.secret.clone(), name: my_name.chars().take(60).collect(), record: my_record }).await.map_err(|e| e.to_string())?;
    let answer: Answer = tokio::time::timeout(LIMIT, read_frame(&mut s)).await.map_err(|_| "No answer from the inviter's PC.".to_string())?.map_err(|e| e.to_string())?;
    if !answer.ok {
        return Err("The inviter's PC didn't accept this invite (used up, expired or withdrawn).".into());
    }
    let record = answer.record.as_deref().and_then(|r| AddressRecord::open(r).ok()).filter(|r| r.peer == invite.from.to_string());
    Ok((answer.name.chars().take(60).collect(), record))
}
