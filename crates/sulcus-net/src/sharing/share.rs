// SPDX-License-Identifier: AGPL-3.0-only
//! `sulcus://share` links and `.sulcus` files. A file is cut into pieces,
//! each sealed with AES-256-GCM under a key that only the link carries, so
//! whoever hosts the pieces cannot read them. The manifest lists a hash of
//! every sealed piece, so a piece from any source is checked before use.

use std::collections::{BTreeSet, HashMap};

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PIECE: usize = 1 << 20;
pub const SCHEME: &str = "sulcus://share/";
/// A source that sends this many bad pieces is dropped.
pub const STRIKES: u32 = 3;
/// Pieces asked of one source at a time.
pub const IN_FLIGHT: usize = 4;
const TAG: usize = 16;
const MAX_LINK: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub size: u64,
    pub piece_size: u32,
    /// Hex sha256 of each sealed piece.
    pub pieces: Vec<String>,
}

impl Manifest {
    pub fn id(&self) -> String {
        hex::encode(Sha256::digest(serde_json::to_vec(self).expect("manifest serializes")))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    /// Manifest id; pins the exact manifest.
    pub id: String,
    /// Piece key, base64url.
    pub key: String,
    /// PeerId of the person hosting it first.
    pub host: String,
    pub name: String,
    pub size: u64,
}

/// What a `.sulcus` file holds: the link and the manifest, so the receiver
/// needs nothing from the host but the pieces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareFile {
    pub v: u32,
    pub link: Link,
    pub manifest: Manifest,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Bad {
    Format,
    TooLong,
    WrongManifest,
    WrongPiece,
    Decrypt,
}

fn nonce(index: u32) -> Nonce {
    let mut n = [0u8; 12];
    n[8..].copy_from_slice(&index.to_be_bytes());
    Nonce::assume_unique_for_key(n)
}

fn sealer(key: &[u8; 32]) -> LessSafeKey {
    LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).expect("32-byte key"))
}

pub fn new_key() -> [u8; 32] {
    let mut k = [0u8; 32];
    getrandom::getrandom(&mut k).expect("OS random source");
    k
}

/// Seals a whole file; returns the manifest and the sealed pieces.
/// Each key is used for one file only, so a counter nonce never repeats.
pub fn seal_file(name: &str, data: &[u8], key: &[u8; 32], piece_size: usize) -> (Manifest, Vec<Vec<u8>>) {
    let k = sealer(key);
    let chunks: Vec<&[u8]> = if data.is_empty() { vec![&[][..]] } else { data.chunks(piece_size).collect() };
    let mut sealed = Vec::with_capacity(chunks.len());
    for (i, chunk) in chunks.into_iter().enumerate() {
        let mut buf = chunk.to_vec();
        k.seal_in_place_append_tag(nonce(i as u32), Aad::from((i as u32).to_be_bytes()), &mut buf).expect("seal");
        sealed.push(buf);
    }
    let pieces = sealed.iter().map(|p| hex::encode(Sha256::digest(p))).collect();
    (Manifest { name: name.to_string(), size: data.len() as u64, piece_size: piece_size as u32, pieces }, sealed)
}

/// Checks a sealed piece against the manifest and opens it.
pub fn open_piece(m: &Manifest, key: &[u8; 32], index: u32, sealed: &[u8]) -> Result<Vec<u8>, Bad> {
    let want = m.pieces.get(index as usize).ok_or(Bad::WrongPiece)?;
    if sealed.len() < TAG || sealed.len() > m.piece_size as usize + TAG {
        return Err(Bad::WrongPiece);
    }
    if hex::encode(Sha256::digest(sealed)) != *want {
        return Err(Bad::WrongPiece);
    }
    let mut buf = sealed.to_vec();
    let plain = sealer(key).open_in_place(nonce(index), Aad::from(index.to_be_bytes()), &mut buf).map_err(|_| Bad::Decrypt)?;
    Ok(plain.to_vec())
}

