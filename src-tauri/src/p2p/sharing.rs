// SPDX-License-Identifier: AGPL-3.0-only
//! Sharing and private networks in the app: share links this PC hosts,
//! model cards received from people, virtual LANs and the remote-control
//! switch. Every part has its own switch and all start off. The lists are
//! kept sealed with the data key; shared pieces stay sealed on disk under
//! their link's key.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use sulcus_net::libp2p::PeerId;
use sulcus_net::sharing::wire::{self, Input, Pieces, RemoteLink};
use sulcus_net::NodeHandle;
use tauri::{AppHandle, Emitter};

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

static REMOTE: OnceLock<Arc<Mutex<Host>>> = OnceLock::new();
static APP: OnceLock<AppHandle> = OnceLock::new();
/// Ends a live remote session from the keyboard, whatever has the focus.
pub const KILL_HOTKEY: &str = "ctrl+alt+Backspace";

fn host() -> Arc<Mutex<Host>> {
    REMOTE.get_or_init(|| Arc::new(Mutex::new(Host::new(false)))).clone()
}

fn with_host<T>(f: impl FnOnce(&mut Host) -> T) -> T {
    let h = host();
    let mut g = h.lock().unwrap();
    let was_live = g.show().border;
    let out = f(&mut g);
    let live = g.show().border;
    drop(g);
    if was_live != live {
        kill_hotkey(live);
    }
    out
}

fn kill_hotkey(on: bool) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let Some(app) = APP.get() else { return };
    let gs = app.global_shortcut();
    if on {
        if !gs.is_registered(KILL_HOTKEY) {
            let _ = gs.register(KILL_HOTKEY);
        }
    } else {
        let _ = gs.unregister(KILL_HOTKEY);
    }
    screen_frame(app, on);
    let _ = app.emit("remote:changed", ());
}

