// SPDX-License-Identifier: AGPL-3.0-only
//! Servers and ports: the ports SulcusAI's models and the programs it
//! started have open (only those: nothing else on this PC is listed or
//! stopped from here), the servers the assistant started (stop, start
//! again), and previews of local servers in SulcusAI's own browser.
//!
//! The browser refuses addresses on this PC and the local network (a page
//! could otherwise steer it at a router or a local admin page). Two kinds of
//! local port are let through: ones the person chose to view here, and ones
//! served by a program one of the assistant's background commands started.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use serde::Serialize;
use tauri::AppHandle;

use crate::tools::shell;
use crate::AppStateRef;

/// Local ports the person chose to view in SulcusAI's browser.
static CHOSEN: LazyLock<Mutex<HashSet<u16>>> = LazyLock::new(Default::default);

/// The port of an address on this PC ("http://localhost:5173" → 5173), or None.
pub fn local_port(url: &str) -> Option<u16> {
    let u = reqwest::Url::parse(url).ok()?;
    let host = u.host_str()?.trim_matches(['[', ']']).to_ascii_lowercase();
    let here = host == "localhost" || host == "0.0.0.0" || host == "::" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback());
    if !here || !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    u.port_or_known_default()
}

/// Lets SulcusAI's browser show a port on this PC (a site preview it serves).
pub fn allow(port: u16) {
    CHOSEN.lock().unwrap().insert(port);
}

/// Whether SulcusAI's browser may show this port on this PC.
pub fn previewable(port: u16) -> bool {
    CHOSEN.lock().unwrap().contains(&port) || ours(port).is_some()
}

/// The assistant's server that has this port open, if any.
fn ours(port: u16) -> Option<String> {
    let servers: Vec<(String, u32)> = shell::servers().into_iter().filter_map(|s| Some((s.id, s.pid?))).collect();
    if servers.is_empty() {
        return None;
    }
    let pids: Vec<u32> = listening().into_iter().filter(|l| l.port == port).map(|l| l.pid).collect();
    let parents = parents();
    pids.iter().find_map(|pid| server_of(*pid, &servers, &parents))
}

/// Ports that a process or anything it started listens on (a server that
/// didn't print its address).
pub fn ports_of(pid: u32) -> Vec<u16> {
    let parents = parents();
    let mut ports: Vec<u16> = listening()
        .into_iter()
        .filter(|l| server_of(l.pid, &[(String::new(), pid)], &parents).is_some())
        .map(|l| l.port)
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// Which of the assistant's servers a process belongs to (it or a parent is the server's shell).
fn server_of(pid: u32, servers: &[(String, u32)], parents: &HashMap<u32, (String, Option<u32>)>) -> Option<String> {
    let mut at = Some(pid);
    for _ in 0..24 {
        let p = at?;
        if let Some((id, _)) = servers.iter().find(|(_, s)| *s == p) {
            return Some(id.clone());
        }
        at = parents.get(&p).and_then(|(_, parent)| *parent);
    }
    None
}

/// Every process: its name and its parent.
fn parents() -> HashMap<u32, (String, Option<u32>)> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    sys.processes().iter().map(|(pid, p)| (pid.as_u32(), (p.name().to_string_lossy().into_owned(), p.parent().map(|x| x.as_u32())))).collect()
}

struct Listening {
    port: u16,
    address: String,
    pid: u32,
}

/// TCP ports something is listening on, from Windows' connection table.
#[cfg(windows)]
fn listening() -> Vec<Listening> {
    use windows::Win32::NetworkManagement::IpHelper::*;
    const AF_INET: u32 = 2;
    const AF_INET6: u32 = 23;
    let mut out = Vec::new();
    for family in [AF_INET, AF_INET6] {
        let mut size = 0u32;
        // SAFETY: the first call only reports the size; the second fills a
        // buffer of that size (8-byte aligned), read only within the row
        // count Windows wrote at its start.
        unsafe {
            let _ = GetExtendedTcpTable(None, &mut size, false, family, TCP_TABLE_OWNER_PID_LISTENER, 0);
            if size == 0 {
                continue;
            }
            let mut buf = vec![0u64; (size as usize).div_ceil(8) + 1];
            if GetExtendedTcpTable(Some(buf.as_mut_ptr().cast()), &mut size, false, family, TCP_TABLE_OWNER_PID_LISTENER, 0) != 0 {
                continue;
            }
            let base = buf.as_ptr() as *const u8;
            let count = *(base as *const u32) as usize;
            if family == AF_INET {
                let rows = base.add(std::mem::size_of::<u32>()) as *const MIB_TCPROW_OWNER_PID;
                for i in 0..count {
                    let r = &*rows.add(i);
                    let ip = std::net::Ipv4Addr::from(u32::from_be(r.dwLocalAddr));
                    out.push(Listening { port: u16::from_be(r.dwLocalPort as u16), address: ip.to_string(), pid: r.dwOwningPid });
                }
            } else {
                let rows = base.add(std::mem::size_of::<u32>()) as *const MIB_TCP6ROW_OWNER_PID;
                for i in 0..count {
                    let r = &*rows.add(i);
                    let ip = std::net::Ipv6Addr::from(r.ucLocalAddr);
                    out.push(Listening { port: u16::from_be(r.dwLocalPort as u16), address: format!("[{ip}]"), pid: r.dwOwningPid });
                }
            }
        }
    }
    out
}

