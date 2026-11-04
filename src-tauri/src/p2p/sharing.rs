// SPDX-License-Identifier: AGPL-3.0-only
//! Sharing and private networks in the app: share links this PC hosts,
//! model cards received from people, virtual LANs and the remote-control
//! switch. Every part has its own switch and all start off. The lists are
//! kept sealed with the data key; shared pieces stay sealed on disk under
//! their link's key.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sulcus_net::sharing::modelcard::{self, Book as CardBook, Card};
use sulcus_net::sharing::remote::{self, Host, Show};
use sulcus_net::sharing::share::{self, Link, ShareFile};
use sulcus_net::sharing::vlan::Network;
use sulcus_net::signed;

use super::invites::{load_sealed, save_sealed};
use crate::{db, AppState, AppStateRef};

const SETTINGS: &str = "p2p_sharing";
const SHARES: &str = "p2p_shares";
const CARDS: &str = "p2p_model_cards";
const NETS: &str = "p2p_vlans";
/// Whole-file sealing keeps the file in memory; bigger files wait for
/// streamed sealing.
const MAX_SHARE: u64 = 2 << 30;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SharingSettings {
    pub file_sharing: bool,
    pub model_cards: bool,
    pub virtual_lan: bool,
    pub remote_control: bool,
}

pub fn load(conn: &rusqlite::Connection) -> SharingSettings {
    db::get(conn, SETTINGS).unwrap_or_default()
}

static REMOTE: Mutex<Option<Host>> = Mutex::new(None);

fn with_host<T>(f: impl FnOnce(&mut Host) -> T) -> T {
    let mut g = REMOTE.lock().unwrap();
    f(g.get_or_insert_with(|| Host::new(false)))
}

#[tauri::command]
pub fn sharing_settings(state: AppStateRef) -> SharingSettings {
    load(&state.db.lock().unwrap())
}

#[tauri::command]
pub fn sharing_set(state: AppStateRef, s: SharingSettings) -> Result<SharingSettings, String> {
    db::set(&state.db.lock().unwrap(), SETTINGS, &s)?;
    with_host(|h| h.set_enabled(s.remote_control));
    Ok(s)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn my_peer(state: &AppState, c: &crate::crypto::Cipher) -> Result<sulcus_net::libp2p::identity::Keypair, String> {
    let conn = state.db.lock().unwrap();
    let key = super::keys::node_key(&conn, c)?;
    sulcus_net::identity::load(&key).map_err(|e| e.to_string())
}

// ---------- share links ----------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Made {
    pub link: String,
    pub file_text: String,
    pub pieces: usize,
}

/// Seals pieces into `dir/<id>/<n>`; returns the share file.
pub fn host_file(dir: &Path, name: &str, data: &[u8], host: &str) -> Result<ShareFile, String> {
    let key = share::new_key();
    let (m, sealed) = share::seal_file(name, data, &key, share::PIECE);
    let link = share::make_link(&m, &key, host);
    let out = dir.join(&link.id);
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    for (i, p) in sealed.iter().enumerate() {
        std::fs::write(out.join(i.to_string()), p).map_err(|e| e.to_string())?;
    }
    Ok(ShareFile { v: 1, link, manifest: m })
}

/// Reads a hosted piece for a peer that asked; only pieces of shares this
/// PC still lists are served.
pub fn hosted_piece(dir: &Path, shares: &[ShareFile], id: &str, index: u32) -> Option<Vec<u8>> {
    let f = shares.iter().find(|f| f.link.id == id)?;
    if index as usize >= f.manifest.pieces.len() {
        return None;
    }
    std::fs::read(dir.join(id).join(index.to_string())).ok()
}

/// Puts a download together from pieces already checked.
pub fn assemble(f: &ShareFile, pieces: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    let key = f.link.key_bytes().map_err(|_| "This link is damaged.")?;
    let mut out = Vec::with_capacity(f.manifest.size as usize);
    for (i, p) in pieces.iter().enumerate() {
        out.extend(share::open_piece(&f.manifest, &key, i as u32, p).map_err(|_| format!("Piece {i} did not check out."))?);
    }
    if out.len() as u64 != f.manifest.size {
        return Err("The download is incomplete.".into());
    }
    Ok(out)
}

fn shares_dir(state: &AppState) -> PathBuf {
    state.paths.data.join("shares")
}

