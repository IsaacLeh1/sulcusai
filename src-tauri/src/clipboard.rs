// SPDX-License-Identifier: AGPL-3.0-only
//! Clipboard history: the last texts copied on this PC, to copy again.
//! Off until turned on in Features. Windows tells the app when the clipboard changes
//! (no timer), and copies that password managers and SulcusAI itself mark
//! as private are skipped. The list is kept encrypted and capped.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::{db, AppState, AppStateRef};

const KEY: &str = "clipboard_history";
const DEFAULT_KEEP: usize = 20;
const MAX_KEEP: usize = 100;
/// Longer copies (a whole document) aren't worth keeping in a quick list.
const MAX_CHARS: usize = 20_000;

#[derive(Serialize, Deserialize, Clone)]
pub struct Settings {
    pub keep: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { keep: DEFAULT_KEEP }
    }
}

fn on(state: &AppState) -> bool {
    crate::features::is_on(&state.db.lock().unwrap(), crate::features::Feature::Clipboard)
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Item {
    pub text: String,
    pub at: i64,
}

/// Newest first, without repeats, at most `keep`.
fn push(items: &mut Vec<Item>, text: &str, at: i64, keep: usize) -> bool {
    let text = text.trim_end_matches(['\r', '\n']);
    if text.trim().is_empty() || text.chars().count() > MAX_CHARS {
        return false;
    }
    if items.first().is_some_and(|i| i.text == text) {
        return false;
    }
    items.retain(|i| i.text != text);
    items.insert(0, Item { text: text.to_string(), at });
    items.truncate(keep.clamp(1, MAX_KEEP));
    true
}

fn settings(state: &AppState) -> Settings {
    db::get(&state.db.lock().unwrap(), "clipboard_settings").unwrap_or_default()
}

fn load(state: &AppState) -> Vec<Item> {
    let Ok(c) = state.cipher() else { return Vec::new() };
    let stored: Option<String> = db::get(&state.db.lock().unwrap(), KEY);
    stored.and_then(|s| c.decrypt(&s).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save(state: &AppState, items: &[Item]) -> Result<(), String> {
    let c = state.cipher()?;
    let text = serde_json::to_string(items).map_err(|e| e.to_string())?;
    db::set(&state.db.lock().unwrap(), KEY, &c.encrypt(&text))
}

/// A copy happened: kept when history is on and the app is unlocked.
fn copied(state: &AppState, text: &str) {
    let s = settings(state);
    if !on(state) || state.cipher().is_err() {
        return;
    }
    let mut items = load(state);
    if push(&mut items, text, db::now_ms(), s.keep) {
        let _ = save(state, &items);
    }
}

#[cfg(windows)]
mod listen {
    use std::sync::Arc;

    use windows::core::w;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::DataExchange::{AddClipboardFormatListener, IsClipboardFormatAvailable, RegisterClipboardFormatW, RemoveClipboardFormatListener};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PostMessageW, PostQuitMessage, RegisterClassW, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLIPBOARDUPDATE, WM_CLOSE, WM_DESTROY, WNDCLASSW,
    };

    use crate::AppState;

    static STATE: std::sync::Mutex<Option<Arc<AppState>>> = std::sync::Mutex::new(None);
    /// The listening window, as an integer so it can sit in a static.
    static WINDOW: std::sync::Mutex<Option<isize>> = std::sync::Mutex::new(None);

    /// Copies marked private by the program that made them.
    fn private_copy() -> bool {
        // SAFETY: registering a format name and asking whether the clipboard
        // holds it only read global clipboard state.
        unsafe {
            [w!("ExcludeClipboardContentFromMonitorProcessing"), w!("Clipboard Viewer Ignore")]
                .into_iter()
                .any(|name| IsClipboardFormatAvailable(RegisterClipboardFormatW(name)).is_ok())
        }
    }

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_CLIPBOARDUPDATE => {
                if !private_copy() {
                    if let (Some(state), Some(text)) = (STATE.lock().unwrap().clone(), crate::quick::clipboard_text()) {
                        super::copied(&state, &text);
                    }
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = RemoveClipboardFormatListener(hwnd);
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }

    pub fn start(state: Arc<AppState>) {
        if WINDOW.lock().unwrap().is_some() {
            return;
        }
        *STATE.lock().unwrap() = Some(state);
        std::thread::spawn(|| {
            // SAFETY: a message-only window owned by this thread; its
            // messages are pumped here until WM_CLOSE destroys it.
            unsafe {
                let Ok(module) = GetModuleHandleW(None) else { return };
                let class = w!("SulcusAIClipboardHistory");
                let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: module.into(), lpszClassName: class, ..Default::default() };
                RegisterClassW(&wc);
                let Ok(hwnd) = CreateWindowExW(WINDOW_EX_STYLE(0), class, w!(""), WINDOW_STYLE(0), 0, 0, 0, 0, HWND_MESSAGE, None, module, None) else { return };
                if AddClipboardFormatListener(hwnd).is_err() {
                    let _ = DestroyWindow(hwnd);
                    return;
                }
                *WINDOW.lock().unwrap() = Some(hwnd.0 as isize);
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    DispatchMessageW(&msg);
                }
                *WINDOW.lock().unwrap() = None;
            }
        });
    }

    pub fn stop() {
        if let Some(h) = *WINDOW.lock().unwrap() {
            // SAFETY: posting to a window this module made; a stale handle
            // only makes the post fail.
            unsafe {
                let _ = PostMessageW(HWND(h as *mut _), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
    }
}

#[cfg(not(windows))]
mod listen {
    pub fn start(_state: std::sync::Arc<crate::AppState>) {}
    pub fn stop() {}
}

/// Listens while history is on; called at start and when the switch
/// changes. Turning it off also forgets what was kept.
pub fn apply(state: std::sync::Arc<AppState>) {
    if on(&state) {
        listen::start(state);
    } else {
        listen::stop();
        if state.cipher().is_ok() {
            let _ = save(&state, &[]);
        }
    }
}

static BUSY: Mutex<()> = Mutex::new(());

#[derive(Serialize)]
pub struct View {
    on: bool,
    keep: usize,
    items: Vec<Item>,
}

fn view(state: &AppState) -> View {
    let s = settings(state);
    let on = on(state);
    View { on, keep: s.keep, items: if on { load(state) } else { Vec::new() } }
}

#[tauri::command(async)]
pub fn clipboard_view(state: AppStateRef) -> Result<View, String> {
    state.cipher()?;
    Ok(view(&state))
}

#[tauri::command(async)]
pub fn clipboard_set(state: AppStateRef, keep: usize) -> Result<View, String> {
    let _g = BUSY.lock().unwrap();
    let s = Settings { keep: keep.clamp(1, MAX_KEEP) };
    db::set(&state.db.lock().unwrap(), "clipboard_settings", &s)?;
    let mut items = load(&state);
    items.truncate(s.keep);
    save(&state, &items)?;
    Ok(view(&state))
}

#[tauri::command(async)]
pub fn clipboard_copy(state: AppStateRef, index: usize) -> Result<(), String> {
    let items = load(&state);
    let item = items.get(index).ok_or("That copy is no longer in the list.")?;
    crate::quick::set_clipboard_text(&item.text)
}

#[tauri::command(async)]
pub fn clipboard_forget(state: AppStateRef, index: Option<usize>) -> Result<View, String> {
    let _g = BUSY.lock().unwrap();
    let mut items = load(&state);
    match index {
        Some(i) if i < items.len() => {
            items.remove(i);
        }
        Some(_) => {}
        None => items.clear(),
    }
    save(&state, &items)?;
    Ok(view(&state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_without_repeats_and_capped() {
        let mut items = Vec::new();
        assert!(push(&mut items, "one", 1, 3));
        assert!(push(&mut items, "two\r\n", 2, 3));
        assert!(!push(&mut items, "two", 3, 3), "the same copy twice in a row is kept once");
        assert!(push(&mut items, "one", 4, 3), "copied again moves to the top");
        assert!(push(&mut items, "three", 5, 3));
        assert!(push(&mut items, "four", 6, 3));
        let texts: Vec<&str> = items.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(texts, ["four", "three", "one"]);
        assert!(!push(&mut items, "   ", 7, 3));
        assert!(!push(&mut items, &"x".repeat(MAX_CHARS + 1), 8, 3));
    }

    #[test]
    fn off_by_default_and_kept_encrypted() {
        let state = crate::testutil::scratch_state();
        crate::features::set(&state.db.lock().unwrap(), crate::features::Feature::Clipboard, false).unwrap();
        assert!(!on(&state));
        copied(&state, "secret one");
        assert!(load(&state).is_empty(), "nothing is kept while it's off");
        crate::features::set(&state.db.lock().unwrap(), crate::features::Feature::Clipboard, true).unwrap();
        copied(&state, "secret one");
        assert_eq!(load(&state)[0].text, "secret one");
        let raw: String = db::get(&state.db.lock().unwrap(), KEY).unwrap();
        assert!(!raw.contains("secret"));
    }
}
