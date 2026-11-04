// SPDX-License-Identifier: AGPL-3.0-only
//! Letters sealed end to end for one recipient (design 22, "Sealed letters"),
//! so a relay's mailbox holds only padded ciphertext. Composition of vetted
//! pieces, nothing hand-rolled: X25519 (RFC 7748), HKDF-SHA256 and
//! ChaCha20-Poly1305, shaped like HPKE's auth mode.

use libp2p::identity::Keypair;
use libp2p::PeerId;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305};
use ring::hkdf;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::mailbox::{Letter, KEEP_SECS, MAX_LETTER};
use crate::signed::{self, Bad};

const KEY_PREFIX: &str = "sulcus:k1";
const KEY_DOMAIN: &str = "SulcusAI seal key v1\n";
const LETTER_INFO: &[u8] = b"SulcusAI letter v1";
const BASEPOINT: [u8; 32] = {
    let mut b = [0u8; 32];
    b[0] = 9;
    b
};
/// Most a header may take; the largest bucket leaves this room so a whole
/// letter stays within the relay's per-letter cap.
const HEADER_ROOM: usize = 1_024;
const TAG: usize = 16;
/// Padded ciphertext sizes; the header is not padded (it is the same size
/// for every letter between the same two PCs).
const BUCKETS: [usize; 5] = [1 << 10, 4 << 10, 16 << 10, 64 << 10, MAX_LETTER - HEADER_ROOM];
pub const MAX_PLAIN: usize = MAX_LETTER - HEADER_ROOM - TAG - 4;
/// Clocks disagree; further ahead than this is a forged or broken letter.
const FUTURE_SLACK: u64 = 600;

/// A node's X25519 seal secret. Kept apart from the Ed25519 node key so no
/// key serves two schemes.
#[derive(Clone)]
pub struct SealSecret([u8; 32]);

impl SealSecret {
    pub fn generate() -> SealSecret {
        let mut b = [0u8; 32];
        getrandom::getrandom(&mut b).expect("the OS random source failed");
        SealSecret(b)
    }
    pub fn from_bytes(b: [u8; 32]) -> SealSecret {
        SealSecret(b)
    }
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }
    pub fn public(&self) -> [u8; 32] {
        x25519_dalek::x25519(self.0, BASEPOINT)
    }
    /// The signed record that lets others believe this key belongs to `node`.
    pub fn record(&self, node: &Keypair, now: u64) -> String {
        let r = SealKey { peer: node.public().to_peer_id().to_string(), key: hex::encode(self.public()), issued: now };
        signed::seal(KEY_PREFIX, KEY_DOMAIN, node, &r)
    }
}

impl Drop for SealSecret {
    fn drop(&mut self) {
        self.0 = [0; 32];
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealKey {
    pub peer: String,
    pub key: String,
    pub issued: u64,
}

impl SealKey {
    /// Opens a seal key record; the key is only believed for its signer.
    pub fn open(text: &str) -> Result<(SealKey, [u8; 32]), Bad> {
        let (r, signer): (SealKey, PeerId) = signed::open(KEY_PREFIX, KEY_DOMAIN, text)?;
        if r.peer != signer.to_string() {
            return Err(Bad::WrongSigner);
        }
        let key: [u8; 32] = hex::decode(&r.key).ok().and_then(|k| k.try_into().ok()).ok_or(Bad::Format)?;
        Ok((r, key))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Header {
    from: String,
    to: String,
    id: String,
    at: u64,
    eph: String,
    /// The sender's signed seal key record.
    sk: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SealError {
    TooBig,
    BadRecipientKey,
    Unreadable,
    WrongRecipient,
    WrongSender,
    Forged,
    Replayed,
    TooOld,
}

impl std::fmt::Display for SealError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SealError::TooBig => "the letter is too big",
            SealError::BadRecipientKey => "the recipient's seal key isn't valid",
            SealError::Unreadable => "the letter couldn't be read",
            SealError::WrongRecipient => "the letter is for someone else",
            SealError::WrongSender => "the letter's sender doesn't match who left it",
            SealError::Forged => "the letter was changed or forged",
            SealError::Replayed => "this letter was already received",
            SealError::TooOld => "the letter is too old or dated in the future",
        })
    }
}

impl std::error::Error for SealError {}

fn dh(secret: &[u8; 32], public: &[u8; 32]) -> Result<[u8; 32], SealError> {
    let out = x25519_dalek::x25519(*secret, *public);
    // A low-order point gives all zeros, which would make the key public.
    if out == [0u8; 32] {
        return Err(SealError::BadRecipientKey);
    }
    Ok(out)
}

fn aead_key(eph_pub: &[u8; 32], ikm: &[u8], header: &[u8]) -> LessSafeKey {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, eph_pub).extract(ikm);
    let info = [LETTER_INFO, header];
    let okm = prk.expand(&info, &CHACHA20_POLY1305).expect("the HKDF output length is fixed");
    LessSafeKey::new(UnboundKey::from(okm))
}

