// SPDX-License-Identifier: AGPL-3.0-only
//! `/sulcus/phone/1`: the person's own relay passes one phone request to the
//! PC per stream and reads the answer back. The request names the PC's link
//! id, so a relay serving several PCs can't get one PC to answer for another.
//! Bodies are already sealed by the phone page; the relay only carries them.
//! The answer is a head frame, body parts and an end frame: a stream the relay
//! cut reads as a normal end, so a missing end frame means failure.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use futures::{AsyncReadExt, AsyncWriteExt, StreamExt};
use libp2p::{PeerId, StreamProtocol};
use serde::{Deserialize, Serialize};

use crate::node::NodeHandle;

pub const PROTOCOL: StreamProtocol = StreamProtocol::new("/sulcus/phone/1");
/// A member PC tells its relay which link id it answers for.
pub const CLAIM: StreamProtocol = StreamProtocol::new("/sulcus/phone-claim/1");
const MAX_LINK: usize = 64;
/// Same ceiling the Cloudflare link puts on one request.
pub const MAX_REQUEST: usize = 40_000_000;
pub const MAX_ANSWER: usize = 64_000_000;
const PART: usize = 512 * 1024;
const MAX_FRAME: usize = PART + 4_096;
const LIMIT: Duration = Duration::from_secs(120);

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
#[serde(rename_all = "snake_case", tag = "t")]
pub enum Request {
    /// One of the page's files.
    Get { link: String, path: String },
    /// A sealed API call from a paired phone.
    Api { link: String, phone: String, body: String },
}

impl Request {
    fn link(&self) -> &str {
        match self {
            Request::Get { link, .. } | Request::Api { link, .. } => link,
        }
    }
}

/// Answer frames start with one of these bytes; parts are raw bytes so a
/// big body isn't blown up by JSON.
const HEAD: u8 = 0;
const PART_TAG: u8 = 1;
const END: u8 = 2;

#[derive(Serialize, Deserialize)]
struct Head {
    status: u16,
    headers: Vec<(String, String)>,
}

fn frame(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(body.len() + 1);
    f.push(tag);
    f.extend_from_slice(body);
    f
}

#[derive(Debug, PartialEq, Clone)]
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

async fn write<S: futures::AsyncWrite + Unpin>(s: &mut S, bytes: &[u8]) -> std::io::Result<()> {
    s.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    s.write_all(bytes).await?;
    s.flush().await
}

