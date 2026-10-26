// SPDX-License-Identifier: AGPL-3.0-only
//! The graphics drivers, checked when SulcusAI starts. Only a driver that
//! needs updating is brought up: one too old for what SulcusAI runs on the
//! card (NVIDIA's before 580 can't run CUDA 13, so song and voice work goes
//! through DirectML, a little slower), one more than a year old, or a card
//! running without its driver. Each says why and where to get the driver;
//! one the person put off ("Not now") isn't brought up again until a newer
//! driver is installed or it gets older still.

use std::sync::Arc;

use serde::Serialize;

use crate::{AppState, AppStateRef};

/// Days before a driver counts as old.
const OLD_DAYS: i64 = 365;
/// Where the drivers put off are kept (their `key`s).
const PUT_OFF: &str = "drivers_put_off";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DriverNote {
    pub card: String,
    pub vendor: String,
    /// The driver's version as its maker numbers it ("566.36"), when known.
    pub version: Option<String>,
    /// Its release date (YYYY-MM-DD), when Windows knows it.
    pub date: Option<String>,
    /// "missing", "too_old" (too old for what SulcusAI runs on it) or "old".
    pub need: String,
    pub why: String,
    /// The maker's driver page.
    pub link: String,
    /// What putting it off remembers: the card and its driver.
    pub key: String,
}

/// A card's driver as Windows lists it: version and date (YYYYMMDD).
#[derive(Debug, Clone, Default)]
struct Listed {
    name: String,
    version: String,
    date: String,
}

fn link_for(vendor: &str) -> &'static str {
    match vendor {
        "NVIDIA" => "https://www.nvidia.com/en-us/drivers/",
        "AMD" => "https://www.amd.com/en/support/download/drivers.html",
        "Intel" => "https://www.intel.com/content/www/us/en/support/detect.html",
        _ => "https://support.microsoft.com/windows/update-drivers-manually-in-windows-ec62f46c-ff14-c91d-eead-d7126dc1f7b6",
    }
}

/// The notes for these cards (pure, for testing): `listed` from Windows,
/// `nvidia` versions from nvidia-smi by card name, `today` as days.
fn notes_for(cards: &[crate::hardware::Gpu], broken: &[crate::hardware::BrokenCard], listed: &[Listed], nvidia: &[(String, String)], min_nvidia: f64, today: chrono::NaiveDate) -> Vec<DriverNote> {
    let mut out = Vec::new();
    for b in broken {
        out.push(DriverNote {
            card: b.name.clone(),
            vendor: b.vendor.clone(),
            version: None,
            date: None,
            need: "missing".into(),
            why: "Windows lists this card without its driver working, so SulcusAI can't use it: everything runs on the processor".into(),
            link: link_for(&b.vendor).into(),
            key: format!("{}|missing", b.name),
        });
    }
    for g in cards.iter().filter(|g| !g.integrated) {
        let row = listed.iter().find(|l| crate::hardware::same_name(&l.name, &g.name));
        let smi = nvidia.iter().find(|(n, _)| crate::hardware::same_name(n, &g.name)).map(|(_, v)| v.clone());
        let version = smi.clone().or_else(|| row.map(|r| r.version.clone()).filter(|v| !v.is_empty()));
        let date = row.and_then(|r| chrono::NaiveDate::parse_from_str(r.date.get(..8)?, "%Y%m%d").ok());
        let key = format!("{}|{}", g.name, version.clone().unwrap_or_default());
        let note = |need: &str, why: String| DriverNote {
            card: g.name.clone(),
            vendor: g.vendor.clone(),
            version: version.clone(),
            date: date.map(|d| d.format("%Y-%m-%d").to_string()),
            need: need.into(),
            why,
            link: link_for(&g.vendor).into(),
            key: key.clone(),
        };
        let nv: Option<f64> = smi.as_deref().and_then(|v| v.parse().ok());
        if g.vendor == "NVIDIA" && nv.is_some_and(|v| v < min_nvidia) {
            out.push(note(
                "too_old",
                format!("Its driver ({}) is older than {min_nvidia:.0}, the first that runs CUDA 13: song and voice work runs through DirectML instead, a little slower", smi.unwrap_or_default()),
            ));
        } else if let Some(d) = date.filter(|d| (today - *d).num_days() > OLD_DAYS) {
            out.push(note("old", format!("Its driver is from {}, more than a year ago: a newer one is often faster and fixes problems with AI work", d.format("%B %Y"))));
        }
    }
    out
}