#[tauri::command]
pub fn share_make(state: AppStateRef, path: String) -> Result<Made, String> {
    if !load(&state.db.lock().unwrap()).file_sharing {
        return Err("Turn on File sharing first.".into());
    }
    let c = state.cipher()?;
    let p = PathBuf::from(&path);
    let size = std::fs::metadata(&p).map_err(|e| e.to_string())?.len();
    if size > MAX_SHARE {
        return Err("Files over 2 GB can't be shared yet.".into());
    }
    let data = std::fs::read(&p).map_err(|e| e.to_string())?;
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
    let me = my_peer(&state, &c)?.public().to_peer_id().to_string();
    let f = host_file(&shares_dir(&state), &name, &data, &me)?;
    let conn = state.db.lock().unwrap();
    let mut list: Vec<ShareFile> = load_sealed(&conn, &c, SHARES);
    list.push(f.clone());
    save_sealed(&conn, &c, SHARES, &list)?;
    Ok(Made { link: f.link.to_text(), file_text: f.to_text(), pieces: f.manifest.pieces.len() })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hosted {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub link: String,
}

#[tauri::command]
pub fn share_list(state: AppStateRef) -> Result<Vec<Hosted>, String> {
    let c = state.cipher()?;
    let list: Vec<ShareFile> = load_sealed(&state.db.lock().unwrap(), &c, SHARES);
    Ok(list.into_iter().map(|f| Hosted { id: f.link.id.clone(), name: f.link.name.clone(), size: f.link.size, link: f.link.to_text() }).collect())
}

/// Stops hosting a share and deletes its sealed pieces.
#[tauri::command]
pub fn share_stop(state: AppStateRef, id: String) -> Result<(), String> {
    let c = state.cipher()?;
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Unknown share.".into());
    }
    let conn = state.db.lock().unwrap();
    let mut list: Vec<ShareFile> = load_sealed(&conn, &c, SHARES);
    list.retain(|f| f.link.id != id);
    save_sealed(&conn, &c, SHARES, &list)?;
    let _ = std::fs::remove_dir_all(shares_dir(&state).join(&id));
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkInfo {
    pub name: String,
    pub size: u64,
    pub host: String,
}

/// Reads a `sulcus://share` link or `.sulcus` file text.
#[tauri::command]
pub fn share_read(text: String) -> Result<LinkInfo, String> {
    let link = if text.trim_start().starts_with('{') {
        ShareFile::parse(&text).map_err(|_| "This .sulcus file is damaged or was changed.")?.link
    } else {
        Link::parse(&text).map_err(|_| "This isn't a SulcusAI share link.")?
    };
    Ok(LinkInfo { name: link.name, size: link.size, host: link.host })
}

// ---------- model cards ----------

#[tauri::command]
pub fn model_card_sign(state: AppStateRef, mut card: Card) -> Result<String, String> {
    if !load(&state.db.lock().unwrap()).model_cards {
        return Err("Turn on Community models first.".into());
    }
    let c = state.cipher()?;
    card.issued = signed::now();
    let text = modelcard::sign(&my_peer(&state, &c)?, &card);
    let conn = state.db.lock().unwrap();
    let mut book: CardBook = load_sealed(&conn, &c, CARDS);
    book.add(&text).map_err(|_| "That card could not be signed.")?;
    save_sealed(&conn, &c, CARDS, &book)?;
    Ok(text)
}

#[tauri::command]
pub fn model_card_add(state: AppStateRef, text: String) -> Result<CardBook, String> {
    if !load(&state.db.lock().unwrap()).model_cards {
        return Err("Turn on Community models first.".into());
    }
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut book: CardBook = load_sealed(&conn, &c, CARDS);
    for t in text.split_whitespace() {
        book.add(t).map_err(|e| match e {
            modelcard::Bad::Signature => "A card's signature doesn't match; it was changed after signing.",
            modelcard::Bad::Format => "This isn't a SulcusAI model card.",
        })?;
    }
    save_sealed(&conn, &c, CARDS, &book)?;
    Ok(book)
}

#[tauri::command]
pub fn model_card_list(state: AppStateRef) -> Result<CardBook, String> {
    let c = state.cipher()?;
    Ok(load_sealed(&state.db.lock().unwrap(), &c, CARDS))
}

#[tauri::command]
pub fn model_card_forget(state: AppStateRef, sha256: String) -> Result<(), String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut book: CardBook = load_sealed(&conn, &c, CARDS);
    book.forget(&sha256);
    save_sealed(&conn, &c, CARDS, &book)
}

// ---------- virtual LAN ----------

#[tauri::command]
pub fn vlan_create(state: AppStateRef, name: String) -> Result<Network, String> {
    if !load(&state.db.lock().unwrap()).virtual_lan {
        return Err("Turn on Virtual LAN first.".into());
    }
    let c = state.cipher()?;
    let me = my_peer(&state, &c)?.public().to_peer_id().to_string();
    let id = hex::encode(share::new_key())[..16].to_string();
    let n = Network::create(&id, name.trim(), &me);
    let conn = state.db.lock().unwrap();
    let mut nets: Vec<Network> = load_sealed(&conn, &c, NETS);
    nets.push(n.clone());
    save_sealed(&conn, &c, NETS, &nets)?;
    Ok(n)
}

#[tauri::command]
pub fn vlan_list(state: AppStateRef) -> Result<Vec<Network>, String> {
    let c = state.cipher()?;
    Ok(load_sealed(&state.db.lock().unwrap(), &c, NETS))
}

