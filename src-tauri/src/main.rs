// SPDX-License-Identifier: AGPL-3.0-only
// Release builds have no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Started by Chrome or Edge for the SulcusAI extension: just relay.
    if std::env::args().skip(1).any(|a| a.starts_with("chrome-extension://")) {
        return sulcusai_lib::run_bridge_host();
    }
    // The Start menu's Pause, Resume or Quit, for the copy already running.
    if sulcusai_lib::hand_over_ask() {
        return;
    }
    // A copy on its own data folder (a test build, a portable install) gets
    // its own WebView2 folder too: sharing the installed copy's, with other
    // browser arguments, makes WebView2 refuse to start.
    if let Some(data) = std::env::var_os("SULCUSAI_DATA_DIR") {
        if std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_none() {
            std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", std::path::Path::new(&data).join("webview"));
        }
    }
    sulcusai_lib::run()
}
