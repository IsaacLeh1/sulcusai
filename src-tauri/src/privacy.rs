// SPDX-License-Identifier: AGPL-3.0-only
//! Private connection: SulcusAI's own traffic goes through Tor (built in,
//! run by volunteers, nothing hosted by us) or a proxy/VPN the user already
//! has, so the sites it contacts see another address, not the user's or
//! where they are. Each kind of traffic is its own switch; mail and model
//! downloads are off by default (mail providers may flag Tor sign-ins; huge
//! downloads crawl). It covers SulcusAI, not the rest of the PC.
//!
//! Tor (Arti) runs in a helper program of its own (src/bin/sulcusai-tor.rs),
//! downloaded the first time it's used, so the app itself stays small. A
//! SOCKS5 gateway here on 127.0.0.1, with a random user name and password,
//! passes each connection to the helper, so every client (HTTP, IMAP, SMTP)
//! uses the same `socks5h://` address and names are looked up through Tor
//! too. It fails closed: while Tor is being got or starting, requests wait;
//! they never go out directly.

use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

use crate::{db, AppState, AppStateRef};

mod gate;

pub(crate) use gate::{browser_address, resolve_through_tor, rotated};

const KEY: &str = "privacy";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    #[default]
    Tor,
    /// The user's own proxy or VPN (socks5://… or http://…).
    Proxy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacySettings {
    pub enabled: bool,
    pub via: Via,
    pub proxy: String,
    /// Web search, reading pages, weather, place names.
    pub web: bool,
    /// Checking for and downloading app updates.
    pub updates: bool,
    /// Email and calendar sync, sending, and their sign-in renewals.
    pub mail: bool,
    /// Model and engine downloads.
    pub models: bool,
}

impl Default for PrivacySettings {
    fn default() -> Self {
        PrivacySettings { enabled: false, via: Via::Tor, proxy: String::new(), web: true, updates: true, mail: false, models: false }
    }
}

/// What a connection is for, to know whether it goes the private way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Web,
    Updates,
    Mail,
    Models,
}

/// The proxy address each kind of traffic uses now (None: direct).
#[derive(Debug, Clone, Default)]
struct Routes {
    web: Option<String>,
    updates: Option<String>,
    mail: Option<String>,
    models: Option<String>,
    /// They go through Tor (not the user's own proxy).
    tor: bool,
}

fn routes() -> &'static RwLock<Routes> {
    static R: OnceLock<RwLock<Routes>> = OnceLock::new();
    R.get_or_init(|| RwLock::new(Routes::default()))
}

/// The proxy for this kind of traffic, if it goes the private way.
pub fn proxy_for(route: Route) -> Option<String> {
    let r = routes().read().unwrap();
    match route {
        Route::Web => r.web.clone(),
        Route::Updates => r.updates.clone(),
        Route::Mail => r.mail.clone(),
        Route::Models => r.models.clone(),
    }
}

/// Whether this kind of traffic goes through Tor (not the user's own proxy,
/// which may look names up inside their network).
pub fn tor_for(route: Route) -> bool {
    goes_through_tor(&routes().read().unwrap(), route)
}

fn goes_through_tor(r: &Routes, route: Route) -> bool {
    r.tor
        && match route {
            Route::Web => r.web.is_some(),
            Route::Updates => r.updates.is_some(),
            Route::Mail => r.mail.is_some(),
            Route::Models => r.models.is_some(),
        }
}

// ---------- Tor ----------

struct Gateway {
    port: u16,
    user: String,
    pass: String,
}

static GATEWAY: OnceLock<Gateway> = OnceLock::new();

/// How Tor is doing, for the window.
#[derive(Debug, Clone, Serialize)]
pub struct TorStatus {
    /// "off" (not started), "getting" (downloading Tor, once), "starting",
    /// "ready" or "stuck".
    pub state: &'static str,
    /// How far getting or starting has got, 0-100.
    pub progress: u8,
    /// What's wrong, in plain words, when stuck.
    pub problem: String,
    #[serde(skip)]
    stuck_since: Option<std::time::Instant>,
}

