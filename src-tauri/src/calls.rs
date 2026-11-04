// SPDX-License-Identifier: AGPL-3.0-only
//! Call reminders: when a calling app (Teams, Zoom, Webex…) starts using the
//! microphone, a notification offers meeting notes and reminds the person to
//! tell everyone first. Windows records which apps use the microphone in the
//! registry; the app is told when that changes, so nothing runs on a timer.
//! Off until turned on in Features. Nothing is recorded by this; it only
//! reminds.

use std::collections::HashSet;

/// Calling apps, by a piece of the name Windows files them under.
const CALL_APPS: &[(&str, &str)] = &[
    ("msteams", "Microsoft Teams"),
    ("teams.exe", "Microsoft Teams"),
    ("zoom", "Zoom"),
    ("webex", "Webex"),
    ("slack", "Slack"),
    ("discord", "Discord"),
    ("skype", "Skype"),
    ("gotomeeting", "GoTo Meeting"),
    ("ringcentral", "RingCentral"),
];

/// The calling app a registry entry belongs to.
pub fn call_app(entry: &str) -> Option<&'static str> {
    let name = entry.to_lowercase();
    CALL_APPS.iter().find(|(needle, _)| name.contains(needle)).map(|(_, shown)| *shown)
}

/// Windows writes a start time when an app begins using the microphone and
/// a stop time when it ends; in use means it started after it last stopped.
pub fn in_use(start: u64, stop: u64) -> bool {
    start > 0 && start > stop
}

/// Apps that newly started a call since the last look.
pub fn newly_started(before: &HashSet<&'static str>, now: &HashSet<&'static str>) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = now.difference(before).copied().collect();
    out.sort();
    out
}

