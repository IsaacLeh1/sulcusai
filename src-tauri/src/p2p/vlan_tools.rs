// SPDX-License-Identifier: AGPL-3.0-only
//! What a network's member rows can do beyond the address: whether the link
//! is direct or relayed (and why), a speed test and file send over the
//! virtual LAN, remote desktop, Wake-on-LAN, LAN games found running,
//! firewall rules per network, and what this PC shares on it.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// One port for the speed test and file send, inside the virtual LAN only.
pub const PORT: u16 = 47790;
const SPEED_BYTES: u64 = 16 * 1024 * 1024;

fn hidden(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

fn output(cmd: &str, args: &[&str]) -> String {
    hidden(std::process::Command::new(cmd).args(args)).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

// ---------- direct or relayed ----------

#[derive(Debug, Clone, Serialize)]
pub struct PathInfo {
    pub relayed: bool,
    pub why: String,
}

fn paths() -> &'static Mutex<HashMap<String, PathInfo>> {
    static P: OnceLock<Mutex<HashMap<String, PathInfo>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

pub fn path_of(peer: &str) -> Option<PathInfo> {
    paths().lock().unwrap().get(peer).cloned()
}

fn private(addr: &libp2p::Multiaddr) -> bool {
    addr.iter().any(|p| match p {
        libp2p::multiaddr::Protocol::Ip4(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
        libp2p::multiaddr::Protocol::Ip6(ip) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
        _ => false,
    })
}

/// Follows the running node's connections; a node restart gets a new watcher.
pub fn watch_paths() {
    static WATCHING: Mutex<Option<String>> = Mutex::new(None);
    let Some(node) = crate::p2p::running() else { return };
    let id = node.peer_id().to_string();
    {
        let mut w = WATCHING.lock().unwrap();
        if w.as_deref() == Some(id.as_str()) {
            return;
        }
        *w = Some(id);
    }
    let mut rx = node.subscribe();
    tauri::async_runtime::spawn(async move {
        use sulcus_net::NodeEvent;
        loop {
            let ev = match rx.recv().await {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            };
            let mut p = paths().lock().unwrap();
            match ev {
                NodeEvent::Connected { peer, addr } => {
                    let relayed = addr.iter().any(|x| matches!(x, libp2p::multiaddr::Protocol::P2pCircuit));
                    let why = if relayed {
                        "Through a relay: neither PC takes incoming connections yet, so a direct path is being tried."
                    } else if private(&addr) {
                        "Direct on the same local network."
                    } else {
                        "Direct over the internet."
                    };
                    // A relayed second connection doesn't undo a direct one.
                    if !(relayed && p.get(&peer.to_string()).is_some_and(|i| !i.relayed)) {
                        p.insert(peer.to_string(), PathInfo { relayed, why: why.into() });
                    }
                }
                NodeEvent::Direct { peer, ok } => {
                    let info = if ok {
                        PathInfo { relayed: false, why: "Direct over the internet (hole punched).".into() }
                    } else {
                        PathInfo { relayed: true, why: "Through a relay: a direct path failed, usually a strict router (NAT) or firewall on one side. Turning on router ports (UPnP) in Settings can help.".into() }
                    };
                    p.insert(peer.to_string(), info);
                }
                NodeEvent::Disconnected { peer } => {
                    p.remove(&peer.to_string());
                }
                _ => {}
            }
        }
    });
}

// ---------- the member server: speed test and files ----------

fn members() -> &'static Mutex<HashSet<Ipv4Addr>> {
    static M: OnceLock<Mutex<HashSet<Ipv4Addr>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

/// Who may reach the server: the members of the network that is on.
pub fn set_members(list: impl IntoIterator<Item = Ipv4Addr>) {
    *members().lock().unwrap() = list.into_iter().collect();
}

pub fn received_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default();
    home.join("Downloads").join("SulcusAI Networks")
}

/// Starts once; only virtual LAN members get past the first check.
pub fn serve(app: AppHandle) {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    std::thread::spawn(move || {
        let Ok(l) = TcpListener::bind(("0.0.0.0", PORT)) else { return };
        for s in l.incoming().flatten() {
            let ok = matches!(s.peer_addr(), Ok(SocketAddr::V4(a)) if members().lock().unwrap().contains(a.ip()));
            if !ok {
                continue;
            }
            let app = app.clone();
            std::thread::spawn(move || {
                let _ = handle(s, &app);
            });
        }
    });
}

fn handle(mut s: TcpStream, app: &AppHandle) -> std::io::Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut kind = [0u8; 1];
    s.read_exact(&mut kind)?;
    match kind[0] {
        b'S' => {
            let n = std::io::copy(&mut (&mut s).take(SPEED_BYTES), &mut std::io::sink())?;
            s.write_all(&n.to_le_bytes())?;
        }
        b'F' => {
            let mut len = [0u8; 2];
            s.read_exact(&mut len)?;
            let mut name = vec![0u8; u16::from_le_bytes(len).min(255) as usize];
            s.read_exact(&mut name)?;
            let mut size = [0u8; 8];
            s.read_exact(&mut size)?;
            let size = u64::from_le_bytes(size);
            let name: String = String::from_utf8_lossy(&name).chars().filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') && !c.is_control()).collect();
            let name = if name.trim().trim_matches('.').is_empty() { "file".to_string() } else { name };
            let dir = received_dir();
            std::fs::create_dir_all(&dir)?;
            let path = free_name(&dir, &name);
            let mut f = std::fs::File::create(&path)?;
            let got = std::io::copy(&mut (&mut s).take(size), &mut f)?;
            if got != size {
                drop(f);
                let _ = std::fs::remove_file(&path);
                return Ok(());
            }
            s.write_all(b"K")?;
            let from = s.peer_addr().map(|a| a.ip().to_string()).unwrap_or_default();
            let _ = app.emit("social:net_file", serde_json::json!({ "from": from, "path": path.to_string_lossy() }));
        }
        _ => {}
    }
    Ok(())
}

fn free_name(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((a, b)) => (a.to_string(), format!(".{b}")),
        None => (name.to_string(), String::new()),
    };
    (2..).map(|i| dir.join(format!("{stem} ({i}){ext}"))).find(|p| !p.exists()).unwrap()
}

