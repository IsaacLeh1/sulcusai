// SPDX-License-Identifier: AGPL-3.0-only
//! "Is it at zero?": every SulcusAI process (the app, its window's WebView2
//! processes, any engine) with its processor, memory, disk writes, graphics
//! memory and network connections since the last look, and what is still
//! on because the person turned it on. Anyone can check for themselves that
//! a paused or idle SulcusAI uses nothing.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use serde::Serialize;

use crate::AppStateRef;

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct ZeroProc {
    pub pid: u32,
    pub name: String,
    /// Percent of one processor core since the last look.
    pub cpu: f32,
    pub memory_mb: f64,
    /// Written to disk since the last look.
    pub written_kb: f64,
    pub card_mb: f64,
    pub connections: u32,
}

#[derive(Serialize)]
pub struct ZeroView {
    paused: bool,
    /// Seconds the numbers cover (0 the first time: look again).
    seconds: f64,
    procs: Vec<ZeroProc>,
    total: ZeroProc,
    /// Still on because it was turned on, in words.
    still_on: Vec<String>,
    at_zero: bool,
}

/// Under these, a measure counts as zero: a look itself costs a little.
const CPU_ZERO: f32 = 0.1;

fn total(procs: &[ZeroProc]) -> ZeroProc {
    procs.iter().fold(ZeroProc { name: "All of SulcusAI".into(), ..Default::default() }, |mut t, p| {
        t.cpu += p.cpu;
        t.memory_mb += p.memory_mb;
        t.written_kb += p.written_kb;
        t.card_mb += p.card_mb;
        t.connections += p.connections;
        t
    })
}

/// Nothing working, writing, on the card or connected.
fn at_zero(t: &ZeroProc) -> bool {
    t.cpu < CPU_ZERO && t.written_kb == 0.0 && t.card_mb < 1.0 && t.connections == 0
}

struct Look {
    sys: sysinfo::System,
    at: Instant,
}

static LOOK: Mutex<Option<Look>> = Mutex::new(None);

fn processes() -> (f64, Vec<ZeroProc>) {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate};
    let ours = crate::perf::our_pids();
    let pids: Vec<Pid> = ours.iter().map(|p| Pid::from_u32(*p)).collect();
    let mut guard = LOOK.lock().unwrap();
    let look = guard.get_or_insert_with(|| Look { sys: sysinfo::System::new(), at: Instant::now() });
    let seconds = look.at.elapsed().as_secs_f64();
    look.at = Instant::now();
    look.sys.refresh_processes_specifics(ProcessesToUpdate::Some(&pids), true, ProcessRefreshKind::nothing().with_cpu().with_memory().with_disk_usage());
    let conns = connections();
    let mut out: Vec<ZeroProc> = look
        .sys
        .processes()
        .iter()
        .filter(|(pid, _)| ours.contains(&pid.as_u32()))
        .map(|(pid, p)| {
            let pid = pid.as_u32();
            ZeroProc {
                pid,
                name: p.name().to_string_lossy().trim_end_matches(".exe").to_string(),
                cpu: p.cpu_usage(),
                memory_mb: p.memory() as f64 / 1048576.0,
                written_kb: p.disk_usage().written_bytes as f64 / 1024.0,
                card_mb: crate::perf::process_card_memory(pid).map_or(0.0, |(d, _)| d as f64 / 1048576.0),
                connections: conns.get(&pid).copied().unwrap_or(0),
            }
        })
        .collect();
    out.sort_by(|a, b| b.memory_mb.total_cmp(&a.memory_mb));
    (seconds, out)
}

