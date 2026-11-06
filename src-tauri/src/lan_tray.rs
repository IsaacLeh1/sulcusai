// SPDX-License-Identifier: AGPL-3.0-only
//! The networks window and its tray icon. The tray shows which network is
//! on and this PC's address in it; it's there while a network is on, and
//! always in the light networks app (`sulcusai.exe --lan`), where closing
//! the window leaves it in the tray.

use std::sync::Arc;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use crate::AppState;

pub const LABEL: &str = "lan";
const TRAY: &str = "lan";

fn light() -> bool {
    crate::parts::light_part().is_some_and(|p| p.id == "lan") && !crate::parts::full_started()
}

/// Opens the networks window, or brings it forward.
pub async fn open_window(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return Ok(());
    }
    WebviewWindowBuilder::new(&app, LABEL, WebviewUrl::App("index.html".into()))
        .additional_browser_args(crate::rest::BROWSER_ARGS)
        .title("Networks - SulcusAI")
        .inner_size(900.0, 640.0)
        .min_inner_size(480.0, 360.0)
        .build()
        .map_err(|e| format!("Couldn't open the networks window: {e}"))?;
    Ok(())
}

/// The tray's words: each network, and which one is on with this PC's address.
pub fn status_line(nets: &[(String, bool, Option<String>)]) -> String {
    match nets.iter().find(|(_, on, _)| *on) {
        Some((name, _, Some(addr))) => format!("SulcusAI LAN: {name} on, {addr}"),
        Some((name, _, None)) => format!("SulcusAI LAN: {name} on"),
        None if nets.is_empty() => "SulcusAI LAN: no networks".into(),
        None => format!("SulcusAI LAN: off ({} networks)", nets.len()),
    }
}

fn networks(app: &AppHandle) -> Vec<(String, String, bool, Option<String>)> {
    let Some(state) = app.try_state::<Arc<AppState>>() else { return vec![] };
    let me = crate::p2p::sharing::my_peer_id(&state).unwrap_or_default();
    let on = crate::p2p::vlan_adapter::running();
    crate::p2p::sharing::networks(&state).unwrap_or_default().into_iter().map(|n| (n.id.clone(), n.name.clone(), on.as_deref() == Some(n.id.as_str()), n.members.get(&me).map(|a| a.to_string()))).collect()
}

fn menu(app: &AppHandle, nets: &[(String, String, bool, Option<String>)]) -> tauri::Result<Menu<tauri::Wry>> {
    let m = Menu::new(app)?;
    m.append(&MenuItem::with_id(app, "open", "Open networks", true, None::<&str>)?)?;
    m.append(&PredefinedMenuItem::separator(app)?)?;
    for (id, name, on, addr) in nets {
        let label = match (on, addr) {
            (true, Some(a)) => format!("{name}: on ({a})"),
            (true, None) => format!("{name}: on"),
            _ => format!("{name}: turn on"),
        };
        m.append(&MenuItem::with_id(app, format!("net:{id}"), label, true, None::<&str>)?)?;
    }
    if nets.iter().any(|n| n.2) {
        m.append(&MenuItem::with_id(app, "off", "Turn the network off", true, None::<&str>)?)?;
    }
    if light() {
        m.append(&PredefinedMenuItem::separator(app)?)?;
        m.append(&MenuItem::with_id(app, "quit", "Quit SulcusAI LAN", true, None::<&str>)?)?;
    }
    Ok(m)
}

/// Shows, updates or removes the tray icon to match.
pub fn refresh(app: &AppHandle) {
    let nets = networks(app);
    let any_on = nets.iter().any(|n| n.2);
    if !light() && !any_on {
        let _ = app.remove_tray_by_id(TRAY);
        return;
    }
    let tip = status_line(&nets.iter().map(|n| (n.1.clone(), n.2, n.3.clone())).collect::<Vec<_>>());
    if let Some(t) = app.tray_by_id(TRAY) {
        let _ = t.set_tooltip(Some(&tip));
        if let Ok(m) = menu(app, &nets) {
            let _ = t.set_menu(Some(m));
        }
        return;
    }
    let Ok(m) = menu(app, &nets) else { return };
    let mut b = TrayIconBuilder::with_id(TRAY).tooltip(&tip).menu(&m).show_menu_on_left_click(false);
    if let Some(icon) = app.default_window_icon() {
        b = b.icon(icon.clone());
    }
    let _ = b
        .on_menu_event(|app, e| {
            let id = e.id.as_ref().to_string();
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                match id.as_str() {
                    "open" => {
                        let _ = open_window(app.clone()).await;
                    }
                    "off" => crate::p2p::vlan_adapter::stop(),
                    "quit" => app.exit(0),
                    other => {
                        if let Some(net) = other.strip_prefix("net:") {
                            let state = app.state::<Arc<AppState>>();
                            if let Err(e) = crate::p2p::sharing::vlan_connect(state, net.to_string()).await {
                                crate::applog::line("lan", &e);
                            }
                        }
                    }
                }
                refresh(&app);
            });
        })
        .on_tray_icon_event(|t, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                let app = t.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    let _ = open_window(app).await;
                });
            }
        })
        .build(app);
}

/// At startup: the tray for the light app.
pub fn apply(app: &AppHandle) {
    if light() {
        refresh(app);
    }
}

/// In the light app, closing the window keeps it in the tray.
pub fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    if window.label() == LABEL && light() {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = window.hide();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn tray_words() {
        let s = |v: Vec<(&str, bool, Option<&str>)>| super::status_line(&v.into_iter().map(|(a, b, c)| (a.to_string(), b, c.map(String::from))).collect::<Vec<_>>());
        assert_eq!(s(vec![]), "SulcusAI LAN: no networks");
        assert_eq!(s(vec![("Games", false, None), ("Work", false, None)]), "SulcusAI LAN: off (2 networks)");
        assert_eq!(s(vec![("Games", true, Some("10.77.1.2"))]), "SulcusAI LAN: Games on, 10.77.1.2");
    }
}