/// While controlled: a red border round the whole screen (clicks pass
/// through it) and a bar with Stop at the top, both above every window and
/// both left out of the pictures sent to the controller.
fn screen_frame(app: &AppHandle, on: bool) {
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
    if !on {
        for l in ["remote-border", "remote-bar"] {
            if let Some(w) = app.get_webview_window(l) {
                let _ = w.close();
            }
        }
        return;
    }
    let Some(m) = app.primary_monitor().ok().flatten() else { return };
    let (w, h) = (m.size().width as f64 / m.scale_factor(), m.size().height as f64 / m.scale_factor());
    let base = |label: &str| {
        WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
            .additional_browser_args(crate::rest::BROWSER_ARGS)
            .title("SulcusAI remote control")
            .decorations(false)
            .resizable(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .focused(false)
            .shadow(false)
    };
    if app.get_webview_window("remote-border").is_none() {
        if let Ok(win) = base("remote-border").transparent(true).inner_size(w, h).position(0.0, 0.0).build() {
            let _ = win.set_ignore_cursor_events(true);
            crate::programs::banner_ready(&win);
        }
    }
    if app.get_webview_window("remote-bar").is_none() {
        let bw = 460.0;
        if let Ok(win) = base("remote-bar").inner_size(bw, 44.0).position(((w - bw) / 2.0).max(0.0), 6.0).build() {
            crate::programs::banner_ready(&win);
        }
    }
}

pub fn is_kill_hotkey(s: &tauri_plugin_global_shortcut::Shortcut) -> bool {
    KILL_HOTKEY.parse::<tauri_plugin_global_shortcut::Shortcut>().is_ok_and(|h| h.id() == s.id())
}

/// The kill hotkey's press: ends any session at once.
pub fn kill() {
    with_host(|h| h.stop());
}

#[tauri::command]
pub fn sharing_settings(state: AppStateRef) -> SharingSettings {
    load(&state.db.lock().unwrap())
}

#[tauri::command]
pub fn sharing_set(state: AppStateRef, s: SharingSettings) -> Result<SharingSettings, String> {
    let before = load(&state.db.lock().unwrap());
    db::set(&state.db.lock().unwrap(), SETTINGS, &s)?;
    with_host(|h| h.set_enabled(s.remote_control));
    if before.file_sharing != s.file_sharing {
        super::open_links::register(s.file_sharing);
    }
    if !s.virtual_lan {
        super::vlan_adapter::stop();
    }
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

/// The virtual LAN switch, for parts outside Sharing that use networks.
pub(crate) fn lan_on(state: &AppState) -> Result<(), String> {
    if load(&state.db.lock().unwrap()).virtual_lan { Ok(()) } else { Err("Turn on Virtual LAN in Settings > Sharing first.".into()) }
}

pub(crate) fn my_peer_id(state: &AppState) -> Result<String, String> {
    let c = state.cipher()?;
    Ok(my_peer(state, &c)?.public().to_peer_id().to_string())
}

pub(crate) fn networks(state: &AppState) -> Result<Vec<Network>, String> {
    let c = state.cipher()?;
    Ok(load_sealed(&state.db.lock().unwrap(), &c, NETS))
}

/// Changes the saved networks in one step.
pub(crate) fn update_networks<R>(state: &AppState, f: impl FnOnce(&mut Vec<Network>) -> Result<R, String>) -> Result<R, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut nets: Vec<Network> = load_sealed(&conn, &c, NETS);
    let out = f(&mut nets)?;
    save_sealed(&conn, &c, NETS, &nets)?;
    Ok(out)
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

/// Seals a downloaded file again under its link's key; the same key and
/// piece numbers give the same pieces, so the manifest must come out equal.
pub fn rehost(dir: &Path, data: &[u8], link: &Link, m: &share::Manifest) -> Result<ShareFile, String> {
    let key = link.key_bytes().map_err(|_| "damaged link")?;
    let (again, sealed) = share::seal_file(&m.name, data, &key, m.piece_size as usize);
    if again != *m {
        return Err("The pieces came out different.".into());
    }
    let out = dir.join(&link.id);
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    for (i, p) in sealed.iter().enumerate() {
        std::fs::write(out.join(i.to_string()), p).map_err(|e| e.to_string())?;
    }
    Ok(ShareFile { v: 1, link: link.clone(), manifest: again })
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
    drop(conn);
    push_roster(&state, &out, &[]);
    Ok(out)
}

#[tauri::command]
pub fn vlan_leave(state: AppStateRef, id: String, peer: Option<String>) -> Result<(), String> {
    let c = state.cipher()?;
    let me = my_peer(&state, &c)?.public().to_peer_id().to_string();
    let conn = state.db.lock().unwrap();
    let mut nets: Vec<Network> = load_sealed(&conn, &c, NETS);
    let who = peer.unwrap_or_else(|| me.clone());
    let mut changed = None;
    if let Some(n) = nets.iter_mut().find(|n| n.id == id) {
        n.remove(&me, &who).map_err(|e| format!("{e:?}"))?;
        changed = Some(n.clone());
    }
    nets.retain(|n| !(n.id == id && (who == me || n.members.is_empty())));
    save_sealed(&conn, &c, NETS, &nets)?;
    drop(conn);
    // An admin's change reaches the others, and the person removed learns it too.
    if let Some(n) = changed.filter(|n| n.admins.contains(&me)) {
        push_roster(&state, &n, &[who]);
    }
    Ok(())
}

/// Signs a network's member list and sends it to its members (and `also`).
pub(crate) fn push_roster(state: &AppState, n: &Network, also: &[String]) {
    let Ok(c) = state.cipher() else { return };
    let Ok(key) = my_peer(state, &c) else { return };
    let Some(node) = super::running() else { return };
    let me = key.public().to_peer_id().to_string();
    let text = sulcus_net::sharing::roster::seal(&key, n);
    let to: Vec<PeerId> = n.members.keys().chain(also.iter()).filter(|p| **p != me).filter_map(|p| p.parse().ok()).collect();
    tauri::async_runtime::spawn(async move {
        for p in to {
            let _ = wire::send_roster(&node, p, &text).await;
        }
    });
}

/// A member list from another member: kept if an admin signed it and it is newer.
pub fn take_roster(nets: &mut Vec<Network>, text: &str, me: &str) -> bool {
    use sulcus_net::sharing::roster;
    let Some(id) = roster::network_id(text) else { return false };
    let at = nets.iter().position(|n| n.id == id);
    let Some(n) = roster::accept(at.map(|i| &nets[i]), text, me) else { return false };
    let still_in = n.members.contains_key(me);
    match at {
        Some(i) if still_in => nets[i] = n,
        Some(i) => {
            nets.remove(i);
        }
        None => nets.push(n),
    }
    true
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
        remote::Refused::TimedOut => "The request lapsed after 30 seconds.".to_string(),
        remote::Refused::WrongCode => "The codes didn't match, so the request was declined.".to_string(),
        e => format!("{e:?}"),
    })?;
    Ok(status())
}