#[cfg(windows)]
mod watch {
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    use tauri::AppHandle;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Registry::{RegCloseKey, RegEnumKeyExW, RegNotifyChangeKeyValue, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, KEY_READ, REG_NOTIFY_CHANGE_LAST_SET};
    use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects, INFINITE};

    const MIC: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";

    /// The stop event of the watcher running now, as an integer for a static.
    static STOP: Mutex<Option<isize>> = Mutex::new(None);
    static RUNNING: AtomicBool = AtomicBool::new(false);

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn open(parent: HKEY, path: &str, access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Option<HKEY> {
        let mut key = HKEY::default();
        let p = wide(path);
        // SAFETY: a valid parent key and a NUL-terminated path; the key is
        // closed by the caller.
        unsafe { RegOpenKeyExW(parent, PCWSTR(p.as_ptr()), 0, access, &mut key).is_ok().then_some(key) }
    }

    fn subkeys(key: HKEY) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0..4096u32 {
            let mut buf = [0u16; 512];
            let mut len = buf.len() as u32;
            // SAFETY: the buffer and its length match; enumeration stops at the first error.
            let r = unsafe { RegEnumKeyExW(key, i, PWSTR(buf.as_mut_ptr()), &mut len, None, PWSTR::null(), None, None) };
            if r.is_err() {
                break;
            }
            out.push(String::from_utf16_lossy(&buf[..len as usize]));
        }
        out
    }

    fn qword(key: HKEY, name: &str) -> u64 {
        let n = wide(name);
        let mut data = [0u8; 8];
        let mut len = 8u32;
        // SAFETY: an 8-byte buffer for a REG_QWORD; a missing value leaves 0.
        let ok = unsafe { RegQueryValueExW(key, PCWSTR(n.as_ptr()), None, None, Some(data.as_mut_ptr()), Some(&mut len)) }.is_ok();
        if ok && len == 8 { u64::from_le_bytes(data) } else { 0 }
    }

    /// Calling apps using the microphone right now.
    fn calling(mic: HKEY) -> HashSet<&'static str> {
        let mut found = HashSet::new();
        let mut check = |parent: HKEY, name: &str| {
            let Some(app) = super::call_app(name) else { return };
            if let Some(k) = open(parent, name, KEY_READ) {
                if super::in_use(qword(k, "LastUsedTimeStart"), qword(k, "LastUsedTimeStop")) {
                    found.insert(app);
                }
                // SAFETY: closing a key this function opened.
                unsafe {
                    let _ = RegCloseKey(k);
                }
            }
        };
        for name in subkeys(mic) {
            if name.eq_ignore_ascii_case("NonPackaged") {
                if let Some(np) = open(mic, &name, KEY_READ) {
                    for inner in subkeys(np) {
                        check(np, &inner);
                    }
                    // SAFETY: closing a key opened just above.
                    unsafe {
                        let _ = RegCloseKey(np);
                    }
                }
            } else {
                check(mic, &name);
            }
        }
        found
    }

    pub fn start(app: AppHandle) {
        if RUNNING.swap(true, Ordering::SeqCst) {
            return;
        }
        std::thread::spawn(move || {
            run(&app);
            RUNNING.store(false, Ordering::SeqCst);
        });
    }

    fn run(app: &AppHandle) {
        let Some(mic) = open(HKEY_CURRENT_USER, MIC, KEY_READ | KEY_NOTIFY) else { return };
        // SAFETY: unnamed auto-reset events owned by this thread, closed below.
        let (changed, stop) = unsafe {
            match (CreateEventW(None, false, false, None), CreateEventW(None, true, false, None)) {
                (Ok(a), Ok(b)) => (a, b),
                _ => {
                    let _ = RegCloseKey(mic);
                    return;
                }
            }
        };
        *STOP.lock().unwrap() = Some(stop.0 as isize);
        let mut before = calling(mic);
        loop {
            // SAFETY: asks Windows to signal `changed` on the next change
            // under the key; both handles stay open while waited on.
            let armed = unsafe { RegNotifyChangeKeyValue(mic, true, REG_NOTIFY_CHANGE_LAST_SET, changed, true) }.is_ok();
            if !armed {
                break;
            }
            // SAFETY: two valid event handles.
            let which = unsafe { WaitForMultipleObjects(&[changed, stop], false, INFINITE) };
            if which != WAIT_OBJECT_0 {
                break;
            }
            let now = calling(mic);
            for name in super::newly_started(&before, &now) {
                crate::notify::show(app, &format!("A call started in {name}"), "Want notes? Open Meetings in SulcusAI and start recording. Tell everyone first: in some places everyone must agree to be recorded.");
            }
            before = now;
        }
        *STOP.lock().unwrap() = None;
        // SAFETY: closing what this function opened.
        unsafe {
            let _ = CloseHandle(changed);
            let _ = CloseHandle(stop);
            let _ = RegCloseKey(mic);
        }
    }

    /// Every app Windows lists as having used the microphone, with whether
    /// it's using it now, for checking the reading on a real PC.
    #[cfg(test)]
    pub(super) fn listed() -> Vec<(String, bool)> {
        let Some(mic) = open(HKEY_CURRENT_USER, MIC, KEY_READ) else { return Vec::new() };
        let mut out = Vec::new();
        for name in subkeys(mic) {
            let Some(k) = open(mic, &name, KEY_READ) else { continue };
            out.push((name, super::in_use(qword(k, "LastUsedTimeStart"), qword(k, "LastUsedTimeStop"))));
            // SAFETY: closing a key opened just above.
            unsafe {
                let _ = RegCloseKey(k);
            }
        }
        // SAFETY: closing the key opened at the start.
        unsafe {
            let _ = RegCloseKey(mic);
        }
        out
    }

    pub fn stop() {
        if let Some(h) = *STOP.lock().unwrap() {
            // SAFETY: the watcher's stop event, still open while STOP holds it.
            unsafe {
                let _ = SetEvent(HANDLE(h as *mut _));
            }
        }
    }
}

#[cfg(not(windows))]
mod watch {
    pub fn start(_app: tauri::AppHandle) {}
    pub fn stop() {}
}

/// Watches while the switch is on; called at start and when it changes.
pub fn apply(app: &tauri::AppHandle, state: &crate::AppState) {
    if crate::features::is_on(&state.db.lock().unwrap(), crate::features::Feature::CallReminder) {
        watch::start(app.clone());
    } else {
        watch::stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calling_apps_are_known_by_their_entries() {
        assert_eq!(call_app("C:#Users#me#AppData#Roaming#Zoom#bin#Zoom.exe"), Some("Zoom"));
        assert_eq!(call_app("MSTeams_8wekyb3d8bbwe"), Some("Microsoft Teams"));
        assert_eq!(call_app("C:#Program Files (x86)#Steam#steam.exe"), None);
        assert!(in_use(200, 100) && !in_use(100, 200) && !in_use(0, 0));
        let before: HashSet<&str> = ["Zoom"].into_iter().collect();
        let now: HashSet<&str> = ["Zoom", "Slack"].into_iter().collect();
        assert_eq!(newly_started(&before, &now), vec!["Slack"]);
        assert!(newly_started(&now, &before).is_empty(), "a call ending says nothing");
    }

    /// Reads this PC's real microphone list (read-only).
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn e2e_reads_the_microphone_list() {
        let listed = watch::listed();
        for (name, now) in &listed {
            println!("{name}: {}", if *now { "in use" } else { "not in use" });
        }
        assert!(!listed.is_empty());
    }
}
