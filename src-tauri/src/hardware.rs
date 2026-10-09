// SPDX-License-Identifier: AGPL-3.0-only
//! Detects what this PC has: graphics cards, memory, processor, disk, power.

use std::path::Path;

use serde::Serialize;
use sysinfo::{Disks, System};

#[derive(Debug, Clone, Serialize, Default)]
pub struct Gpu {
    pub name: String,
    pub vendor: String,
    pub vram_bytes: u64,
    /// Integrated graphics share system memory and aren't used for models yet.
    pub integrated: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Hardware {
    pub os: String,
    pub cpu_name: String,
    pub cpu_threads: usize,
    pub cpu_cores: Option<usize>,
    pub avx2: bool,
    pub avx512: bool,
    pub ram_total: u64,
    pub ram_available: u64,
    pub gpus: Vec<Gpu>,
    pub models_drive: String,
    pub disk_free: u64,
    pub disk_total: u64,
    pub on_battery: Option<bool>,
}

impl Hardware {
    /// The discrete GPU with the most dedicated memory.
    pub fn best_gpu(&self) -> Option<&Gpu> {
        self.gpus.iter().filter(|g| !g.integrated).max_by_key(|g| g.vram_bytes)
    }
}

pub fn detect(models_dir: &Path) -> Hardware {
    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu_all();

    let (models_drive, disk_free, disk_total) = disk_for(models_dir);

    Hardware {
        os: System::long_os_version().unwrap_or_else(|| std::env::consts::OS.to_string()),
        cpu_name: sys.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default(),
        cpu_threads: sys.cpus().len(),
        cpu_cores: sys.physical_core_count(),
        avx2: cpu_feature("avx2"),
        avx512: cpu_feature("avx512f"),
        ram_total: sys.total_memory(),
        ram_available: sys.available_memory(),
        gpus: gpus(sys.total_memory()),
        models_drive,
        disk_free,
        disk_total,
        on_battery: on_battery(),
    }
}

fn disk_for(dir: &Path) -> (String, u64, u64) {
    let disks = Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|d| dir.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| (d.mount_point().display().to_string(), d.available_space(), d.total_space()))
        .unwrap_or_default()
}

fn cpu_feature(name: &str) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        match name {
            "avx2" => std::arch::is_x86_feature_detected!("avx2"),
            "avx512f" => std::arch::is_x86_feature_detected!("avx512f"),
            _ => false,
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = name;
        false
    }
}

#[cfg(windows)]
fn gpus(_ram: u64) -> Vec<Gpu> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};

    const MICROSOFT: u32 = 0x1414; // Basic Render Driver and remote adapters
    let mut out: Vec<Gpu> = Vec::new();
    // SAFETY: plain COM calls; every returned interface is owned and released on drop.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return out;
        };
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let Ok(desc) = adapter.GetDesc1() else { continue };
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 || desc.VendorId == MICROSOFT {
                continue;
            }
            let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
            let name = String::from_utf16_lossy(&desc.Description[..len]);
            let vram = desc.DedicatedVideoMemory as u64;
            if out.iter().any(|g| g.name == name && g.vram_bytes == vram) {
                continue; // same card listed once per output
            }
            out.push(Gpu {
                vendor: vendor_name(desc.VendorId).to_string(),
                // Integrated chips report a small carve-out of system memory.
                integrated: vram < crate::catalog::GIB,
                vram_bytes: vram,
                name,
            });
        }
    }
    out
}

/// Linux: NVIDIA cards from `nvidia-smi`; AMD and Intel from sysfs (AMD
/// reports its memory there; Intel graphics share system memory).
#[cfg(target_os = "linux")]
fn gpus(_ram: u64) -> Vec<Gpu> {
    let mut out: Vec<Gpu> = Vec::new();
    if let Ok(o) = std::process::Command::new("nvidia-smi").args(["--query-gpu=name,memory.total", "--format=csv,noheader,nounits"]).output() {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            if let Some((name, mib)) = line.rsplit_once(',') {
                if let Ok(mib) = mib.trim().parse::<u64>() {
                    out.push(Gpu { name: name.trim().to_string(), vendor: "NVIDIA".into(), vram_bytes: mib * 1024 * 1024, integrated: false });
                }
            }
        }
    }
    let Ok(cards) = std::fs::read_dir("/sys/class/drm") else { return out };
    for card in cards.flatten() {
        let name = card.file_name().to_string_lossy().to_string();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let dev = card.path().join("device");
        let read = |f: &str| std::fs::read_to_string(dev.join(f)).map(|s| s.trim().to_string()).unwrap_or_default();
        let vendor = u32::from_str_radix(read("vendor").trim_start_matches("0x"), 16).unwrap_or(0);
        match vendor {
            0x1002 => {
                let vram: u64 = read("mem_info_vram_total").parse().unwrap_or(0);
                let product = read("product_name");
                out.push(Gpu {
                    name: if product.is_empty() { "AMD graphics".into() } else { product },
                    vendor: "AMD".into(),
                    integrated: vram < crate::catalog::GIB,
                    vram_bytes: vram,
                });
            }
            0x8086 => out.push(Gpu { name: "Intel graphics".into(), vendor: "Intel".into(), vram_bytes: 0, integrated: true }),
            _ => {}
        }
    }
    out
}

/// Apple silicon: the GPU shares memory with the processor, and Metal lets
/// it use about two thirds of it. Intel Macs run models on the processor.
#[cfg(target_os = "macos")]
fn gpus(ram: u64) -> Vec<Gpu> {
    if cfg!(target_arch = "aarch64") {
        vec![Gpu { name: "Apple GPU".into(), vendor: "Apple".into(), vram_bytes: ram / 3 * 2, integrated: false }]
    } else {
        Vec::new()
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn gpus(_ram: u64) -> Vec<Gpu> {
    Vec::new()
}

#[cfg_attr(not(windows), allow(dead_code))]
fn vendor_name(id: u32) -> &'static str {
    match id {
        0x10DE => "NVIDIA",
        0x1002 | 0x1022 => "AMD",
        0x8086 => "Intel",
        0x5143 => "Qualcomm",
        _ => "Other",
    }
}

/// Whether the PC has a battery at all (a laptop or tablet).
#[cfg(windows)]
pub fn has_battery() -> bool {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut s = SYSTEM_POWER_STATUS::default();
    // SAFETY: writes into the struct we own.
    if unsafe { GetSystemPowerStatus(&mut s) }.is_err() {
        return false;
    }
    const NO_BATTERY: u8 = 128;
    const UNKNOWN: u8 = 255;
    s.BatteryFlag != NO_BATTERY && s.BatteryFlag != UNKNOWN
}

#[cfg(not(windows))]
pub fn has_battery() -> bool {
    false
}

#[cfg(windows)]
pub fn on_battery() -> Option<bool> {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut s = SYSTEM_POWER_STATUS::default();
    // SAFETY: writes into the struct we own.
    unsafe { GetSystemPowerStatus(&mut s) }.ok()?;
    const NO_BATTERY: u8 = 128;
    if s.BatteryFlag == NO_BATTERY {
        return Some(false);
    }
    match s.ACLineStatus {
        0 => Some(true),
        1 => Some(false),
        _ => None,
    }
}

#[cfg(not(windows))]
pub fn on_battery() -> Option<bool> {
    None
}
