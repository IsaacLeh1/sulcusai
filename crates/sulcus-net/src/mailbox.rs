// SPDX-License-Identifier: AGPL-3.0-only
//! Mailboxes on a relay (design 9.1): letters wait for a PC that's offline.
//! The relay only holds bytes the sender sealed for the recipient; it sees
//! who to and how big, not what. Only the recipient (by the PeerId of its
//! connection) can take its letters. Capped per recipient and in age.
//!
//! `/sulcus/mail/1`: one request and one reply per stream.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{AsyncReadExt, AsyncWriteExt, StreamExt};
use libp2p::{PeerId, StreamProtocol};
use serde::{Deserialize, Serialize};

use crate::node::NodeHandle;
use crate::signed;

pub const PROTOCOL: StreamProtocol = StreamProtocol::new("/sulcus/mail/1");
pub const MAX_LETTER: usize = 256 * 1024;
pub const BOX_BYTES: usize = 200 * 1024 * 1024;
pub const BOX_LETTERS: usize = 5_000;
pub const KEEP_SECS: u64 = 30 * 86_400;
const MAX_FRAME: usize = MAX_LETTER + 1_024;
const LIMIT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Letter {
    pub from: String,
    pub id: String,
    /// Sealed by the sender for the recipient; opaque here.
    pub body: Vec<u8>,
    pub at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Store {
    pub boxes: HashMap<String, Vec<Letter>>,
}

impl Store {
    /// Leaves a letter; refused when too big or the box is full.
    pub fn put(&mut self, to: &str, letter: Letter, now: u64) -> Result<(), &'static str> {
        if letter.body.len() > MAX_LETTER {
            return Err("the letter is too big");
        }
        let b = self.boxes.entry(to.to_string()).or_default();
        b.retain(|l| now.saturating_sub(l.at) <= KEEP_SECS);
        if b.iter().any(|l| l.id == letter.id && l.from == letter.from) {
            return Ok(());
        }
        let used: usize = b.iter().map(|l| l.body.len()).sum();
        if b.len() >= BOX_LETTERS || used + letter.body.len() > BOX_BYTES {
            return Err("the recipient's mailbox is full");
        }
        b.push(letter);
        Ok(())
    }

    /// Everything waiting for `who`, removed from the box.
    pub fn take(&mut self, who: &str, now: u64) -> Vec<Letter> {
        let mut letters = self.boxes.remove(who).unwrap_or_default();
        letters.retain(|l| now.saturating_sub(l.at) <= KEEP_SECS);
        letters
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Request {
    Put { to: String, id: String, body: Vec<u8> },
    Take,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "is", rename_all = "snake_case")]
pub enum Reply {
    Kept,
    Refused { why: String },
    Letters { letters: Vec<Letter> },
}

pub(crate) async fn write<S: futures::AsyncWrite + Unpin>(s: &mut S, bytes: &[u8]) -> std::io::Result<()> {
    if bytes.len() > MAX_FRAME * 64 {
        return Err(std::io::Error::other("frame too large"));
    }
    s.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    s.write_all(bytes).await?;
    s.flush().await
}

pub(crate) async fn read<S: futures::AsyncRead + Unpin>(s: &mut S, cap: usize) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    if len > cap {
        return Err(std::io::Error::other("frame too large"));
    }
    let mut b = vec![0u8; len];
    s.read_exact(&mut b).await?;
    Ok(b)
}

/// The relay side. `may_use` says whether a peer may leave letters (members);
/// `changed` runs after each change so the caller can save.
pub fn serve(node: &NodeHandle, store: Arc<Mutex<Store>>, may_use: impl Fn(&PeerId) -> bool + Send + 'static, changed: impl Fn() + Send + 'static) {
    let Ok(mut incoming) = node.streams().accept(PROTOCOL) else { return };
    tokio::spawn(async move {
        while let Some((peer, mut s)) = incoming.next().await {
            let Ok(Ok(raw)) = tokio::time::timeout(LIMIT, read(&mut s, MAX_FRAME * 4)).await else { continue };
            let reply = match serde_json::from_slice::<Request>(&raw) {
                _ if !may_use(&peer) => Reply::Refused { why: "this relay's mailboxes are for its members".into() },
                Ok(Request::Put { to, id, body }) => match store.lock().unwrap().put(&to, Letter { from: peer.to_string(), id, body, at: signed::now() }, signed::now()) {
                    Ok(()) => Reply::Kept,
                    Err(why) => Reply::Refused { why: why.into() },
                },
                Ok(Request::Take) => Reply::Letters { letters: store.lock().unwrap().take(&peer.to_string(), signed::now()) },
                Err(_) => Reply::Refused { why: "unreadable request".into() },
            };
            let _ = write(&mut s, &serde_json::to_vec(&reply).unwrap_or_default()).await;
            let _ = s.close().await;
            changed();
        }
    });
}

pub async fn ask(node: &NodeHandle, relay: PeerId, req: &Request) -> Result<Reply, String> {
    let mut s = tokio::time::timeout(LIMIT, node.streams().open_stream(relay, PROTOCOL))
        .await
        .map_err(|_| "The relay didn't answer.".to_string())?
        .map_err(|e| format!("Couldn't reach the relay: {e}"))?;
    write(&mut s, &serde_json::to_vec(req).map_err(|e| e.to_string())?).await.map_err(|e| e.to_string())?;
    let raw = tokio::time::timeout(LIMIT, read(&mut s, BOX_BYTES * 4)).await.map_err(|_| "The relay didn't answer.".to_string())?.map_err(|e| e.to_string())?;
    serde_json::from_slice(&raw).map_err(|_| "The relay's answer couldn't be read.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn letter(from: &str, id: &str, size: usize, at: u64) -> Letter {
        Letter { from: from.into(), id: id.into(), body: vec![7; size], at }
    }

    #[test]
    fn letters_wait_and_only_their_recipient_takes_them() {
        let mut s = Store::default();
        s.put("bo", letter("ana", "1", 10, 100), 100).unwrap();
        s.put("bo", letter("ana", "1", 10, 100), 100).unwrap();
        assert_eq!(s.boxes["bo"].len(), 1, "the same letter twice is kept once");
        assert!(s.put("bo", letter("ana", "2", MAX_LETTER + 1, 100), 100).is_err());
        assert!(s.take("eve", 100).is_empty());
        assert_eq!(s.take("bo", 100).len(), 1);
        assert!(s.take("bo", 100).is_empty(), "taken means gone");
        s.put("cy", letter("ana", "3", 10, 0), 0).unwrap();
        assert!(s.take("cy", KEEP_SECS + 1).is_empty(), "old letters expire");
    }
}