#[derive(Debug, Clone, Serialize)]
pub struct Speed {
    pub mbps: f64,
    pub ms: u64,
}

/// Sends 16 MB to the member and times it until they confirm.
pub fn speed_test(to: Ipv4Addr) -> Result<Speed, String> {
    let mut s = TcpStream::connect_timeout(&(to, PORT).into(), Duration::from_secs(5)).map_err(|_| "They didn't answer. Their network has to be on, with SulcusAI open.".to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(60))).ok();
    let start = Instant::now();
    s.write_all(b"S").map_err(|e| e.to_string())?;
    let chunk = vec![0u8; 64 * 1024];
    let mut left = SPEED_BYTES;
    while left > 0 {
        let n = left.min(chunk.len() as u64) as usize;
        s.write_all(&chunk[..n]).map_err(|e| e.to_string())?;
        left -= n as u64;
    }
    let mut got = [0u8; 8];
    s.read_exact(&mut got).map_err(|_| "The test stopped partway.".to_string())?;
    let secs = start.elapsed().as_secs_f64().max(0.001);
    Ok(Speed { mbps: (u64::from_le_bytes(got) as f64 * 8.0 / 1_000_000.0) / secs, ms: (secs * 1000.0) as u64 })
}

pub fn send_file(to: Ipv4Addr, path: &Path) -> Result<(), String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let size = f.metadata().map_err(|e| e.to_string())?.len();
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let name: Vec<u8> = name.into_bytes().into_iter().take(255).collect();
    let mut s = TcpStream::connect_timeout(&(to, PORT).into(), Duration::from_secs(5)).map_err(|_| "They didn't answer. Their network has to be on, with SulcusAI open.".to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(60))).ok();
    s.write_all(b"F").map_err(|e| e.to_string())?;
    s.write_all(&(name.len() as u16).to_le_bytes()).map_err(|e| e.to_string())?;
    s.write_all(&name).map_err(|e| e.to_string())?;
    s.write_all(&size.to_le_bytes()).map_err(|e| e.to_string())?;
    std::io::copy(&mut f, &mut s).map_err(|e| e.to_string())?;
    let mut k = [0u8; 1];
    s.read_exact(&mut k).map_err(|_| "They didn't confirm the file arrived.".to_string())?;
    Ok(())
}

// ---------- remote desktop ----------