static TOR_STATUS: RwLock<TorStatus> = RwLock::new(TorStatus { state: "off", progress: 0, problem: String::new(), stuck_since: None });

fn set_tor_status(app: Option<&AppHandle>, state: &'static str, progress: u8, problem: String) {
    {
        let mut s = TOR_STATUS.write().unwrap();
        let stuck_since = if state == "stuck" { s.stuck_since.or(Some(std::time::Instant::now())) } else { None };
        *s = TorStatus { state, progress, problem, stuck_since };
    }
    if let Some(app) = app {
        app.emit("privacy:status", ()).ok();
    }
}

/// The helper's reason for being stuck, in words people can act on.
fn problem_words(blocked: &str) -> String {
    match blocked {
        "offline" => "Tor can't connect: this PC seems to be offline.".into(),
        "filtering" => "This network seems to block Tor (school and work networks often do). Try another network, or use your own proxy or VPN instead.".into(),
        "clock" => "This PC's clock is off, so Tor can't connect. Turn on \"Set time automatically\" in Windows' date and time settings.".into(),
        _ => "Tor can't get its list of relays right now. It keeps trying.".into(),
    }
}

/// The Tor helper (sulcusai-tor) while it runs: its SOCKS port, and its
/// standard input, which keeps it alive and takes "start".
struct Helper {
    port: u16,
    stdin: tokio::process::ChildStdin,
    child: tokio::process::Child,
}

static HELPER: tokio::sync::Mutex<Option<Helper>> = tokio::sync::Mutex::const_new(None);
/// The running helper keeps one circuit per site and rotation when told
/// them in the SOCKS user name (helpers from before the address shield
/// don't, and take only their plain name).
static HELPER_ISOLATES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static HELPER_SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the Tor helper gives each site and rotation its own route; None
/// until it has started.
pub fn helper_isolates() -> Option<bool> {
    use std::sync::atomic::Ordering::SeqCst;
    HELPER_SEEN.load(SeqCst).then(|| HELPER_ISOLATES.load(SeqCst))
}
/// Connecting to the Tor network was asked for (it carries over a restart
/// of the helper).
static START_ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(windows)]
const HELPER_EXE: &str = "sulcusai-tor.exe";
#[cfg(not(windows))]
const HELPER_EXE: &str = "sulcusai-tor";

