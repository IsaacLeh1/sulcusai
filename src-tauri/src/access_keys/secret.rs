// SPDX-License-Identifier: AGPL-3.0-only
//! Making, hashing and comparing key secrets.
//!
//! A secret is 256 random bits, so a single SHA-256 is enough to store it:
//! a slow hash like Argon2 only helps against guessing, and nobody can guess
//! 256 bits. The prefix stays "sk-sulcus-" so `secret_scrub` already takes
//! keys out of logs, crash reports and feedback.

use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::OsRng;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub const PREFIX: &str = "sk-sulcus-ak";

/// Separates these hashes from every other SHA-256 the app takes.
const DOMAIN: &[u8] = b"sulcusai access key v1\0";

pub fn generate() -> Zeroizing<String> {
    let mut bytes = Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(bytes.as_mut());
    Zeroizing::new(format!("{PREFIX}{}", hex::encode(&bytes[..])))
}

/// Hex SHA-256 of the key as presented, surrounding spaces ignored since
/// pasted keys often carry them.
pub fn hash(key: &str) -> String {
    let mut h = Sha256::new();
    h.update(DOMAIN);
    h.update(key.trim().as_bytes());
    hex::encode(h.finalize())
}

/// The last characters, enough to tell keys apart on a settings page.
pub fn hint(key: &str) -> String {
    let k = key.trim();
    k.get(k.len().saturating_sub(4)..).unwrap_or_default().to_string()
}

#[cfg(test)]
thread_local! {
    pub static COMPARES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Equal length and bytes, looking at every byte whatever it finds, so the
/// time taken says nothing about how much of a hash matched.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    #[cfg(test)]
    COMPARES.with(|c| c.set(c.get() + 1));
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
}
