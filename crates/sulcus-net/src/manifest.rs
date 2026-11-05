// SPDX-License-Identifier: AGPL-3.0-only
//! Updates and engines from peers (design 12): the developer signs a release
//! manifest with the security-notice keys under its own prefix; a file a
//! peer sends is used only if its SHA-256 matches the manifest, checked
//! piece by piece (4 MiB) so a bad peer is dropped mid-download.

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::security_notice::{verified_body_in, NoticeError, PublicKey};

pub const DOMAIN: &[u8] = b"SulcusAI release manifest v1\n";
pub const PIECE: usize = 4 * 1024 * 1024;
const MAX_FILES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    pub id: String,
    pub sha256: String,
    pub size: u64,
    /// SHA-256 of each 4 MiB piece, in order.
    pub pieces: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub issued: i64,
    pub installer: File,
    /// The installer's minisign signature (the updater's .sig file), checked
    /// before a peer-delivered installer runs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub installer_sig: String,
    #[serde(default)]
    pub engines: Vec<File>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn sha256(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

/// Checks the signature, then the shape. Nothing is parsed before the
/// signature holds.
pub fn accept(raw: &[u8], keys: &[PublicKey]) -> Result<Manifest, NoticeError> {
    let body = verified_body_in(raw, keys, DOMAIN)?;
    let m: Manifest = serde_json::from_slice(&body).map_err(|e| NoticeError::Malformed(e.to_string()))?;
    if m.engines.len() > MAX_FILES {
        return Err(NoticeError::InvalidContent("too many files".into()));
    }
    for f in std::iter::once(&m.installer).chain(&m.engines) {
        let want = f.size.div_ceil(PIECE as u64) as usize;
        if f.sha256.len() != 64 || f.pieces.len() != want || f.pieces.iter().any(|p| p.len() != 64) {
            return Err(NoticeError::InvalidContent(format!("{} has the wrong pieces", f.id)));
        }
    }
    Ok(m)
}

/// Whether piece `n` of a file is the one the manifest names.
pub fn piece_ok(f: &File, n: usize, bytes: &[u8]) -> bool {
    let last = f.size.div_ceil(PIECE as u64) as usize;
    let len_ok = if n + 1 == last { bytes.len() as u64 == f.size - (PIECE * n) as u64 } else { bytes.len() == PIECE };
    n < last && len_ok && f.pieces.get(n).is_some_and(|p| *p == sha256(bytes))
}

/// The whole file, once every piece passed.
pub fn file_ok(f: &File, bytes: &[u8]) -> bool {
    bytes.len() as u64 == f.size && sha256(bytes) == f.sha256
}

/// For the release script and tests: the entry for a file's bytes.
pub fn describe(id: &str, bytes: &[u8]) -> File {
    File { id: id.into(), sha256: sha256(bytes), size: bytes.len() as u64, pieces: bytes.chunks(PIECE).map(sha256).collect() }
}

#[allow(dead_code)]
fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security_notice::dev_sign::DevKey;

    fn signed(k: &DevKey, m: &Manifest, domain: &[u8]) -> String {
        let body = serde_json::to_vec(m).unwrap();
        let mut msg = domain.to_vec();
        msg.extend_from_slice(&body);
        serde_json::json!({ "v": 1, "body": b64(&body), "key": k.public_hex(), "sig": b64(&k.sign_exact(&msg)) }).to_string()
    }

    #[test]
    fn manifest_and_pieces() {
        let k = DevKey::from_seed([3; 32]);
        let data: Vec<u8> = (0..PIECE + 10).map(|i| i as u8).collect();
        let m = Manifest { version: "1.0.0".into(), issued: 1, installer: describe("setup.exe", &data), installer_sig: String::new(), engines: vec![] };
        let got = accept(signed(&k, &m, DOMAIN).as_bytes(), &[k.public()]).unwrap();
        assert_eq!(got, m);
        assert!(accept(signed(&k, &m, crate::security_notice::SIGNING_DOMAIN).as_bytes(), &[k.public()]).is_err(), "a notice signature isn't a manifest's");
        assert!(accept(signed(&k, &m, DOMAIN).as_bytes(), &[DevKey::from_seed([4; 32]).public()]).is_err());
        assert!(piece_ok(&m.installer, 0, &data[..PIECE]));
        assert!(piece_ok(&m.installer, 1, &data[PIECE..]));
        let mut bad = data[PIECE..].to_vec();
        bad[0] ^= 1;
        assert!(!piece_ok(&m.installer, 1, &bad), "a tampered piece is refused");
        assert!(file_ok(&m.installer, &data));
    }
}
