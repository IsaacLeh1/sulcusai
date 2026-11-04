// SPDX-License-Identifier: AGPL-3.0-only
//! Opening files, folders and web addresses with the system.

/// Opens a file, folder or web address with the system: Explorer on
/// Windows, `open` on macOS, `xdg-open` on Linux.
pub(crate) fn open_with_system(target: &std::ffi::OsStr) -> Result<(), String> {
    #[cfg(windows)]
    if let Some(url) = target.to_str().filter(|t| t.starts_with("https://") || t.starts_with("http://")) {
        return open_url_windows(url);
    }
    let program = if cfg!(windows) {
        "explorer.exe"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program).arg(target).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Web addresses go to the default browser through the shell: Explorer
/// opens a File Explorer window instead when an address is long, such as
/// a sign-in page's.
#[cfg(windows)]
fn open_url_windows(url: &str) -> Result<(), String> {
    use windows::core::{w, HSTRING};
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let url = HSTRING::from(url);
    // The shell wants a single-threaded COM apartment: use a thread of its own.
    std::thread::spawn(move || {
        // SAFETY: plain Win32 calls with valid, NUL-terminated strings; COM
        // is initialized for this thread only and released before it ends.
        unsafe {
            let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok();
            let r = ShellExecuteW(None, w!("open"), &url, None, None, SW_SHOWNORMAL);
            if com {
                CoUninitialize();
            }
            // Values above 32 mean success.
            if r.0 as isize > 32 { Ok(()) } else { Err(format!("Couldn't open your browser (error {}).", r.0 as isize)) }
        }
    })
    .join()
    .map_err(|_| "Couldn't open your browser.".to_string())?
}