pub fn remote_desktop(to: Ipv4Addr) -> Result<(), String> {
    hidden(std::process::Command::new("mstsc").arg(format!("/v:{to}"))).spawn().map(|_| ()).map_err(|_| "Remote Desktop isn't on this PC.".to_string())
}

// ---------- Wake-on-LAN ----------

/// This PC's wired or wireless card addresses, for others to wake it.
pub fn own_macs() -> Vec<String> {
    output("getmac", &["/fo", "csv", "/nh"])
        .lines()
        .filter_map(|l| l.split(',').next())
        .map(|m| m.trim_matches('"').to_string())
        .filter(|m| parse_mac(m).is_some())
        .collect()
}

pub fn parse_mac(s: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = s.split(|c| c == '-' || c == ':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut out = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        out[i] = u8::from_str_radix(p, 16).ok()?;
    }
    (out != [0; 6]).then_some(out)
}

/// The magic packet on this PC's own network and the virtual LAN.
pub fn wake(mac: [u8; 6]) -> Result<(), String> {
    let mut pkt = vec![0xffu8; 6];
    for _ in 0..16 {
        pkt.extend_from_slice(&mac);
    }
    let s = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    s.set_broadcast(true).map_err(|e| e.to_string())?;
    let mut sent = false;
    for to in [Ipv4Addr::BROADCAST, sulcus_net::sharing::vlan::BROADCAST] {
        for port in [9u16, 7] {
            sent |= s.send_to(&pkt, (to, port)).is_ok();
        }
    }
    if sent { Ok(()) } else { Err("Couldn't send the wake packet.".into()) }
}

// ---------- LAN games ----------

#[derive(Debug, Clone, Serialize)]
pub struct Game {
    pub name: &'static str,
    pub exe: &'static str,
    /// How to host and join over a virtual LAN.
    pub notes: &'static str,
    pub running: bool,
}

const GAMES: &[(&str, &str, &str)] = &[
    ("Minecraft (Java)", "javaw.exe", "Host: open a world, Esc, Open to LAN. Join: Multiplayer, Direct Connection, the host's address and the port the host's chat shows."),
    ("Minecraft (Bedrock)", "minecraft.windows.exe", "Host: turn on Visible to LAN players. Join: Friends tab, LAN Games, or Servers with the host's address and port 19132."),
    ("Terraria", "terraria.exe", "Host: Multiplayer, Host & Play. Join: Join via IP, the host's address, port 7777."),
    ("Stardew Valley", "stardew valley.exe", "Host: Co-op, Host. Join: Co-op, Join LAN Game, Enter IP, the host's address."),
    ("Age of Empires II", "aoe2de_s.exe", "Host: Multiplayer, Create Game, LAN. Join: Browse Games, LAN tab."),
    ("Age of Empires II (2013)", "age2_x1.exe", "Host: Multiplayer, Local Area Network. Join: the same screen lists the game."),
    ("StarCraft", "starcraft.exe", "Host and join: Multiplayer, Local Area Network (UDP)."),
    ("Warcraft III", "warcraft iii.exe", "Host and join: Local Area Network. Games show up by themselves."),
    ("Diablo II", "game.exe", "Host and join: Other Multiplayer, TCP/IP, the host's address."),
    ("Counter-Strike 1.6", "hl.exe", "Host: Create Server, LAN. Join: Find Servers, LAN tab, or connect <address> in the console."),
    ("Left 4 Dead 2", "left4dead2.exe", "Host: console sv_lan 1, map <name>. Join: console connect <address>."),
    ("Factorio", "factorio.exe", "Host: Multiplayer, Host new game. Join: Multiplayer, Browse LAN games or Connect to address."),
    ("Valheim", "valheim.exe", "Host: Start, Start server. Join: Join IP, <address>:2456."),
    ("Don't Starve Together", "dontstarve_steam_x64.exe", "Host: Host Game, server set to LAN. Join: Browse Games, LAN filter."),
    ("Civilization V", "civilizationv.exe", "Host and join: Multiplayer, Local Network."),
    ("Civilization VI", "civilizationvi.exe", "Host and join: Multiplayer, LAN."),
    ("Borderlands 2", "borderlands2.exe", "Host and join: Network: LAN in the play menu."),
    ("Unreal Tournament 2004", "ut2004.exe", "Host: Host Game. Join: Join Game, LAN tab."),
    ("Command & Conquer: Red Alert 2", "gamemd.exe", "Host and join: Network, the game shows up on the list."),
    ("Halo: Combat Evolved", "halo.exe", "Host: Multiplayer, Create Game. Join: Join Game, LAN."),
    ("Risk of Rain 2", "risk of rain 2.exe", "Host and join through Steam lobbies; LAN with a mod only."),
    ("Project Zomboid", "projectzomboid64.exe", "Host: Host. Join: Join, the host's address, port 16261."),
];