/// Remote control from a group call: the same switch, border, Stop bar and
/// kill hotkey as from a friend; the "code" is the asker's name, which the
/// person at this PC types to allow it.
fn name_code(name: &str) -> String {
    name.trim().to_lowercase()
}

pub(crate) fn call_request(controller: &str, name: &str) -> Result<(), String> {
    with_host(|h| h.request(controller, &name_code(name), now_ms())).map_err(|e| match e {
        remote::Refused::SwitchOff => "Remote control is off on their PC.".to_string(),
        remote::Refused::Busy => "Someone else is already asking for or using control of their PC.".to_string(),
        e => format!("{e:?}"),
    })
}

/// Allowed only when the typed name matches the asker's; returns who it is.
pub(crate) fn call_accept(typed: &str) -> Result<String, String> {
    let controller = match with_host(|h| h.state().clone()) {
        remote::State::Asking { controller, .. } => controller,
        _ => return Err("The request lapsed after 30 seconds.".into()),
    };
    with_host(|h| h.accept(&name_code(typed), now_ms())).map_err(|e| match e {
        remote::Refused::TimedOut => "The request lapsed after 30 seconds.".to_string(),
        remote::Refused::WrongCode => "That name didn't match, so the request was declined.".to_string(),
        e => format!("{e:?}"),
    })?;
    Ok(controller)
}

pub(crate) fn call_may_input(from: &str) -> bool {
    with_host(|h| h.may_input(from))
}

/// Who is asking for or has control now.
pub(crate) fn call_controller() -> Option<String> {
    match with_host(|h| h.state().clone()) {
        remote::State::Asking { controller, .. } | remote::State::Live { controller, .. } => Some(controller),
        remote::State::Idle => None,
    }
}

pub(crate) fn call_stop() {
    with_host(|h| h.stop());
}

/// Stop on the bar, Decline on the prompt and the kill hotkey all land here.
#[tauri::command]
pub fn remote_stop() -> RemoteStatus {
    with_host(|h| h.stop());
    status()
}

/// Keeps the remote host in step with the saved switch at startup.
pub fn init(app: &AppHandle, state: &AppState) {
    let _ = APP.set(app.clone());
    let on = load(&state.db.lock().unwrap()).remote_control;
    with_host(|h| h.set_enabled(on));
}

// ---------- over the node ----------

fn friends(state: &AppState) -> Vec<String> {
    let Ok(c) = state.work_cipher() else { return Vec::new() };
    let list: Vec<super::invites::Friend> = load_sealed(&state.db.lock().unwrap(), &c, super::invites::FRIENDS);
    list.into_iter().map(|f| f.peer).collect()
}

fn friend_with(state: &AppState, peer: &PeerId, on: impl Fn(&SharingSettings) -> bool) -> bool {
    on(&load(&state.db.lock().unwrap())) && friends(state).contains(&peer.to_string())
}

struct AppPieces(Arc<AppState>);

impl AppPieces {
    fn list(&self) -> Vec<ShareFile> {
        let Ok(c) = self.0.work_cipher() else { return Vec::new() };
        load_sealed(&self.0.db.lock().unwrap(), &c, SHARES)
    }
}

impl Pieces for AppPieces {
    fn manifest(&self, id: &str) -> Option<share::Manifest> {
        self.list().into_iter().find(|f| f.link.id == id).map(|f| f.manifest)
    }
    fn have(&self, id: &str) -> Vec<u32> {
        let dir = shares_dir(&self.0).join(id);
        self.list().into_iter().find(|f| f.link.id == id).map(|f| (0..f.manifest.pieces.len() as u32).filter(|i| dir.join(i.to_string()).exists()).collect()).unwrap_or_default()
    }
    fn piece(&self, id: &str, index: u32) -> Option<Vec<u8>> {
        hosted_piece(&shares_dir(&self.0), &self.list(), id, index)
    }
}

