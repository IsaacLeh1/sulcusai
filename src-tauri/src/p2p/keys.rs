// SPDX-License-Identifier: AGPL-3.0-only
//! The node key, sealed with the data key in the settings table. Made on the
//! first start of the node, never derived from the data key or a device key,
//! and never sent anywhere.

use zeroize::Zeroizing;

use crate::crypto::Cipher;
use crate::db;

const KEY: &str = "p2p_node_key";

/// The saved key, or a new one saved now. A saved value that can't be
/// opened is an error, not a reason to replace it: a new key would change
/// this PC's node id for everyone who knows it.
pub fn node_key(conn: &rusqlite::Connection, c: &Cipher) -> Result<Zeroizing<Vec<u8>>, String> {
    if let Some(stored) = db::get::<String>(conn, KEY) {
        if !Cipher::is_encrypted(&stored) {
            return Err("the saved node key isn't sealed".into());
        }
        let hex_key = Zeroizing::new(c.decrypt(&stored)?);
        let bytes = Zeroizing::new(hex::decode(hex_key.as_bytes()).map_err(|_| "the saved node key can't be read".to_string())?);
        sulcus_net::identity::load(&bytes).map_err(|e| e.to_string())?;
        return Ok(bytes);
    }
    let bytes = Zeroizing::new(sulcus_net::identity::generate());
    db::set(conn, KEY, &c.encrypt(&Zeroizing::new(hex::encode(&bytes[..]))))?;
    Ok(bytes)
}

const SEAL: &str = "p2p_seal_key";

/// The X25519 key letters are sealed to, kept like the node key and for the
/// same reason never replaced when it can't be opened.
pub fn seal_key(conn: &rusqlite::Connection, c: &Cipher) -> Result<sulcus_net::seal::SealSecret, String> {
    if let Some(stored) = db::get::<String>(conn, SEAL) {
        if !Cipher::is_encrypted(&stored) {
            return Err("the saved seal key isn't sealed".into());
        }
        let hex_key = Zeroizing::new(c.decrypt(&stored)?);
        let bytes: [u8; 32] = hex::decode(hex_key.as_bytes()).ok().and_then(|b| b.try_into().ok()).ok_or("the saved seal key can't be read")?;
        return Ok(sulcus_net::seal::SealSecret::from_bytes(bytes));
    }
    let k = sulcus_net::seal::SealSecret::generate();
    db::set(conn, SEAL, &c.encrypt(&Zeroizing::new(hex::encode(k.to_bytes()))))?;
    Ok(k)
}