#[cfg(not(windows))]
fn listening() -> Vec<Listening> {
    Vec::new()
}

#[derive(Serialize)]
pub struct PortRow {
    port: u16,
    /// Where it listens: 127.0.0.1 (this PC only), 0.0.0.0 (the network too)…
    addresses: Vec<String>,
    /// Reachable only from this PC.
    local_only: bool,
    pid: u32,
    program: String,
    /// The assistant's server it belongs to.
    server: Option<String>,
    /// Windows itself or a service: not stopped from here.
    system: bool,
    /// What it is ("A model (the AI engine)"), when known.
    what: Option<String>,
}

#[derive(Serialize)]
pub struct PortsView {
    ports: Vec<PortRow>,
    servers: Vec<shell::ServerInfo>,
    /// Port listing works on Windows only for now.
    supported: bool,
}

/// Programs Windows needs: never stopped from here.
const SYSTEM: &[&str] = &["system", "idle", "svchost.exe", "lsass.exe", "services.exe", "wininit.exe", "csrss.exe", "smss.exe", "winlogon.exe", "spoolsv.exe", "registry", "memory compression"];

fn is_system(pid: u32, program: &str) -> bool {
    pid <= 4 || SYSTEM.contains(&program.to_ascii_lowercase().as_str())
}

/// A program SulcusAI started, or one those started (an AI engine, a
/// server the assistant ran): not SulcusAI itself, and nothing else.
fn started_by_us(pid: u32, parents: &HashMap<u32, (String, Option<u32>)>) -> bool {
    let me = std::process::id();
    let mut at = pid;
    for _ in 0..32 {
        let Some((_, Some(parent))) = parents.get(&at) else { return false };
        if *parent == me {
            return true;
        }
        if *parent == at || *parent == 0 {
            return false;
        }
        at = *parent;
    }
    false
}

/// What a program of SulcusAI's is, in words.
fn what_it_is(program: &str) -> Option<&'static str> {
    let p = program.to_ascii_lowercase();
    if p.starts_with("llama-server") {
        Some("A model (the AI engine)")
    } else if p.contains("python") || p.contains("comfy") {
        Some("A picture, video, voice or music model")
    } else {
        None
    }
}

fn view() -> PortsView {
    let servers = shell::servers();
    let ids: Vec<(String, u32)> = servers.iter().filter_map(|s| Some((s.id.clone(), s.pid?))).collect();
    let parents = parents();
    let mut rows: Vec<PortRow> = Vec::new();
    for l in listening() {
        // Only what SulcusAI started: nothing else on this PC shows here.
        if !started_by_us(l.pid, &parents) {
            continue;
        }
        let local = l.address.starts_with("127.") || l.address == "[::1]";
        if let Some(r) = rows.iter_mut().find(|r| r.port == l.port && r.pid == l.pid) {
            if !r.addresses.contains(&l.address) {
                r.addresses.push(l.address);
            }
            r.local_only &= local;
            continue;
        }
        let program = parents.get(&l.pid).map(|(n, _)| n.clone()).unwrap_or_else(|| if l.pid == 4 { "System".into() } else { "?".into() });
        rows.push(PortRow {
            port: l.port,
            local_only: local,
            addresses: vec![l.address],
            pid: l.pid,
            system: is_system(l.pid, &program),
            server: server_of(l.pid, &ids, &parents),
            // A server the assistant ran is named by its command instead.
            what: what_it_is(&program).filter(|_| server_of(l.pid, &ids, &parents).is_none()).map(str::to_string),
            program,
        });
    }
    // The assistant's servers first, then by port.
    rows.sort_by_key(|r| (r.server.is_none(), r.port));
    PortsView { ports: rows, servers, supported: cfg!(windows) }
}

// ---------- commands ----------

