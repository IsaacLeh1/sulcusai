// SPDX-License-Identifier: AGPL-3.0-only
//! Security notices passed between peers and relays with no server
//! (`/sulcus/notice/1`): the asker sends the sequence number it has; the
//! other side answers with its notice exactly as received when it has a
//! newer one, or nothing. The asker checks it with `security_notice::accept`
//! before using or passing it on, so a peer can't make anyone take a notice
//! that doesn't verify.

use std::time::Duration;

use futures::{AsyncWriteExt, StreamExt};
use libp2p::{PeerId, StreamProtocol};

use crate::mailbox::{read, write};
use crate::node::NodeHandle;
use crate::security_notice::MAX_NOTICE_BYTES;

pub const PROTOCOL: StreamProtocol = StreamProtocol::new("/sulcus/notice/1");
const LIMIT: Duration = Duration::from_secs(30);

/// Answers askers. `current` gives this side's sequence and raw notice.
pub fn serve(node: &NodeHandle, current: impl Fn() -> (u64, Option<String>) + Send + 'static) {
    let Ok(mut incoming) = node.streams().accept(PROTOCOL) else { return };
    tokio::spawn(async move {
        while let Some((_, mut s)) = incoming.next().await {
            let Ok(Ok(raw)) = tokio::time::timeout(LIMIT, read(&mut s, 16)).await else { continue };
            let theirs = raw.as_slice().try_into().map(u64::from_be_bytes).unwrap_or(u64::MAX);
            let (mine, notice) = current();
            let answer = if mine > theirs { notice.unwrap_or_default().into_bytes() } else { Vec::new() };
            let _ = write(&mut s, &answer).await;
            let _ = s.close().await;
        }
    });
}

/// Asks a connected peer for a notice newer than `have`. The bytes still
/// need `security_notice::accept`.
pub async fn fetch(node: &NodeHandle, peer: PeerId, have: u64) -> Result<Option<Vec<u8>>, String> {
    let mut s = tokio::time::timeout(LIMIT, node.streams().open_stream(peer, PROTOCOL)).await.map_err(|_| "no answer".to_string())?.map_err(|e| e.to_string())?;
    write(&mut s, &have.to_be_bytes()).await.map_err(|e| e.to_string())?;
    let raw = tokio::time::timeout(LIMIT, read(&mut s, MAX_NOTICE_BYTES)).await.map_err(|_| "no answer".to_string())?.map_err(|e| e.to_string())?;
    Ok((!raw.is_empty()).then_some(raw))
}