fn padded_len(plain: usize) -> Option<usize> {
    BUCKETS.iter().map(|b| b - TAG).find(|room| *room >= plain + 4)
}

/// Seals `plain` from `node` (with its seal secret) for the recipient whose
/// signed seal key record is `their_record`.
pub fn seal(node: &Keypair, mine: &SealSecret, their_record: &str, id: &str, plain: &[u8], now: u64) -> Result<Vec<u8>, SealError> {
    let (theirs, their_key) = SealKey::open(their_record).map_err(|_| SealError::BadRecipientKey)?;
    let size = padded_len(plain.len()).ok_or(SealError::TooBig)?;
    let eph = SealSecret::generate();
    let eph_pub = eph.public();
    let mut ikm = dh(&eph.0, &their_key)?.to_vec();
    ikm.extend_from_slice(&dh(&mine.0, &their_key)?);
    let header = Header {
        from: node.public().to_peer_id().to_string(),
        to: theirs.peer,
        id: id.to_string(),
        at: now,
        eph: hex::encode(eph_pub),
        sk: mine.record(node, now),
    };
    let h = serde_json::to_vec(&header).map_err(|_| SealError::Unreadable)?;
    if h.len() > HEADER_ROOM - 2 {
        return Err(SealError::TooBig);
    }
    let mut buf = Vec::with_capacity(size + TAG);
    buf.extend_from_slice(&(plain.len() as u32).to_be_bytes());
    buf.extend_from_slice(plain);
    buf.resize(size, 0);
    // Each key comes from a fresh ephemeral secret and is used once.
    aead_key(&eph_pub, &ikm, &h)
        .seal_in_place_append_tag(Nonce::assume_unique_for_key([0; 12]), Aad::from(&h[..]), &mut buf)
        .map_err(|_| SealError::Unreadable)?;
    let mut out = Vec::with_capacity(2 + h.len() + buf.len());
    out.extend_from_slice(&(h.len() as u16).to_be_bytes());
    out.extend_from_slice(&h);
    out.extend_from_slice(&buf);
    Ok(out)
}

/// What an opened letter says, after every check passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub from: PeerId,
    pub id: String,
    pub at: u64,
    pub body: Vec<u8>,
}

/// Opens a letter taken from a mailbox. `letter.from` is what the relay saw
/// on the connection; it must agree with the sealed sender. `seen` refuses
/// replays.
pub fn open(me: &PeerId, mine: &SealSecret, letter: &Letter, seen: &mut Seen, now: u64) -> Result<Opened, SealError> {
    let raw = &letter.body;
    if raw.len() < 2 {
        return Err(SealError::Unreadable);
    }
    let hlen = u16::from_be_bytes([raw[0], raw[1]]) as usize;
    if raw.len() < 2 + hlen + TAG {
        return Err(SealError::Unreadable);
    }
    let h = &raw[2..2 + hlen];
    let header: Header = serde_json::from_slice(h).map_err(|_| SealError::Unreadable)?;
    if header.to != me.to_string() {
        return Err(SealError::WrongRecipient);
    }
    if header.from != letter.from || header.id != letter.id {
        return Err(SealError::WrongSender);
    }
    let (sk, sender_key) = SealKey::open(&header.sk).map_err(|_| SealError::Forged)?;
    if sk.peer != header.from {
        return Err(SealError::Forged);
    }
    let eph_pub: [u8; 32] = hex::decode(&header.eph).ok().and_then(|k| k.try_into().ok()).ok_or(SealError::Unreadable)?;
    let mut ikm = dh(&mine.0, &eph_pub).map_err(|_| SealError::Forged)?.to_vec();
    ikm.extend_from_slice(&dh(&mine.0, &sender_key).map_err(|_| SealError::Forged)?);
    let mut buf = raw[2 + hlen..].to_vec();
    let plain = aead_key(&eph_pub, &ikm, h)
        .open_in_place(Nonce::assume_unique_for_key([0; 12]), Aad::from(h), &mut buf)
        .map_err(|_| SealError::Forged)?;
    if plain.len() < 4 {
        return Err(SealError::Forged);
    }
    let n = u32::from_be_bytes([plain[0], plain[1], plain[2], plain[3]]) as usize;
    if n > plain.len() - 4 {
        return Err(SealError::Forged);
    }
    // Checked only after the tag, so an attacker can't burn a real letter's
    // id in `seen` with a forgery.
    seen.admit(&header.from, &header.id, header.at, now)?;
    let from = header.from.parse().map_err(|_| SealError::Forged)?;
    Ok(Opened { from, id: header.id, at: header.at, body: plain[4..4 + n].to_vec() })
}