pub fn make_link(m: &Manifest, key: &[u8; 32], host: &str) -> Link {
    Link { id: m.id(), key: B64.encode(key), host: host.to_string(), name: m.name.clone(), size: m.size }
}

impl Link {
    pub fn to_text(&self) -> String {
        format!("{SCHEME}{}", B64.encode(serde_json::to_vec(self).expect("link serializes")))
    }

    pub fn parse(text: &str) -> Result<Link, Bad> {
        let text = text.trim();
        if text.len() > MAX_LINK {
            return Err(Bad::TooLong);
        }
        let body = text.strip_prefix(SCHEME).ok_or(Bad::Format)?;
        let link: Link = serde_json::from_slice(&B64.decode(body).map_err(|_| Bad::Format)?).map_err(|_| Bad::Format)?;
        link.key_bytes()?;
        if link.id.len() != 64 || !link.id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Bad::Format);
        }
        Ok(link)
    }

    pub fn key_bytes(&self) -> Result<[u8; 32], Bad> {
        B64.decode(&self.key).ok().and_then(|k| k.try_into().ok()).ok_or(Bad::Format)
    }

    /// A manifest fetched from anyone is only used if it is the one linked.
    pub fn check(&self, m: &Manifest) -> Result<(), Bad> {
        if m.id() == self.id && m.size == self.size {
            Ok(())
        } else {
            Err(Bad::WrongManifest)
        }
    }
}

impl ShareFile {
    pub fn to_text(&self) -> String {
        serde_json::to_string_pretty(self).expect("share file serializes")
    }

    pub fn parse(text: &str) -> Result<ShareFile, Bad> {
        let f: ShareFile = serde_json::from_str(text).map_err(|_| Bad::Format)?;
        f.link.key_bytes()?;
        f.link.check(&f.manifest)?;
        Ok(f)
    }
}

/// Plans a download from many sources at once: rarest pieces first, a few
/// pieces in flight per source, and sources that send bad pieces dropped.
#[derive(Debug, Default)]
pub struct Swarm {
    total: u32,
    done: BTreeSet<u32>,
    have: HashMap<String, BTreeSet<u32>>,
    asked: HashMap<u32, String>,
    strikes: HashMap<String, u32>,
}

impl Swarm {
    pub fn new(total: u32) -> Self {
        Swarm { total, ..Default::default() }
    }

    pub fn add_source(&mut self, peer: &str, pieces: impl IntoIterator<Item = u32>) {
        if self.strikes.get(peer).copied().unwrap_or(0) >= STRIKES {
            return;
        }
        let total = self.total;
        self.have.entry(peer.to_string()).or_default().extend(pieces.into_iter().filter(|p| *p < total));
    }

    pub fn drop_source(&mut self, peer: &str) {
        self.have.remove(peer);
        self.asked.retain(|_, s| s != peer);
    }

    pub fn has_source(&self, peer: &str) -> bool {
        self.have.contains_key(peer)
    }

    fn in_flight(&self, peer: &str) -> usize {
        self.asked.values().filter(|s| *s == peer).count()
    }

    /// Next requests to send, as (source, piece).
    pub fn next(&mut self) -> Vec<(String, u32)> {
        let mut wanted: Vec<(usize, u32)> = (0..self.total)
            .filter(|p| !self.done.contains(p) && !self.asked.contains_key(p))
            .map(|p| (self.have.values().filter(|h| h.contains(&p)).count(), p))
            .filter(|(n, _)| *n > 0)
            .collect();
        wanted.sort();
        let mut out = Vec::new();
        for (_, p) in wanted {
            let mut peers: Vec<String> = self.have.iter().filter(|(_, h)| h.contains(&p)).map(|(s, _)| s.clone()).collect();
            peers.sort_by_key(|s| (self.in_flight(s), s.clone()));
            if let Some(s) = peers.into_iter().find(|s| self.in_flight(s) < IN_FLIGHT) {
                self.asked.insert(p, s.clone());
                out.push((s, p));
            }
        }
        out
    }

