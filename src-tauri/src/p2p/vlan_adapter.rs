// SPDX-License-Identifier: AGPL-3.0-only
//! The virtual LAN's network adapter (Wintun). While one network is on,
//! packets Windows sends to 10.77.0.0/16 go to its members over the node,
//! and packets from members come out of the adapter as if from the LAN.
//! wintun.dll is downloaded (pinned hash) and loaded only when this starts.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use sulcus_net::libp2p::PeerId;
use sulcus_net::sharing::vlan::Network;
use sulcus_net::sharing::wire::VlanLink;

pub const ADAPTER_NAME: &str = "SulcusAI LAN";

#[cfg(windows)]
struct On {
    net: String,
    session: Arc<wintun::Session>,
}

#[cfg(windows)]
static ON: Mutex<Option<On>> = Mutex::new(None);

/// Open sending streams, one per member, kept between packets.
static LINKS: tokio::sync::Mutex<Option<HashMap<PeerId, VlanLink>>> = tokio::sync::Mutex::const_new(None);

/// A DLL placed here by hand is used instead of the download.
pub fn dll_path(data: &Path) -> std::path::PathBuf {
    data.join("tools").join("wintun").join("wintun.dll")
}

/// The official signed build from wintun.net. Pinned so a changed file on
/// the server is refused rather than loaded into the app.
pub fn wintun_zip() -> crate::catalog::Asset {
    crate::catalog::Asset {
        url: "https://www.wintun.net/builds/wintun-0.14.1.zip".into(),
        size: 750_540,
        sha256: "07c256185d6ee3652e09fa55c0b673e2624b565e02c4b9091c79ca7d2f24ef51".into(),
    }
}

pub fn wintun_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86" => "x86",
        "arm" => "arm",
        _ => "amd64",
    }
}

/// The DLL, fetched the first time the virtual LAN is used.
pub async fn ensure_dll(paths: &crate::paths::Paths, data: &Path) -> Result<std::path::PathBuf, String> {
    let manual = dll_path(data);
    if manual.is_file() {
        return Ok(manual);
    }
    let dir = "wintun-0.14.1";
    let cancel = std::sync::atomic::AtomicBool::new(false);
    crate::engine::ensure_unpacked(paths, dir, &wintun_zip(), "wintun.dll", &cancel, |_, _| {}).await.map_err(|e| format!("Couldn't get wintun.dll from wintun.net: {e}"))?;
    let dll = paths.engines.join(dir).join("wintun").join("bin").join(wintun_arch()).join("wintun.dll");
    if dll.is_file() { Ok(dll) } else { Err("The Wintun download has no DLL for this PC.".into()) }
}

pub fn running() -> Option<String> {
    #[cfg(windows)]
    {
        ON.lock().unwrap().as_ref().map(|o| o.net.clone())
    }
    #[cfg(not(windows))]
    None
}

/// A packet from a member for this PC.
pub fn deliver(net: &str, packet: &[u8]) {
    #[cfg(windows)]
    {
        let g = ON.lock().unwrap();
        let Some(on) = g.as_ref().filter(|o| o.net == net) else { return };
        if let Ok(mut p) = on.session.allocate_send_packet(packet.len() as u16) {
            p.bytes_mut().copy_from_slice(packet);
            on.session.send_packet(p);
        }
    }
    #[cfg(not(windows))]
    let _ = (net, packet);
}

/// Sends a packet to its receivers in the network, reusing open streams.
pub async fn send(node: &sulcus_net::NodeHandle, n: &Network, packet: &[u8]) -> usize {
    let Ok(to) = n.route(&node.peer_id().to_string(), packet) else { return 0 };
    let mut g = LINKS.lock().await;
    let links = g.get_or_insert_with(HashMap::new);
    let mut sent = 0;
    for p in to.iter().filter_map(|p| p.parse::<PeerId>().ok()) {
        if !links.contains_key(&p) {
            match VlanLink::open(node, p, &n.id).await {
                Ok(l) => {
                    links.insert(p, l);
                }
                Err(_) => continue,
            }
        }
        if let Some(l) = links.get_mut(&p) {
            if l.send(packet).await.is_ok() {
                sent += 1;
            } else {
                links.remove(&p);
            }
        }
    }
    sent
}

/// Brings the adapter up for one network with this PC's address in it.
#[cfg(windows)]
pub fn start(dll: &Path, n: Network, me: &str) -> Result<(), String> {
    stop();
    let addr = *n.members.get(me).ok_or("You aren't a member of that network.")?;
    // Safety: the DLL is Wintun's own, hash-checked or placed by hand.
    let w = unsafe { wintun::load_from_path(dll) }.map_err(|e| format!("Wintun couldn't load: {e}"))?;
    let adapter = match wintun::Adapter::open(&w, ADAPTER_NAME) {
        Ok(a) => a,
        Err(_) => wintun::Adapter::create(&w, ADAPTER_NAME, "SulcusAI", None).map_err(|e| format!("The adapter couldn't be made (Windows asks for admin rights the first time): {e}"))?,
    };
    adapter.set_network_addresses_tuple(addr.into(), std::net::Ipv4Addr::new(255, 255, 0, 0).into(), None).map_err(|e| e.to_string())?;
    let session = Arc::new(adapter.start_session(wintun::MAX_RING_CAPACITY).map_err(|e| e.to_string())?);
    *ON.lock().unwrap() = Some(On { net: n.id.clone(), session: session.clone() });
    let rt = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        while let Ok(p) = session.receive_blocking() {
            let bytes = p.bytes().to_vec();
            let Some(node) = super::running() else { continue };
            let n = n.clone();
            rt.spawn(async move {
                send(&node, &n, &bytes).await;
            });
        }
    });
    Ok(())
}

#[cfg(not(windows))]
pub fn start(_: &Path, _: Network, _: &str) -> Result<(), String> {
    Err("The virtual LAN adapter is Windows-only for now.".into())
}

pub fn stop() {
    #[cfg(windows)]
    if let Some(on) = ON.lock().unwrap().take() {
        let _ = on.session.shutdown();
    }
    if let Ok(mut g) = LINKS.try_lock() {
        *g = None;
    }
}

#[cfg(test)]
mod wintun_tests {
    #[test]
    fn pinned_download_is_the_official_zip() {
        let z = super::wintun_zip();
        assert!(z.url.starts_with("https://www.wintun.net/"));
        assert_eq!(z.sha256.len(), 64);
        assert_eq!(super::wintun_arch(), if cfg!(target_arch = "x86_64") { "amd64" } else { super::wintun_arch() });
    }
}