/// Letters already opened, per sender and id, so a relay (or anyone) handing
/// a letter over twice gets it read once.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Seen {
    pub letters: HashMap<String, u64>,
}

impl Seen {
    fn admit(&mut self, from: &str, id: &str, at: u64, now: u64) -> Result<(), SealError> {
        if now.saturating_sub(at) > KEEP_SECS || at > now + FUTURE_SLACK {
            return Err(SealError::TooOld);
        }
        // Forgetting is safe: anything older is refused by the date above.
        self.letters.retain(|_, t| now.saturating_sub(*t) <= KEEP_SECS + FUTURE_SLACK);
        let key = format!("{from}\n{id}");
        if self.letters.contains_key(&key) {
            return Err(SealError::Replayed);
        }
        self.letters.insert(key, at);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Who {
        node: Keypair,
        seal: SealSecret,
    }
    fn who() -> Who {
        Who { node: Keypair::generate_ed25519(), seal: SealSecret::generate() }
    }
    fn pid(w: &Who) -> PeerId {
        w.node.public().to_peer_id()
    }
    fn letter(from: &Who, id: &str, body: Vec<u8>) -> Letter {
        Letter { from: pid(from).to_string(), id: id.into(), body, at: 1_000 }
    }

    #[test]
    fn a_sealed_letter_opens_for_its_recipient_only() {
        let (ana, bo, eve) = (who(), who(), who());
        let body = seal(&ana.node, &ana.seal, &bo.seal.record(&bo.node, 1_000), "1", b"hello bo", 1_000).unwrap();
        assert!(!body.windows(8).any(|w| w == b"hello bo"));
        let l = letter(&ana, "1", body);
        let got = open(&pid(&bo), &bo.seal, &l, &mut Seen::default(), 1_010).unwrap();
        assert_eq!((got.from, got.id.as_str(), got.body.as_slice()), (pid(&ana), "1", &b"hello bo"[..]));
        assert_eq!(open(&pid(&eve), &eve.seal, &l, &mut Seen::default(), 1_010), Err(SealError::WrongRecipient));
        // Eve claiming to be Bo still lacks Bo's seal secret.
        assert_eq!(open(&pid(&bo), &eve.seal, &l, &mut Seen::default(), 1_010), Err(SealError::Forged));
    }

    #[test]
    fn sizes_are_padded_to_buckets() {
        let (ana, bo) = (who(), who());
        let r = bo.seal.record(&bo.node, 1_000);
        let a = seal(&ana.node, &ana.seal, &r, "1", b"x", 1_000).unwrap();
        let b = seal(&ana.node, &ana.seal, &r, "2", &[1; 900], 1_000).unwrap();
        let h = |v: &[u8]| 2 + u16::from_be_bytes([v[0], v[1]]) as usize;
        assert_eq!(a.len() - h(&a), b.len() - h(&b));
        assert!(a.len() <= 1_024 + HEADER_ROOM);
        let big = seal(&ana.node, &ana.seal, &r, "3", &vec![2; MAX_PLAIN], 1_000).unwrap();
        assert!(big.len() <= MAX_LETTER);
        assert_eq!(seal(&ana.node, &ana.seal, &r, "4", &vec![2; MAX_PLAIN + 1], 1_000), Err(SealError::TooBig));
    }

    #[test]
    fn tampering_anywhere_is_refused() {
        let (ana, bo) = (who(), who());
        let body = seal(&ana.node, &ana.seal, &bo.seal.record(&bo.node, 1_000), "1", b"hello", 1_000).unwrap();
        for i in (0..body.len()).step_by(7) {
            let mut b = body.clone();
            b[i] ^= 1;
            assert!(open(&pid(&bo), &bo.seal, &letter(&ana, "1", b), &mut Seen::default(), 1_010).is_err(), "byte {i}");
        }
    }

    #[test]
    fn the_relay_recorded_sender_and_id_must_match() {
        let (ana, bo, eve) = (who(), who(), who());
        let body = seal(&ana.node, &ana.seal, &bo.seal.record(&bo.node, 1_000), "1", b"hi", 1_000).unwrap();
        let mut l = letter(&eve, "1", body.clone());
        assert_eq!(open(&pid(&bo), &bo.seal, &l, &mut Seen::default(), 1_010), Err(SealError::WrongSender));
        l = letter(&ana, "2", body);
        assert_eq!(open(&pid(&bo), &bo.seal, &l, &mut Seen::default(), 1_010), Err(SealError::WrongSender));
    }

    #[test]
    fn a_sender_seal_key_signed_by_someone_else_is_refused() {
        let (ana, bo, eve) = (who(), who(), who());
        // Eve seals with her own seal key but names herself as Ana.
        let mut body = seal(&eve.node, &eve.seal, &bo.seal.record(&bo.node, 1_000), "1", b"hi", 1_000).unwrap();
        let hl = u16::from_be_bytes([body[0], body[1]]) as usize;
        let text = String::from_utf8(body[2..2 + hl].to_vec()).unwrap().replace(&pid(&eve).to_string(), &pid(&ana).to_string());
        let mut b = (text.len() as u16).to_be_bytes().to_vec();
        b.extend_from_slice(text.as_bytes());
        b.extend_from_slice(&body.split_off(2 + hl));
        assert!(open(&pid(&bo), &bo.seal, &letter(&ana, "1", b), &mut Seen::default(), 1_010).is_err());
    }

    #[test]
    fn replays_and_stale_letters_are_refused() {
        let (ana, bo) = (who(), who());
        let r = bo.seal.record(&bo.node, 1_000);
        let l = letter(&ana, "1", seal(&ana.node, &ana.seal, &r, "1", b"hi", 1_000).unwrap());
        let mut seen = Seen::default();
        open(&pid(&bo), &bo.seal, &l, &mut seen, 1_010).unwrap();
        assert_eq!(open(&pid(&bo), &bo.seal, &l, &mut seen, 1_020), Err(SealError::Replayed));
        let old = letter(&ana, "2", seal(&ana.node, &ana.seal, &r, "2", b"hi", 1_000).unwrap());
        assert_eq!(open(&pid(&bo), &bo.seal, &old, &mut seen, 1_001 + KEEP_SECS), Err(SealError::TooOld));
        let ahead = letter(&ana, "3", seal(&ana.node, &ana.seal, &r, "3", b"hi", 1_000_000).unwrap());
        assert_eq!(open(&pid(&bo), &bo.seal, &ahead, &mut seen, 1_000), Err(SealError::TooOld));
    }

    #[test]
    fn seal_key_records_belong_to_their_signer() {
        let (ana, bo) = (who(), who());
        let r = ana.seal.record(&ana.node, 5);
        let (k, key) = SealKey::open(&r).unwrap();
        assert_eq!((k.peer, key), (pid(&ana).to_string(), ana.seal.public()));
        // Bo signing a record that names Ana is refused.
        let forged = signed::seal(KEY_PREFIX, KEY_DOMAIN, &bo.node, &SealKey { peer: pid(&ana).to_string(), key: hex::encode(bo.seal.public()), issued: 5 });
        assert!(SealKey::open(&forged).is_err());
        assert!(seal(&bo.node, &bo.seal, &forged, "1", b"x", 5).is_err());
    }
}
