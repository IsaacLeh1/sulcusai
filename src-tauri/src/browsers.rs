// SPDX-License-Identifier: AGPL-3.0-only
//! Your main browser: where links and the 🌐 button open. The built-in
//! browser (the one the assistant can use), Windows' default, or any
//! browser Windows has registered (Chrome, Edge, DuckDuckGo, Firefox…).
//!
//! The choice is stored by name only; the program is looked up in the
//! registry again each time, so a stored setting can't point at anything
//! else.

use serde::{Deserialize, Serialize};

use crate::{db, AppStateRef};

const KEY: &str = "main_browser";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MainBrowser {
    /// SulcusAI's own browser.
    Builtin,
    /// Whatever Windows opens links with.
    System,
    /// A registered browser, by its registry name.
    App { name: String },
}

impl Default for MainBrowser {
    fn default() -> Self {
        MainBrowser::Builtin
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Installed {
    pub name: String,
    #[serde(skip)]
    pub exe: String,
}

/// Browsers registered with Windows (Clients\StartMenuInternet).
#[cfg(windows)]
pub fn installed() -> Vec<Installed> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::System::Registry::*;
    let mut out: Vec<Installed> = Vec::new();
    let read = |root: HKEY, sub: &str| -> Option<String> {
        let mut buf = vec![0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        // SAFETY: the buffer and its length are ours.
        unsafe { RegGetValueW(root, &HSTRING::from(sub), PCWSTR::null(), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut len)) }.ok().ok()?;
        let s = String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]);
        Some(s)
    };
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let base = r"SOFTWARE\Clients\StartMenuInternet";
        let mut key = HKEY::default();
        // SAFETY: opens a read-only handle that is closed below.
        if unsafe { RegOpenKeyExW(root, &HSTRING::from(base), 0, KEY_READ, &mut key) }.is_err() {
            continue;
        }
        for i in 0.. {
            let mut name = vec![0u16; 256];
            let mut len = name.len() as u32;
            // SAFETY: enumerates into our buffer.
            if unsafe { RegEnumKeyExW(key, i, windows::core::PWSTR(name.as_mut_ptr()), &mut len, None, windows::core::PWSTR::null(), None, None) }.is_err() {
                break;
            }
            let id = String::from_utf16_lossy(&name[..len as usize]);
            if id.eq_ignore_ascii_case("IEXPLORE.EXE") {
                continue;
            }
            let shown = read(root, &format!(r"{base}\{id}")).filter(|s| !s.is_empty()).unwrap_or_else(|| id.clone());
            let Some(cmd) = read(root, &format!(r"{base}\{id}\shell\open\command")) else { continue };
            let exe = exe_of(&cmd);
            if exe.is_empty() || !std::path::Path::new(&exe).exists() || out.iter().any(|b| b.name == shown) {
                continue;
            }
            out.push(Installed { name: shown, exe });
        }
        // SAFETY: closes the handle opened above.
        unsafe {
            let _ = RegCloseKey(key);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(not(windows))]
pub fn installed() -> Vec<Installed> {
    Vec::new()
}

/// The program from a registry command line: `"C:\...\chrome.exe" --flag`.
fn exe_of(cmd: &str) -> String {
    let cmd = cmd.trim();
    match cmd.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or("").to_string(),
        None => cmd.split(".exe").next().map(|p| format!("{p}.exe")).filter(|_| cmd.to_ascii_lowercase().contains(".exe")).unwrap_or_default(),
    }
}

pub fn choice(conn: &rusqlite::Connection) -> MainBrowser {
    db::get(conn, KEY).unwrap_or_default()
}

fn web_url(url: &str) -> Result<String, String> {
    let u = reqwest::Url::parse(url.trim()).map_err(|_| "That isn't a web address.".to_string())?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err("Only web addresses (http and https) open in a browser.".into());
    }
    Ok(u.to_string())
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct BrowsersView {
    current: MainBrowser,
    installed: Vec<Installed>,
}

#[tauri::command]
pub fn browsers_view(state: AppStateRef) -> BrowsersView {
    BrowsersView { current: choice(&state.db.lock().unwrap()), installed: installed() }
}

#[tauri::command]
pub fn set_main_browser(state: AppStateRef, choice: MainBrowser) -> Result<(), String> {
    if let MainBrowser::App { name } = &choice {
        if !installed().iter().any(|b| &b.name == name) {
            return Err(format!("{name} isn't installed on this PC."));
        }
    }
    let conn = state.db.lock().unwrap();
    db::set(&conn, KEY, &choice)?;
    let label = match &choice {
        MainBrowser::Builtin => "SulcusAI's browser".to_string(),
        MainBrowser::System => "the Windows default".to_string(),
        MainBrowser::App { name } => name.clone(),
    };
    db::log_action(&conn, "settings", &format!("Main browser set to {label}"));
    Ok(())
}

/// Opens a web address (or just the browser) in the main browser.
#[tauri::command]
pub async fn open_web(app: tauri::AppHandle, state: AppStateRef<'_>, url: Option<String>) -> Result<(), String> {
    let url = url.map(|u| web_url(&u)).transpose()?;
    let current = choice(&state.db.lock().unwrap());
    match current {
        MainBrowser::Builtin => {
            crate::browser::open_browser(app.clone(), state).await?;
            if let (Some(u), Some(w)) = (url, crate::browser::window(&app)) {
                w.navigate(u.parse().map_err(|_| "That isn't a web address.".to_string())?).map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        MainBrowser::System => {
            let target = url.unwrap_or_else(|| "https://duckduckgo.com/".into());
            crate::open_with_system(std::ffi::OsStr::new(&target))
        }
        MainBrowser::App { name } => {
            let b = installed().into_iter().find(|b| b.name == name).ok_or_else(|| format!("{name} isn't installed anymore. Pick another main browser in Settings."))?;
            let mut cmd = std::process::Command::new(&b.exe);
            if let Some(u) = url {
                cmd.arg(u);
            }
            cmd.spawn().map_err(|e| format!("Couldn't start {name}: {e}"))?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_are_read_from_command_lines() {
        assert_eq!(exe_of(r#""C:\Program Files\Google\Chrome\Application\chrome.exe" -- "%1""#), r"C:\Program Files\Google\Chrome\Application\chrome.exe");
        assert_eq!(exe_of(r"C:\Program Files\Internet Explorer\iexplore.exe"), r"C:\Program Files\Internet Explorer\iexplore.exe");
        assert_eq!(exe_of("nothing here"), "");
    }

    #[test]
    fn only_web_addresses_open() {
        assert!(web_url("https://example.com/a?b=1").is_ok());
        assert!(web_url("file:///C:/Windows/system32/calc.exe").is_err());
        assert!(web_url("javascript:alert(1)").is_err());
        assert!(web_url("ms-settings:").is_err());
    }

    /// Lists this PC's browsers: `cargo test --lib lists_browsers -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn lists_browsers() {
        for b in installed() {
            println!("{} → {}", b.name, b.exe);
        }
    }
}
