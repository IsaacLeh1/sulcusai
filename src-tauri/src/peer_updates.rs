// SPDX-License-Identifier: AGPL-3.0-only
//! Updates passed between PCs (design 12, Q11) for when GitHub can't be
//! reached. Two switches, both off: take updates from peers, and share the
//! ones this PC has. Nothing a peer sends is trusted by itself: the release
//! manifest must carry the developer's offline signature, every 4 MiB piece
//! must match it, and the installer must pass the same minisign check the
//! Tauri updater makes before it runs.

use std::path::PathBuf;
use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::{Deserialize, Serialize};
use sulcus_net::libp2p::PeerId;
use sulcus_net::manifest::{self, Manifest, PIECE};
use sulcus_net::piece_wire;
use sulcus_net::NodeHandle;

use crate::{db, AppState, AppStateRef};

const KEY: &str = "peer_updates";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PeerUpdates {
    /// Fetch updates from other PCs when GitHub can't be reached.
    pub from_peers: bool,
    /// Hand updates this PC holds to PCs that ask.
    pub share: bool,
}

pub fn settings(state: &AppState) -> PeerUpdates {
    db::get(&state.db.lock().unwrap(), KEY).unwrap_or_default()
}

#[tauri::command(async)]
pub fn peer_updates_view(state: AppStateRef) -> PeerUpdates {
    settings(&state)
}

#[tauri::command(async)]
pub fn set_peer_updates(state: AppStateRef, s: PeerUpdates) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::log_action(&conn, "settings", &format!("Updates from other PCs: take {}, share {}", on_off(s.from_peers), on_off(s.share)));
    db::set(&conn, KEY, &s)
}

fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

/// The updater's minisign key, read from the same config the updater uses
/// so the two can never disagree.
fn updater_key() -> Result<minisign_verify::PublicKey, String> {
    let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).map_err(|e| e.to_string())?;
    let b64 = conf["plugins"]["updater"]["pubkey"].as_str().ok_or("no updater key")?;
    let text = String::from_utf8(B64.decode(b64).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    minisign_verify::PublicKey::decode(&text).map_err(|e| e.to_string())
}

/// The whole check before an installer from a peer may run: its hash is
/// the signed manifest's, and its minisign signature (carried in the signed
/// manifest, base64 of the .sig file like latest.json) holds for `key`.
pub fn check_installer(m: &Manifest, bytes: &[u8], key: &minisign_verify::PublicKey) -> Result<(), String> {
    if !manifest::file_ok(&m.installer, bytes) {
        return Err("The installer doesn't match the signed release manifest.".into());
    }
    let sig_text = String::from_utf8(B64.decode(m.installer_sig.trim()).map_err(|_| "The installer's signature isn't readable.")?).map_err(|_| "The installer's signature isn't readable.")?;
    let sig = minisign_verify::Signature::decode(&sig_text).map_err(|_| "The installer's signature isn't readable.")?;
    key.verify(bytes, &sig, true).map_err(|_| "The installer's signature doesn't match SulcusAI's update key.".to_string())
}

fn newer(v: &str) -> bool {
    let p = |s: &str| -> Vec<u32> { s.trim_start_matches('v').split('.').map(|x| x.parse().unwrap_or(0)).collect() };
    p(v) > p(env!("CARGO_PKG_VERSION"))
}

fn dir(state: &AppState) -> PathBuf {
    state.paths.data.join("peer-updates")
}

/// What this PC can share: the signed manifest and its installer.
fn held(state: &AppState) -> Option<(Vec<u8>, Manifest, PathBuf)> {
    let raw = std::fs::read(dir(state).join("manifest.json")).ok()?;
    let m = manifest::accept(&raw, &sulcus_net::security_notice::built_in_keys()).ok()?;
    Some((raw, m, dir(state).join("setup.exe")))
}

/// Answers piece requests while sharing is on. Checked on every ask, so
/// turning the switch off stops sharing at once.
pub fn serve(state: &Arc<AppState>, node: &NodeHandle) {
    let st = state.clone();
    piece_wire::serve(node, move |sha, n| {
        if !settings(&st).share {
            return None;
        }
        let (raw, m, exe) = held(&st)?;
        if sha == piece_wire::MANIFEST {
            return Some(raw);
        }
        if sha != m.installer.sha256 {
            return None;
        }
        use std::io::{Read, Seek, SeekFrom};
        let mut f = std::fs::File::open(exe).ok()?;
        f.seek(SeekFrom::Start(n.checked_mul(PIECE as u64)?)).ok()?;
        let mut buf = Vec::with_capacity(PIECE);
        f.take(PIECE as u64).read_to_end(&mut buf).ok()?;
        Some(buf)
    });
}

/// Fetches the update a peer holds, checks it, keeps it for sharing, and
/// runs it the way the updater does (passive, restarting the app).
#[tauri::command]
pub async fn install_update_from_peer(state: AppStateRef<'_>, peer: String) -> Result<String, String> {
    if !settings(&state).from_peers {
        return Err("Turn on \"Updates from other PCs\" first.".into());
    }
    let node = crate::p2p::running().ok_or("Turn on Network first.")?;
    let peer: PeerId = peer.parse().map_err(|_| "That isn't a PC's id.".to_string())?;
    let raw = piece_wire::ask_manifest(&node, peer).await?;
    let m = manifest::accept(&raw, &sulcus_net::security_notice::built_in_keys()).map_err(|e| format!("The release manifest isn't signed by SulcusAI: {e}"))?;
    if !newer(&m.version) {
        return Err(format!("That PC has {}, which isn't newer than this one.", m.version));
    }
    state.log("network", &format!("Downloading SulcusAI {} from {peer}", m.version));
    let bytes = piece_wire::fetch(&node, peer, &m.installer).await?;
    check_installer(&m, &bytes, &updater_key()?)?;
    let d = dir(&state);
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let exe = d.join("setup.exe");
    std::fs::write(&exe, &bytes).map_err(|e| e.to_string())?;
    std::fs::write(d.join("manifest.json"), &raw).map_err(|e| e.to_string())?;
    // Read back and checked again: the file on disk is what runs.
    check_installer(&m, &std::fs::read(&exe).map_err(|e| e.to_string())?, &updater_key()?)?;
    state.log("settings", &format!("Installing SulcusAI {} from another PC", m.version));
    std::process::Command::new(&exe).args(["/P", "/R"]).spawn().map_err(|e| format!("The installer didn't start: {e}"))?;
    crate::crash::closing();
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_start_off_and_the_updater_key_reads() {
        assert_eq!(PeerUpdates::default(), PeerUpdates { from_peers: false, share: false });
        assert!(updater_key().is_ok());
    }

    #[test]
    fn an_installer_must_match_hash_and_minisign() {
        let bytes = b"not really an installer".to_vec();
        let m = Manifest { version: "99.0.0".into(), issued: 1, installer: manifest::describe("setup.exe", &bytes), installer_sig: B64.encode("untrusted comment: x\nRWQ=\n"), engines: vec![] };
        let key = updater_key().unwrap();
        let mut changed = bytes.clone();
        changed[0] ^= 1;
        assert!(check_installer(&m, &changed, &key).unwrap_err().contains("manifest"));
        // Right hash but a signature not made by the update key.
        assert!(check_installer(&m, &bytes, &key).is_err());
        assert!(newer("99.0.0") && !newer("0.0.1"));
    }
}