async fn read<S: futures::AsyncRead + Unpin>(s: &mut S, cap: usize) -> std::io::Result<Vec<u8>> {
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

/// The PC side. `who` names the relay that may ask and the link it may ask
/// for (none: nobody, so the switch can turn off without a new node);
/// `answer` builds the reply (the same work the Cloudflare link does).
pub fn serve<W, F, Fut>(node: &NodeHandle, who: W, answer: F)
where
    W: Fn() -> Option<(PeerId, String)> + Send + 'static,
    F: Fn(Request) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Answer> + Send + 'static,
{
    let Ok(mut incoming) = node.streams().accept(PROTOCOL) else { return };
    let answer = Arc::new(answer);
    tokio::spawn(async move {
        while let Some((peer, mut s)) = incoming.next().await {
            let Some((_, link)) = who().filter(|(r, _)| *r == peer) else {
                let _ = s.close().await;
                continue;
            };
            let answer = answer.clone();
            tokio::spawn(async move {
                let Ok(Ok(raw)) = tokio::time::timeout(LIMIT, read(&mut s, MAX_REQUEST + 4_096)).await else { return };
                let reply = match serde_json::from_slice::<Request>(&raw) {
                    Ok(req) if req.link() == link => answer(req).await,
                    Ok(_) => Answer { status: 404, headers: Vec::new(), body: Vec::new() },
                    Err(_) => Answer { status: 400, headers: Vec::new(), body: Vec::new() },
                };
                let _ = send(&mut s, &reply).await;
                let _ = s.close().await;
            });
        }
    });
}

async fn send<S: futures::AsyncWrite + Unpin>(s: &mut S, a: &Answer) -> std::io::Result<()> {
    let head = serde_json::to_vec(&Head { status: a.status, headers: a.headers.clone() }).unwrap_or_default();
    write(s, &frame(HEAD, &head)).await?;
    for c in a.body.chunks(PART) {
        write(s, &frame(PART_TAG, c)).await?;
    }
    write(s, &[END]).await
}

/// The relay side: passes one request to `pc` (already connected).
pub async fn ask(node: &NodeHandle, pc: PeerId, req: &Request) -> Result<Answer, String> {
    let raw = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    if raw.len() > MAX_REQUEST {
        return Err("request too large".into());
    }
    let mut s = tokio::time::timeout(LIMIT, node.streams().open_stream(pc, PROTOCOL))
        .await
        .map_err(|_| "the PC didn't answer".to_string())?
        .map_err(|e| format!("couldn't reach the PC: {e}"))?;
    write(&mut s, &raw).await.map_err(|e| e.to_string())?;
    tokio::time::timeout(LIMIT, receive(&mut s)).await.map_err(|_| "the PC didn't answer".to_string())?
}

async fn receive<S: futures::AsyncRead + Unpin>(s: &mut S) -> Result<Answer, String> {
    let mut answer: Option<Answer> = None;
    loop {
        let raw = read(s, MAX_FRAME).await.map_err(|_| "the answer was cut off".to_string())?;
        let (&tag, body) = raw.split_first().ok_or("an empty frame")?;
        match (tag, answer.as_mut()) {
            (HEAD, None) => {
                let h: Head = serde_json::from_slice(body).map_err(|_| "unreadable answer".to_string())?;
                answer = Some(Answer { status: h.status, headers: h.headers, body: Vec::new() });
            }
            (PART_TAG, Some(a)) => {
                if a.body.len() + body.len() > MAX_ANSWER {
                    return Err("answer too large".into());
                }
                a.body.extend_from_slice(body);
            }
            (END, Some(_)) if body.is_empty() => return Ok(answer.take().unwrap()),
            _ => return Err("frames out of order".into()),
        }
    }
}

/// Which PC answers each link id on a relay. The first member to claim an id
/// keeps it, so another member can't take over someone else's phone page.
#[derive(Default)]
pub struct Links(std::collections::HashMap<String, PeerId>);

impl Links {
    pub fn claim(&mut self, link: &str, pc: PeerId) -> bool {
        if link.is_empty() || link.len() > MAX_LINK || !link.chars().all(|c| c.is_ascii_alphanumeric()) {
            return false;
        }
        *self.0.entry(link.to_string()).or_insert(pc) == pc
    }

    pub fn pc(&self, link: &str) -> Option<PeerId> {
        self.0.get(link).copied()
    }
}

/// The relay side of claims; `may_use` says who is a member.
pub fn serve_claims(node: &NodeHandle, links: Arc<std::sync::Mutex<Links>>, may_use: impl Fn(&PeerId) -> bool + Send + 'static) {
    let Ok(mut incoming) = node.streams().accept(CLAIM) else { return };
    tokio::spawn(async move {
        while let Some((peer, mut s)) = incoming.next().await {
            let Ok(Ok(raw)) = tokio::time::timeout(Duration::from_secs(30), read(&mut s, MAX_LINK)).await else { continue };
            let link = String::from_utf8(raw).unwrap_or_default();
            let ok = may_use(&peer) && links.lock().unwrap().claim(&link, peer);
            let _ = write(&mut s, &[u8::from(ok)]).await;
            let _ = s.close().await;
        }
    });
}

/// The PC side: claims `link` on `relay` (already connected).
pub async fn claim(node: &NodeHandle, relay: PeerId, link: &str) -> Result<(), String> {
    let mut s = tokio::time::timeout(Duration::from_secs(30), node.streams().open_stream(relay, CLAIM))
        .await
        .map_err(|_| "the relay didn't answer".to_string())?
        .map_err(|e| format!("couldn't reach the relay: {e}"))?;
    write(&mut s, link.as_bytes()).await.map_err(|e| e.to_string())?;
    let r = tokio::time::timeout(Duration::from_secs(30), read(&mut s, 1)).await.map_err(|_| "the relay didn't answer".to_string())?.map_err(|e| e.to_string())?;
    if r == [1] {
        Ok(())
    } else {
        Err("the relay refused: this PC isn't a member, or another PC holds that link".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_stays_with_its_first_pc() {
        let (a, b) = (PeerId::random(), PeerId::random());
        let mut l = Links::default();
        assert!(l.claim("ab12", a));
        assert!(l.claim("ab12", a), "again is fine");
        assert!(!l.claim("ab12", b));
        assert_eq!(l.pc("ab12"), Some(a));
        assert!(!l.claim("../x", a) && !l.claim("", a) && !l.claim(&"a".repeat(65), a));
    }

    #[tokio::test]
    async fn a_cut_answer_is_a_failure() {
        let a = Answer { status: 200, headers: vec![("content-type".into(), "x".into())], body: vec![9u8; PART * 2 + 5] };
        let mut buf = Vec::new();
        send(&mut buf, &a).await.unwrap();
        assert_eq!(receive(&mut futures::io::Cursor::new(buf.clone())).await.unwrap(), a);
        let cut = buf[..buf.len() - 5].to_vec();
        assert!(receive(&mut futures::io::Cursor::new(cut)).await.is_err());
        let mut bad = Vec::new();
        write(&mut bad, &frame(PART_TAG, &[1])).await.unwrap();
        assert!(receive(&mut futures::io::Cursor::new(bad)).await.is_err(), "a part before the head");
        let mut huge = u32::MAX.to_be_bytes().to_vec();
        huge.extend_from_slice(b"xx");
        assert!(receive(&mut futures::io::Cursor::new(huge)).await.is_err());
        assert!(receive(&mut futures::io::Cursor::new(b"garbage!".to_vec())).await.is_err());
    }
}
