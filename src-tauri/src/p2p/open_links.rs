// SPDX-License-Identifier: AGPL-3.0-only
//! Opening `sulcus://share` links and `.sulcus` files from outside the app.
//! Windows learns the link type and file type only while File sharing is
//! on (per user, nothing system-wide), and forgets them when it goes off.
//! A second start with a link hands it to the running copy and ends.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tauri::{AppHandle, Emitter};

const MAX_FILE: u64 = 4 << 20;

/// What a start was asked to open: a link's text or a `.sulcus` file's.
pub fn opened(args: &[String]) -> Option<String> {
    for a in args.iter().skip(1) {
        if a.starts_with(sulcus_net::sharing::share::SCHEME) {
            return Some(a.clone());
        }
        let p = Path::new(a);
        if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("sulcus")) && std::fs::metadata(p).is_ok_and(|m| m.len() <= MAX_FILE) {
            return std::fs::read_to_string(p).ok();
        }
    }
    None
}

/// The one-slot handover file, in the user's own temp folder.
fn handover() -> PathBuf {
    std::env::temp_dir().join(if cfg!(test) { "SulcusAI-test-open.txt" } else { "SulcusAI-open.txt" })
}

#[cfg(windows)]
fn event_name() -> &'static str {
    if cfg!(test) {
        "Local\\SulcusAI-test.open"
    } else {
        "Local\\SulcusAI.open"
    }
}

/// Hands the text to a running copy. False when none is running.
#[cfg(windows)]
pub fn forward(text: &str) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenEventW, SetEvent, EVENT_MODIFY_STATE};
    // SAFETY: a named event opened, set and closed; no memory is shared.
    unsafe {
        let Ok(h) = OpenEventW(EVENT_MODIFY_STATE, false, &HSTRING::from(event_name())) else { return false };
        let ok = std::fs::write(handover(), text).is_ok() && SetEvent(h).is_ok();
        let _ = CloseHandle(h);
        ok
    }
}

#[cfg(not(windows))]
pub fn forward(_: &str) -> bool {
    false
}

/// Opened before the window could listen; the window asks for it once.
static PENDING: Mutex<Option<String>> = Mutex::new(None);

pub fn hold(text: String) {
    *PENDING.lock().unwrap() = Some(text);
}

#[tauri::command]
pub fn share_opened() -> Option<String> {
    PENDING.lock().unwrap().take()
}

/// In the running copy: shows each handed-over link in the window.
#[cfg(windows)]
pub fn listen(app: &AppHandle) {
    use windows::core::HSTRING;
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
    // SAFETY: an auto-reset event this process keeps for its life.
    let Ok(h) = (unsafe { CreateEventW(None, false, false, &HSTRING::from(event_name())) }) else { return };
    let raw = h.0 as isize;
    let app = app.clone();
    std::thread::Builder::new()
        .name("share-links".into())
        .spawn(move || loop {
            // SAFETY: waits on the event above, never closed.
            unsafe { WaitForSingleObject(windows::Win32::Foundation::HANDLE(raw as *mut _), INFINITE) };
            let path = handover();
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let _ = std::fs::remove_file(&path);
            hold(text.clone());
            let _ = app.emit("share:opened", text);
        })
        .ok();
}

#[cfg(not(windows))]
pub fn listen(_: &AppHandle) {}

/// The registry entries (per user) that send links and files here.
pub fn entries(exe: &str) -> Vec<(String, String, String)> {
    let cmd = format!("\"{exe}\" \"%1\"");
    let c = "HKCU\\Software\\Classes";
    vec![
        (format!("{c}\\sulcus"), String::new(), "URL:SulcusAI share link".into()),
        (format!("{c}\\sulcus"), "URL Protocol".into(), String::new()),
        (format!("{c}\\sulcus\\shell\\open\\command"), String::new(), cmd.clone()),
        (format!("{c}\\.sulcus"), String::new(), "SulcusAI.Share".into()),
        (format!("{c}\\SulcusAI.Share"), String::new(), "SulcusAI share".into()),
        (format!("{c}\\SulcusAI.Share\\shell\\open\\command"), String::new(), cmd),
    ]
}

/// Adds or removes the link and file types to match the File sharing switch.
#[cfg(windows)]
pub fn register(on: bool) {
    use std::os::windows::process::CommandExt;
    const NO_WINDOW: u32 = 0x0800_0000;
    let reg = |args: &[&str]| {
        let _ = std::process::Command::new("reg").args(args).creation_flags(NO_WINDOW).output();
    };
    if on {
        let Ok(exe) = std::env::current_exe() else { return };
        for (key, name, value) in entries(&exe.to_string_lossy()) {
            if name.is_empty() {
                reg(&["add", &key, "/ve", "/d", &value, "/f"]);
            } else {
                reg(&["add", &key, "/v", &name, "/d", &value, "/f"]);
            }
        }
    } else {
        for key in ["HKCU\\Software\\Classes\\sulcus", "HKCU\\Software\\Classes\\.sulcus", "HKCU\\Software\\Classes\\SulcusAI.Share"] {
            reg(&["delete", key, "/f"]);
        }
    }
}

#[cfg(not(windows))]
pub fn register(_: bool) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_and_files_on_the_command_line() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(opened(&a(&["x.exe", "sulcus://share/abc"])), Some("sulcus://share/abc".into()));
        assert_eq!(opened(&a(&["x.exe", "--pause"])), None);
        let f = std::env::temp_dir().join(format!("t{}.sulcus", std::process::id()));
        std::fs::write(&f, "{\"v\":1}").unwrap();
        assert_eq!(opened(&a(&["x.exe", &f.to_string_lossy()])), Some("{\"v\":1}".into()));
        let _ = std::fs::remove_file(f);
    }

    #[test]
    fn entries_point_at_this_program_per_user() {
        let e = entries("C:\\A\\sulcusai.exe");
        assert!(e.iter().all(|(k, _, _)| k.starts_with("HKCU\\Software\\Classes\\")));
        assert!(e.iter().any(|(_, n, _)| n == "URL Protocol"));
        assert!(e.iter().any(|(_, _, v)| v == "\"C:\\A\\sulcusai.exe\" \"%1\""));
    }

    #[test]
    fn a_held_link_is_taken_once() {
        hold("sulcus://share/x".into());
        assert_eq!(share_opened(), Some("sulcus://share/x".into()));
        assert_eq!(share_opened(), None);
    }
}
