// SPDX-License-Identifier: AGPL-3.0-only
// Release builds have no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    synapseai_lib::run()
}
