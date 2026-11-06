// SPDX-License-Identifier: AGPL-3.0-only
//! The LAN adapter's helper. Wintun lets only administrators open the
//! adapter, so a small copy of SulcusAI runs with admin rights just for it:
//! set up once (one admin prompt, a scheduled task that only starts it),
//! then started on demand without a prompt. The app passes packets to it
//! over 127.0.0.1, after a secret only this user's files hold.
//!
//! It also fixes a Windows habit LAN games trip over: a broadcast to
//! 255.255.255.255 (or the local subnet) leaves by the main network adapter,
//! never reaching the virtual one. The helper watches outgoing broadcasts on
//! the other adapters and sends a copy over the virtual LAN, as Radmin does.

use std::io::{Read, Write};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use serde::Serialize;

pub const SETUP_ARG: &str = "--lan-setup";
pub const HELPER_ARG: &str = "--lan-helper";
const TASK: &str = "SulcusAI LAN helper";
const PORT: u16 = 47_791;

/// Frames between the app and the helper: a kind byte, a length, the bytes.
pub const CONFIG: u8 = 1;
pub const PACKET: u8 = 2;
pub const NOTE: u8 = 3;

pub fn write_frame(w: &mut impl Write, kind: u8, data: &[u8]) -> std::io::Result<()> {
    let mut b = Vec::with_capacity(5 + data.len());
    b.push(kind);
    b.extend_from_slice(&(data.len() as u32).to_be_bytes());
    b.extend_from_slice(data);
    w.write_all(&b)
}

pub fn read_frame(r: &mut impl Read) -> std::io::Result<(u8, Vec<u8>)> {
    let mut h = [0u8; 5];
    r.read_exact(&mut h)?;
    let len = u32::from_be_bytes([h[1], h[2], h[3], h[4]]) as usize;
    if len > 70_000 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "frame too big"));
    }
    let mut d = vec![0u8; len];
    r.read_exact(&mut d)?;
    Ok((h[0], d))
}

fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for c in data.chunks(2) {
        sum += u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]) as u32;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// A copy for the virtual LAN of a UDP broadcast this PC sent out of
/// another adapter (`from`/`prefix`), sent from this PC's LAN address.
pub fn rebroadcast(packet: &[u8], from: Ipv4Addr, prefix: u8, mine: Ipv4Addr) -> Option<Vec<u8>> {
    if packet.len() < 28 || packet[0] >> 4 != 4 || packet[9] != 17 {
        return None;
    }
    let ihl = ((packet[0] & 0x0f) as usize) * 4;
    let total = u16::from_be_bytes([packet[2], packet[3]]) as usize;
    if ihl < 20 || total > packet.len() || total < ihl + 8 {
        return None;
    }
    let src = Ipv4Addr::new(packet[12], packet[13], packet[14], packet[15]);
    let dst = Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]);
    let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix.min(32)) };
    let subnet_bcast = Ipv4Addr::from(u32::from(from) | !mask);
    if src != from || !(dst == Ipv4Addr::BROADCAST || (prefix < 31 && dst == subnet_bcast)) {
        return None;
    }
    let mut p = packet[..total].to_vec();
    p[12..16].copy_from_slice(&mine.octets());
    p[16..20].copy_from_slice(&Ipv4Addr::BROADCAST.octets());
    p[10] = 0;
    p[11] = 0;
    let c = checksum(&p[..ihl]);
    p[10..12].copy_from_slice(&c.to_be_bytes());
    // A zero UDP checksum means "none" in IPv4; the addresses changed under it.
    p[ihl + 6] = 0;
    p[ihl + 7] = 0;
    Some(p)
}

fn token_path(dir: &Path) -> PathBuf {
    dir.join("helper.token")
}

/// The secret the app and helper share; written by the app, user-only like the rest of the data folder.
fn token(dir: &Path) -> Result<String, String> {
    let p = token_path(dir);
    if let Ok(t) = std::fs::read_to_string(&p) {
        if t.trim().len() == 64 {
            return Ok(t.trim().to_string());
        }
    }
    let t = hex::encode(sulcus_net::sharing::share::new_key());
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(&p, &t).map_err(|e| e.to_string())?;
    Ok(t)
}

