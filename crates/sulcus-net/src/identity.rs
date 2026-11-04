// SPDX-License-Identifier: AGPL-3.0-only
//! The node key: one Ed25519 keypair per device. Bytes go in and out here;
//! sealing them at rest is the caller's job, so this crate never sees the
//! data key.

use libp2p::identity::Keypair;
use libp2p::PeerId;

#[derive(Debug)]
pub struct BadKey;

impl std::fmt::Display for BadKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the saved node key can't be read")
    }
}

impl std::error::Error for BadKey {}

/// A new key, as the bytes to seal and keep.
pub fn generate() -> Vec<u8> {
    Keypair::generate_ed25519().to_protobuf_encoding().expect("ed25519 keys always encode")
}

pub fn load(bytes: &[u8]) -> Result<Keypair, BadKey> {
    let k = Keypair::from_protobuf_encoding(bytes).map_err(|_| BadKey)?;
    // Rosters and invites sign with Ed25519; any other key type in the
    // settings table means it was tampered with or corrupted.
    if k.key_type() != libp2p::identity::KeyType::Ed25519 {
        return Err(BadKey);
    }
    Ok(k)
}

pub fn peer_id(bytes: &[u8]) -> Result<PeerId, BadKey> {
    Ok(load(bytes)?.public().to_peer_id())
}
