// SPDX-License-Identifier: AGPL-3.0-only
//! Windows notifications (task reminders, finished background work).

use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

pub fn show(app: &AppHandle, title: &str, body: &str) {
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        eprintln!("notification failed: {e}");
    }
}
