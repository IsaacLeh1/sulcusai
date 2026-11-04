// SPDX-License-Identifier: AGPL-3.0-only
//! The shield while SulcusAI runs: the saved switches, the rotation
//! generation and own-list ring the gateway reads, the minute timer that
//! rotates addresses and moves ports, and the window's commands.

use std::sync::{Arc, LazyLock, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use super::ports::{self, Listener};
use super::proxies::ProxyRing;
use super::rotation::{RotationPlan, Rotator};
use super::settings::{ShieldSettings, KEY};
use crate::{db, AppState, AppStateRef};

static SETTINGS: RwLock<Option<ShieldSettings>> = RwLock::new(None);
static ROTATOR: LazyLock<Mutex<Rotator>> = LazyLock::new(|| Mutex::new(Rotator::new(now())));
static RING: Mutex<Option<ProxyRing>> = Mutex::new(None);
/// Wakes the timer when the shield is turned on (it sleeps while off).
static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Seconds on a steady clock, for rotation, port and proxy-pause timing.
pub fn now() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs()
}

/// The switches in use. Off until the saved ones are loaded at start.
pub fn current() -> ShieldSettings {
    SETTINGS.read().unwrap().clone().unwrap_or_default()
}

/// The isolation generation requests carry now.
pub fn generation() -> u64 {
    ROTATOR.lock().unwrap().generation()
}

pub fn saved(state: &AppState) -> ShieldSettings {
    db::get::<ShieldSettings>(&state.db.lock().unwrap(), KEY).unwrap_or_default().cleaned()
}

fn install(s: ShieldSettings) {
    {
        let mut ring = RING.lock().unwrap();
        match ring.as_mut() {
            Some(r) => r.replace(s.proxies.clone()),
            None => *ring = Some(ProxyRing::new(s.proxies.clone(), super::random_u64() as usize)),
        }
    }
    *SETTINGS.write().unwrap() = Some(s);
}

/// The own list's ring, for the gateway to pick from and report to.
pub fn with_ring<R>(f: impl FnOnce(&mut ProxyRing) -> R) -> R {
    let mut ring = RING.lock().unwrap();
    f(ring.get_or_insert_with(|| ProxyRing::new(Vec::new(), 0)))
}

/// Loads the saved switches (before the private connection applies its
/// routes, which read them) and starts the timer.
pub fn start(app: AppHandle, state: Arc<AppState>) {
    install(saved(&state));
    tauri::async_runtime::spawn(async move {
        loop {
            if current().enabled {
                tick(&app, &state);
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(60)) => {}
                    _ = WAKE.notified() => {}
                }
            } else {
                WAKE.notified().await;
            }
        }
    });
}

fn tick(app: &AppHandle, state: &Arc<AppState>) {
    let s = current();
    let plan = {
        let mut rotator = ROTATOR.lock().unwrap();
        let mut ring = RING.lock().unwrap();
        rotator.tick(&s, now(), ring.as_mut())
    };
    if let Some(p) = plan {
        carry_out(app, &p);
    }
    crate::phone::shield_port_tick(state, &s);
    crate::sync::shield_port_tick(&s);
}

fn carry_out(app: &AppHandle, p: &RotationPlan) {
    if p.drop_pooled_connections {
        crate::privacy::rotated(p.generation, p.reconnect_long_lived);
    }
    app.emit("shield:changed", ()).ok();
}

/// A listener on every network for `l` at a random port, when the shield
/// moves that listener; None means bind it as usual. Falls back to a port
/// Windows picks when every random try is taken or reserved.
pub fn random_listener(s: &ShieldSettings, l: Listener, avoid: &[u16]) -> Option<Result<std::net::TcpListener, String>> {
    if !ports::randomized(s, l) {
        return None;
    }
    let any = std::net::Ipv4Addr::UNSPECIFIED;
    let mut got = None;
    ports::pick(avoid, &mut super::random_u64, |p| match std::net::TcpListener::bind((any, p)) {
        Ok(listener) => {
            got = Some(listener);
            true
        }
        Err(_) => false,
    });
    let listener = match got {
        Some(l) => Ok(l),
        None => std::net::TcpListener::bind((any, 0)).map_err(|e| e.to_string()),
    };
    Some(listener.and_then(|l| l.set_nonblocking(true).map(|()| l).map_err(|e| e.to_string())))
}

