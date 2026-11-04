// SPDX-License-Identifier: AGPL-3.0-only
//! Strangers pay before being helped (`/sulcus/admit/1`): the helper sends a
//! fresh challenge and the difficulty; the stranger answers with a nonce
//! whose stamp (`limits::stamp_ok`) has that many leading zero bits. Only
//! used when the helper turned on "Help strangers"; friends, group members
//! and relay members never pay.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{AsyncWriteExt, StreamExt};
use libp2p::{PeerId, StreamProtocol};

use crate::limits;
use crate::mailbox::{read, write};
use crate::node::NodeHandle;

pub const PROTOCOL: StreamProtocol = StreamProtocol::new("/sulcus/admit/1");
const LIMIT: Duration = Duration::from_secs(30);
/// A paid stamp is good this long.
pub const GOOD_FOR: Duration = Duration::from_secs(3600);

/// Strangers who paid, and until when.
pub type Paid = Arc<Mutex<HashMap<PeerId, std::time::Instant>>>;

pub fn paid(p: &Paid, peer: &PeerId) -> bool {
    p.lock().unwrap().get(peer).is_some_and(|t| t.elapsed() < GOOD_FOR)
}

/// The helper's side. `bits` gives the difficulty for the current load.
pub fn serve(node: &NodeHandle, paid_list: Paid, bits: impl Fn() -> u32 + Send + 'static) {
    let Ok(mut incoming) = node.streams().accept(PROTOCOL) else { return };
    let me = node.peer_id().to_string();
    tokio::spawn(async move {
        while let Some((peer, mut s)) = incoming.next().await {
            let b = bits();
            let mut challenge = [0u8; 16];
            getrandom::getrandom(&mut challenge).expect("the OS random source works");
            let mut out = challenge.to_vec();
            out.push(b as u8);
            if write(&mut s, &out).await.is_err() {
                continue;
            }
            let Ok(Ok(raw)) = tokio::time::timeout(LIMIT, read(&mut s, 8)).await else { continue };
            let Ok(n) = raw.as_slice().try_into().map(u64::from_be_bytes) else { continue };
            let ok = limits::stamp_ok(&me, &peer.to_string(), &challenge, n, b);
            if ok {
                paid_list.lock().unwrap().insert(peer, std::time::Instant::now());
            }
            let _ = write(&mut s, &[ok as u8]).await;
            let _ = s.close().await;
        }
    });
}

/// The stranger's side: fetch a challenge, work out the stamp, send it.
pub async fn pay(node: &NodeHandle, helper: PeerId) -> Result<bool, String> {
    let mut s = tokio::time::timeout(LIMIT, node.streams().open_stream(helper, PROTOCOL)).await.map_err(|_| "no answer".to_string())?.map_err(|e| e.to_string())?;
    let raw = tokio::time::timeout(LIMIT, read(&mut s, 32)).await.map_err(|_| "no answer".to_string())?.map_err(|e| e.to_string())?;
    if raw.len() != 17 || raw[16] > 28 {
        return Err("a challenge that can't be paid".into());
    }
    let (challenge, bits) = (raw[..16].to_vec(), raw[16] as u32);
    let (h, me) = (helper.to_string(), node.peer_id().to_string());
    let n = tokio::task::spawn_blocking(move || limits::make_stamp(&h, &me, &challenge, bits)).await.map_err(|e| e.to_string())?;
    write(&mut s, &n.to_be_bytes()).await.map_err(|e| e.to_string())?;
    let ok = tokio::time::timeout(LIMIT, read(&mut s, 1)).await.map_err(|_| "no answer".to_string())?.map_err(|e| e.to_string())?;
    Ok(ok == [1])
}