#[derive(Debug, Clone, Serialize)]
pub struct HelperStatus {
    /// This copy has admin rights itself (no helper needed).
    pub elevated: bool,
    pub set_up: bool,
    pub dll: bool,
}

#[cfg(windows)]
pub fn elevated() -> bool {
    // SAFETY: a plain query.
    unsafe { windows::Win32::UI::Shell::IsUserAnAdmin().as_bool() }
}

#[cfg(not(windows))]
pub fn elevated() -> bool {
    false
}

fn hidden(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

pub fn set_up() -> bool {
    hidden(std::process::Command::new("schtasks").args(["/query", "/tn", TASK])).output().is_ok_and(|o| o.status.success())
}

#[tauri::command]
pub fn lan_helper_status(state: crate::AppStateRef) -> HelperStatus {
    HelperStatus { elevated: elevated(), set_up: set_up(), dll: super::vlan_adapter::dll_path(&state.paths.data).is_file() }
}

/// Fetches Wintun if needed, then asks Windows for admin rights once to make
/// the adapter and the helper's task.
#[tauri::command]
pub async fn lan_helper_setup(state: crate::AppStateRef<'_>) -> Result<HelperStatus, String> {
    let dll = super::vlan_adapter::ensure_dll(&state.paths, &state.paths.data).await?;
    let dir = dll.parent().ok_or("No Wintun folder.")?.to_path_buf();
    token(&dir)?;
    if !set_up() {
        let sid = user_sid()?;
        tokio::task::spawn_blocking(move || run_elevated(&[SETUP_ARG, &dir.to_string_lossy(), &sid])).await.map_err(|e| e.to_string())??;
    }
    Ok(HelperStatus { elevated: elevated(), set_up: set_up(), dll: true })
}

fn user_sid() -> Result<String, String> {
    let o = hidden(std::process::Command::new("whoami").args(["/user", "/fo", "csv", "/nh"])).output().map_err(|e| e.to_string())?;
    let s = String::from_utf8_lossy(&o.stdout);
    let sid = s.trim().rsplit(',').next().unwrap_or("").trim_matches('"').to_string();
    if sid.starts_with("S-1-") && sid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') { Ok(sid) } else { Err("Couldn't read this Windows user's id.".into()) }
}

/// Starts this program with admin rights (the Windows prompt) and waits.
#[cfg(windows)]
fn run_elevated(args: &[&str]) -> Result<(), String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    let exe = HSTRING::from(std::env::current_exe().map_err(|e| e.to_string())?.as_os_str());
    let params = HSTRING::from(args.iter().map(|a| format!("\"{a}\"")).collect::<Vec<_>>().join(" "));
    let verb = HSTRING::from("runas");
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(exe.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: 0,
        ..Default::default()
    };
    // SAFETY: the strings outlive the call; the handle is waited on and closed.
    unsafe {
        ShellExecuteExW(&mut info).map_err(|_| "Admin rights weren't given, so the LAN adapter can't be made.".to_string())?;
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        if code != 0 {
            return Err(format!("Setting up the LAN adapter failed (code {code}); see the app log."));
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn run_elevated(_: &[&str]) -> Result<(), String> {
    Err("The virtual LAN adapter is Windows-only for now.".into())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The helper's task: no trigger (started only by the app), highest rights,
/// runnable by this user without a prompt.
pub fn task_xml(exe: &str, dir: &str, sid: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.3" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Runs the SulcusAI LAN adapter while a virtual network is on.</Description><SecurityDescriptor>D:(A;;FA;;;BA)(A;;FA;;;SY)(A;;GRGX;;;{sid})</SecurityDescriptor></RegistrationInfo>
  <Principals><Principal id="Author"><UserId>{sid}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><AllowStartOnDemand>true</AllowStartOnDemand><Hidden>true</Hidden></Settings>
  <Actions Context="Author"><Exec><Command>{}</Command><Arguments>{} "{}"</Arguments></Exec></Actions>
</Task>"#,
        xml_escape(exe),
        HELPER_ARG,
        xml_escape(dir)
    )
}

/// In the elevated copy: makes the adapter and registers the task, then ends.
pub fn run_setup(dir: &str, sid: &str) -> i32 {
    let r = (|| -> Result<(), String> {
        #[cfg(windows)]
        {
            let w = unsafe { wintun::load_from_path(Path::new(dir).join("wintun.dll")) }.map_err(|e| e.to_string())?;
            if wintun::Adapter::open(&w, super::vlan_adapter::ADAPTER_NAME).is_err() {
                wintun::Adapter::create(&w, super::vlan_adapter::ADAPTER_NAME, "SulcusAI", None).map_err(|e| e.to_string())?;
            }
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let xml = task_xml(&exe.to_string_lossy(), dir, sid);
        let file = std::env::temp_dir().join("sulcusai-lan-task.xml");
        let mut wide: Vec<u8> = vec![0xff, 0xfe];
        for u in xml.encode_utf16() {
            wide.extend_from_slice(&u.to_le_bytes());
        }
        std::fs::write(&file, wide).map_err(|e| e.to_string())?;
        let o = hidden(std::process::Command::new("schtasks").args(["/create", "/f", "/tn", TASK, "/xml"]).arg(&file)).output().map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&file);
        if !o.status.success() {
            return Err(String::from_utf8_lossy(&o.stderr).into_owned());
        }
        Ok(())
    })();
    match r {
        Ok(()) => 0,
        Err(e) => {
            let _ = std::fs::write(Path::new(dir).join("setup-error.txt"), e);
            2
        }
    }
}

/// From the app: starts the helper (no prompt) and connects to it.
pub fn connect(dir: &Path) -> Result<std::net::TcpStream, String> {
    let t = token(dir)?;
    let try_once = || -> Option<std::net::TcpStream> {
        let mut s = std::net::TcpStream::connect_timeout(&(Ipv4Addr::LOCALHOST, PORT).into(), std::time::Duration::from_millis(400)).ok()?;
        write_frame(&mut s, CONFIG, t.as_bytes()).ok()?;
        let (k, d) = read_frame(&mut s).ok()?;
        (k == NOTE && d == b"ok").then_some(s)
    };
    if let Some(s) = try_once() {
        return Ok(s);
    }
    if !set_up() {
        return Err("The LAN adapter isn't set up yet. Press Set up the adapter (Windows asks for admin rights once).".into());
    }
    let o = hidden(std::process::Command::new("schtasks").args(["/run", "/tn", TASK])).output().map_err(|e| e.to_string())?;
    if !o.status.success() {
        return Err(format!("The LAN helper didn't start: {}", String::from_utf8_lossy(&o.stderr).trim()));
    }
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(250));
        if let Some(s) = try_once() {
            return Ok(s);
        }
    }
    Err("The LAN helper started but didn't answer.".into())
}

#[derive(serde::Deserialize)]
struct Config {
    address: Ipv4Addr,
}

/// The helper itself (admin rights): one app connection at a time; ends
/// when the app goes away.
#[cfg(windows)]
pub fn run_helper(dir: &str) -> i32 {
    use std::sync::Arc;
    let dir = PathBuf::from(dir);
    let log = |m: &str| {
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("helper.log")).and_then(|mut f| writeln!(f, "{m}"));
    };
    let Ok(want) = std::fs::read_to_string(token_path(&dir)) else { return 2 };
    let Ok(listener) = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, PORT)) else { return 3 };
    let _ = listener.set_nonblocking(false);
    let w = match unsafe { wintun::load_from_path(dir.join("wintun.dll")) } {
        Ok(w) => w,
        Err(e) => {
            log(&format!("wintun: {e}"));
            return 4;
        }
    };
    // The first app connection gets a minute; after it ends, so does the helper.
    let (mut s, _) = match listener.accept() {
        Ok(x) => x,
        Err(_) => return 5,
    };
    let Ok((CONFIG, t)) = read_frame(&mut s) else { return 6 };
    if t != want.trim().as_bytes() {
        log("wrong secret");
        return 6;
    }
    let _ = write_frame(&mut s, NOTE, b"ok");
    let Ok((CONFIG, cfg)) = read_frame(&mut s) else { return 7 };
    let Ok(cfg) = serde_json::from_slice::<Config>(&cfg) else { return 7 };
    let adapter = match wintun::Adapter::open(&w, super::vlan_adapter::ADAPTER_NAME).or_else(|_| wintun::Adapter::create(&w, super::vlan_adapter::ADAPTER_NAME, "SulcusAI", None)) {
        Ok(a) => a,
        Err(e) => {
            log(&format!("adapter: {e}"));
            let _ = write_frame(&mut s, NOTE, format!("The adapter couldn't open: {e}").as_bytes());
            return 8;
        }
    };
    if let Err(e) = adapter.set_network_addresses_tuple(cfg.address.into(), Ipv4Addr::new(255, 255, 0, 0).into(), None) {
        log(&format!("address: {e}"));
    }
    let session = match adapter.start_session(wintun::MAX_RING_CAPACITY) {
        Ok(x) => Arc::new(x),
        Err(e) => {
            let _ = write_frame(&mut s, NOTE, format!("The adapter couldn't start: {e}").as_bytes());
            return 9;
        }
    };
    let _ = write_frame(&mut s, NOTE, b"up");
    let out = Arc::new(std::sync::Mutex::new(s.try_clone().expect("socket clone")));
    // Adapter to app.
    {
        let (session, out) = (session.clone(), out.clone());
        std::thread::spawn(move || {
            while let Ok(p) = session.receive_blocking() {
                if write_frame(&mut *out.lock().unwrap(), PACKET, p.bytes()).is_err() {
                    break;
                }
            }
        });
    }
    // Broadcasts that left by another adapter, to the app as if sent here.
    for (ip, prefix) in other_addresses() {
        let out = out.clone();
        let mine = cfg.address;
        std::thread::spawn(move || watch_broadcasts(ip, prefix, mine, out));
    }
    // App to adapter, until the app hangs up.
    while let Ok((kind, d)) = read_frame(&mut s) {
        if kind == PACKET && !d.is_empty() && d.len() <= u16::MAX as usize {
            if let Ok(mut p) = session.allocate_send_packet(d.len() as u16) {
                p.bytes_mut().copy_from_slice(&d);
                session.send_packet(p);
            }
        }
    }
    let _ = session.shutdown();
    0
}