/// Connections out of this PC (not listening ports, not this PC talking to
/// itself), by process.
#[cfg(windows)]
fn connections() -> HashMap<u32, u32> {
    use windows::Win32::NetworkManagement::IpHelper::*;
    const AF_INET: u32 = 2;
    const AF_INET6: u32 = 23;
    const ESTABLISHED: u32 = 5;
    let mut out = HashMap::new();
    for family in [AF_INET, AF_INET6] {
        let mut size = 0u32;
        // SAFETY: as in ports.rs: the first call sizes the buffer, the
        // second fills it, and rows are read within the count at its start.
        unsafe {
            let _ = GetExtendedTcpTable(None, &mut size, false, family, TCP_TABLE_OWNER_PID_CONNECTIONS, 0);
            if size == 0 {
                continue;
            }
            let mut buf = vec![0u64; (size as usize).div_ceil(8) + 1];
            if GetExtendedTcpTable(Some(buf.as_mut_ptr().cast()), &mut size, false, family, TCP_TABLE_OWNER_PID_CONNECTIONS, 0) != 0 {
                continue;
            }
            let base = buf.as_ptr() as *const u8;
            let count = *(base as *const u32) as usize;
            let rows = base.add(std::mem::size_of::<u32>());
            for i in 0..count {
                let (state, remote_local, pid) = if family == AF_INET {
                    let r = &*(rows as *const MIB_TCPROW_OWNER_PID).add(i);
                    (r.dwState, std::net::Ipv4Addr::from(u32::from_be(r.dwRemoteAddr)).is_loopback(), r.dwOwningPid)
                } else {
                    let r = &*(rows as *const MIB_TCP6ROW_OWNER_PID).add(i);
                    let ip = std::net::Ipv6Addr::from(r.ucRemoteAddr);
                    (r.dwState, ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|v| v.is_loopback()), r.dwOwningPid)
                };
                if state == ESTABLISHED && !remote_local {
                    *out.entry(pid).or_insert(0) += 1;
                }
            }
        }
    }
    out
}

#[cfg(not(windows))]
fn connections() -> HashMap<u32, u32> {
    HashMap::new()
}

/// What's still on by the person's choice, and so may use a little.
async fn still_on(state: &crate::AppState) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(mut e) = state.engine.try_lock() {
        if let Some(m) = e.status().model_id.clone() {
            out.push(format!("A chat model is loaded ({m}); it unloads by itself when idle."));
        }
    }
    if state.speech.any_loaded().await {
        out.push("A speech model is loaded.".into());
    }
    if crate::phone::link::status().connected {
        out.push("The phone link is connected (a small keep-alive every 25 seconds).".into());
    }
    if crate::sync::listening() {
        out.push("Listening on this network for your other PCs or People.".into());
    }
    if crate::telegram::running() {
        out.push("Telegram is waiting for messages (one open request).".into());
    }
    if crate::wake::running() {
        out.push("Listening for the wake word (the microphone and a small speech model).".into());
    }
    if crate::api_server::running() {
        out.push("The local API is listening for other programs on this PC.".into());
    }
    if crate::perf::away_now() {
        out.push("Out of the way: efficiency mode, nothing on the graphics card.".into());
    }
    out
}

#[tauri::command]
pub async fn zero_view(state: AppStateRef<'_>) -> Result<ZeroView, String> {
    let (seconds, procs) = tauri::async_runtime::spawn_blocking(processes).await.map_err(|e| e.to_string())?;
    let total = total(&procs);
    Ok(ZeroView { paused: crate::quiet::paused(), seconds, at_zero: seconds > 0.0 && at_zero(&total), procs, total, still_on: still_on(&state).await })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_means_no_work_no_writes_nothing_on_the_card_and_no_connections() {
        let idle = ZeroProc { cpu: 0.02, memory_mb: 80.0, ..Default::default() };
        assert!(at_zero(&total(&[idle.clone(), idle.clone()])), "memory held isn't work");
        assert!(!at_zero(&total(&[idle.clone(), ZeroProc { written_kb: 4.0, ..Default::default() }])));
        assert!(!at_zero(&total(&[idle.clone(), ZeroProc { connections: 1, ..Default::default() }])));
        assert!(!at_zero(&total(&[ZeroProc { card_mb: 31.0, ..Default::default() }])));
        assert!(!at_zero(&total(&[ZeroProc { cpu: 0.06, ..Default::default() }, ZeroProc { cpu: 0.06, ..Default::default() }])), "small parts add up");
    }

    #[test]
    fn this_process_shows_up_with_its_memory() {
        let (_, first) = processes();
        assert!(first.iter().any(|p| p.pid == std::process::id() && p.memory_mb > 1.0));
        let (seconds, _) = processes();
        assert!(seconds >= 0.0);
    }
}