/// The browser window's options under the shield, or why it stays closed.
pub fn browser_plan() -> Result<super::browser::BrowserPlan, String> {
    let s = current();
    let path = super::path(&s)?;
    super::browser::plan(&s, path, crate::privacy::browser_address().as_deref())
}

// ---------- the window ----------

#[derive(Serialize)]
pub struct ShieldView {
    settings: ShieldSettings,
    /// What's wrong with what was typed or chosen, in plain words.
    problems: Vec<String>,
    /// Some(false): the Tor helper in use is too old to give each site or
    /// rotation its own route (it still hides the address).
    tor_isolates: Option<bool>,
    /// Seconds until the next timed new address.
    next_rotation: Option<u64>,
}

fn view(problems: Vec<String>) -> ShieldView {
    let settings = current();
    let next_rotation = ROTATOR.lock().unwrap().next_at(&settings).map(|t| t.saturating_sub(now()));
    let tor_isolates = (settings.enabled && settings.via == super::Via::Tor).then(crate::privacy::helper_isolates).flatten();
    let mut problems = problems;
    if let Err(e) = super::path(&settings) {
        if !problems.contains(&e) {
            problems.push(e);
        }
    }
    ShieldView { settings, problems, tor_isolates, next_rotation }
}

#[tauri::command(async)]
pub fn shield_view() -> ShieldView {
    view(current().problems())
}

/// Saves what can be used of `settings` and applies it everywhere it
/// matters: the private connection's routes, the listeners' ports and the
/// browser window (which takes its options only when it opens).
#[tauri::command]
pub async fn set_shield(app: AppHandle, state: AppStateRef<'_>, settings: ShieldSettings) -> Result<ShieldView, String> {
    let problems = settings.problems();
    let next = settings.cleaned();
    let before = current();
    {
        let conn = state.db.lock().unwrap();
        db::set(&conn, KEY, &next)?;
        if before.enabled != next.enabled {
            db::log_action(&conn, "network", if next.enabled { "Turned on the address shield" } else { "Turned off the address shield" });
        }
    }
    install(next.clone());
    WAKE.notify_one();
    crate::privacy::apply(state.inner().clone(), Some(app.clone()), true).await?;
    let ports_changed = ports::randomized(&before, Listener::PhonePage) != ports::randomized(&next, Listener::PhonePage);
    if ports_changed {
        crate::phone::apply(state.inner().clone());
    }
    if ports_changed || super::peers::rules(&before) != super::peers::rules(&next) {
        crate::sync::listen_again();
    }
    let browser = |s: &ShieldSettings| (super::path(s), s.browser_common_fingerprint, s.browser_block_third_party_cookies, s.browser_clear_on_close);
    if browser(&before) != browser(&next) {
        crate::browser::close_for_new_options(&app);
    }
    app.emit("shield:changed", ()).ok();
    Ok(view(problems))
}

/// "New address now".
#[tauri::command]
pub fn shield_new_address(app: AppHandle, state: AppStateRef) -> Result<(), String> {
    let s = current();
    let plan = {
        let mut rotator = ROTATOR.lock().unwrap();
        let mut ring = RING.lock().unwrap();
        rotator.new_address_now(&s, now(), ring.as_mut())?
    };
    carry_out(&app, &plan);
    state.log("network", "Asked for a new address");
    Ok(())
}

/// Opens Windows' Wi-Fi settings page (random hardware addresses).
#[tauri::command]
pub fn shield_wifi_settings() -> Result<(), String> {
    crate::open_with_system(std::ffi::OsStr::new(super::text::WIFI_SETTINGS_URI))
}
