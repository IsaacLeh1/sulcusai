// SPDX-License-Identifier: AGPL-3.0-only
//! A hidden window hands its memory back and stops running.
//!
//! The window asks for rest a while after it's hidden (window_rest), when
//! nothing in it is playing. WebView2 is then told to use little memory and
//! is suspended: its scripts, timers and drawing stop. Anything the core
//! sends it (an event) wakes it on its own, and showing the window wakes it
//! fully (`wake`, also on focus as a safety net).

use tauri::{AppHandle, Manager, WebviewWindow, WindowEvent};

/// WebView2's start-up arguments for every web view: Tauri's own defaults
/// (no mini menu, no SmartScreen), plus a 100 MB cap on each cache, which
/// otherwise grows for good. Web views sharing a data folder must all start
/// with the same arguments, so every builder uses this, and the main
/// window's (tauri.conf.json) is checked against it in a test.
pub const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --disk-cache-size=104857600";

#[cfg(windows)]
fn set_rest(w: &WebviewWindow, rest: bool) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2_19, ICoreWebView2_3, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL};
    use webview2_com::TrySuspendCompletedHandler;
    use windows_core62::Interface;
    // A minimized window is only out of sight: it must draw at once when
    // its taskbar button is clicked, which no code here hears.
    let hidden = !w.is_visible().unwrap_or(true);
    let _ = w.with_webview(move |pw| {
        // SAFETY: COM calls on the webview's own thread, on interfaces it
        // hands out; the handler outlives the call.
        unsafe {
            let controller = pw.controller();
            let Ok(core) = controller.CoreWebView2() else { return };
            if let Ok(c19) = core.cast::<ICoreWebView2_19>() {
                let _ = c19.SetMemoryUsageTargetLevel(if rest { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW } else { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL });
            }
            let Ok(c3) = core.cast::<ICoreWebView2_3>() else { return };
            if rest && hidden {
                // Suspending needs the webview marked not visible.
                let _ = controller.SetIsVisible(false);
                let done = TrySuspendCompletedHandler::create(Box::new(|_, _| Ok(())));
                let _ = c3.TrySuspend(&done);
            } else if !rest {
                let _ = controller.SetIsVisible(true);
                let _ = c3.Resume();
            }
        }
    });
}

#[cfg(not(windows))]
fn set_rest(_w: &WebviewWindow, _rest: bool) {}

/// The main window is shown again: memory back to normal, scripts running.
pub fn wake(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        set_rest(&w, false);
    }
}

/// The main window, already hidden, rests now (Pause completely).
pub fn rest_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        set_rest(&w, true);
    }
}

/// Focus means it's on screen, however it got there.
pub fn on_main_event(window: &tauri::Window, event: &WindowEvent) {
    if window.label() == "main" && matches!(event, WindowEvent::Focused(true)) {
        wake(window.app_handle());
    }
}

/// From the window, a while after it was hidden with nothing playing.
#[tauri::command]
pub fn window_rest(window: WebviewWindow) {
    set_rest(&window, true);
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_main_window_starts_with_the_same_arguments_as_every_other() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["app"]["windows"][0]["additionalBrowserArgs"].as_str(), Some(super::BROWSER_ARGS));
    }

    #[test]
    fn every_web_view_builder_uses_the_arguments() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut missing = Vec::new();
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") && !p.ends_with("rest.rs") {
                    let text = std::fs::read_to_string(&p).unwrap();
                    let builders = text.matches("WebviewWindowBuilder::new(").count() + text.matches("WebviewBuilder::new(").count();
                    let with_args = text.matches(".additional_browser_args(crate::rest::BROWSER_ARGS)").count();
                    if builders != with_args {
                        missing.push(format!("{} ({builders} builders, {with_args} with the arguments)", p.display()));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "web views made without BROWSER_ARGS: {missing:?}");
    }
}
