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
    /// Windows' id for the card (DXGI adapter LUID), to read its usage and temperature.
    #[serde(skip)]
    pub luid: Option<i64>,
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
    /// Graphics cards Windows lists that can't be used: no driver (shown as
    /// "Microsoft Basic Display Adapter"), stopped or turned off. Models then
    /// run on the processor although the PC has a card.
    pub broken_cards: Vec<BrokenCard>,
    pub models_drive: String,
    pub disk_free: u64,
    pub disk_total: u64,
    pub on_battery: Option<bool>,
    /// The card the person picked for models, by name (None: automatic, the
    /// dedicated card with the most memory). A picked card that isn't found
    /// counts as automatic.
    pub main: Option<String>,
}

/// The card the person picked, by name: kept by name rather than by number,
/// since numbers change with drivers and with cards added or removed.
static MAIN: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Sets the card models use (None: automatic). Takes effect at the next
/// check of the PC and for engines started from now on.
pub fn set_main(name: Option<String>) {
    *MAIN.lock().unwrap() = name;
}

/// The card the person picked (None: automatic).
pub fn main_choice() -> Option<String> {
    MAIN.lock().unwrap().clone()
}

/// The card models use: the dedicated card picked by name when it's there,
/// else the dedicated card with the most memory. Integrated graphics never.
pub fn main_of<'a>(gpus: &'a [Gpu], chosen: Option<&str>) -> Option<&'a Gpu> {
    let dedicated = || gpus.iter().filter(|g| !g.integrated);
    chosen.and_then(|name| dedicated().find(|g| g.name == name)).or_else(|| dedicated().max_by_key(|g| g.vram_bytes))
}

/// A graphics card that's there but not working.
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct BrokenCard {
    /// What Windows calls it ("Microsoft Basic Display Adapter", "Video Controller").
    pub name: String,
    /// The maker, from its hardware id ("NVIDIA"), when known.
    pub vendor: String,
    /// Device Manager's problem code: 0 when Windows reports none (the
    /// stand-in driver runs it), 22 turned off, 28 no driver, 43 stopped.
    pub code: u32,
}

/// An adapter as Windows describes it, for `broken_of`.
#[derive(Debug, Clone, Default)]
pub struct AdapterInfo {
    pub name: String,
    /// Its hardware id ("PCI\VEN_10DE&DEV_1F99…"); empty when unknown (DXGI).
    pub pnp_id: String,
    /// Device Manager's problem code (0: working).
    pub problem: u32,
    /// Its device class ("Display"), when it has one.
    pub class: String,
}

/// Of the adapters Windows lists, the graphics cards that can't be used:
/// one running on Microsoft's stand-in driver (Basic Display Adapter), or a
/// display device with a problem (no driver, stopped, turned off). Virtual
/// and remote adapters, and Microsoft's software renderer, aren't cards.
pub fn broken_of(adapters: &[AdapterInfo]) -> Vec<BrokenCard> {
    let mut out: Vec<BrokenCard> = Vec::new();
    for a in adapters {
        let name = a.name.to_lowercase();
        let pnp = a.pnp_id.to_uppercase();
        // Hardware on the PCI bus, or unknown (what DXGI reports).
        let hardware = pnp.is_empty() || pnp.starts_with("PCI\\");
        let stand_in = name.contains("basic display");
        let display = a.class.eq_ignore_ascii_case("display") || name.contains("video controller") || name.contains("display") || name.contains("graphics");
        let broken = stand_in || (a.problem != 0 && display);
        if !hardware || !broken || name.contains("basic render") || name.contains("remote") || name.contains("hyper-v") || name.contains("virtual") {
            continue;
        }
        let vendor = pnp.split("VEN_").nth(1).and_then(|v| u32::from_str_radix(v.get(..4)?, 16).ok()).map_or("", vendor_name);
        let card = BrokenCard { name: a.name.trim().to_string(), vendor: if vendor == "Other" { String::new() } else { vendor.to_string() }, code: a.problem };
        // DXGI and Device Manager can both list the same one.
        if let Some(seen) = out.iter_mut().find(|c| c.name == card.name && (c.vendor.is_empty() || card.vendor.is_empty() || c.vendor == card.vendor)) {
            if seen.vendor.is_empty() {
                seen.vendor = card.vendor;
            }
            seen.code = seen.code.max(card.code);
            continue;
        }
        out.push(card);
    }
    out
}

