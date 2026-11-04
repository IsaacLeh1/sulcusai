// SPDX-License-Identifier: AGPL-3.0-only
//! Signed records passed between people: invites, address records and
//! rosters. Each is a JSON body signed by a node key over a domain line plus
//! the body, so a signature made for one kind can't pass as another. Text
//! form: `<prefix>.<base64url body>.<base64url public key>.<base64url signature>`.
//! Parsing is strict and size-capped; nothing here trusts a record until
//! its signature checks against the key, and the key matches the PeerId the
//! record names.

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine as _;
use libp2p::identity::{Keypair, PublicKey};
use libp2p::PeerId;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Largest text form accepted; an invite must also fit a QR code.
pub const MAX_TEXT: usize = 4_096;

#[derive(Debug, PartialEq, Eq)]
pub enum Bad {
    TooLong,
    Format,
    Signature,
    WrongSigner,
    Body(String),
}

impl std::fmt::Display for Bad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Bad::TooLong => f.write_str("it's too long to be one"),
            Bad::Format => f.write_str("it isn't in the right form"),
            Bad::Signature => f.write_str("its signature doesn't match"),
            Bad::WrongSigner => f.write_str("it wasn't signed by the PC it names"),
            Bad::Body(e) => write!(f, "its contents can't be read ({e})"),
        }
    }
}

impl std::error::Error for Bad {}

fn signed_bytes(domain: &str, body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(domain.len() + body.len());
    v.extend_from_slice(domain.as_bytes());
    v.extend_from_slice(body);
    v
}

/// Signs `value` and returns its text form.
pub fn seal<T: Serialize>(prefix: &str, domain: &str, key: &Keypair, value: &T) -> String {
    let body = serde_json::to_vec(value).expect("records serialize");
    let sig = key.sign(&signed_bytes(domain, &body)).expect("ed25519 signs");
    format!("{prefix}.{}.{}.{}", B64.encode(&body), B64.encode(key.public().encode_protobuf()), B64.encode(sig))
}

/// Checks and reads a text form; returns the body and the signer's PeerId.
pub fn open<T: DeserializeOwned>(prefix: &str, domain: &str, text: &str) -> Result<(T, PeerId), Bad> {
    let text = text.trim();
    if text.len() > MAX_TEXT {
        return Err(Bad::TooLong);
    }
    let mut parts = text.split('.');
    if parts.next() != Some(prefix) {
        return Err(Bad::Format);
    }
    let (Some(body), Some(key), Some(sig), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else { return Err(Bad::Format) };
    let (body, key, sig) = (B64.decode(body).map_err(|_| Bad::Format)?, B64.decode(key).map_err(|_| Bad::Format)?, B64.decode(sig).map_err(|_| Bad::Format)?);
    let public = PublicKey::try_decode_protobuf(&key).map_err(|_| Bad::Format)?;
    if public.key_type() != libp2p::identity::KeyType::Ed25519 {
        return Err(Bad::Format);
    }
    if !public.verify(&signed_bytes(domain, &body), &sig) {
        return Err(Bad::Signature);
    }
    let value = serde_json::from_slice(&body).map_err(|e| Bad::Body(e.to_string()))?;
    Ok((value, public.to_peer_id()))
}

/// Seconds since 1970, for issue and expiry times.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