/// Takes a packet if the sender is a member sending from its own address
/// and this PC is one of the packet's receivers.
pub fn vlan_accept(nets: &[Network], me: &str, net: &str, from: &str, packet: &[u8]) -> bool {
    nets.iter().find(|n| n.id == net).and_then(|n| n.route(from, packet).ok()).is_some_and(|to| to.iter().any(|p| p == me))
}

/// Starts answering sharing requests on a new node. Each request checks
/// its switch and that the asker is a friend at the time it arrives.
pub fn serve(state: &Arc<AppState>, node: &NodeHandle) {
    let s = state.clone();
    wire::serve_share(node, Arc::new(AppPieces(state.clone())), move |p| friend_with(&s, p, |x| x.file_sharing));
    let s = state.clone();
    wire::serve_cards(node, move |p| {
        if !friend_with(&s, p, |x| x.model_cards) {
            return None;
        }
        let c = s.work_cipher().ok()?;
        let book: CardBook = load_sealed(&s.db.lock().unwrap(), &c, CARDS);
        Some(book.export())
    });
    let (s, me) = (state.clone(), node.peer_id().to_string());
    wire::serve_vlan(node, move |net, from, packet| {
        if !load(&s.db.lock().unwrap()).virtual_lan {
            return;
        }
        let Ok(c) = s.work_cipher() else { return };
        let nets: Vec<Network> = load_sealed(&s.db.lock().unwrap(), &c, NETS);
        if vlan_accept(&nets, &me, net, &from.to_string(), &packet) {
            super::vlan_adapter::deliver(net, &packet);
        }
    });
    let (s, me) = (state.clone(), node.peer_id().to_string());
    wire::serve_roster(node, move |_, text| {
        if !load(&s.db.lock().unwrap()).virtual_lan {
            return;
        }
        let Ok(c) = s.work_cipher() else { return };
        let conn = s.db.lock().unwrap();
        let mut nets: Vec<Network> = load_sealed(&conn, &c, NETS);
        if take_roster(&mut nets, &text, &me) {
            let _ = save_sealed(&conn, &c, NETS, &nets);
            if let Some(app) = APP.get() {
                let _ = app.emit("social:networks", ());
            }
        }
    });
    let s = state.clone();
    wire::serve_remote(
        node,
        host(),
        now_ms,
        move |peer, code| {
            // Only friends can even ask; anyone else's request lapses unseen.
            if !friends(&s).contains(&peer.to_string()) {
                with_host(|h| h.stop());
                return;
            }
            if let Some(app) = APP.get() {
                let _ = app.emit("remote:asked", serde_json::json!({ "peer": peer.to_string(), "code": code }));
            }
        },
        super::remote_io::apply,
        super::remote_io::frame,
    );
}

/// Downloads a share from its host and any connected friends who have it.
#[tauri::command]
pub async fn share_fetch(state: AppStateRef<'_>, text: String, save_dir: String) -> Result<String, String> {
    if !load(&state.db.lock().unwrap()).file_sharing {
        return Err("Turn on File sharing first.".into());
    }
    let node = super::running().ok_or("Turn on Network first.")?;
    let link = if text.trim_start().starts_with('{') { ShareFile::parse(&text).map_err(|_| "This .sulcus file is damaged.")?.link } else { Link::parse(&text).map_err(|_| "This isn't a SulcusAI share link.")? };
    let mut sources: Vec<PeerId> = link.host.parse().into_iter().collect();
    for p in node.connected() {
        if !sources.contains(&p) {
            sources.push(p);
        }
    }
    let app = APP.get().cloned();
    let id = link.id.clone();
    let (m, data) = wire::fetch(&node, &link, &sources, |d, t| {
        if let Some(a) = &app {
            let _ = a.emit("share:progress", serde_json::json!({ "id": id, "done": d, "total": t }));
        }
    })
    .await?;
    let name = Path::new(&m.name).file_name().and_then(|n| n.to_str()).filter(|n| !n.starts_with('.')).unwrap_or("download").to_string();
    let out = Path::new(&save_dir).join(name);
    std::fs::write(&out, &data).map_err(|e| e.to_string())?;
    // Kept sealed so this PC becomes one more source for the friends it shares with.
    if let Ok(f) = rehost(&shares_dir(&state), &data, &link, &m) {
        if let Ok(c) = state.cipher() {
            let conn = state.db.lock().unwrap();
            let mut list: Vec<ShareFile> = load_sealed(&conn, &c, SHARES);
            if !list.iter().any(|x| x.link.id == f.link.id) {
                list.push(f);
                let _ = save_sealed(&conn, &c, SHARES, &list);
            }
        }
    }
    Ok(out.to_string_lossy().into_owned())
}