/// The known games, with the ones running now marked, running first.
pub fn games() -> Vec<Game> {
    let running: HashSet<String> = output("tasklist", &["/fo", "csv", "/nh"])
        .lines()
        .filter_map(|l| l.split(',').next())
        .map(|s| s.trim_matches('"').to_lowercase())
        .collect();
    let mut out: Vec<Game> = GAMES.iter().map(|(name, exe, notes)| Game { name, exe, notes, running: running.contains(*exe) }).collect();
    out.sort_by(|a, b| b.running.cmp(&a.running).then(a.name.cmp(b.name)));
    out
}

// ---------- firewall ----------

fn rule_name(net: &str) -> String {
    format!("SulcusAI network {net}")
}

pub fn has_rule(net: &str) -> bool {
    let name = format!("name={}", rule_name(net));
    output("netsh", &["advfirewall", "firewall", "show", "rule", &name]).contains("10.77.")
}

fn netsh(args: &[String]) -> Result<(), String> {
    if crate::p2p::lan_helper::elevated() {
        let ok = hidden(std::process::Command::new("netsh").args(args)).status().map(|s| s.success()).unwrap_or(false);
        return if ok { Ok(()) } else { Err("Windows didn't take the firewall rule.".into()) };
    }
    #[cfg(windows)]
    {
        let args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        crate::p2p::lan_helper::run_elevated_exe(Path::new("netsh.exe"), &args)
    }
    #[cfg(not(windows))]
    {
        Err("Firewall rules are made on Windows only.".into())
    }
}

/// Lets members (and only them) reach games on this PC, in and out.
pub fn add_rules(net: &str) -> Result<(), String> {
    if has_rule(net) {
        return Ok(());
    }
    let name = format!("name={}", rule_name(net));
    netsh(&["advfirewall", "firewall", "add", "rule", &name, "dir=in", "action=allow", "protocol=any", "remoteip=10.77.0.0/16", "localip=10.77.0.0/16"].map(String::from))
}

pub fn remove_rules(net: &str) -> Result<(), String> {
    if !has_rule(net) {
        return Ok(());
    }
    let name = format!("name={}", rule_name(net));
    netsh(&["advfirewall", "firewall", "delete", "rule", &name].map(String::from))
}

// ---------- shared folders and printers ----------

#[derive(Debug, Clone, Serialize)]
pub struct Shared {
    pub name: String,
    pub path: String,
    pub printer: bool,
}

pub fn shared() -> Vec<Shared> {
    let mut out = vec![];
    let text = output("net", &["share"]);
    let mut past_header = false;
    for l in text.lines() {
        if l.starts_with("---") {
            past_header = true;
            continue;
        }
        if !past_header || l.trim().is_empty() || l.starts_with("The command") {
            continue;
        }
        let mut it = l.split_whitespace();
        let Some(name) = it.next() else { continue };
        // Admin shares (C$, IPC$) are for the PC's own admins, not members.
        if name.ends_with('$') {
            continue;
        }
        let path = it.next().unwrap_or("").to_string();
        out.push(Shared { name: name.into(), path, printer: false });
    }
    let printers = output("powershell", &["-NoProfile", "-Command", "Get-Printer | Where-Object Shared | ForEach-Object ShareName"]);
    out.extend(printers.lines().map(str::trim).filter(|s| !s.is_empty()).map(|s| Shared { name: s.into(), path: String::new(), printer: true }));
    out
}

/// Whether Windows lets the virtual LAN see shares at all (File and Printer Sharing on).
pub fn sharing_open() -> bool {
    let t = output("netsh", &["advfirewall", "firewall", "show", "rule", "name=all", "dir=in", "status=enabled"]);
    t.contains("445")
}