    /// Records a piece's arrival; `good` is the result of `open_piece`.
    pub fn arrived(&mut self, peer: &str, piece: u32, good: bool) {
        if self.asked.get(&piece).map(|s| s == peer) == Some(true) {
            self.asked.remove(&piece);
        }
        if good {
            self.done.insert(piece);
        } else {
            let n = self.strikes.entry(peer.to_string()).or_default();
            *n += 1;
            if *n >= STRIKES {
                self.drop_source(peer);
            }
        }
    }

    pub fn finished(&self) -> bool {
        self.done.len() as u32 == self.total
    }

    pub fn progress(&self) -> (u32, u32) {
        (self.done.len() as u32, self.total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ([u8; 32], Manifest, Vec<Vec<u8>>, Vec<u8>) {
        let data: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let key = new_key();
        let (m, sealed) = seal_file("song.mp3", &data, &key, 1024);
        (key, m, sealed, data)
    }

    #[test]
    fn pieces_round_trip_and_stay_sealed() {
        let (key, m, sealed, data) = sample();
        assert_eq!(m.pieces.len(), 10);
        assert!(!sealed[0].windows(16).any(|w| w == &data[..16]));
        let mut back = Vec::new();
        for (i, p) in sealed.iter().enumerate() {
            back.extend(open_piece(&m, &key, i as u32, p).unwrap());
        }
        assert_eq!(back, data);
    }

    #[test]
    fn tampered_or_swapped_pieces_are_refused() {
        let (key, m, sealed, _) = sample();
        let mut bad = sealed[1].clone();
        bad[0] ^= 1;
        assert_eq!(open_piece(&m, &key, 1, &bad), Err(Bad::WrongPiece));
        assert_eq!(open_piece(&m, &key, 2, &sealed[1]), Err(Bad::WrongPiece));
        assert_eq!(open_piece(&m, &new_key(), 1, &sealed[1]), Err(Bad::Decrypt));
    }

    #[test]
    fn link_and_file_round_trip() {
        let (key, m, _, _) = sample();
        let link = make_link(&m, &key, "12D3KooWhost");
        let text = link.to_text();
        assert!(text.starts_with(SCHEME));
        let back = Link::parse(&text).unwrap();
        assert_eq!(back, link);
        assert_eq!(back.key_bytes().unwrap(), key);
        let f = ShareFile { v: 1, link: link.clone(), manifest: m.clone() };
        assert_eq!(ShareFile::parse(&f.to_text()).unwrap(), f);
        let mut other = m.clone();
        other.name = "else".into();
        assert_eq!(link.check(&other), Err(Bad::WrongManifest));
        assert_eq!(Link::parse("sulcus://share/!!"), Err(Bad::Format));
        assert_eq!(Link::parse("https://x"), Err(Bad::Format));
    }

    #[test]
    fn empty_file_has_one_piece() {
        let key = new_key();
        let (m, sealed) = seal_file("empty", &[], &key, PIECE);
        assert_eq!(sealed.len(), 1);
        assert!(open_piece(&m, &key, 0, &sealed[0]).unwrap().is_empty());
    }

    #[test]
    fn swarm_spreads_load_and_drops_liars() {
        let mut s = Swarm::new(10);
        s.add_source("a", 0..10);
        s.add_source("b", 0..10);
        let first = s.next();
        assert_eq!(first.len(), 8, "four in flight per source");
        for (p, n) in &first {
            s.arrived(p, *n, p != "b");
        }
        assert!(!s.has_source("b"), "dropped after three bad pieces");
        s.add_source("b", 0..10);
        assert!(!s.has_source("b"), "a dropped source stays out");
        while !s.finished() {
            let batch = s.next();
            assert!(!batch.is_empty());
            assert!(batch.iter().all(|(p, _)| p == "a"));
            for (p, n) in batch {
                s.arrived(&p, n, true);
            }
        }
        assert_eq!(s.progress(), (10, 10));
    }

    #[test]
    fn swarm_asks_rarest_first() {
        let mut s = Swarm::new(3);
        s.add_source("a", [0, 1, 2]);
        s.add_source("b", [0, 1]);
        assert_eq!(s.next()[0].1, 2);
    }
}
