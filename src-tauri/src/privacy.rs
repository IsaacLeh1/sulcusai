// SPDX-License-Identifier: AGPL-3.0-only
//! Private connection: SulcusAI's own traffic goes through Tor (built in,
//! run by volunteers, nothing hosted by us) or a proxy/VPN the user already
//! has, so the sites it contacts see another address, not the user's or
//! where they are. Each kind of traffic is its own switch; mail and model
//! downloads are off by default (mail providers may flag Tor sign-ins; huge
//! downloads crawl). It covers SulcusAI, not the rest of the PC.
//!
//! Tor runs in-process (Arti) behind a SOCKS5 gateway on 127.0.0.1 with a
//! random user name and password, so every client (HTTP, IMAP, SMTP) uses
//! the same `socks5h://` address and names are looked up through Tor too.
//! It fails closed: while Tor is starting, requests wait; they never go out
//! directly.

use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use arti_client::config::TorClientConfigBuilder;
use arti_client::TorClient;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::{db, AppState, AppStateRef};

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

// ---------- Tor ----------

struct Gateway {
    port: u16,
    user: String,
    pass: String,
}

static GATEWAY: OnceLock<Gateway> = OnceLock::new();
static TOR: tokio::sync::OnceCell<Arc<TorClient<tor_rtcompat::PreferredRuntime>>> = tokio::sync::OnceCell::const_new();