/// The cards found last, for code that runs without the app state (engine
/// launches, the temperature watch).
static CARDS: std::sync::Mutex<Vec<Gpu>> = std::sync::Mutex::new(Vec::new());

/// The graphics cards found when the PC was last checked.
pub fn cards() -> Vec<Gpu> {
    CARDS.lock().unwrap().clone()
}

/// The dedicated card models use (`main_of`), if there is one.
pub fn dedicated() -> Option<Gpu> {
    main_of(&cards(), main_choice().as_deref()).cloned()
}

impl Hardware {
    /// The dedicated card models use: the one picked, else the one with the
    /// most memory. Its memory is what models are fitted to.
    pub fn best_gpu(&self) -> Option<&Gpu> {
        main_of(&self.gpus, self.main.as_deref())
    }
}

pub fn detect(models_dir: &Path) -> Hardware {
    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu_all();

    let (models_drive, disk_free, disk_total) = disk_for(models_dir);
    let (found, mut adapters) = adapters(sys.total_memory());
    *CARDS.lock().unwrap() = found.clone();
    // Device Manager only when no dedicated card turned up: it's slower, and
    // only then does a card that isn't working explain anything.
    if !found.iter().any(|g| !g.integrated) {
        adapters.extend(problem_devices());
    }
    let broken = broken_of(&adapters);

    Hardware {
        os: System::long_os_version().unwrap_or_else(|| std::env::consts::OS.to_string()),
        cpu_name: sys.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default(),
        cpu_threads: sys.cpus().len(),
        cpu_cores: sys.physical_core_count(),
        avx2: cpu_feature("avx2"),
        avx512: cpu_feature("avx512f"),
        ram_total: sys.total_memory(),
        ram_available: sys.available_memory(),
        gpus: found.clone(),
        broken_cards: broken,
        models_drive,
        disk_free,
        disk_total,
        on_battery: on_battery(),
        main: main_choice(),
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

/// The graphics cards found, and the adapters left out that may be a card
/// without its driver (Windows' stand-in, "Microsoft Basic Display Adapter").
#[cfg(not(windows))]
fn adapters(ram: u64) -> (Vec<Gpu>, Vec<AdapterInfo>) {
    (gpus(ram), Vec::new())
}

#[cfg(not(windows))]
fn problem_devices() -> Vec<AdapterInfo> {
    Vec::new()
}

/// Display devices Device Manager reports a problem for (no driver,
/// stopped, turned off), or that run on Windows' stand-in driver.
#[cfg(windows)]
fn problem_devices() -> Vec<AdapterInfo> {
    std::thread::spawn(|| unsafe {
        let found = wmi_display_devices();
        windows::Win32::System::Com::CoUninitialize();
        found
    })
    .join()
    .ok()
    .flatten()
    .unwrap_or_default()
}

#[cfg(windows)]
unsafe fn wmi_display_devices() -> Option<Vec<AdapterInfo>> {
    use windows::core::{BSTR, VARIANT};
    use windows::Win32::System::Com::*;
    use windows::Win32::System::Wmi::*;
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER).ok()?;
    let svc = locator.ConnectServer(&BSTR::from("ROOT\\CIMV2"), &BSTR::new(), &BSTR::new(), &BSTR::new(), 0, &BSTR::new(), None).ok()?;
    const RPC_C_AUTHN_WINNT: u32 = 10;
    const RPC_C_AUTHZ_NONE: u32 = 0;
    CoSetProxyBlanket(&svc, RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE, None, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, None, EOAC_NONE).ok()?;
    let rows = svc
        .ExecQuery(
            &BSTR::from("WQL"),
            &BSTR::from("SELECT Name, PNPDeviceID, ConfigManagerErrorCode, PNPClass FROM Win32_PnPEntity WHERE PNPClass = 'Display' OR ConfigManagerErrorCode <> 0"),
            WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
            None,
        )
        .ok()?;
    let mut out = Vec::new();
    loop {
        let mut objs = [None];
        let mut n = 0u32;
        let _ = rows.Next(WBEM_INFINITE, &mut objs, &mut n);
        if n == 0 {
            break;
        }
        let Some(obj) = objs[0].take() else { break };
        let text = |field: windows::core::PCWSTR| {
            let mut v = VARIANT::default();
            if obj.Get(field, 0, &mut v, None, None).is_ok() {
                BSTR::try_from(&v).map(|b| b.to_string()).unwrap_or_default()
            } else {
                String::new()
            }
        };
        let mut code = VARIANT::default();
        let problem = if obj.Get(windows::core::w!("ConfigManagerErrorCode"), 0, &mut code, None, None).is_ok() { u32::try_from(&code).unwrap_or(0) } else { 0 };
        out.push(AdapterInfo {
            name: text(windows::core::w!("Name")),
            pnp_id: text(windows::core::w!("PNPDeviceID")),
            problem,
            class: text(windows::core::w!("PNPClass")),
        });
    }
    Some(out)
}

#[cfg(windows)]
fn adapters(_ram: u64) -> (Vec<Gpu>, Vec<AdapterInfo>) {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};

    const MICROSOFT: u32 = 0x1414; // Basic Render Driver, Basic Display Adapter and remote adapters
    let mut out: Vec<Gpu> = Vec::new();
    let mut left_out: Vec<AdapterInfo> = Vec::new();
    // SAFETY: plain COM calls; every returned interface is owned and released on drop.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return (out, left_out);
        };
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let Ok(desc) = adapter.GetDesc1() else { continue };
            let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
            let name = String::from_utf16_lossy(&desc.Description[..len]);
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            // Microsoft's own: not a card models can use, but the stand-in
            // driver ("Microsoft Basic Display Adapter", whatever vendor id
            // it reports) means a card is there without its own driver.
            if desc.VendorId == MICROSOFT || name.to_lowercase().contains("basic display") {
                left_out.push(AdapterInfo { name, ..Default::default() });
                continue;
            }
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
                luid: Some(((desc.AdapterLuid.HighPart as i64) << 32) | desc.AdapterLuid.LowPart as i64),
            });
        }
    }
    (out, left_out)
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
                    out.push(Gpu { name: name.trim().to_string(), vendor: "NVIDIA".into(), vram_bytes: mib * 1024 * 1024, integrated: false, luid: None });
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
                    luid: None,
                });
            }
            0x8086 => out.push(Gpu { name: "Intel graphics".into(), vendor: "Intel".into(), vram_bytes: 0, integrated: true, luid: None }),
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
        vec![Gpu { name: "Apple GPU".into(), vendor: "Apple".into(), vram_bytes: ram / 3 * 2, integrated: false, luid: None }]
    } else {
        Vec::new()
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn gpus(_ram: u64) -> Vec<Gpu> {
    Vec::new()
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(name: &str, pnp: &str, problem: u32, class: &str) -> AdapterInfo {
        AdapterInfo { name: name.into(), pnp_id: pnp.into(), problem, class: class.into() }
    }

    /// A laptop whose dedicated card has no driver: Windows runs it on its
    /// stand-in driver, and the card used to vanish from the list unnoticed.
    #[test]
    fn a_card_without_its_driver_is_noticed() {
        // As DXGI reports it (no hardware id), and as Device Manager does.
        let dxgi = [adapter("Microsoft Basic Display Adapter", "", 0, "")];
        assert_eq!(broken_of(&dxgi), vec![BrokenCard { name: "Microsoft Basic Display Adapter".into(), vendor: String::new(), code: 0 }]);
        let both = [dxgi[0].clone(), adapter("Microsoft Basic Display Adapter", "PCI\\VEN_10DE&DEV_1F99&SUBSYS_13CF1043&REV_A1\\4&1234", 0, "Display")];
        assert_eq!(broken_of(&both), vec![BrokenCard { name: "Microsoft Basic Display Adapter".into(), vendor: "NVIDIA".into(), code: 0 }], "listed once, with its maker");
        // No driver at all (code 28), stopped (43) or turned off (22).
        let none = adapter("Video Controller (VGA Compatible)", "PCI\\VEN_1002&DEV_73DF&SUBSYS_0", 28, "");
        assert_eq!(broken_of(&[none]), vec![BrokenCard { name: "Video Controller (VGA Compatible)".into(), vendor: "AMD".into(), code: 28 }]);
        let stopped = adapter("NVIDIA GeForce RTX 3050 Laptop GPU", "PCI\\VEN_10DE&DEV_25A2", 43, "Display");
        assert_eq!(broken_of(&[stopped])[0].code, 43);
        let off = adapter("NVIDIA GeForce GTX 1650", "PCI\\VEN_10DE&DEV_1F91", 22, "Display");
        assert_eq!(broken_of(&[off])[0].vendor, "NVIDIA");
    }

    /// The card picked by name is the one models use; without a pick, or
    /// when the picked card is gone, the dedicated card with the most memory.
    #[test]
    fn the_picked_card_is_the_main_one() {
        let gpu = |name: &str, gb: u64, integrated: bool| Gpu { name: name.into(), vram_bytes: gb << 30, integrated, ..Default::default() };
        let desktop = [gpu("Intel(R) UHD Graphics 770", 0, true), gpu("NVIDIA GeForce RTX 3060", 12, false), gpu("NVIDIA GeForce RTX 4060", 8, false)];
        assert_eq!(main_of(&desktop, None).unwrap().name, "NVIDIA GeForce RTX 3060");
        assert_eq!(main_of(&desktop, Some("NVIDIA GeForce RTX 4060")).unwrap().name, "NVIDIA GeForce RTX 4060");
        assert_eq!(main_of(&desktop, Some("AMD Radeon RX 7600")).unwrap().name, "NVIDIA GeForce RTX 3060", "gone: automatic");
        assert_eq!(main_of(&desktop, Some("Intel(R) UHD Graphics 770")).unwrap().name, "NVIDIA GeForce RTX 3060", "never the integrated graphics");
        assert!(main_of(&desktop[..1], None).is_none());
        let hw = Hardware { gpus: desktop.to_vec(), main: Some("NVIDIA GeForce RTX 4060".into()), ..Default::default() };
        assert_eq!(hw.best_gpu().unwrap().vram_bytes, 8 << 30, "its memory is what models are fitted to");
    }

    #[test]
    fn working_and_virtual_adapters_are_not_broken() {
        let fine = [
            adapter("NVIDIA GeForce RTX 5070", "PCI\\VEN_10DE&DEV_2F04", 0, "Display"),
            adapter("Intel(R) UHD Graphics 770", "PCI\\VEN_8086&DEV_A780", 0, "Display"),
            // Microsoft's software renderer and remote or virtual adapters.
            adapter("Microsoft Basic Render Driver", "", 0, ""),
            adapter("Microsoft Remote Display Adapter", "SWD\\REMOTEDISPLAYENUM\\RDPIDD", 0, "Display"),
            adapter("Microsoft Hyper-V Video", "VMBUS\\{DA0A7802}", 0, "Display"),
            // Other devices with problems: not graphics cards.
            adapter("USB Composite Device", "USB\\VID_046D&PID_C52B", 28, "USB"),
            adapter("PCI Simple Communications Controller", "PCI\\VEN_8086&DEV_A3BA", 28, ""),
            adapter("DisplayLink USB Graphics", "USB\\VID_17E9&PID_4301", 10, "Display"),
        ];
        assert!(broken_of(&fine).is_empty(), "{:?}", broken_of(&fine));
    }
}
