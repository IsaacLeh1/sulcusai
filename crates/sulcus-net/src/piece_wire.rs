// SPDX-License-Identifier: AGPL-3.0-only
//! Pieces of updates and engines from peers (`/sulcus/piece/1`): ask for
//! piece `n` of the file with a given SHA-256; the sharer answers with the
//! bytes or nothing. The asker checks each piece with `manifest::piece_ok`
//! and moves to another peer on the first bad one.

use std::time::Duration;

use futures::{AsyncWriteExt, StreamExt};
use libp2p::{PeerId, StreamProtocol};
use serde::{Deserialize, Serialize};

use crate::mailbox::{read, write};
use crate::manifest::{self, File, PIECE};
use crate::node::NodeHandle;

pub const PROTOCOL: StreamProtocol = StreamProtocol::new("/sulcus/piece/1");
const LIMIT: Duration = Duration::from_secs(60);

#[derive(Debug, Serialize, Deserialize)]
struct Ask {
    sha256: String,
    n: u64,
}

/// The sharer. `piece` reads piece `n` of a file it shares (by SHA-256),
/// only for files in a verified manifest.
pub fn serve(node: &NodeHandle, piece: impl Fn(&str, u64) -> Option<Vec<u8>> + Send + Sync + 'static) {
    let Ok(mut incoming) = node.streams().accept(PROTOCOL) else { return };
    let piece = std::sync::Arc::new(piece);
    tokio::spawn(async move {
        while let Some((_, mut s)) = incoming.next().await {
            let piece = piece.clone();
            tokio::spawn(async move {
                let Ok(Ok(raw)) = tokio::time::timeout(LIMIT, read(&mut s, 256)).await else { return };
                let answer = serde_json::from_slice::<Ask>(&raw).ok().and_then(|a| piece(&a.sha256, a.n)).filter(|b| b.len() <= PIECE).unwrap_or_default();
                let _ = write(&mut s, &answer).await;
                let _ = s.close().await;
            });
        }
    });
}

/// Fetches a whole file from one peer, checking every piece.
pub async fn fetch(node: &NodeHandle, peer: PeerId, f: &File) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(f.size as usize);
    for n in 0..f.pieces.len() {
        let mut s = tokio::time::timeout(LIMIT, node.streams().open_stream(peer, PROTOCOL)).await.map_err(|_| "no answer".to_string())?.map_err(|e| e.to_string())?;
        let ask = serde_json::to_vec(&Ask { sha256: f.sha256.clone(), n: n as u64 }).map_err(|e| e.to_string())?;
        write(&mut s, &ask).await.map_err(|e| e.to_string())?;
        let b = tokio::time::timeout(LIMIT, read(&mut s, PIECE)).await.map_err(|_| "no answer".to_string())?.map_err(|e| e.to_string())?;
        if !manifest::piece_ok(f, n, &b) {
            return Err(format!("piece {n} from {peer} doesn't match the signed manifest"));
        }
        out.extend_from_slice(&b);
    }
    if !manifest::file_ok(f, &out) {
        return Err("the file doesn't match the signed manifest".into());
    }
    Ok(out)
}