/// The helper: beside the app when built with it (development, tests),
/// else downloaded into the engines folder the first time.
async fn helper_exe(state: &AppState, app: Option<&AppHandle>) -> Result<std::path::PathBuf, String> {
    if let Ok(me) = std::env::current_exe() {
        for dir in me.ancestors().skip(1).take(2) {
            let p = dir.join(HELPER_EXE);
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    let spec = state.catalog.tor.as_ref().ok_or("The private connection through Tor isn't available on this system yet. Use your own proxy or VPN instead.")?;
    let key = crate::engine::cpu_key();
    let asset = spec.assets.get(key).ok_or("The private connection through Tor isn't available on this system yet. Use your own proxy or VPN instead.")?;
    let dir = format!("tor-{}-{key}", spec.build);
    if crate::engine::find_exe(&state.paths.engines.join(&dir), HELPER_EXE).is_none() {
        // Straight from the developer's release: it can't come through Tor before Tor is here.
        state.log("network", &format!("Downloading Tor ({}) from {}, once", crate::size_label(asset.size), crate::host_of(&asset.url)));
        set_tor_status(app, "getting", 0, String::new());
    }
    let app = app.cloned();
    crate::engine::ensure_unpacked(&state.paths, &dir, asset, HELPER_EXE, &std::sync::atomic::AtomicBool::new(false), move |r, t| {
        set_tor_status(app.as_ref(), "getting", ((r as f64 / t.max(1) as f64) * 100.0) as u8, String::new());
    })
    .await
    .map_err(|e| format!("Couldn't get Tor: {e}"))
}

/// The helper's SOCKS port, starting it (and getting it, once) if needed.
async fn helper_port(state: &AppState, app: Option<AppHandle>) -> Result<u16, String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut slot = HELPER.lock().await;
    if let Some(h) = slot.as_mut() {
        if matches!(h.child.try_wait(), Ok(None)) {
            return Ok(h.port);
        }
    }
    let g = GATEWAY.get().ok_or("The private connection isn't set up.")?;
    let exe = helper_exe(state, app.as_ref()).await?;
    let mut cmd = tokio::process::Command::new(&exe);
    cmd.arg(state.paths.data.join("tor")).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    #[cfg(target_os = "linux")]
    // SAFETY: prctl is async-signal-safe and touches only the child.
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| format!("Couldn't start Tor: {e}"))?;
    #[cfg(windows)]
    if let (Some(job), Some(pid)) = (state.job(), child.id()) {
        job.assign(pid).ok();
    }
    let mut stdin = child.stdin.take().ok_or("Couldn't start Tor.")?;
    let stdout = child.stdout.take().ok_or("Couldn't start Tor.")?;
    stdin.write_all(format!("{} {}\n", g.user, g.pass).as_bytes()).await.map_err(|e| e.to_string())?;
    let mut lines = BufReader::new(stdout).lines();
    let port = tokio::time::timeout(Duration::from_secs(30), async {
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            if let Some(p) = v["port"].as_u64() {
                HELPER_ISOLATES.store(v["isolation"].as_bool() == Some(true), std::sync::atomic::Ordering::SeqCst);
                HELPER_SEEN.store(true, std::sync::atomic::Ordering::SeqCst);
                return Some(p as u16);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
    .ok_or("Tor didn't start.")?;
    // Its progress and problems, as it reports them.
    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            let progress = v["progress"].as_u64().unwrap_or(0).min(100) as u8;
            match v["state"].as_str() {
                Some("ready") => set_tor_status(app.as_ref(), "ready", 100, String::new()),
                Some("stuck") => set_tor_status(app.as_ref(), "stuck", progress, problem_words(v["blocked"].as_str().unwrap_or("other"))),
                Some("starting") => set_tor_status(app.as_ref(), "starting", progress, String::new()),
                _ => {}
            }
        }
        // Gone: the next connection starts it again.
        set_tor_status(app.as_ref(), "off", 0, String::new());
    });
    if START_ASKED.load(std::sync::atomic::Ordering::SeqCst) {
        stdin.write_all(b"start\n").await.ok();
    }
    *slot = Some(Helper { port, stdin, child });
    Ok(port)
}

/// Starts connecting to the Tor network (once). Until this, Tor sends
/// nothing, and isn't even downloaded.
fn start_tor(state: Arc<AppState>, app: Option<AppHandle>) {
    if START_ASKED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        if let Err(e) = helper_port(&state, app.clone()).await {
            set_tor_status(app.as_ref(), "stuck", 0, e);
            START_ASKED.store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        }
        if let Some(h) = HELPER.lock().await.as_mut() {
            h.stdin.write_all(b"start\n").await.ok();
        }
    });
}

/// Ends the helper (Pause completely); the next connection starts it again.
pub async fn stop_tor() {
    if let Some(mut h) = HELPER.lock().await.take() {
        let _ = h.child.kill().await;
    }
    START_ASKED.store(false, std::sync::atomic::Ordering::SeqCst);
    let mut s = TOR_STATUS.write().unwrap();
    if s.state != "off" {
        *s = TorStatus { state: "off", progress: 0, problem: String::new(), stuck_since: None };
    }
}

/// How long Tor must be stuck before requests fail at once instead of waiting.
const STUCK_GRACE: Duration = Duration::from_secs(20);

/// Ok when this kind of traffic can go now (directly, through the user's
/// proxy, or through Tor that's ready or still starting). When Tor has been
/// stuck a while, the reason, so people see it instead of a long wait.
pub fn check(route: Route) -> Result<(), String> {
    let shield = crate::shield::live::current();
    if shield.enabled {
        crate::shield::path(&shield)?;
    }
    if !routes().read().unwrap().tor || proxy_for(route).is_none() {
        return Ok(());
    }
    match stuck(&TOR_STATUS.read().unwrap()) {
        Some(problem) => Err(format!("The private connection isn't working: {problem}")),
        None => Ok(()),
    }
}

