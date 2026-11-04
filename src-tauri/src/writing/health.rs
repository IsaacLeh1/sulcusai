// SPDX-License-Identifier: AGPL-3.0-only
//! PC health: a one-off look at disk space, memory, swap and how long the
//! PC has run since a restart, each with plain advice. It reads only; it
//! changes nothing and runs only when asked (no background watcher).

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Disk {
    pub mount: String,
    pub free: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Snapshot {
    pub disks: Vec<Disk>,
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub uptime_secs: u64,
    pub cpus: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Finding {
    /// "ok", "watch" or "act".
    pub level: &'static str,
    pub area: &'static str,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Health {
    pub snapshot: Snapshot,
    pub findings: Vec<Finding>,
    /// The worst level found.
    pub overall: &'static str,
}

const GB: f64 = 1024.0 * 1024.0 * 1024.0;

pub fn take() -> Snapshot {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let disks = sysinfo::Disks::new_with_refreshed_list()
        .iter()
        .filter(|d| d.total_space() > 0)
        .map(|d| Disk { mount: d.mount_point().display().to_string(), free: d.available_space(), total: d.total_space() })
        .collect();
    Snapshot {
        disks,
        mem_used: sys.used_memory(),
        mem_total: sys.total_memory(),
        swap_used: sys.used_swap(),
        swap_total: sys.total_swap(),
        uptime_secs: sysinfo::System::uptime(),
        cpus: std::thread::available_parallelism().map_or(1, |n| n.get()),
    }
}

pub fn judge(s: Snapshot) -> Health {
    let mut f = Vec::new();
    for d in &s.disks {
        let pct = if d.total == 0 { 100.0 } else { d.free as f64 * 100.0 / d.total as f64 };
        let free_gb = d.free as f64 / GB;
        let (level, text) = if free_gb < 5.0 || pct < 5.0 {
            ("act", format!("{} has {:.1} GB free ({:.0}%). Programs can fail to save. Clear downloads, empty the recycle bin, or move big files.", d.mount, free_gb, pct))
        } else if pct < 15.0 {
            ("watch", format!("{} has {:.1} GB free ({:.0}%). Worth tidying before it fills.", d.mount, free_gb, pct))
        } else {
            ("ok", format!("{} has {:.0} GB free ({:.0}%).", d.mount, free_gb, pct))
        };
        f.push(Finding { level, area: "disk", text });
    }
    if s.mem_total > 0 {
        let pct = s.mem_used as f64 * 100.0 / s.mem_total as f64;
        let (level, text) = if pct >= 92.0 {
            ("act", format!("Memory is {pct:.0}% used. Close programs you aren't using; the PC will feel slow until then."))
        } else if pct >= 80.0 {
            ("watch", format!("Memory is {pct:.0}% used."))
        } else {
            ("ok", format!("Memory is {pct:.0}% used."))
        };
        f.push(Finding { level, area: "memory", text });
    }
    if s.swap_total > 0 && s.swap_used as f64 > s.swap_total as f64 * 0.5 && s.swap_used as f64 > 2.0 * GB {
        f.push(Finding { level: "watch", area: "swap", text: format!("{:.1} GB of memory has spilled onto the disk, which is slow.", s.swap_used as f64 / GB) });
    }
    let days = s.uptime_secs / 86_400;
    let (level, text) = if days >= 14 {
        ("watch", format!("Running for {days} days without a restart. A restart finishes updates and frees memory."))
    } else {
        ("ok", format!("Running for {days} days since the last restart."))
    };
    f.push(Finding { level, area: "uptime", text });
    let overall = if f.iter().any(|x| x.level == "act") {
        "act"
    } else if f.iter().any(|x| x.level == "watch") {
        "watch"
    } else {
        "ok"
    };
    Health { snapshot: s, findings: f, overall }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(free: u64, used: u64) -> Snapshot {
        Snapshot {
            disks: vec![Disk { mount: "C:\\".into(), free, total: 500 * GB as u64 }],
            mem_used: used,
            mem_total: 100,
            swap_used: 0,
            swap_total: 0,
            uptime_secs: 3 * 86_400,
            cpus: 8,
        }
    }

    #[test]
    fn healthy_pc() {
        assert_eq!(judge(snap(200 * GB as u64, 40)).overall, "ok");
    }

    #[test]
    fn full_disk_needs_action() {
        let h = judge(snap(GB as u64, 40));
        assert_eq!(h.overall, "act");
        assert!(h.findings[0].text.contains("1.0 GB free"));
    }

    #[test]
    fn busy_memory_and_long_uptime_are_watched() {
        let mut s = snap(200 * GB as u64, 85);
        s.uptime_secs = 20 * 86_400;
        let h = judge(s);
        assert_eq!(h.overall, "watch");
        assert!(h.findings.iter().any(|f| f.area == "uptime" && f.level == "watch"));
    }

    #[test]
    fn a_real_snapshot_reads() {
        let s = take();
        assert!(s.mem_total > 0 && s.cpus > 0);
    }
}