/// How Tor is doing, for the window.
#[derive(Debug, Clone, Serialize)]
pub struct TorStatus {
    /// "off" (not started), "starting", "ready" or "stuck".
    pub state: &'static str,
    /// How far starting up has got, 0-100.
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

/// Arti's reason for being stuck, in words people can act on.
fn problem_words(kind: arti_client::status::BlockageKind) -> String {
    use arti_client::status::BlockageKind as K;
    match kind {
        K::Offline => "Tor can't connect: this PC seems to be offline.".into(),
        K::Filtering | K::CantReachTor => "This network seems to block Tor (school and work networks often do). Try another network, or use your own proxy or VPN instead.".into(),
        K::ClockSkewed => "This PC's clock is off, so Tor can't connect. Turn on \"Set time automatically\" in Windows' date and time settings.".into(),
        _ => "Tor can't get its list of relays right now. It keeps trying.".into(),
    }
}

async fn tor(state: &AppState, app: Option<AppHandle>) -> Result<Arc<TorClient<tor_rtcompat::PreferredRuntime>>, String> {
    TOR.get_or_try_init(|| async {
        let dir = state.paths.data.join("tor");
        let config = TorClientConfigBuilder::from_directories(dir.join("state"), dir.join("cache")).build().map_err(|e| e.to_string())?;
        let client = TorClient::builder().config(config).create_unbootstrapped().map_err(|e| e.to_string())?;
        // Progress and problems as Tor reports them.
        let (c, app2) = (client.clone(), app.clone());
        tokio::spawn(async move {
            use futures_util::StreamExt;
            let mut events = c.bootstrap_events();
            while let Some(s) = events.next().await {
                let progress = (s.as_frac() * 100.0).round().clamp(0.0, 100.0) as u8;
                if s.ready_for_traffic() {
                    set_tor_status(app2.as_ref(), "ready", 100, String::new());
                } else if let Some(b) = s.blocked() {
                    set_tor_status(app2.as_ref(), "stuck", progress, problem_words(b.kind()));
                } else {
                    set_tor_status(app2.as_ref(), "starting", progress, String::new());
                }
            }
        });
        Ok::<_, String>(client)
    })
    .await
    .cloned()
}

/// Starts connecting to the Tor network (once). Until this, Tor sends nothing.
fn start_tor(app: Option<&AppHandle>) {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let Some(client) = TOR.get() else { return };
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    set_tor_status(app, "starting", 0, String::new());
    let (c, app) = (client.clone(), app.cloned());
    tokio::spawn(async move {
        if let Err(e) = c.bootstrap().await {
            set_tor_status(app.as_ref(), "stuck", 0, format!("Couldn't reach the Tor network: {e}"));
        }
    });
}

/// How long Tor must be stuck before requests fail at once instead of waiting.
const STUCK_GRACE: Duration = Duration::from_secs(20);

/// Ok when this kind of traffic can go now (directly, through the user's
/// proxy, or through Tor that's ready or still starting). When Tor has been
/// stuck a while, the reason, so people see it instead of a long wait.
pub fn check(route: Route) -> Result<(), String> {
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
async fn gateway(state: Arc<AppState>, app: Option<AppHandle>) -> Result<String, String> {
    if let Some(g) = GATEWAY.get() {
        return Ok(format!("socks5h://{}:{}@127.0.0.1:{}", g.user, g.pass, g.port));
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| format!("Couldn't start the private connection: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let g = Gateway { port, user: random_token(), pass: random_token() };
    let (user, pass) = (g.user.clone(), g.pass.clone());
    if GATEWAY.set(g).is_err() {
        // Another call won the race; use its gateway.
        return Box::pin(gateway(state, app)).await;
    }
    let client = tor(&state, app.clone()).await?;
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            start_tor(app.as_ref());
            let (client, user, pass) = (client.clone(), user.clone(), pass.clone());
            tokio::spawn(async move {
                if let Err(e) = serve(sock, &client, &user, &pass).await {
                    eprintln!("private connection: {e}");
                }
            });
        }
    });
    let g = GATEWAY.get().unwrap();
    Ok(format!("socks5h://{}:{}@127.0.0.1:{}", g.user, g.pass, g.port))
}

/// One SOCKS5 connection (RFC 1928 + 1929): user name and password, then
/// CONNECT to a name or address, carried through Tor.
async fn serve(mut s: TcpStream, tor: &TorClient<tor_rtcompat::PreferredRuntime>, user: &str, pass: &str) -> Result<(), String> {
    let e = |e: std::io::Error| e.to_string();
    let mut head = [0u8; 2];
    s.read_exact(&mut head).await.map_err(e)?;
    let mut methods = vec![0u8; head[1] as usize];
    s.read_exact(&mut methods).await.map_err(e)?;
    if head[0] != 5 || !methods.contains(&2) {
        s.write_all(&[5, 0xFF]).await.ok();
        return Err("no password method".into());
    }
    s.write_all(&[5, 2]).await.map_err(e)?;
    // Sub-negotiation: version 1, user, password.
    let mut v = [0u8; 2];
    s.read_exact(&mut v).await.map_err(e)?;
    let mut u = vec![0u8; v[1] as usize];
    s.read_exact(&mut u).await.map_err(e)?;
    let mut pl = [0u8; 1];
    s.read_exact(&mut pl).await.map_err(e)?;
    let mut p = vec![0u8; pl[0] as usize];
    s.read_exact(&mut p).await.map_err(e)?;
    if u != user.as_bytes() || p != pass.as_bytes() {
        s.write_all(&[1, 1]).await.ok();
        return Err("wrong password".into());
    }
    s.write_all(&[1, 0]).await.map_err(e)?;
    // The request.
    let mut req = [0u8; 4];
    s.read_exact(&mut req).await.map_err(e)?;
    if req[0] != 5 || req[1] != 1 {
        s.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await.ok();
        return Err("only CONNECT".into());
    }
    let host = match req[3] {
        1 => {
            let mut a = [0u8; 4];
            s.read_exact(&mut a).await.map_err(e)?;
            std::net::Ipv4Addr::from(a).to_string()
        }
        3 => {
            let mut l = [0u8; 1];
            s.read_exact(&mut l).await.map_err(e)?;
            let mut name = vec![0u8; l[0] as usize];
            s.read_exact(&mut name).await.map_err(e)?;
            String::from_utf8(name).map_err(|e| e.to_string())?
        }
        4 => {
            let mut a = [0u8; 16];
            s.read_exact(&mut a).await.map_err(e)?;
            std::net::Ipv6Addr::from(a).to_string()
        }
        _ => return Err("bad address".into()),
    };
    let mut pb = [0u8; 2];
    s.read_exact(&mut pb).await.map_err(e)?;
    let port = u16::from_be_bytes(pb);
    // Through Tor (this waits while Tor is still starting, but not when it's
    // been stuck a while).
    let blocked = stuck(&TOR_STATUS.read().unwrap()).is_some();
    if blocked {
        s.write_all(&[5, 1, 0, 1, 0, 0, 0, 0, 0, 0]).await.ok();
        return Err("Tor is stuck".into());
    }
    match tokio::time::timeout(Duration::from_secs(120), tor.connect((host.as_str(), port))).await {
        Ok(Ok(mut stream)) => {
            s.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await.map_err(e)?;
            tokio::io::copy_bidirectional(&mut s, &mut stream).await.map_err(e)?;
            Ok(())
        }
        Ok(Err(err)) => {
            s.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await.ok();
            Err(err.to_string())
        }
        Err(_) => {
            s.write_all(&[5, 6, 0, 1, 0, 0, 0, 0, 0, 0]).await.ok();
            Err("timed out".into())
        }
    }
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
        start_tor(app.as_ref());
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
    /// "off", "waiting" (Tor starts when first used), "starting", "ready" or
    /// "stuck", for the private connection as set now.
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
    let (state, progress, problem) = if !settings.enabled {
        ("off", 0, String::new())
    } else if !start_problem.is_empty() {
        ("stuck", 0, start_problem)
    } else if settings.via == Via::Proxy {
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
        let state = crate::e2e::scratch_state();
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
        let state = crate::e2e::scratch_state();
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