/// Tor's problem, once it has been stuck longer than the grace period.
fn stuck(s: &TorStatus) -> Option<String> {
    match s.stuck_since {
        Some(t) if s.state == "stuck" && t.elapsed() >= STUCK_GRACE => Some(s.problem.clone()),
        _ => None,
    }
}

fn random_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Starts the local SOCKS5 gateway into Tor (once), returning its address.
/// Every client uses this one address; each connection is passed to the
/// helper as it is (the helper checks the same user name and password), so
/// nothing ever goes out directly while Tor is being got or started.
async fn gateway(state: Arc<AppState>, app: Option<AppHandle>) -> Result<String, String> {
    if let Some(g) = GATEWAY.get() {
        return Ok(format!("socks5h://{}:{}@127.0.0.1:{}", g.user, g.pass, g.port));
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| format!("Couldn't start the private connection: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let g = Gateway { port, user: random_token(), pass: random_token() };
    if GATEWAY.set(g).is_err() {
        // Another call won the race; use its gateway.
        return Box::pin(gateway(state, app)).await;
    }
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            start_tor(state.clone(), app.clone());
            let (state, app) = (state.clone(), app.clone());
            tokio::spawn(async move {
                let port = match helper_port(&state, app.clone()).await {
                    Ok(p) => p,
                    Err(e) => {
                        set_tor_status(app.as_ref(), "stuck", 0, e);
                        return;
                    }
                };
                if let Ok(mut tor) = TcpStream::connect(("127.0.0.1", port)).await {
                    let _ = tokio::io::copy_bidirectional(&mut sock, &mut tor).await;
                }
            });
        }
    });
    let g = GATEWAY.get().unwrap();
    Ok(format!("socks5h://{}:{}@127.0.0.1:{}", g.user, g.pass, g.port))
}

// ---------- settings ----------

pub fn settings(state: &AppState) -> PrivacySettings {
    db::get(&state.db.lock().unwrap(), KEY).unwrap_or_default()
}

/// A proxy address the user typed, in the form clients take.
fn normalize(proxy: &str) -> Result<String, String> {
    let p = proxy.trim();
    let p = if p.contains("://") { p.to_string() } else { format!("socks5h://{p}") };
    let url = reqwest::Url::parse(&p).map_err(|_| "That proxy address isn't valid. Use host:port, or socks5://host:port or http://host:port.".to_string())?;
    let p = match url.scheme() {
        // Names looked up by the proxy, not here.
        "socks5" => p.replacen("socks5://", "socks5h://", 1),
        "socks5h" | "http" | "https" => p,
        _ => return Err("Use a SOCKS5 or HTTP proxy address.".into()),
    };
    if url.host_str().is_none() || url.port_or_known_default().is_none() {
        return Err("The proxy address needs a host and a port, like 127.0.0.1:1080.".into());
    }
    Ok(p)
}

/// Which traffic goes through `via` (the proxy or Tor gateway address).
fn routes_for(s: &PrivacySettings, via: Option<String>) -> Routes {
    let pick = |on: bool| if on { via.clone() } else { None };
    Routes { web: pick(s.web), updates: pick(s.updates), mail: pick(s.mail), models: pick(s.models), tor: s.enabled && s.via == Via::Tor && via.is_some() }
}

/// Sets each kind of traffic's route from the settings. Tor connects now
/// when `start_now`, else the first time something goes through it.
pub async fn apply(state: Arc<AppState>, app: Option<AppHandle>, start_now: bool) -> Result<(), String> {
    let s = settings(&state);
    let shield = crate::shield::live::current();
    if shield.enabled {
        return apply_shield(state, app, start_now, &shield).await;
    }
    let via = if !s.enabled {
        None
    } else {
        match s.via {
            Via::Tor => Some(gateway(state.clone(), app.clone()).await?),
            Via::Proxy => Some(normalize(&s.proxy)?),
        }
    };
    *routes().write().unwrap() = routes_for(&s, via);
    if start_now && s.enabled && s.via == Via::Tor {
        start_tor(state.clone(), app.clone());
    }
    if let Some(app) = &app {
        app.emit("privacy:status", ()).ok();
    }
    Ok(())
}

