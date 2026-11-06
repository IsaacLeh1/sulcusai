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

/// Where packets for this PC go into the adapter: a session this copy
/// holds (it has admin rights) or the helper that holds it.
#[cfg(windows)]
enum Sink {
    Session(Arc<wintun::Session>),
    Helper(Mutex<std::net::TcpStream>),
}

#[cfg(windows)]
struct On {
    net: String,
    into: Sink,
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

/// SHA-256 of wintun.dll inside the pinned zip, per processor.
pub fn wintun_dll_sha256(arch: &str) -> &'static str {
    match arch {
        "arm64" => "f7ba89005544be9d85231a9e0d5f23b2d15b3311667e2dad0debd344918a3f80",
        "arm" => "daad267411ecdc70a0535e274d2c3e9da3d0084bdac7662cb8424dd4a031b4d9",
        "x86" => "d694fa46ab4cfebcb2632d094c7aa97278eef2f8052438621766d863ae98a931",
        _ => "e5da8447dc2c320edc0fc52fa01885c103de8c118481f683643cacc3220dafce",
    }
}

fn sha256_file(p: &Path) -> Option<String> {
    use sha2::Digest;
    Some(hex::encode(sha2::Sha256::digest(std::fs::read(p).ok()?)))
}

/// The DLL in <data>/tools/wintun, fetched from wintun.net the first time
/// the virtual LAN is used; a file there that doesn't match is replaced.
pub async fn ensure_dll(paths: &crate::paths::Paths, data: &Path) -> Result<std::path::PathBuf, String> {
    let dll = dll_path(data);
    let want = wintun_dll_sha256(wintun_arch());
    if sha256_file(&dll).as_deref() == Some(want) {
        return Ok(dll);
    }
    let dir = "wintun-0.14.1";
    let cancel = std::sync::atomic::AtomicBool::new(false);
    crate::engine::ensure_unpacked(paths, dir, &wintun_zip(), "wintun.dll", &cancel, |_, _| {}).await.map_err(|e| format!("Couldn't get wintun.dll from wintun.net: {e}"))?;
    let got = paths.engines.join(dir).join("wintun").join("bin").join(wintun_arch()).join("wintun.dll");
    if sha256_file(&got).as_deref() != Some(want) {
        return Err("The Wintun download didn't match the expected file, so it wasn't used.".into());
    }
    std::fs::create_dir_all(dll.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::copy(&got, &dll).map_err(|e| e.to_string())?;
    Ok(dll)
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
        match &on.into {
            Sink::Session(s) => {
                if let Ok(mut p) = s.allocate_send_packet(packet.len() as u16) {
                    p.bytes_mut().copy_from_slice(packet);
                    s.send_packet(p);
                }
            }
            Sink::Helper(h) => {
                let _ = super::lan_helper::write_frame(&mut *h.lock().unwrap(), super::lan_helper::PACKET, packet);
            }
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
    if !super::lan_helper::elevated() {
        return start_with_helper(dll.parent().ok_or("No Wintun folder.")?, n, addr);
    }
    // Safety: the DLL is Wintun's own, hash-checked.
    let w = unsafe { wintun::load_from_path(dll) }.map_err(|e| format!("Wintun couldn't load: {e}"))?;
    let adapter = match wintun::Adapter::open(&w, ADAPTER_NAME) {
        Ok(a) => a,
        Err(_) => wintun::Adapter::create(&w, ADAPTER_NAME, "SulcusAI", None).map_err(|e| format!("The adapter couldn't be made (Windows asks for admin rights the first time): {e}"))?,
    };
    adapter.set_network_addresses_tuple(addr.into(), std::net::Ipv4Addr::new(255, 255, 0, 0).into(), None).map_err(|e| e.to_string())?;
    let session = Arc::new(adapter.start_session(wintun::MAX_RING_CAPACITY).map_err(|e| e.to_string())?);
    *ON.lock().unwrap() = Some(On { net: n.id.clone(), into: Sink::Session(session.clone()) });
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

/// Without admin rights: the helper holds the adapter; packets go both ways
/// over its local connection.
#[cfg(windows)]
fn start_with_helper(dir: &Path, n: Network, addr: std::net::Ipv4Addr) -> Result<(), String> {
    use super::lan_helper::{read_frame, write_frame, CONFIG, NOTE, PACKET};
    let mut s = super::lan_helper::connect(dir)?;
    write_frame(&mut s, CONFIG, &serde_json::to_vec(&serde_json::json!({ "address": addr })).unwrap()).map_err(|e| e.to_string())?;
    match read_frame(&mut s) {
        Ok((NOTE, d)) if d == b"up" => {}
        Ok((NOTE, d)) => return Err(String::from_utf8_lossy(&d).into_owned()),
        _ => return Err("The LAN helper stopped answering.".into()),
    }
    let mut reader = s.try_clone().map_err(|e| e.to_string())?;
    *ON.lock().unwrap() = Some(On { net: n.id.clone(), into: Sink::Helper(Mutex::new(s)) });
    let rt = tokio::runtime::Handle::current();
    let net = n.id.clone();
    std::thread::spawn(move || {
        while let Ok((kind, bytes)) = read_frame(&mut reader) {
            if kind != PACKET {
                continue;
            }
            let Some(node) = super::running() else { continue };
            let n = n.clone();
            rt.spawn(async move {
                send(&node, &n, &bytes).await;
            });
        }
        // The helper went away: the network is off.
        let mut g = ON.lock().unwrap();
        if g.as_ref().is_some_and(|o| o.net == net) {
            *g = None;
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
        match on.into {
            Sink::Session(s) => {
                let _ = s.shutdown();
            }
            // Hanging up ends the helper, which closes the adapter.
            Sink::Helper(h) => {
                let _ = h.into_inner().map(|s| s.shutdown(std::net::Shutdown::Both));
            }
        }
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
        for a in ["amd64", "arm64", "arm", "x86"] {
            assert_eq!(super::wintun_dll_sha256(a).len(), 64);
        }
        assert!(super::dll_path(std::path::Path::new("d")).ends_with("tools/wintun/wintun.dll") || super::dll_path(std::path::Path::new("d")).ends_with("tools\\wintun\\wintun.dll"));
    }
}