#[tauri::command]
pub fn vlan_admit(state: AppStateRef, id: String, peer: String) -> Result<Network, String> {
    let c = state.cipher()?;
    let me = my_peer(&state, &c)?.public().to_peer_id().to_string();
    let conn = state.db.lock().unwrap();
    let mut nets: Vec<Network> = load_sealed(&conn, &c, NETS);
    let n = nets.iter_mut().find(|n| n.id == id).ok_or("Unknown network.")?;
    n.admit(&me, &peer).map_err(|e| format!("{e:?}"))?;
    let out = n.clone();
    save_sealed(&conn, &c, NETS, &nets)?;
    Ok(out)
}

#[tauri::command]
pub fn vlan_leave(state: AppStateRef, id: String, peer: Option<String>) -> Result<(), String> {
    let c = state.cipher()?;
    let me = my_peer(&state, &c)?.public().to_peer_id().to_string();
    let conn = state.db.lock().unwrap();
    let mut nets: Vec<Network> = load_sealed(&conn, &c, NETS);
    let who = peer.unwrap_or_else(|| me.clone());
    if let Some(n) = nets.iter_mut().find(|n| n.id == id) {
        n.remove(&me, &who).map_err(|e| format!("{e:?}"))?;
    }
    nets.retain(|n| !(n.id == id && (who == me || n.members.is_empty())));
    save_sealed(&conn, &c, NETS, &nets)
}

// ---------- remote control ----------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    pub enabled: bool,
    pub state: remote::State,
    pub show: Show,
}

fn status() -> RemoteStatus {
    with_host(|h| {
        h.tick(now_ms());
        RemoteStatus { enabled: h.enabled, state: h.state().clone(), show: h.show() }
    })
}

#[tauri::command]
pub fn remote_status() -> RemoteStatus {
    status()
}

#[tauri::command]
pub fn remote_accept(code: String) -> Result<RemoteStatus, String> {
    with_host(|h| h.accept(code.trim(), now_ms())).map_err(|e| match e {
        remote::Refused::TimedOut => "The request lapsed after 10 seconds.".to_string(),
        remote::Refused::WrongCode => "The codes didn't match, so the request was declined.".to_string(),
        e => format!("{e:?}"),
    })?;
    Ok(status())
}

/// Stop on the bar, Decline on the prompt and the kill hotkey all land here.
#[tauri::command]
pub fn remote_stop() -> RemoteStatus {
    with_host(|h| h.stop());
    status()
}

/// Keeps the remote host in step with the saved switch at startup.
pub fn init(state: &AppState) {
    let on = load(&state.db.lock().unwrap()).remote_control;
    with_host(|h| h.set_enabled(on));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_starts_off() {
        let s = SharingSettings::default();
        assert!(!s.file_sharing && !s.model_cards && !s.virtual_lan && !s.remote_control);
        let s: SharingSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, SharingSettings::default());
    }

    #[test]
    fn hosted_pieces_download_and_check() {
        let dir = std::env::temp_dir().join(format!("sulcus-share-{}", std::process::id()));
        let data: Vec<u8> = (0..(share::PIECE + 77)).map(|i| (i % 253) as u8).collect();
        let f = host_file(&dir, "a.bin", &data, "me").unwrap();
        let list = vec![f.clone()];
        let got: Vec<Vec<u8>> = (0..f.manifest.pieces.len() as u32).map(|i| hosted_piece(&dir, &list, &f.link.id, i).unwrap()).collect();
        assert_eq!(got.len(), 2);
        assert_eq!(assemble(&f, &got).unwrap(), data);
        assert!(hosted_piece(&dir, &list, &f.link.id, 9).is_none());
        assert!(hosted_piece(&dir, &[], &f.link.id, 0).is_none());
        let mut bad = got.clone();
        bad[1][0] ^= 1;
        assert!(assemble(&f, &bad).is_err());
        let info = share_read(f.link.to_text()).unwrap();
        assert_eq!((info.name.as_str(), info.size), ("a.bin", data.len() as u64));
        assert!(share_read(f.to_text()).is_ok());
        assert!(share_read("hello".into()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remote_is_off_until_switched_on() {
        with_host(|h| h.set_enabled(false));
        assert!(!remote_status().enabled);
        assert!(with_host(|h| h.request("c", "1", now_ms())).is_err());
        with_host(|h| h.set_enabled(true));
        with_host(|h| h.request("c", "123456", now_ms())).unwrap();
        assert!(remote_status().show.prompt);
        assert!(remote_accept("000000".into()).is_err());
        with_host(|h| h.request("c", "123456", now_ms())).unwrap();
        assert!(remote_accept("123456".into()).unwrap().show.bar);
        assert!(!remote_stop().show.border);
        with_host(|h| h.set_enabled(false));
    }
}