#[tauri::command(async)]
pub fn ports_view(state: AppStateRef) -> Result<PortsView, String> {
    state.cipher()?;
    Ok(view())
}

/// Stops the program that has a port open (asked first in the window).
#[tauri::command(async)]
pub fn stop_port_program(state: AppStateRef, pid: u32) -> Result<PortsView, String> {
    state.cipher()?;
    let parents = parents();
    let program = parents.get(&pid).map(|(n, _)| n.clone()).ok_or("That program has already ended.")?;
    if is_system(pid, &program) || pid == std::process::id() {
        return Err("That's part of Windows (or SulcusAI itself), so it isn't stopped from here.".into());
    }
    // Only programs SulcusAI started.
    if !started_by_us(pid, &parents) {
        return Err("SulcusAI didn't start that program, so it isn't stopped from here.".into());
    }
    // One of the assistant's servers: stop it as a whole, so it can start again.
    let ids: Vec<(String, u32)> = shell::servers().into_iter().filter_map(|s| Some((s.id, s.pid?))).collect();
    if let Some(id) = server_of(pid, &ids, &parents) {
        shell::stop_server(&id)?;
    } else {
        kill(pid)?;
    }
    state.log("settings", &format!("Stopped {program} (process {pid}) from Servers and ports"));
    std::thread::sleep(std::time::Duration::from_millis(300));
    Ok(view())
}

#[cfg(windows)]
fn kill(pid: u32) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let ok = std::process::Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(0x0800_0000)
        .status()
        .map_err(|e| e.to_string())?
        .success();
    if ok { Ok(()) } else { Err("Windows didn't let SulcusAI stop it (it may belong to another user or need administrator rights).".into()) }
}

#[cfg(not(windows))]
fn kill(_pid: u32) -> Result<(), String> {
    Err("Stopping programs from here works on Windows only for now.".into())
}

#[tauri::command(async)]
pub fn server_stop(state: AppStateRef, id: String) -> Result<PortsView, String> {
    state.cipher()?;
    shell::stop_server(&id)?;
    state.log("settings", "Stopped a server from Servers and ports");
    Ok(view())
}

#[tauri::command]
pub async fn server_start(state: AppStateRef<'_>, id: String) -> Result<PortsView, String> {
    state.cipher()?;
    shell::start_server(&id).await?;
    state.log("settings", "Started a server again from Servers and ports");
    Ok(view())
}

/// Shows a port on this PC in SulcusAI's browser.
#[tauri::command]
pub async fn preview_port(app: AppHandle, state: AppStateRef<'_>, port: u16) -> Result<(), String> {
    state.cipher()?;
    CHOSEN.lock().unwrap().insert(port);
    let w = crate::browser::open(&app, true)?;
    let url = format!("http://localhost:{port}/");
    w.navigate(url.parse().map_err(|_| "That isn't a port.".to_string())?).map_err(|e| e.to_string())?;
    let _ = w.set_focus();
    state.log("settings", &format!("Opened port {port} on this PC in SulcusAI's browser"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_addresses_on_this_pc_have_a_local_port() {
        assert_eq!(local_port("http://localhost:5173/"), Some(5173));
        assert_eq!(local_port("http://127.0.0.1:8000/index.html"), Some(8000));
        assert_eq!(local_port("http://[::1]:3000"), Some(3000));
        assert_eq!(local_port("http://localhost/"), Some(80));
        assert_eq!(local_port("http://192.168.1.20:8080/"), None, "the local network isn't this PC");
        assert_eq!(local_port("https://example.com/"), None);
        assert_eq!(local_port("file:///C:/x.html"), None);
    }

    #[test]
    fn a_port_belongs_to_the_server_whose_shell_started_it() {
        let parents: HashMap<u32, (String, Option<u32>)> =
            [(300, ("node.exe".to_string(), Some(200))), (200, ("cmd.exe".to_string(), Some(100))), (100, ("powershell.exe".to_string(), Some(1))), (999, ("other.exe".to_string(), Some(1)))].into();
        let servers = vec![("bg1".to_string(), 100)];
        assert_eq!(server_of(300, &servers, &parents).as_deref(), Some("bg1"));
        assert_eq!(server_of(999, &servers, &parents), None);
        assert!(is_system(4, "System") && is_system(880, "svchost.exe") && !is_system(5000, "node.exe"));
    }

    /// Lists this PC's listening ports: `cargo test --lib lists_this_pcs_ports -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn lists_this_pcs_ports() {
        for r in view().ports {
            println!("{:>5} {:<28} {} ({}){}", r.port, r.addresses.join(" "), r.program, r.pid, if r.system { " [system]" } else { "" });
        }
    }
}