/// Asks a friend for the model cards they pass on, and adds them here.
#[tauri::command]
pub async fn model_card_fetch(state: AppStateRef<'_>, peer: String) -> Result<CardBook, String> {
    if !load(&state.db.lock().unwrap()).model_cards {
        return Err("Turn on Community models first.".into());
    }
    let node = super::running().ok_or("Turn on Network first.")?;
    let pid: PeerId = peer.parse().map_err(|_| "Unknown person.")?;
    let texts = wire::fetch_cards(&node, pid).await?;
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut book: CardBook = load_sealed(&conn, &c, CARDS);
    for t in texts.iter().take(500) {
        let _ = book.add(t);
    }
    save_sealed(&conn, &c, CARDS, &book)?;
    Ok(book)
}

/// Turns this PC's adapter on for one network (one at a time).
#[tauri::command]
pub async fn vlan_connect(state: AppStateRef<'_>, id: String) -> Result<String, String> {
    if !load(&state.db.lock().unwrap()).virtual_lan {
        return Err("Turn on Virtual LAN first.".into());
    }
    let node = super::running().ok_or("Turn on Network first.")?;
    let c = state.cipher()?;
    let nets: Vec<Network> = load_sealed(&state.db.lock().unwrap(), &c, NETS);
    let n = nets.into_iter().find(|n| n.id == id).ok_or("Unknown network.")?;
    let me = node.peer_id().to_string();
    let addr = n.members.get(&me).map(|a| a.to_string()).unwrap_or_default();
    #[cfg(windows)]
    let dll = super::vlan_adapter::ensure_dll(&state.paths, &state.paths.data).await?;
    #[cfg(not(windows))]
    let dll = std::path::PathBuf::new();
    super::vlan_adapter::start(&dll, n, &me)?;
    if let Some(app) = APP.get() {
        crate::lan_tray::refresh(app);
    }
    Ok(addr)
}

#[tauri::command]
pub fn vlan_disconnect() {
    super::vlan_adapter::stop();
    if let Some(app) = APP.get() {
        crate::lan_tray::refresh(app);
    }
}

