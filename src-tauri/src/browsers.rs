// SPDX-License-Identifier: AGPL-3.0-only
//! Your main browser: where links and the globe button open. Chrome when
//! nothing was picked (Windows' default without it), Windows' default, any
//! browser Windows has registered (Chrome, Edge, DuckDuckGo, Firefox…), or
//! SulcusAI's web viewer, inside its window (viewer.rs; it has its own
//! button too).
//!
//! The choice is stored by name only; the program is looked up in the
//! registry again each time, so a stored setting can't point at anything
//! else.

use serde::{Deserialize, Serialize};
use tauri::Emitter;

use crate::{db, AppStateRef};

const KEY: &str = "main_browser";
/// Set when the web viewer is picked: "Builtin" stored before it existed
/// was the old default (a separate window), not a choice of it.
const PICKED_VIEWER: &str = "main_browser_viewer";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MainBrowser {
    /// SulcusAI's web viewer, inside its window.
    Builtin,
    /// Whatever Windows opens links with.
    System,
    /// A registered browser, by its registry name.
    App { name: String },
}

/// Nothing picked: Chrome if it's installed, else Windows' default.
fn unpicked(installed: &[Installed]) -> MainBrowser {
    match installed.iter().find(|b| b.name.to_lowercase().contains("chrome") && !b.name.to_lowercase().contains("chromium")) {
        Some(b) => MainBrowser::App { name: b.name.clone() },
        None => MainBrowser::System,
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

const SEARCH_KEY: &str = "browser_search";

/// What SulcusAI's own browser (the web viewer, and the assistant's
/// browser) starts on and searches what's typed with. DuckDuckGo unless
/// another was picked: it turns some people's searches away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Search {
    #[default]
    Duckduckgo,
    Google,
    Bing,
    Brave,
}

impl Search {
    pub fn home(self) -> &'static str {
        match self {
            Search::Duckduckgo => "https://duckduckgo.com/",
            Search::Google => "https://www.google.com/",
            Search::Bing => "https://www.bing.com/",
            Search::Brave => "https://search.brave.com/",
        }
    }

    /// A search for `q`.
    pub fn of(self, q: &str) -> String {
        let at = match self {
            Search::Duckduckgo => "https://duckduckgo.com/",
            Search::Google => "https://www.google.com/search",
            Search::Bing => "https://www.bing.com/search",
            Search::Brave => "https://search.brave.com/search",
        };
        reqwest::Url::parse_with_params(at, &[("q", q)]).map(|u| u.to_string()).unwrap_or_else(|_| self.home().into())
    }

    fn name(self) -> &'static str {
        match self {
            Search::Duckduckgo => "DuckDuckGo",
            Search::Google => "Google",
            Search::Bing => "Bing",
            Search::Brave => "Brave Search",
        }
    }
}

pub fn search(conn: &rusqlite::Connection) -> Search {
    db::get(conn, SEARCH_KEY).unwrap_or_default()
}

pub fn choice(conn: &rusqlite::Connection) -> MainBrowser {
    match db::get::<MainBrowser>(conn, KEY) {
        Some(MainBrowser::Builtin) if !db::get::<bool>(conn, PICKED_VIEWER).unwrap_or(false) => unpicked(&installed()),
        Some(c) => c,
        None => unpicked(&installed()),
    }
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
    search: Search,
}

#[tauri::command(async)]
pub fn browsers_view(state: AppStateRef) -> BrowsersView {
    let conn = state.db.lock().unwrap();
    BrowsersView { current: choice(&conn), installed: installed(), search: search(&conn) }
}

#[tauri::command(async)]
pub fn set_browser_search(state: AppStateRef, search: Search) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::set(&conn, SEARCH_KEY, &search)?;
    db::log_action(&conn, "settings", &format!("SulcusAI's browser searches with {}", search.name()));
    Ok(())
}

#[tauri::command(async)]
pub fn set_main_browser(state: AppStateRef, choice: MainBrowser) -> Result<(), String> {
    if let MainBrowser::App { name } = &choice {
        if !installed().iter().any(|b| &b.name == name) {
            return Err(format!("{name} isn't installed on this PC."));
        }
    }
    let conn = state.db.lock().unwrap();
    db::set(&conn, KEY, &choice)?;
    db::set(&conn, PICKED_VIEWER, &(choice == MainBrowser::Builtin))?;
    let label = match &choice {
        MainBrowser::Builtin => "SulcusAI's web viewer".to_string(),
        MainBrowser::System => "the Windows default".to_string(),
        MainBrowser::App { name } => name.clone(),
    };
    db::log_action(&conn, "settings", &format!("Main browser set to {label}"));
    Ok(())
}

/// Opens a web address (or just the browser) in the main browser: the web
/// viewer's page opens in the window (`viewer:open`), others start outside.
#[tauri::command]
pub async fn open_web(app: tauri::AppHandle, state: AppStateRef<'_>, url: Option<String>) -> Result<(), String> {
    let url = url.map(|u| web_url(&u)).transpose()?;
    let current = choice(&state.db.lock().unwrap());
    match current {
        MainBrowser::Builtin => app.emit("viewer:open", serde_json::json!({ "url": url })).map_err(|e| e.to_string()),
        outside => open_in(outside, url, search(&state.db.lock().unwrap())),
    }
}

/// Opens a web address in a browser outside SulcusAI (the web viewer's
/// "Open in Chrome"): the main browser, or Chrome or Windows' default when
/// the main browser is the viewer itself.
#[tauri::command(async)]
pub fn open_outside(state: AppStateRef<'_>, url: String) -> Result<(), String> {
    let url = web_url(&url)?;
    let current = choice(&state.db.lock().unwrap());
    let outside = if current == MainBrowser::Builtin { unpicked(&installed()) } else { current };
    open_in(outside, Some(url), Search::default())
}

fn open_in(browser: MainBrowser, url: Option<String>, home: Search) -> Result<(), String> {
    match browser {
        MainBrowser::App { name } => {
            let b = installed().into_iter().find(|b| b.name == name).ok_or_else(|| format!("{name} isn't installed anymore. Pick another main browser in Settings."))?;
            let mut cmd = std::process::Command::new(&b.exe);
            if let Some(u) = url {
                cmd.arg(u);
            }
            cmd.spawn().map_err(|e| format!("Couldn't start {name}: {e}"))?;
            Ok(())
        }
        _ => {
            let target = url.unwrap_or_else(|| home.home().into());
            crate::open_with_system(std::ffi::OsStr::new(&target))
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

    #[test]
    fn chrome_unless_something_else_was_picked() {
        let found = |names: &[&str]| names.iter().map(|n| Installed { name: n.to_string(), exe: String::new() }).collect::<Vec<_>>();
        assert_eq!(unpicked(&found(&["Firefox", "Google Chrome", "Microsoft Edge"])), MainBrowser::App { name: "Google Chrome".into() });
        assert_eq!(unpicked(&found(&["Chromium", "Microsoft Edge"])), MainBrowser::System);
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::init_for_test(&conn);
        let none = choice(&conn);
        assert_ne!(none, MainBrowser::Builtin, "nothing picked: not the viewer");
        // "Builtin" stored before the viewer existed was the old default.
        db::set(&conn, KEY, &MainBrowser::Builtin).unwrap();
        assert_eq!(choice(&conn), none);
        db::set(&conn, PICKED_VIEWER, &true).unwrap();
        assert_eq!(choice(&conn), MainBrowser::Builtin);
        db::set(&conn, KEY, &MainBrowser::System).unwrap();
        assert_eq!(choice(&conn), MainBrowser::System);
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