/// The drivers that need updating now (put-off ones left out).
pub async fn check(state: &Arc<AppState>) -> Vec<DriverNote> {
    let (cards, broken) = {
        let hw = state.hardware.read().unwrap();
        (hw.gpus.clone(), hw.broken_cards.clone())
    };
    let nvidia: Vec<(String, String)> = if cards.iter().any(|g| g.vendor == "NVIDIA") { crate::natural::nvidia_smi().await.into_iter().map(|s| (s.name, s.driver)).collect() } else { Vec::new() };
    let listed = tokio::task::spawn_blocking(listed_drivers).await.unwrap_or_default();
    let min = state.catalog.voices.cards.as_ref().map_or(580.0, |r| r.min_driver);
    let put_off: Vec<String> = crate::db::get(&state.db.lock().unwrap(), PUT_OFF).unwrap_or_default();
    let notes: Vec<DriverNote> = notes_for(&cards, &broken, &listed, &nvidia, min, chrono::Local::now().date_naive()).into_iter().filter(|n| !put_off.contains(&format!("{}|{}", n.key, n.need))).collect();
    for n in &notes {
        crate::applog::line("card", &format!("Driver for {} needs updating ({}): {}", n.card, n.need, n.why));
    }
    notes
}

#[tauri::command(async)]
pub async fn driver_check(state: AppStateRef<'_>) -> Result<Vec<DriverNote>, String> {
    Ok(check(&state).await)
}

/// "Not now": this card's driver isn't brought up again for this reason.
#[tauri::command]
pub fn driver_put_off(state: AppStateRef, key: String, need: String) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    let mut put_off: Vec<String> = crate::db::get(&conn, PUT_OFF).unwrap_or_default();
    put_off.push(format!("{key}|{need}"));
    crate::db::set(&conn, PUT_OFF, &put_off)
}

#[cfg(windows)]
fn listed_drivers() -> Vec<Listed> {
    std::thread::spawn(|| unsafe {
        let found = wmi_video_controllers();
        windows::Win32::System::Com::CoUninitialize();
        found
    })
    .join()
    .ok()
    .flatten()
    .unwrap_or_default()
}

#[cfg(not(windows))]
fn listed_drivers() -> Vec<Listed> {
    Vec::new()
}

#[cfg(windows)]
unsafe fn wmi_video_controllers() -> Option<Vec<Listed>> {
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
        .ExecQuery(&BSTR::from("WQL"), &BSTR::from("SELECT Name, DriverVersion, DriverDate FROM Win32_VideoController"), WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY, None)
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
        out.push(Listed { name: text(windows::core::w!("Name")), version: text(windows::core::w!("DriverVersion")), date: text(windows::core::w!("DriverDate")) });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::{BrokenCard, Gpu};

    /// This PC's drivers as Windows and nvidia-smi list them, and what's said.
    ///     cargo test real_drivers_on_this_pc -- --ignored --nocapture
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn real_drivers_on_this_pc() {
        for l in listed_drivers() {
            println!("listed: {} · {} · {}", l.name, l.version, l.date);
        }
        let state = crate::e2e::temp_state(None);
        println!("notes: {:#?}", check(&state).await);
    }

    #[test]
    fn only_drivers_that_need_updating_are_brought_up() {
        let card = |name: &str, vendor: &str| Gpu { name: name.into(), vendor: vendor.into(), vram_bytes: 8 << 30, integrated: false, luid: None, dxgi: None };
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let listed = [
            Listed { name: "NVIDIA GeForce RTX 4090 Laptop GPU".into(), version: "32.0.15.6636".into(), date: "20250115000000.000000-000".into() },
            Listed { name: "NVIDIA GeForce RTX 5070".into(), version: "32.0.15.8157".into(), date: "20260918000000.000000-000".into() },
            Listed { name: "AMD Radeon RX 6700 XT".into(), version: "31.0.21001.45002".into(), date: "20240610000000.000000-000".into() },
            Listed { name: "Intel(R) UHD Graphics 770".into(), version: "31.0.101.4502".into(), date: "20230601000000.000000-000".into() },
        ];
        let nvidia = [("NVIDIA GeForce RTX 4090 Laptop GPU".to_string(), "566.36".to_string()), ("NVIDIA GeForce RTX 5070".to_string(), "581.57".to_string())];
        let mut igpu = card("Intel(R) UHD Graphics 770", "Intel");
        igpu.integrated = true;
        let cards = [card("NVIDIA GeForce RTX 4090 Laptop GPU", "NVIDIA"), card("NVIDIA GeForce RTX 5070", "NVIDIA"), card("AMD Radeon RX 6700 XT", "AMD"), igpu];
        let broken = [BrokenCard { name: "NVIDIA GeForce GTX 1080 Ti".into(), vendor: "NVIDIA".into(), code: 28 }];
        let notes = notes_for(&cards, &broken, &listed, &nvidia, 580.0, today);
        let by = |n: &str| notes.iter().find(|x| x.card == n).map(|x| x.need.as_str());
        assert_eq!(by("NVIDIA GeForce RTX 4090 Laptop GPU"), Some("too_old"));
        assert_eq!(by("NVIDIA GeForce RTX 5070"), None, "a new driver is left alone");
        assert_eq!(by("AMD Radeon RX 6700 XT"), Some("old"));
        assert_eq!(by("Intel(R) UHD Graphics 770"), None, "built-in graphics aren't used");
        assert_eq!(by("NVIDIA GeForce GTX 1080 Ti"), Some("missing"));
        assert_eq!(notes.iter().find(|x| x.need == "too_old").unwrap().version.as_deref(), Some("566.36"));
        assert!(notes.iter().all(|n| n.link.starts_with("https://")));
    }
}
