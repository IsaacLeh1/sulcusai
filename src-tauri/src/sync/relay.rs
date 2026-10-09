// SPDX-License-Identifier: AGPL-3.0-only
//! Syncing through a relay when two paired PCs aren't on the same network
//! (Cloud level only; DESIGN.md §4.9). The relay is a mailbox server
//! (`relay/worker.js` is one for Cloudflare Workers): each direction of a
//! pair has its own box, named by a hash of the pair key, and holds sealed
//! messages until the other PC fetches and deletes them.
//!
//! The relay never sees the pair key or anything readable: box names and
//! the token that guards them are derived from the pair key, and every
//! message is AES-256-GCM sealed with another key derived from it, bound to
//! its box and number so messages can't be swapped or replayed elsewhere.

use std::sync::Arc;
use std::time::Duration;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroizing;

use super::{config, device_id, pair_key, track, update, Peer};
use crate::net::{self, Connectivity, Purpose};
use crate::{db, AppState};

/// Changes read per round, and the most a letter holds before sealing.
const BATCH: usize = 300;
const LETTER_BYTES: usize = 1_400_000;

fn derive(pair: &[u8], salt: &[u8], info: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut out = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(salt), pair).expand(info, out.as_mut()).expect("32 bytes");
    out
}

/// The box `from` writes to for the other PC of this pair.
pub fn box_id(pair: &[u8], from: &str) -> String {
    hex::encode(*derive(pair, from.as_bytes(), b"sulcusai relay box v1"))
}

fn token(pair: &[u8], box_id: &str) -> String {
    hex::encode(*derive(pair, box_id.as_bytes(), b"sulcusai relay token v1"))
}

#[cfg(test)]
pub fn token_for_test(pair: &[u8], box_id: &str) -> String {
    token(pair, box_id)
}

fn seal(pair: &[u8], box_id: &str, seq: u64, plain: &[u8]) -> Vec<u8> {
    let key = derive(pair, box_id.as_bytes(), b"sulcusai relay seal v1");
    let nonce: [u8; 12] = super::wire::random();
    let aad = format!("{box_id}/{seq}");
    let ct = Aes256Gcm::new((&*key).into()).encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad: aad.as_bytes() }).expect("AES-GCM");
    [nonce.as_slice(), &ct].concat()
}

fn open(pair: &[u8], box_id: &str, seq: u64, sealed: &[u8]) -> Result<Vec<u8>, String> {
    if sealed.len() < 12 + 16 {
        return Err("A relay message was damaged.".into());
    }
    let key = derive(pair, box_id.as_bytes(), b"sulcusai relay seal v1");
    let aad = format!("{box_id}/{seq}");
    Aes256Gcm::new((&*key).into())
        .decrypt(Nonce::from_slice(&sealed[..12]), Payload { msg: &sealed[12..], aad: aad.as_bytes() })
        .map_err(|_| "A relay message didn't check out.".to_string())
}

#[derive(Serialize, Deserialize)]
struct Letter {
    /// The sender's database (a restore changes it).
    epoch: String,
    items: Vec<track::Change>,
    /// The sender's position after these items.
    up_to: i64,
}

#[derive(Deserialize)]
struct Stored {
    seq: u64,
    data: String,
}

/// Where this pair's relay exchange stands, kept with the peer.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RelayState {
    /// Our position already sent through the relay, and in which epoch.
    pub sent: i64,
    pub sent_epoch: String,
    /// The last message number we wrote (numbers start at 1).
    pub next: u64,
    /// The last message number we've read from the other PC.
    pub got: u64,
}

fn client(state: &AppState) -> Result<reqwest::Client, String> {
    net::external_client(state.settings().connectivity, Purpose::Cloud, false)
}

