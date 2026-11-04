// SPDX-License-Identifier: AGPL-3.0-only
//! Community model cards: signed notes about a model file, passed between
//! people and groups. There is no central list; a card book only holds the
//! cards someone chose to pass on, and shows who vouched for each file.

use std::collections::BTreeMap;

use libp2p::identity::Keypair;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::signed;

pub const PREFIX: &str = "smc1";
const DOMAIN: &str = "sulcus-model-card-v1\n";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    pub name: String,
    /// Hex sha256 of the model file; the card's identity.
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub notes: String,
    /// Optional `sulcus://share` link to get the file.
    #[serde(default)]
    pub link: String,
    pub issued: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Bad {
    Signature,
    Format,
}

pub fn sign(key: &Keypair, card: &Card) -> String {
    signed::seal(PREFIX, DOMAIN, key, card)
}

/// Reads a passed-on card; returns it with the PeerId of whoever signed it.
pub fn read(text: &str) -> Result<(Card, String), Bad> {
    let (card, who): (Card, _) = signed::open(PREFIX, DOMAIN, text).map_err(|e| match e {
        signed::Bad::Signature => Bad::Signature,
        _ => Bad::Format,
    })?;
    if card.sha256.len() != 64 || !card.sha256.bytes().all(|b| b.is_ascii_hexdigit()) || card.name.trim().is_empty() {
        return Err(Bad::Format);
    }
    Ok((card, who.to_string()))
}

/// Whether a downloaded file is the one the card describes.
pub fn matches(card: &Card, data: &[u8]) -> bool {
    data.len() as u64 == card.size && hex::encode(Sha256::digest(data)).eq_ignore_ascii_case(&card.sha256)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub card: Card,
    /// PeerIds that signed a card for this file, each with their own text.
    pub vouched: BTreeMap<String, String>,
}

/// The cards this person has received, one entry per file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Book {
    pub entries: BTreeMap<String, Entry>,
}

impl Book {
    /// Adds a card text; a newer card from the same signer replaces theirs.
    pub fn add(&mut self, text: &str) -> Result<String, Bad> {
        let (card, who) = read(text)?;
        let id = card.sha256.to_ascii_lowercase();
        let e = self.entries.entry(id.clone()).or_insert_with(|| Entry { card: card.clone(), vouched: BTreeMap::new() });
        let newer = e.vouched.get(&who).and_then(|t| read(t).ok()).map(|(c, _)| card.issued >= c.issued).unwrap_or(true);
        if newer {
            e.vouched.insert(who, text.trim().to_string());
            if card.issued >= e.card.issued {
                e.card = card;
            }
        }
        Ok(id)
    }

    pub fn forget(&mut self, sha256: &str) {
        self.entries.remove(&sha256.to_ascii_lowercase());
    }

    /// Cards to pass on to a group: every voucher's own signed text.
    pub fn export(&self) -> Vec<String> {
        self.entries.values().flat_map(|e| e.vouched.values().cloned()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(data: &[u8], issued: u64) -> Card {
        Card { name: "tiny".into(), sha256: hex::encode(Sha256::digest(data)), size: data.len() as u64, license: "MIT".into(), notes: String::new(), link: String::new(), issued }
    }

    #[test]
    fn signed_cards_round_trip_and_tamper_fails() {
        let k = Keypair::generate_ed25519();
        let c = card(b"weights", 1);
        let t = sign(&k, &c);
        let (back, who) = read(&t).unwrap();
        assert_eq!(back, c);
        assert_eq!(who, k.public().to_peer_id().to_string());
        assert!(matches(&back, b"weights"));
        assert!(!matches(&back, b"weightz"));
        let other = sign(&Keypair::generate_ed25519(), &card(b"x", 1));
        let p: Vec<&str> = t.split('.').collect();
        let q: Vec<&str> = other.split('.').collect();
        let forged = format!("{}.{}.{}.{}", p[0], q[1], p[2], p[3]);
        assert_eq!(read(&forged), Err(Bad::Signature));
        assert_eq!(read("nope"), Err(Bad::Format));
    }

    #[test]
    fn book_counts_vouchers_and_keeps_newest() {
        let (a, b) = (Keypair::generate_ed25519(), Keypair::generate_ed25519());
        let mut book = Book::default();
        let id = book.add(&sign(&a, &card(b"w", 1))).unwrap();
        book.add(&sign(&b, &card(b"w", 2))).unwrap();
        let mut old = card(b"w", 0);
        old.notes = "old".into();
        book.add(&sign(&a, &old)).unwrap();
        let e = &book.entries[&id];
        assert_eq!(e.vouched.len(), 2);
        assert_eq!(e.card.issued, 2);
        assert!(read(&e.vouched[&a.public().to_peer_id().to_string()]).unwrap().0.notes.is_empty());
        let mut other = Book::default();
        for t in book.export() {
            other.add(&t).unwrap();
        }
        assert_eq!(other.entries[&id].vouched.len(), 2);
        book.forget(&id);
        assert!(book.entries.is_empty());
    }
}