/// With the address shield on, everything the private connection can carry
/// goes through the shield's gateway, whatever the switches above it say:
/// the shield promises to hide the address from every site and service.
/// Tor's own gateway is made too, for its user name and password, when Tor
/// carries traffic or looks names up.
async fn apply_shield(state: Arc<AppState>, app: Option<AppHandle>, start_now: bool, shield: &crate::shield::ShieldSettings) -> Result<(), String> {
    use crate::shield::{settings::DnsSetting, Via as ShieldVia};
    let tor = shield.via == ShieldVia::Tor;
    if tor || shield.dns == DnsSetting::Tor {
        gateway(state.clone(), app.clone()).await?;
    }
    gate::start(state.clone(), app.clone()).await?;
    let at = gate::address;
    *routes().write().unwrap() = Routes { web: at(Route::Web), updates: at(Route::Updates), mail: at(Route::Mail), models: at(Route::Models), tor };
    if start_now && tor {
        start_tor(state.clone(), app.clone());
    }
    if let Some(app) = &app {
        app.emit("privacy:status", ()).ok();
    }
    Ok(())
}

/// A problem applying the saved settings at startup (a bad proxy address).
static START_PROBLEM: RwLock<String> = RwLock::new(String::new());

/// At startup: the saved routes. Tor starts connecting in the background,
/// except at the Offline level, where it waits until something uses it (an
/// update check or a download), so Offline stays quiet.
pub fn start(app: AppHandle, state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        let start_now = state.settings().connectivity != crate::net::Connectivity::Offline;
        let problem = apply(state, Some(app.clone()), start_now).await.err().unwrap_or_default();
        *START_PROBLEM.write().unwrap() = problem;
        app.emit("privacy:status", ()).ok();
    });
}

#[derive(Serialize)]
pub struct PrivacyView {
    settings: PrivacySettings,
    /// "off", "waiting" (Tor starts when first used), "getting" (downloading
    /// Tor, once), "starting", "ready" or "stuck", for the private connection
    /// as set now.
    state: &'static str,
    /// Tor's start-up progress, 0-100.
    progress: u8,
    /// What's wrong, in plain words.
    problem: String,
}

#[tauri::command(async)]
pub fn privacy_view(state: AppStateRef) -> PrivacyView {
    let settings = settings(&state);
    let start_problem = START_PROBLEM.read().unwrap().clone();
    // The address shield, when on, decides the way for everything.
    let shield = crate::shield::live::current();
    let (on, own_proxy) = if shield.enabled { (true, shield.via == crate::shield::Via::OwnList) } else { (settings.enabled, settings.via == Via::Proxy) };
    let shield_problem = if shield.enabled { crate::shield::path(&shield).err() } else { None };
    let (state, progress, problem) = if !on {
        ("off", 0, String::new())
    } else if !start_problem.is_empty() {
        ("stuck", 0, start_problem)
    } else if let Some(p) = shield_problem {
        ("stuck", 0, p)
    } else if own_proxy {
        // Not tested here; the Test button shows whether it works.
        ("ready", 100, String::new())
    } else {
        let t = TOR_STATUS.read().unwrap().clone();
        // "off": made but not connecting yet (Offline level, nothing used it).
        (if t.state == "off" { "waiting" } else { t.state }, t.progress, t.problem)
    };
    PrivacyView { settings, state, progress, problem }
}

/// Stores the settings, noting in Activity when it was turned on or off.
/// The old settings are read before taking the lock: `settings()` takes it
/// too, and the database lock isn't reentrant (taking it twice froze the
/// whole app in 0.8.9).
fn save(state: &AppState, settings: &PrivacySettings) -> Result<(), String> {
    let before = self::settings(state);
    let conn = state.db.lock().unwrap();
    db::set(&conn, KEY, settings)?;
    if before.enabled != settings.enabled {
        db::log_action(&conn, "network", if settings.enabled { "Turned on the private connection" } else { "Turned off the private connection" });
    }
    Ok(())
}