/// One exchange with `p` through the relay at `base`: sends what it hasn't
/// had, then takes and deletes what it left for us. Returns changes taken.
pub async fn exchange(state: &Arc<AppState>, base: &str, p: &Peer) -> Result<usize, String> {
    if state.settings().connectivity != Connectivity::Cloud {
        return Err("Syncing through the relay needs the Cloud level.".into());
    }
    let base = base.trim_end_matches('/');
    let c = state.work_cipher()?;
    let key = pair_key(&c, p)?;
    let me = device_id(&state.paths.data);
    let http = client(state)?;
    let mut rs = p.relay.clone();
    let epoch = config(&state.db.lock().unwrap()).epoch;

    // Send.
    let outbox = box_id(&key, &me);
    let out_token = token(&key, &outbox);
    let (since, up_to) = {
        let conn = state.db.lock().unwrap();
        let max = track::max_seq(&conn);
        let since = if rs.sent_epoch == epoch && rs.sent <= max { rs.sent } else { 0 };
        (since, max)
    };
    let mut sent_items = 0;
    if since < up_to {
        let mut after: track::Mark = (since, "\u{10FFFF}".into(), String::new());
        loop {
            let (items, end) = {
                let conn = state.db.lock().unwrap();
                track::changes(&conn, &c, &me, &p.id, &after, up_to, BATCH)?
            };
            let last = end.is_none() || items.len() < BATCH;
            // Letters stay well under the relay's 2 MB per message.
            let mut letters: Vec<Vec<track::Change>> = vec![Vec::new()];
            let mut size = 0;
            for item in items {
                let n = serde_json::to_vec(&item).map(|v| v.len()).unwrap_or(0);
                if size + n > LETTER_BYTES && !letters.last().unwrap().is_empty() {
                    letters.push(Vec::new());
                    size = 0;
                }
                size += n;
                letters.last_mut().unwrap().push(item);
            }
            for items in letters {
                if items.is_empty() && !last {
                    continue;
                }
                let letter = Letter { epoch: epoch.clone(), items, up_to };
                sent_items += letter.items.len();
                let seq = rs.next + 1;
                let body = seal(&key, &outbox, seq, &serde_json::to_vec(&letter).map_err(|e| e.to_string())?);
                if body.len() > 2_000_000 {
                    return Err("One item is too big to go through the relay (over 2 MB); it syncs on the local network.".into());
                }
                let resp = http
                    .put(format!("{base}/v1/box/{outbox}/{seq}"))
                    .bearer_auth(&out_token)
                    .timeout(Duration::from_secs(120))
                    .body(body)
                    .send()
                    .await
                    .map_err(|e| format!("Couldn't reach the relay: {e}"))?;
                if !resp.status().is_success() {
                    return Err(format!("The relay refused a message ({}).", resp.status()));
                }
                rs.next = seq;
            }
            match end {
                Some(m) if !last => after = m,
                _ => break,
            }
        }
        rs.sent = up_to;
        rs.sent_epoch = epoch.clone();
    }

    // Receive.
    let inbox = box_id(&key, &p.id);
    let in_token = token(&key, &inbox);
    let mut taken = 0;
    let mut cols = track::Columns::default();
    let mut later = Vec::new();
    loop {
        let resp = http
            .get(format!("{base}/v1/box/{inbox}?after={}", rs.got))
            .bearer_auth(&in_token)
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| format!("Couldn't reach the relay: {e}"))?;
        if resp.status().as_u16() == 404 {
            break;
        }
        if !resp.status().is_success() {
            return Err(format!("The relay refused to hand over messages ({}).", resp.status()));
        }
        let batch: Vec<Stored> = resp.json().await.map_err(|e| e.to_string())?;
        if batch.is_empty() {
            break;
        }
        for m in batch {
            if m.seq <= rs.got {
                continue;
            }
            let sealed = B64.decode(&m.data).map_err(|_| "A relay message was damaged.".to_string())?;
            let letter: Letter = serde_json::from_slice(&open(&key, &inbox, m.seq, &sealed)?).map_err(|e| e.to_string())?;
            let conn = state.db.lock().unwrap();
            let (n, l) = track::apply(&conn, &c, &me, &p.id, &letter.items, &mut cols)?;
            taken += n;
            later.extend(l);
            rs.got = m.seq;
        }
    }
    if !later.is_empty() {
        let conn = state.db.lock().unwrap();
        taken += track::apply(&conn, &c, &me, &p.id, &later, &mut cols)?.0;
    }
    if rs.got > p.relay.got {
        let _ = http.delete(format!("{base}/v1/box/{inbox}?through={}", rs.got)).bearer_auth(&in_token).timeout(Duration::from_secs(60)).send().await;
    }

    let name = p.name.clone();
    update(state, |cfg| {
        if let Some(x) = cfg.peers.iter_mut().find(|x| x.id == p.id) {
            x.relay = rs.clone();
            if taken > 0 || sent_items > 0 {
                x.last_sync = Some(db::now_ms());
            }
        }
    })?;
    if sent_items > 0 || taken > 0 {
        state.log("network", &format!("Synced with {name} through the relay: sent {sent_items} change(s), received {taken}"));
    }
    Ok(taken)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_differ_by_direction_and_letters_are_bound_to_their_place() {
        let pair = [9u8; 32];
        let (a, b) = (box_id(&pair, "pc-a"), box_id(&pair, "pc-b"));
        assert_ne!(a, b);
        assert_eq!(a.len(), 64);
        assert_ne!(token(&pair, &a), token(&pair, &b));
        let sealed = seal(&pair, &a, 3, b"hello");
        assert_eq!(open(&pair, &a, 3, &sealed).unwrap(), b"hello");
        assert!(open(&pair, &a, 4, &sealed).is_err(), "another number");
        assert!(open(&pair, &b, 3, &sealed).is_err(), "another box");
        assert!(open(&[8u8; 32], &a, 3, &sealed).is_err(), "another pair");
    }
}
