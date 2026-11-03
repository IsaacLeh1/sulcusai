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
    sulcusai_lib::run()
}