/// The network whose adapter is on, if any.
#[tauri::command]
pub fn vlan_status() -> Option<String> {
    super::vlan_adapter::running()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Vpn {
    pub adapter: String,
    pub kind: String,
    pub up: bool,
}

/// Every VPN adapter on this PC, SulcusAI's own included, in one list.
#[tauri::command]
pub fn vpn_list() -> Vec<Vpn> {
    adapters().into_iter().filter_map(|(name, desc, up)| {
        let kind = sulcus_net::sharing::vlan::vpn_kind(&name).or_else(|| sulcus_net::sharing::vlan::vpn_kind(&desc))?;
        Some(Vpn { adapter: name, kind: kind.to_string(), up })
    }).collect()
}

/// (friendly name, description, connected) for each network adapter.
#[cfg(windows)]
fn adapters() -> Vec<(String, String, bool)> {
    use windows::Win32::NetworkManagement::IpHelper::{GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH};
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    let mut size = 32_000u32;
    let mut buf = vec![0u8; size as usize];
    let mut out = Vec::new();
    unsafe {
        let first = buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
        if GetAdaptersAddresses(0, GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST, None, Some(first), &mut size) != 0 {
            return out;
        }
        let mut p = first;
        while !p.is_null() {
            let a = &*p;
            let name = a.FriendlyName.to_string().unwrap_or_default();
            let desc = a.Description.to_string().unwrap_or_default();
            out.push((name, desc, a.OperStatus == IfOperStatusUp));
            p = a.Next;
        }
    }
    out
}

#[cfg(not(windows))]
fn adapters() -> Vec<(String, String, bool)> {
    Vec::new()
}

static LINK: tokio::sync::Mutex<Option<RemoteLink>> = tokio::sync::Mutex::const_new(None);

/// Controls a friend's PC: shows the code to read out, waits for Accept,
/// then streams their screen as `remote:frame` events.
#[tauri::command]
pub async fn remote_connect(peer: String) -> Result<String, String> {
    let node = super::running().ok_or("Turn on Network first.")?;
    let pid: PeerId = peer.parse().map_err(|_| "Unknown person.")?;
    let mut nonce = [0u8; 16];
    nonce.copy_from_slice(&share::new_key()[..16]);
    let code = remote::matching_code(&node.peer_id().to_string(), &peer, &nonce);
    if let Some(app) = APP.get() {
        let _ = app.emit("remote:code", serde_json::json!({ "code": code }));
    }
    let mut link = RemoteLink::ask(&node, pid, &code).await?;
    let mut frames = std::mem::replace(&mut link.frames, tokio::sync::mpsc::channel(1).1);
    *LINK.lock().await = Some(link);
    tokio::spawn(async move {
        use base64::Engine;
        while let Some(f) = frames.recv().await {
            if let Some(app) = APP.get() {
                let _ = app.emit("remote:frame", base64::engine::general_purpose::STANDARD.encode(f));
            }
        }
        *LINK.lock().await = None;
        if let Some(app) = APP.get() {
            let _ = app.emit("remote:ended", ());
        }
    });
    Ok(code)
}

#[tauri::command]
pub async fn remote_send(input: Input) -> Result<(), String> {
    let mut g = LINK.lock().await;
    let l = g.as_mut().ok_or("Not connected.")?;
    l.send(&input).await
}

#[tauri::command]
pub async fn remote_disconnect() {
    let link = LINK.lock().await.take();
    if let Some(l) = link {
        l.close().await;
    }
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
        let other = dir.join("again");
        let again = rehost(&other, &data, &f.link, &f.manifest).unwrap();
        assert_eq!(std::fs::read(other.join(&f.link.id).join("1")).unwrap(), got[1]);
        assert_eq!(again.manifest, f.manifest);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn vlan_takes_only_packets_meant_for_this_pc() {
        let mut n = Network::create("n1", "x", "me");
        let a = n.admit("me", "ana").unwrap();
        let mine = n.members["me"];
        let mut p = vec![0u8; 20];
        p[0] = 0x45;
        p[12..16].copy_from_slice(&a.octets());
        p[16..20].copy_from_slice(&mine.octets());
        let nets = vec![n];
        assert!(vlan_accept(&nets, "me", "n1", "ana", &p));
        assert!(!vlan_accept(&nets, "me", "n2", "ana", &p));
        assert!(!vlan_accept(&nets, "me", "n1", "eve", &p));
        p[12] = 9;
        assert!(!vlan_accept(&nets, "me", "n1", "ana", &p), "spoofed sender");
    }

    #[test]
    fn member_lists_arrive_join_and_remove() {
        let admin = sulcus_net::libp2p::identity::Keypair::generate_ed25519();
        let a = admin.public().to_peer_id().to_string();
        let mut n = Network::create("n1", "x", &a);
        n.admit(&a, "bob").unwrap();
        let mut nets = Vec::new();
        assert!(take_roster(&mut nets, &sulcus_net::sharing::roster::seal(&admin, &n), "bob"));
        assert_eq!(nets.len(), 1);
        n.remove(&a, "bob").unwrap();
        assert!(take_roster(&mut nets, &sulcus_net::sharing::roster::seal(&admin, &n), "bob"));
        assert!(nets.is_empty(), "removed: the network goes");
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