#[cfg(not(windows))]
pub fn run_helper(_: &str) -> i32 {
    1
}

/// Up IPv4 addresses with their prefix, on adapters other than SulcusAI's.
#[cfg(windows)]
pub fn other_addresses() -> Vec<(Ipv4Addr, u8)> {
    use windows::Win32::NetworkManagement::IpHelper::{GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH};
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};
    let mut size = 64_000u32;
    let mut buf = vec![0u8; size as usize];
    let mut out = Vec::new();
    // SAFETY: the list lives in `buf` for the whole walk.
    unsafe {
        let first = buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
        if GetAdaptersAddresses(AF_INET.0 as u32, GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST, None, Some(first), &mut size) != 0 {
            return out;
        }
        let mut p = first;
        while !p.is_null() {
            let a = &*p;
            let name = a.FriendlyName.to_string().unwrap_or_default();
            if a.OperStatus == IfOperStatusUp && name != super::vlan_adapter::ADAPTER_NAME {
                let mut u = a.FirstUnicastAddress;
                while !u.is_null() {
                    let ua = &*u;
                    let sa = ua.Address.lpSockaddr;
                    if !sa.is_null() && (*sa).sa_family == AF_INET {
                        let sin = &*(sa as *const SOCKADDR_IN);
                        let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.S_un.S_addr));
                        if !ip.is_loopback() && !ip.is_link_local() && ip.octets()[..2] != sulcus_net::sharing::vlan::NET {
                            out.push((ip, ua.OnLinkPrefixLength));
                        }
                    }
                    u = ua.Next;
                }
            }
            p = a.Next;
        }
    }
    out
}