#[tauri::command]
pub async fn set_privacy(app: AppHandle, state: AppStateRef<'_>, settings: PrivacySettings) -> Result<(), String> {
    if settings.enabled && settings.via == Via::Proxy {
        normalize(&settings.proxy)?;
    }
    save(&state, &settings)?;
    apply(state.inner().clone(), Some(app), true).await?;
    START_PROBLEM.write().unwrap().clear();
    Ok(())
}

#[derive(Serialize)]
pub struct PrivacyTest {
    /// The address sites see now.
    ip: String,
    /// Where sites guess that address is ("Amsterdam, North Holland, NL").
    place: String,
}

/// Asks ipinfo.io which address and place it sees, going the way web traffic
/// goes now (falling back to ipify.org for just the address).
#[tauri::command]
pub async fn test_privacy(state: AppStateRef<'_>) -> Result<PrivacyTest, String> {
    crate::offline::check(crate::offline::Keep::Web)?;
    check(Route::Web)?;
    let mut b = reqwest::Client::builder().connect_timeout(Duration::from_secs(60)).timeout(Duration::from_secs(90));
    if let Some(p) = proxy_for(Route::Web) {
        b = b.proxy(reqwest::Proxy::all(&p).map_err(|e| e.to_string())?);
    } else {
        b = b.no_proxy();
    }
    let client = b.build().map_err(|e| e.to_string())?;
    state.log("network", "Checked which address and place sites see (ipinfo.io)");
    let get = |url: &'static str| {
        let client = client.clone();
        async move { client.get(url).send().await?.error_for_status()?.json::<serde_json::Value>().await }
    };
    match get("https://ipinfo.io/json").await {
        Ok(v) if v["ip"].is_string() => {
            let place = ["city", "region", "country"].iter().filter_map(|k| v[*k].as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
            Ok(PrivacyTest { ip: v["ip"].as_str().unwrap_or("?").to_string(), place })
        }
        _ => {
            let v = get("https://api.ipify.org?format=json").await.map_err(|e| format!("The test didn't get through: {e}"))?;
            Ok(PrivacyTest { ip: v["ip"].as_str().unwrap_or("?").to_string(), place: String::new() })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_addresses_are_checked_and_names_go_through_the_proxy() {
        assert_eq!(normalize("127.0.0.1:1080").unwrap(), "socks5h://127.0.0.1:1080");
        assert_eq!(normalize("socks5://10.0.0.2:9050").unwrap(), "socks5h://10.0.0.2:9050");
        assert_eq!(normalize("http://proxy.example:3128").unwrap(), "http://proxy.example:3128");
        assert!(normalize("ftp://x:21").is_err());
        assert!(normalize("not a proxy").is_err());
    }

    /// Saving took the database lock twice and froze the app (0.8.9).
    #[cfg(windows)]
    #[test]
    fn saving_settings_does_not_freeze() {
        let state = crate::testutil::scratch_state();
        let (tx, rx) = std::sync::mpsc::channel();
        let s = state.clone();
        std::thread::spawn(move || {
            save(&s, &PrivacySettings { enabled: true, mail: true, ..Default::default() }).unwrap();
            save(&s, &PrivacySettings::default()).unwrap();
            tx.send(()).unwrap();
        });
        rx.recv_timeout(Duration::from_secs(10)).expect("saving the private connection settings froze");
        assert!(!settings(&state).enabled);
        let conn = state.db.lock().unwrap();
        let logged: i64 = conn.query_row("SELECT COUNT(*) FROM action_log WHERE category = 'network'", [], |r| r.get(0)).unwrap_or(-1);
        assert_eq!(logged, 2, "turning it on and off is noted in Activity");
    }

    #[test]
    fn each_switch_routes_only_its_own_traffic() {
        let via = Some("socks5h://127.0.0.1:9".to_string());
        let r = routes_for(&PrivacySettings { enabled: true, ..Default::default() }, via.clone());
        assert_eq!((r.web.is_some(), r.updates.is_some(), r.mail.is_some(), r.models.is_some()), (true, true, false, false), "mail and models are off by default");
        assert!(r.tor);
        let r = routes_for(&PrivacySettings { enabled: true, via: Via::Proxy, web: false, mail: true, ..Default::default() }, via);
        assert_eq!((r.web.is_some(), r.mail.is_some()), (false, true));
        assert!(!r.tor, "the user's own proxy isn't Tor");
        // Only Tor skips the web tools' own name check.
        assert!(!goes_through_tor(&r, Route::Mail) && !goes_through_tor(&r, Route::Web));
        let tor = routes_for(&PrivacySettings { enabled: true, ..Default::default() }, Some("socks5h://127.0.0.1:9".into()));
        assert!(goes_through_tor(&tor, Route::Web) && !goes_through_tor(&tor, Route::Mail), "mail is off by default");
        let r = routes_for(&PrivacySettings::default(), None);
        assert!(r.web.is_none() && r.updates.is_none() && !r.tor, "off means direct");
    }

    #[test]
    fn requests_wait_for_tor_but_fail_fast_once_its_stuck() {
        let mut s = TorStatus { state: "starting", progress: 8, problem: String::new(), stuck_since: None };
        assert!(stuck(&s).is_none(), "still starting: wait");
        s.state = "stuck";
        s.problem = "This network seems to block Tor".into();
        s.stuck_since = Some(std::time::Instant::now());
        assert!(stuck(&s).is_none(), "briefly stuck: keep waiting");
        s.stuck_since = Some(std::time::Instant::now() - STUCK_GRACE - Duration::from_secs(1));
        assert_eq!(stuck(&s).as_deref(), Some("This network seems to block Tor"));
    }

    /// A server that greets, a SOCKS5 proxy and an HTTP proxy, all local:
    /// mail-style connections reach the server through either proxy.
    #[tokio::test]
    async fn mail_connections_go_through_socks5_and_http_proxies() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::AsyncReadExt;
        let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_port = server.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = server.accept().await {
                s.write_all(b"* OK hello\r\n").await.ok();
            }
        });
        let used = Arc::new(AtomicUsize::new(0));
        // SOCKS5 with a user name and password.
        let socks = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let socks_port = socks.local_addr().unwrap().port();
        let u = used.clone();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = socks.accept().await {
                u.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut b = [0u8; 2];
                    s.read_exact(&mut b).await.unwrap();
                    let mut m = vec![0u8; b[1] as usize];
                    s.read_exact(&mut m).await.unwrap();
                    s.write_all(&[5, 2]).await.unwrap();
                    s.read_exact(&mut b).await.unwrap();
                    let mut user = vec![0u8; b[1] as usize];
                    s.read_exact(&mut user).await.unwrap();
                    let mut l = [0u8; 1];
                    s.read_exact(&mut l).await.unwrap();
                    let mut pass = vec![0u8; l[0] as usize];
                    s.read_exact(&mut pass).await.unwrap();
                    assert_eq!((user.as_slice(), pass.as_slice()), (&b"me"[..], &b"p@ss"[..]));
                    s.write_all(&[1, 0]).await.unwrap();
                    let mut req = [0u8; 4];
                    s.read_exact(&mut req).await.unwrap();
                    assert_eq!(req[3], 3, "the name goes to the proxy, not looked up here");
                    s.read_exact(&mut l).await.unwrap();
                    let mut name = vec![0u8; l[0] as usize + 2];
                    s.read_exact(&mut name).await.unwrap();
                    let port = u16::from_be_bytes([name[name.len() - 2], name[name.len() - 1]]);
                    let host = String::from_utf8(name[..name.len() - 2].to_vec()).unwrap();
                    let mut out = TcpStream::connect((if host == "mail.test" { "127.0.0.1" } else { host.as_str() }, port)).await.unwrap();
                    s.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await.unwrap();
                    tokio::io::copy_bidirectional(&mut s, &mut out).await.ok();
                });
            }
        });
        // HTTP CONNECT.
        let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http_port = http.local_addr().unwrap().port();
        let u = used.clone();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = http.accept().await {
                u.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut b = [0u8; 1];
                    while !head.ends_with(b"\r\n\r\n") {
                        s.read_exact(&mut b).await.unwrap();
                        head.push(b[0]);
                    }
                    let head = String::from_utf8(head).unwrap();
                    let target = head.split_whitespace().nth(1).unwrap().replace("mail.test", "127.0.0.1");
                    let mut out = TcpStream::connect(target).await.unwrap();
                    s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").await.unwrap();
                    tokio::io::copy_bidirectional(&mut s, &mut out).await.ok();
                });
            }
        });
        for proxy in [format!("socks5h://me:p%40ss@127.0.0.1:{socks_port}"), format!("http://127.0.0.1:{http_port}")] {
            let mut s = crate::net::connect_through("mail.test", server_port, Some(&proxy)).await.unwrap();
            let mut buf = [0u8; 32];
            let n = s.read(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"* OK hello\r\n", "through {proxy}");
        }
        assert_eq!(used.load(Ordering::SeqCst), 2, "both went through their proxy");
    }

    /// Through the real Tor network: the check service sees a Tor exit,
    /// not this PC, and mail servers answer through it (greeting only).
    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore]
    async fn tor_hides_the_address() {
        use tokio::io::AsyncReadExt;
        let state = crate::testutil::scratch_state();
        db::set(&state.db.lock().unwrap(), KEY, &PrivacySettings { enabled: true, mail: true, ..Default::default() }).unwrap();
        let started = std::time::Instant::now();
        apply(state.clone(), None, true).await.unwrap();
        // This PC's own address, from a neutral service (some networks block torproject.org directly).
        let direct: serde_json::Value = match reqwest::Client::builder().no_proxy().build().unwrap().get("https://api.ipify.org?format=json").send().await {
            Ok(r) => r.json().await.unwrap_or_default(),
            Err(e) => serde_json::json!({ "error": e.to_string() }),
        };
        let p = proxy_for(Route::Web).expect("web goes the private way");
        // Wait for Tor, showing its progress; on a network that blocks Tor
        // (UVU's does), say so and stop.
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let t = TOR_STATUS.read().unwrap().clone();
            println!("tor: {} {}% {}", t.state, t.progress, t.problem);
            if t.state == "ready" {
                break;
            }
            if let Some(problem) = stuck(&t) {
                println!("SKIPPED: Tor can't start here: {problem}");
                return;
            }
            assert!(started.elapsed() < Duration::from_secs(300), "Tor took too long to start");
        }
        let via = reqwest::Client::builder().proxy(reqwest::Proxy::all(&p).unwrap()).timeout(Duration::from_secs(180)).build().unwrap().get("https://check.torproject.org/api/ip").send().await;
        let via: serde_json::Value = via.unwrap().json().await.unwrap();
        println!("direct: {direct}\nthrough Tor: {via} (after {:.1} s)", started.elapsed().as_secs_f32());
        assert_eq!(via["IsTor"], true);
        assert_ne!(via["IP"], direct["ip"]);
        assert!(proxy_for(Route::Models).is_none(), "model downloads stay direct unless turned on");
        // Mail servers, through Tor: the TLS greeting.
        for (host, port) in [("imap.gmail.com", 993u16), ("smtp.gmail.com", 465)] {
            let t = std::time::Instant::now();
            let raw = crate::net::connect(host, port, Route::Mail).await.unwrap();
            let name = tokio_rustls::rustls::pki_types::ServerName::try_from(host.to_string()).unwrap();
            let mut tls = tokio_rustls::TlsConnector::from(crate::mail::tls_config()).connect(name, raw).await.unwrap();
            let mut buf = [0u8; 64];
            let n = tls.read(&mut buf).await.unwrap();
            println!("{host}: {} ({:.1} s)", String::from_utf8_lossy(&buf[..n]).trim(), t.elapsed().as_secs_f32());
            assert!(n > 0);
        }
    }
}
