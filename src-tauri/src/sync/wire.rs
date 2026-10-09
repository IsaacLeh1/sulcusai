// SPDX-License-Identifier: AGPL-3.0-only
//! The link between two PCs: length-prefixed frames, sealed with AES-256-GCM
//! under per-session keys.
//!
//! - Pairing: both PCs run SPAKE2 with the code shown on one of them, then
//!   prove they reached the same key before keeping a long-term pair key. A
//!   wrong guess learns nothing that helps offline, and each code allows only
//!   a few tries.
//! - Syncing: fresh random values from both sides and the pair key give the
//!   session keys, one per direction, so a recorded session can't be replayed
//!   and a PC that wasn't paired can't read or send anything.

use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use zeroize::Zeroizing;

/// The biggest frame either side accepts.
const MAX_FRAME: usize = 32 << 20;

pub async fn write_frame(w: &mut (impl AsyncWrite + Unpin), bytes: &[u8]) -> std::io::Result<()> {
    w.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    w.write_all(bytes).await?;
    w.flush().await
}

pub async fn read_frame(r: &mut (impl AsyncRead + Unpin)) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "frame too big"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(buf)
}

pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    OsRng.fill_bytes(&mut b);
    b
}

/// One direction of a session: a key and a counter as the nonce.
pub struct Sealer {
    aead: Aes256Gcm,
    n: u64,
}

impl Sealer {
    pub fn new(key: &[u8; 32]) -> Sealer {
        Sealer { aead: Aes256Gcm::new(key.into()), n: 0 }
    }

    fn nonce(&mut self) -> [u8; 12] {
        let mut nonce = [0u8; 12];
        nonce[4..].copy_from_slice(&self.n.to_be_bytes());
        self.n += 1;
        nonce
    }

    pub fn seal(&mut self, plain: &[u8]) -> Vec<u8> {
        let nonce = self.nonce();
        self.aead.encrypt(Nonce::from_slice(&nonce), plain).expect("AES-GCM encryption")
    }

    pub fn open(&mut self, sealed: &[u8]) -> Result<Vec<u8>, String> {
        let nonce = self.nonce();
        self.aead.decrypt(Nonce::from_slice(&nonce), sealed).map_err(|_| "The other PC's data didn't check out.".to_string())
    }
}

fn expand(ikm: &[u8], salt: &[u8], info: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut out = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(salt), ikm).expand(info, out.as_mut()).expect("32 bytes is a valid HKDF length");
    out
}

/// (from the connecting PC, to it) keys for one session.
pub fn session_keys(pair_key: &[u8], client_nonce: &[u8], server_nonce: &[u8]) -> (Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>) {
    let salt = [client_nonce, server_nonce].concat();
    (expand(pair_key, &salt, b"sulcusai sync v1 client"), expand(pair_key, &salt, b"sulcusai sync v1 server"))
}

const PAIR_ID: &[u8] = b"sulcusai pairing v1";

/// Starts pairing with a code: what to send, and the state to finish with.
pub fn pair_start(code: &str) -> (spake2::Spake2<spake2::Ed25519Group>, Vec<u8>) {
    spake2::Spake2::<spake2::Ed25519Group>::start_symmetric(&spake2::Password::new(code.as_bytes()), &spake2::Identity::new(PAIR_ID))
}

pub fn pair_finish(state: spake2::Spake2<spake2::Ed25519Group>, theirs: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
    state.finish(theirs).map(Zeroizing::new).map_err(|_| "The other PC's pairing reply was damaged.".to_string())
}

/// Proof that this side reached the same key (`role` is "client" or "server").
pub fn confirm(shared: &[u8], role: &str, transcript: &[u8]) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(shared).expect("HMAC takes any key length");
    mac.update(role.as_bytes());
    mac.update(transcript);
    mac.finalize().into_bytes().to_vec()
}

pub fn confirm_ok(shared: &[u8], role: &str, transcript: &[u8], proof: &[u8]) -> bool {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(shared).expect("HMAC takes any key length");
    mac.update(role.as_bytes());
    mac.update(transcript);
    mac.verify_slice(proof).is_ok()
}

/// The long-term key two paired PCs keep.
pub fn pair_key(shared: &[u8], transcript: &[u8]) -> Zeroizing<[u8; 32]> {
    expand(shared, transcript, b"sulcusai pair key v1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_agrees_only_on_the_same_code() {
        let (a, ma) = pair_start("K7Q29XMB");
        let (b, mb) = pair_start("K7Q29XMB");
        let ka = pair_finish(a, &mb).unwrap();
        let kb = pair_finish(b, &ma).unwrap();
        assert_eq!(*ka, *kb);
        let t = b"transcript";
        assert!(confirm_ok(&kb, "client", t, &confirm(&ka, "client", t)));
        assert!(!confirm_ok(&kb, "server", t, &confirm(&ka, "client", t)));

        let (a, ma) = pair_start("K7Q29XMB");
        let (b, mb) = pair_start("K7Q29XMC");
        let ka = pair_finish(a, &mb).unwrap();
        let kb = pair_finish(b, &ma).unwrap();
        assert!(!confirm_ok(&kb, "client", t, &confirm(&ka, "client", t)));
    }

    #[test]
    fn sessions_seal_each_direction_separately() {
        let key = random::<32>();
        let (c, s) = session_keys(&key, b"one", b"two");
        assert_ne!(*c, *s);
        let mut tx = Sealer::new(&c);
        let mut rx = Sealer::new(&c);
        let a = tx.seal(b"hello");
        let b = tx.seal(b"again");
        assert_eq!(rx.open(&a).unwrap(), b"hello");
        assert_eq!(rx.open(&b).unwrap(), b"again");
        // Replayed or reordered frames fail.
        let mut rx2 = Sealer::new(&c);
        assert!(rx2.open(&b).is_err());
        // Another session's keys can't read it.
        let (c2, _) = session_keys(&key, b"one", b"three");
        assert!(Sealer::new(&c2).open(&a).is_err());
    }

    #[tokio::test]
    async fn frames_round_trip() {
        let (mut a, mut b) = tokio::io::duplex(1 << 16);
        write_frame(&mut a, b"abc").await.unwrap();
        write_frame(&mut a, b"").await.unwrap();
        assert_eq!(read_frame(&mut b).await.unwrap(), b"abc");
        assert_eq!(read_frame(&mut b).await.unwrap(), b"");
    }
}