#[cfg(not(windows))]
pub fn other_addresses() -> Vec<(Ipv4Addr, u8)> {
    Vec::new()
}

/// Reads every IP packet on one adapter (raw socket, admin only) and passes
/// on its outgoing UDP broadcasts.
#[cfg(windows)]
fn watch_broadcasts(ip: Ipv4Addr, prefix: u8, mine: Ipv4Addr, out: std::sync::Arc<std::sync::Mutex<std::net::TcpStream>>) {
    let Ok(sock) = raw_socket(ip) else { return };
    let mut buf = [std::mem::MaybeUninit::<u8>::uninit(); 65_536];
    while let Ok(n) = sock.recv(&mut buf) {
        // SAFETY: recv filled the first n bytes.
        let bytes: &[u8] = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, n) };
        if let Some(p) = rebroadcast(bytes, ip, prefix, mine) {
            if write_frame(&mut *out.lock().unwrap(), PACKET, &p).is_err() {
                return;
            }
        }
    }
}

/// A raw socket on `ip` that sees all IP traffic in and out of that adapter.
#[cfg(windows)]
pub fn raw_socket(ip: Ipv4Addr) -> Result<socket2::Socket, String> {
    use socket2::{Domain, Protocol, Socket, Type};
    use std::os::windows::io::AsRawSocket;
    use windows::Win32::Networking::WinSock::{WSAIoctl, SOCKET};
    const SIO_RCVALL: u32 = 0x9800_0001;
    let sock = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::from(0))).map_err(|e| e.to_string())?;
    sock.bind(&std::net::SocketAddrV4::new(ip, 0).into()).map_err(|e| e.to_string())?;
    let on: u32 = 1;
    let mut got = 0u32;
    // SAFETY: the in-buffer is a u32 that outlives the call.
    let r = unsafe { WSAIoctl(SOCKET(sock.as_raw_socket() as usize), SIO_RCVALL, Some(&on as *const u32 as *const _), 4, None, 0, &mut got, None, None) };
    if r != 0 {
        return Err("Couldn't watch the adapter (needs admin rights).".into());
    }
    Ok(sock)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn udp(src: Ipv4Addr, dst: Ipv4Addr) -> Vec<u8> {
        let mut p = vec![0u8; 32];
        p[0] = 0x45;
        p[2..4].copy_from_slice(&32u16.to_be_bytes());
        p[8] = 64;
        p[9] = 17;
        p[12..16].copy_from_slice(&src.octets());
        p[16..20].copy_from_slice(&dst.octets());
        p[20..22].copy_from_slice(&5000u16.to_be_bytes());
        p[22..24].copy_from_slice(&5000u16.to_be_bytes());
        p[24..26].copy_from_slice(&12u16.to_be_bytes());
        p[26] = 0xab;
        p
    }

    #[test]
    fn outgoing_broadcasts_are_copied_from_this_pcs_lan_address() {
        let (eth, mine) = (Ipv4Addr::new(192, 168, 1, 20), Ipv4Addr::new(10, 77, 3, 4));
        let p = rebroadcast(&udp(eth, Ipv4Addr::BROADCAST), eth, 24, mine).unwrap();
        assert_eq!(&p[12..16], &mine.octets());
        assert_eq!(&p[16..20], &[255; 4]);
        assert_eq!(checksum(&p[..20]), 0, "header checksum holds");
        assert_eq!(&p[26..28], &[0, 0], "UDP checksum cleared");
        // The subnet's own broadcast address counts too.
        assert!(rebroadcast(&udp(eth, Ipv4Addr::new(192, 168, 1, 255)), eth, 24, mine).is_some());
        // Not: unicast, incoming, other protocols.
        assert!(rebroadcast(&udp(eth, Ipv4Addr::new(192, 168, 1, 7)), eth, 24, mine).is_none());
        assert!(rebroadcast(&udp(Ipv4Addr::new(192, 168, 1, 9), Ipv4Addr::BROADCAST), eth, 24, mine).is_none());
        let mut tcp = udp(eth, Ipv4Addr::BROADCAST);
        tcp[9] = 6;
        assert!(rebroadcast(&tcp, eth, 24, mine).is_none());
        // The copy routes to every member of the network.
        let mut n = sulcus_net::sharing::vlan::Network::create("n", "n", "me");
        n.members.insert("me".into(), mine);
        n.admit("me", "bob").unwrap();
        assert_eq!(n.route("me", &p).unwrap(), vec!["bob".to_string()]);
    }

    #[test]
    fn frames_round_trip_and_the_task_is_on_demand() {
        let mut b = Vec::new();
        write_frame(&mut b, PACKET, b"hello").unwrap();
        assert_eq!(read_frame(&mut &b[..]).unwrap(), (PACKET, b"hello".to_vec()));
        let x = task_xml(r"C:\A&B\sulcusai.exe", r"C:\d", "S-1-5-21-1");
        assert!(x.contains("A&amp;B") && x.contains("HighestAvailable") && !x.contains("<Triggers>"));
        assert!(x.contains("(A;;GRGX;;;S-1-5-21-1)"));
    }
}
