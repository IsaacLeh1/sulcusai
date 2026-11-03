// SPDX-License-Identifier: AGPL-3.0-only
//! The Security page: one read-only overview of what protects this PC's data
//! and what is reachable from outside, plus checks over it. A fix is a
//! description of an existing setting call (`FixCall`) and runs only from
//! `security_apply_fix`, which the page calls after the person confirms.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::connectors::ToolMode;
use crate::{db, sitekeys, AppState, AppStateRef};

const SEEN_KEYS: &str = "security_page_key_seen";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// Past this a PIN-protected app is open for long enough to walk up to.
const AUTO_LOCK_LONG_MINUTES: u32 = 60;
const AUTO_LOCK_FIX_MINUTES: u32 = 15;
const STALE_PHONE_DAYS: u64 = 90;
const OLD_KEY_DAYS: u64 = 365;
/// How long a phone stays signed in without being used (phone/mod.rs).
const SESSION_IDLE_MS: i64 = 12 * 60 * 60 * 1000;

// ---------- the overview ----------

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Overview {
    pub version: String,
    pub now: i64,
    pub protection: Protection,
    pub phones: Phones,
    pub listeners: Vec<Listener>,
    pub addons: Vec<AddonInfo>,
    pub keys: Vec<KeyInfo>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Protection {
    /// The data key exists and protects chats, notes and keys on disk.
    pub encryption: bool,
    pub lock_enabled: bool,
    pub hello_enabled: bool,
    /// 0 means never.
    pub auto_lock_minutes: u32,
    pub keep_working: bool,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Phones {
    pub page_on: bool,
    pub anywhere: bool,
    pub while_locked: bool,
    pub devices: Vec<Device>,
    /// Paired phones used within the session's idle limit.
    pub open_sessions: usize,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub added: i64,
    pub last_seen: Option<i64>,
    pub has_password: bool,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Listener {
    /// "api", "ollama" or "phone".
    pub id: String,
    pub name: String,
    pub port: Option<u16>,
    pub enabled: bool,
    pub running: bool,
    /// Bound to this PC only; the phone page is on the local network by design.
    pub local_only: bool,
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct AddonInfo {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// The host it talks to when that is off this PC.
    pub host: Option<String>,
    pub tools: usize,
    /// Tools that run without asking and could change something or send
    /// data off this PC.
    pub auto_run: Vec<String>,
    pub ask: usize,
    pub off: usize,
    pub signed_in: bool,
    pub has_secrets: bool,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct KeyInfo {
    /// "api_server" or "site".
    pub kind: String,
    pub label: String,
    /// The host for a site key.
    pub host: Option<String>,
    /// Counted from when this page first saw the key: keys carry no
    /// creation date of their own.
    pub age_days: u64,
    #[serde(skip)]
    pub fingerprint: String,
}

fn flag(v: &Value, k: &str) -> bool {
    v.get(k).and_then(Value::as_bool).unwrap_or(false)
}

fn text(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

fn port(v: &Value, k: &str) -> Option<u16> {
    v.get(k).and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok())
}

fn fingerprint(secret: &str) -> String {
    hex::encode(&Sha256::digest(secret.as_bytes())[..8])
}

/// The port in an address such as "http://192.168.1.4:47818".
fn port_of(address: &str) -> Option<u16> {
    address.trim_end_matches('/').rsplit(':').next().and_then(|p| p.parse().ok())
}

pub fn age_days(seen: &BTreeMap<String, i64>, fingerprint: &str, now: i64) -> u64 {
    seen.get(fingerprint).map_or(0, |first| ((now - first).max(0) / DAY_MS) as u64)
}

/// Adds keys not seen before and forgets ones that are gone; whether it changed.
pub fn update_seen(seen: &mut BTreeMap<String, i64>, current: &[&str], now: i64) -> bool {
    let before = seen.clone();
    seen.retain(|fp, _| current.contains(&fp.as_str()));
    for fp in current {
        seen.entry((*fp).to_string()).or_insert(now);
    }
    *seen != before
}

/// Reads everything from the app's state and the views of the phone page and
/// the local servers (`api`, `phone`, as the window gets them).
pub fn build(state: &AppState, api: &Value, phone: &Value) -> Result<Overview, String> {
    let c = state.cipher()?;
    let now = db::now_ms();
    // The database lock is taken after these: none of them may run under it.
    let auto_lock_minutes = state.settings().auto_lock_minutes;
    let keep_working = crate::security::keep_working(state);
    let (lock_enabled, hello_enabled) = {
        let v = state.vault.lock().unwrap();
        (v.lock_enabled(), v.hello_enabled())
    };

    let devices: Vec<Device> = phone
        .get("phones")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|p| Device {
                    id: text(p, "id").unwrap_or_default(),
                    name: text(p, "name").unwrap_or_default(),
                    added: p.get("added").and_then(Value::as_i64).unwrap_or(0),
                    last_seen: p.get("last_seen").and_then(Value::as_i64),
                    has_password: flag(p, "has_password"),
                })
                .collect()
        })
        .unwrap_or_default();
    let open_sessions = devices.iter().filter(|d| d.last_seen.is_some_and(|t| now - t < SESSION_IDLE_MS)).count();
    let phone_on = flag(phone, "enabled");

    let api_on = flag(api, "enabled");
    let ollama_on = flag(api, "ollama");
    let listeners = vec![
        Listener {
            id: "api".into(),
            name: "Local API server".into(),
            port: port(api, "port"),
            enabled: api_on,
            running: api_on && flag(api, "running"),
            local_only: true,
            problem: text(api, "problem").filter(|_| api_on),
        },
        Listener {
            id: "ollama".into(),
            name: "Server for programs made for Ollama".into(),
            port: port(api, "ollama_port"),
            enabled: ollama_on,
            running: ollama_on && flag(api, "ollama_running"),
            local_only: true,
            problem: text(api, "ollama_problem").filter(|_| ollama_on),
        },
        Listener {
            id: "phone".into(),
            name: "Phone page".into(),
            port: text(phone, "address").as_deref().and_then(port_of),
            enabled: phone_on,
            running: phone_on && flag(phone, "running"),
            local_only: false,
            problem: text(phone, "problem").filter(|_| phone_on),
        },
    ];

    let conn = state.db.lock().unwrap();
    let seen: BTreeMap<String, i64> = db::get(&conn, SEEN_KEYS).unwrap_or_default();

    let addons = crate::connectors::connectors(&conn, &c)
        .into_iter()
        .map(|k| {
            let mut info = AddonInfo {
                id: k.id.clone(),
                name: k.name.clone(),
                enabled: k.enabled,
                host: k.web(),
                tools: k.tools.len(),
                signed_in: k.auth.is_some(),
                has_secrets: !k.spec.env.is_empty() || !k.spec.headers.is_empty(),
                ..Default::default()
            };
            for tool in &k.tools {
                match k.mode(tool) {
                    // A read-only tool on this PC changes nothing and sends nothing.
                    ToolMode::Allow if info.host.is_some() || !tool.read_only => info.auto_run.push(tool.name.clone()),
                    ToolMode::Allow => {}
                    ToolMode::Ask => info.ask += 1,
                    ToolMode::Off => info.off += 1,
                }
            }
            info
        })
        .collect();

    let mut keys = Vec::new();
    if api_on {
        if let Some(key) = text(api, "key").filter(|k| !k.is_empty()) {
            let fp = fingerprint(&key);
            keys.push(KeyInfo { kind: "api_server".into(), label: "Local API server".into(), host: None, age_days: age_days(&seen, &fp, now), fingerprint: fp });
        }
    }
    for k in sitekeys::all(&conn, &c) {
        let fp = fingerprint(&format!("{}\n{}", k.host, k.key));
        keys.push(KeyInfo { kind: "site".into(), label: k.platform.clone(), host: Some(k.host.clone()), age_days: age_days(&seen, &fp, now), fingerprint: fp });
    }

    Ok(Overview {
        version: env!("CARGO_PKG_VERSION").to_string(),
        now,
        protection: Protection { encryption: true, lock_enabled, hello_enabled, auto_lock_minutes, keep_working },
        phones: Phones { page_on: phone_on, anywhere: flag(phone, "anywhere"), while_locked: flag(phone, "while_locked"), devices, open_sessions },
        listeners,
        addons,
        keys,
    })
}

/// Records when each key was first seen, for its age. Only this page's own
/// bookkeeping is written.
fn remember_keys(state: &AppState, keys: &[KeyInfo]) {
    let conn = state.db.lock().unwrap();
    let mut seen: BTreeMap<String, i64> = db::get(&conn, SEEN_KEYS).unwrap_or_default();
    let current: Vec<&str> = keys.iter().map(|k| k.fingerprint.as_str()).collect();
    if update_seen(&mut seen, &current, db::now_ms()) {
        let _ = db::set(&conn, SEEN_KEYS, &seen);
    }
}

// ---------- the checks ----------

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    Warning,
    Notice,
    Info,
}

/// An existing setting or function to call, with what it needs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FixCall {
    SetAutoLock { minutes: u32 },
    SetPhoneWhileLocked { on: bool },
    SetPhone { enabled: bool },
    SetPhoneLink { anywhere: bool },
    RemovePhone { id: String },
    SetApiServer { enabled: bool, port: u16 },
    SetOllamaServer { enabled: bool, port: u16 },
    NewApiKey,
    SetToolsAsk { connector: String, tools: Vec<String> },
    ForgetSiteKey { host: String },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Fix {
    /// The button's text.
    pub label: String,
    /// The question asked before anything changes.
    pub confirm: String,
    pub call: FixCall,
}

/// Texts are English templates with {name} placeholders filled from `args`,
/// so the window can translate them before filling.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Finding {
    pub id: String,
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    pub args: BTreeMap<String, String>,
    pub fix: Option<Fix>,
    /// A Settings section that has the control when there is no one-click fix.
    pub open: Option<String>,
}

impl Finding {
    fn new(id: impl Into<String>, severity: Severity, title: &str, detail: &str) -> Finding {
        Finding { id: id.into(), severity, title: title.into(), detail: detail.into(), args: BTreeMap::new(), fix: None, open: None }
    }

    fn arg(mut self, name: &str, value: impl ToString) -> Finding {
        self.args.insert(name.into(), value.to_string());
        self
    }

    fn fix(mut self, label: &str, confirm: &str, call: FixCall) -> Finding {
        self.fix = Some(Fix { label: label.into(), confirm: confirm.into(), call });
        self
    }

    fn open(mut self, section: &str) -> Finding {
        self.open = Some(section.into());
        self
    }
}

/// Findings over an overview, worst first. Pure: no state, no clock.
pub fn checks(o: &Overview) -> Vec<Finding> {
    let mut out = Vec::new();
    let p = &o.protection;
    let paired = !o.phones.devices.is_empty();

    if !p.encryption {
        out.push(Finding::new("encryption_off", Severity::Critical, "Your data is not encrypted", "SulcusAI could not find its data key, so chats and notes on this PC are not protected on disk. Restart SulcusAI; if this stays, reinstall it."));
    }
    if !p.lock_enabled {
        let severity = if paired { Severity::Warning } else { Severity::Notice };
        out.push(Finding::new("lock_off", severity, "App lock is off", "Anyone signed in to this Windows account can open SulcusAI and read your chats. A PIN keeps them closed.").open("security"));
    } else if p.auto_lock_minutes == 0 || p.auto_lock_minutes > AUTO_LOCK_LONG_MINUTES {
        let title = if p.auto_lock_minutes == 0 { "SulcusAI never locks by itself" } else { "SulcusAI stays unlocked for a long time" };
        out.push(
            Finding::new("auto_lock_long", Severity::Notice, title, "If you walk away, SulcusAI stays open for anyone at the screen. Locking after {minutes} minutes of no use is a common choice.")
                .arg("minutes", AUTO_LOCK_FIX_MINUTES)
                .fix("Lock after {minutes} minutes", "Lock SulcusAI after {minutes} minutes of no use?", FixCall::SetAutoLock { minutes: AUTO_LOCK_FIX_MINUTES }),
        );
    }

    if o.phones.page_on {
        if !paired {
            out.push(
                Finding::new("phone_page_idle", Severity::Notice, "The phone page is on but no phone is paired", "It still listens on your local network. Turn it off until you pair a phone.")
                    .fix("Turn off the phone page", "Turn off the phone page?", FixCall::SetPhone { enabled: false }),
            );
        }
        if paired && o.phones.anywhere {
            out.push(
                Finding::new("phone_anywhere", Severity::Info, "Your phone can reach this PC from anywhere", "It goes through SulcusAI's relay, protected by the pairing key and the phone's password. Turn this off to allow your home or office network only.")
                    .fix("Local network only", "Let phones reach this PC on the local network only?", FixCall::SetPhoneLink { anywhere: false }),
            );
        }
    }
    if p.lock_enabled && paired && o.phones.while_locked {
        out.push(
            Finding::new("phone_while_locked", Severity::Warning, "Phones keep working while SulcusAI is locked", "Locking the screen does not cut off a paired phone, so someone with the phone can still use your chats.")
                .fix("Stop phones while locked", "Stop paired phones from working while SulcusAI is locked?", FixCall::SetPhoneWhileLocked { on: false }),
        );
    }
    for d in &o.phones.devices {
        if !d.has_password {
            out.push(
                Finding::new(format!("phone_no_password:{}", d.id), Severity::Warning, "{name} has no password", "It was paired before phones needed one. Remove it and pair it again with a password.")
                    .arg("name", &d.name)
                    .fix("Remove {name}", "Remove {name}? You can pair it again afterwards.", FixCall::RemovePhone { id: d.id.clone() }),
            );
        }
        let idle_days = ((o.now - d.last_seen.unwrap_or(d.added)).max(0) / DAY_MS) as u64;
        if idle_days >= STALE_PHONE_DAYS {
            out.push(
                Finding::new(format!("phone_stale:{}", d.id), Severity::Notice, "{name} has not been used for {days} days", "A paired phone you no longer use is a way in you do not need.")
                    .arg("name", &d.name)
                    .arg("days", idle_days)
                    .fix("Remove {name}", "Remove {name}? You can pair it again afterwards.", FixCall::RemovePhone { id: d.id.clone() }),
            );
        }
    }

    for l in &o.listeners {
        if l.id != "phone" && l.enabled && l.running && !l.local_only {
            let off = match (l.id.as_str(), l.port) {
                ("api", Some(port)) => Some(FixCall::SetApiServer { enabled: false, port }),
                ("ollama", Some(port)) => Some(FixCall::SetOllamaServer { enabled: false, port }),
                _ => None,
            };
            let f = Finding::new(format!("listener_exposed:{}", l.id), Severity::Critical, "{name} is open to other computers", "It should only answer programs on this PC. Turn it off until that is sorted out.").arg("name", &l.name);
            out.push(match off {
                Some(call) => f.fix("Turn off", "Turn off {name}?", call),
                None => f.open("servers"),
            });
        }
        if l.enabled && !l.running {
            if let Some(problem) = &l.problem {
                out.push(Finding::new(format!("listener_problem:{}", l.id), Severity::Notice, "{name} could not start", "{problem}").arg("name", &l.name).arg("problem", problem).open("servers"));
            }
        }
        if l.id == "api" && l.enabled {
            if let Some(port) = l.port {
                out.push(
                    Finding::new("api_server_on", Severity::Info, "Programs on this PC can use your models", "Any program that has the local API key can ask your models questions. Turn it off when you are not using it.")
                        .fix("Turn off", "Turn off the local API server?", FixCall::SetApiServer { enabled: false, port }),
                );
            }
        }
        if l.id == "ollama" && l.enabled {
            if let Some(port) = l.port {
                out.push(
                    Finding::new("ollama_server_on", Severity::Info, "Programs made for Ollama can use your models", "They reach SulcusAI on this PC without a key. Turn it off when you are not using it.")
                        .fix("Turn off", "Turn off the server for programs made for Ollama?", FixCall::SetOllamaServer { enabled: false, port }),
                );
            }
        }
    }

    for a in o.addons.iter().filter(|a| a.enabled && !a.auto_run.is_empty()) {
        let web = a.host.is_some();
        let (severity, detail) = if web {
            (Severity::Warning, "These tools run without asking, and what they work on goes to {host}: {tools}.")
        } else {
            (Severity::Notice, "These tools can change things on this PC and run without asking: {tools}.")
        };
        out.push(
            Finding::new(format!("addon_auto_run:{}", a.id), severity, "{name} runs actions without asking", detail)
                .arg("name", &a.name)
                .arg("host", a.host.clone().unwrap_or_default())
                .arg("tools", a.auto_run.join(", "))
                .fix("Ask first", "Make {name} ask before using these tools?", FixCall::SetToolsAsk { connector: a.id.clone(), tools: a.auto_run.clone() }),
        );
    }

    for k in o.keys.iter().filter(|k| k.age_days >= OLD_KEY_DAYS) {
        let (id, fix) = match (k.kind.as_str(), &k.host) {
            ("api_server", _) => ("key_old:api_server".to_string(), Some(Fix { label: "Make a new key".into(), confirm: "Make a new key? Programs using the old one stop working until you give them the new one.".into(), call: FixCall::NewApiKey })),
            (_, Some(host)) => (
                format!("key_old:site:{host}"),
                Some(Fix { label: "Remove the key".into(), confirm: "Remove the saved key for {name}? SulcusAI asks again the next time a site needs it.".into(), call: FixCall::ForgetSiteKey { host: host.clone() } }),
            ),
            _ => (format!("key_old:{}", k.label), None),
        };
        let mut f = Finding::new(id, Severity::Notice, "The key for {name} is old", "SulcusAI has had it for {days} days. Keys that live a long time are more likely to have leaked; replacing them is cheap.").arg("name", &k.label).arg("days", k.age_days);
        f.fix = fix;
        out.push(f);
    }

    out.sort_by_key(|f| f.severity);
    out
}

pub fn find_fix<'a>(findings: &'a [Finding], id: &str) -> Option<&'a Finding> {
    findings.iter().find(|f| f.id == id && f.fix.is_some())
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct Report {
    pub overview: Overview,
    pub findings: Vec<Finding>,
}

fn gather(state: &AppStateRef<'_>) -> Result<Report, String> {
    let api = serde_json::to_value(crate::api_server::api_server_view(state.clone())).map_err(|e| e.to_string())?;
    let phone = serde_json::to_value(crate::phone::phone_view(state.clone())).map_err(|e| e.to_string())?;
    let overview = build(state, &api, &phone)?;
    remember_keys(state, &overview.keys);
    let findings = checks(&overview);
    Ok(Report { overview, findings })
}

#[tauri::command(async)]
pub fn security_overview(state: AppStateRef) -> Result<Report, String> {
    gather(&state)
}

/// Applies one finding's fix. Looked up again from the current state, so a
/// stale or invented id changes nothing, and refused unless the person has
/// said yes in the window.
#[tauri::command]
pub async fn security_apply_fix(state: AppStateRef<'_>, id: String, confirmed: bool) -> Result<Report, String> {
    if !confirmed {
        return Err("Nothing was changed: the fix was not confirmed.".into());
    }
    let report = gather(&state)?;
    let fix = find_fix(&report.findings, &id).and_then(|f| f.fix.clone()).ok_or("That finding is gone, so there is nothing to fix.")?;
    run(&state, fix.call).await?;
    state.log("security", &format!("Applied a fix from the Security page ({id})"));
    gather(&state)
}

async fn run(state: &AppStateRef<'_>, call: FixCall) -> Result<(), String> {
    match call {
        FixCall::SetAutoLock { minutes } => crate::security::set_auto_lock(state.clone(), minutes),
        FixCall::SetPhoneWhileLocked { on } => crate::phone::set_phone_while_locked(state.clone(), on).map(|_| ()),
        FixCall::SetPhone { enabled } => crate::phone::set_phone(state.clone(), enabled).await.map(|_| ()),
        FixCall::SetPhoneLink { anywhere } => crate::phone::set_phone_link(state.clone(), anywhere).map(|_| ()),
        FixCall::RemovePhone { id } => crate::phone::remove_phone(state.clone(), id).map(|_| ()),
        FixCall::SetApiServer { enabled, port } => crate::api_server::set_api_server(state.clone(), enabled, port).await.map(|_| ()),
        FixCall::SetOllamaServer { enabled, port } => crate::api_server::set_ollama_server(state.clone(), enabled, port).await.map(|_| ()),
        FixCall::NewApiKey => crate::api_server::new_api_key(state.clone()).map(|_| ()),
        FixCall::SetToolsAsk { connector, tools } => {
            for tool in tools {
                crate::connectors::set_tool_mode(state.clone(), connector.clone(), tool, ToolMode::Ask)?;
            }
            Ok(())
        }
        FixCall::ForgetSiteKey { host } => sitekeys::site_key_forget(state.clone(), host),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_800_000_000_000;

    /// A tidy setup: lock on with a short timeout, nothing listening, no
    /// phones, no add-ons, no keys.
    fn tidy() -> Overview {
        let mut o = Overview { version: "1.0.0".into(), now: NOW, ..Default::default() };
        o.protection = Protection { encryption: true, lock_enabled: true, hello_enabled: false, auto_lock_minutes: 10, keep_working: false };
        o.listeners = vec![
            Listener { id: "api".into(), name: "Local API server".into(), port: Some(7340), local_only: true, ..Default::default() },
            Listener { id: "ollama".into(), name: "Ollama server".into(), port: Some(11434), local_only: true, ..Default::default() },
            Listener { id: "phone".into(), name: "Phone page".into(), ..Default::default() },
        ];
        o
    }

    fn device(id: &str) -> Device {
        Device { id: id.into(), name: format!("Phone {id}"), added: NOW - DAY_MS, last_seen: Some(NOW - 1000), has_password: true }
    }

    fn ids(o: &Overview) -> Vec<String> {
        checks(o).into_iter().map(|f| f.id).collect()
    }

    fn finding(o: &Overview, id: &str) -> Finding {
        checks(o).into_iter().find(|f| f.id == id).unwrap_or_else(|| panic!("no finding {id}: {:?}", ids(o)))
    }

    #[test]
    fn a_tidy_setup_has_no_findings() {
        assert!(checks(&tidy()).is_empty());
    }

    #[test]
    fn missing_encryption_is_critical_and_has_no_one_click_fix() {
        let mut o = tidy();
        o.protection.encryption = false;
        let f = finding(&o, "encryption_off");
        assert_eq!(f.severity, Severity::Critical);
        assert!(f.fix.is_none());
    }

    #[test]
    fn app_lock_off_points_to_settings_and_is_worse_with_a_phone() {
        let mut o = tidy();
        o.protection.lock_enabled = false;
        let f = finding(&o, "lock_off");
        assert_eq!(f.severity, Severity::Notice);
        assert!(f.fix.is_none(), "a PIN has to be chosen by the person");
        assert_eq!(f.open.as_deref(), Some("security"));
        o.phones.devices.push(device("a"));
        assert_eq!(finding(&o, "lock_off").severity, Severity::Warning);
    }

    #[test]
    fn auto_lock_that_never_comes_or_comes_late_is_flagged_with_a_fix() {
        let mut o = tidy();
        for minutes in [0, 61, 24 * 60] {
            o.protection.auto_lock_minutes = minutes;
            let f = finding(&o, "auto_lock_long");
            assert_eq!(f.fix.unwrap().call, FixCall::SetAutoLock { minutes: AUTO_LOCK_FIX_MINUTES });
        }
        for minutes in [1, 15, 60] {
            o.protection.auto_lock_minutes = minutes;
            assert!(!ids(&o).contains(&"auto_lock_long".to_string()), "{minutes}");
        }
    }

    #[test]
    fn auto_lock_is_not_judged_when_app_lock_is_off() {
        let mut o = tidy();
        o.protection.lock_enabled = false;
        o.protection.auto_lock_minutes = 0;
        assert!(!ids(&o).contains(&"auto_lock_long".to_string()));
    }

    #[test]
    fn a_phone_page_with_no_phone_can_be_switched_off() {
        let mut o = tidy();
        o.phones.page_on = true;
        let f = finding(&o, "phone_page_idle");
        assert_eq!(f.fix.unwrap().call, FixCall::SetPhone { enabled: false });
        o.phones.devices.push(device("a"));
        assert!(!ids(&o).contains(&"phone_page_idle".to_string()));
        o.phones.page_on = false;
        o.phones.devices.clear();
        assert!(!ids(&o).contains(&"phone_page_idle".to_string()));
    }

    #[test]
    fn reaching_phones_from_anywhere_is_only_noted_while_the_page_is_on() {
        let mut o = tidy();
        o.phones.devices.push(device("a"));
        o.phones.anywhere = true;
        assert!(!ids(&o).contains(&"phone_anywhere".to_string()));
        o.phones.page_on = true;
        let f = finding(&o, "phone_anywhere");
        assert_eq!(f.severity, Severity::Info);
        assert_eq!(f.fix.unwrap().call, FixCall::SetPhoneLink { anywhere: false });
        o.phones.anywhere = false;
        assert!(!ids(&o).contains(&"phone_anywhere".to_string()));
    }

    #[test]
    fn phones_working_while_locked_matter_only_with_app_lock_on() {
        let mut o = tidy();
        o.phones.devices.push(device("a"));
        o.phones.while_locked = true;
        let f = finding(&o, "phone_while_locked");
        assert_eq!(f.severity, Severity::Warning);
        assert_eq!(f.fix.unwrap().call, FixCall::SetPhoneWhileLocked { on: false });
        o.phones.while_locked = false;
        assert!(!ids(&o).contains(&"phone_while_locked".to_string()));
        o.phones.while_locked = true;
        o.protection.lock_enabled = false;
        assert!(!ids(&o).contains(&"phone_while_locked".to_string()));
    }

    #[test]
    fn a_phone_without_a_password_is_flagged_and_can_be_removed() {
        let mut o = tidy();
        let mut d = device("a");
        d.has_password = false;
        o.phones.devices.push(d);
        o.phones.devices.push(device("b"));
        let f = finding(&o, "phone_no_password:a");
        assert_eq!(f.fix.unwrap().call, FixCall::RemovePhone { id: "a".into() });
        assert!(!ids(&o).contains(&"phone_no_password:b".to_string()));
    }

    #[test]
    fn a_phone_unused_for_ninety_days_is_flagged() {
        let mut o = tidy();
        let mut old = device("a");
        old.last_seen = Some(NOW - 90 * DAY_MS);
        let mut recent = device("b");
        recent.last_seen = Some(NOW - 89 * DAY_MS);
        let mut never = device("c");
        never.last_seen = None;
        never.added = NOW - 120 * DAY_MS;
        let mut new = device("d");
        new.last_seen = None;
        new.added = NOW - DAY_MS;
        o.phones.devices = vec![old, recent, never, new];
        let found = ids(&o);
        assert!(found.contains(&"phone_stale:a".to_string()));
        assert!(found.contains(&"phone_stale:c".to_string()));
        assert!(!found.contains(&"phone_stale:b".to_string()));
        assert!(!found.contains(&"phone_stale:d".to_string()));
        let f = finding(&o, "phone_stale:a");
        assert_eq!(f.args["days"], "90");
        assert_eq!(f.fix.unwrap().call, FixCall::RemovePhone { id: "a".into() });
    }

    #[test]
    fn a_server_open_beyond_this_pc_is_critical_with_a_turn_off_fix() {
        let mut o = tidy();
        o.listeners[0] = Listener { id: "api".into(), name: "Local API server".into(), port: Some(7340), enabled: true, running: true, local_only: false, problem: None };
        o.listeners[1] = Listener { id: "ollama".into(), name: "Ollama server".into(), port: Some(11434), enabled: true, running: true, local_only: false, problem: None };
        let api = finding(&o, "listener_exposed:api");
        assert_eq!(api.severity, Severity::Critical);
        assert_eq!(api.fix.unwrap().call, FixCall::SetApiServer { enabled: false, port: 7340 });
        let ollama = finding(&o, "listener_exposed:ollama");
        assert_eq!(ollama.fix.unwrap().call, FixCall::SetOllamaServer { enabled: false, port: 11434 });
    }

    #[test]
    fn the_phone_page_is_on_the_local_network_by_design() {
        let mut o = tidy();
        o.listeners[2] = Listener { id: "phone".into(), name: "Phone page".into(), port: Some(47818), enabled: true, running: true, local_only: false, problem: None };
        o.phones.page_on = true;
        o.phones.devices.push(device("a"));
        assert!(!ids(&o).iter().any(|i| i.starts_with("listener_exposed")));
    }

    #[test]
    fn a_stopped_server_with_a_problem_is_noted_but_not_when_it_is_off() {
        let mut o = tidy();
        o.listeners[0].enabled = true;
        o.listeners[0].running = false;
        o.listeners[0].problem = Some("Port 7340 is in use.".into());
        let f = finding(&o, "listener_problem:api");
        assert_eq!(f.args["problem"], "Port 7340 is in use.");
        assert_eq!(f.open.as_deref(), Some("servers"));
        o.listeners[0].enabled = false;
        assert!(!ids(&o).contains(&"listener_problem:api".to_string()));
    }

    #[test]
    fn running_servers_are_noted_with_a_turn_off_fix() {
        let mut o = tidy();
        o.listeners[0].enabled = true;
        o.listeners[0].running = true;
        o.listeners[1].enabled = true;
        o.listeners[1].running = true;
        assert_eq!(finding(&o, "api_server_on").fix.unwrap().call, FixCall::SetApiServer { enabled: false, port: 7340 });
        assert_eq!(finding(&o, "ollama_server_on").fix.unwrap().call, FixCall::SetOllamaServer { enabled: false, port: 11434 });
        o.listeners[0].enabled = false;
        o.listeners[1].enabled = false;
        assert!(checks(&o).is_empty());
    }

    fn addon(id: &str, host: Option<&str>, auto: &[&str]) -> AddonInfo {
        AddonInfo { id: id.into(), name: id.to_uppercase(), enabled: true, host: host.map(str::to_string), tools: 4, auto_run: auto.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    #[test]
    fn a_web_addon_running_tools_unasked_is_a_warning_a_local_one_a_notice() {
        let mut o = tidy();
        o.addons = vec![addon("web", Some("mcp.example.com"), &["search"]), addon("local", None, &["write_file", "move_file"])];
        let web = finding(&o, "addon_auto_run:web");
        assert_eq!(web.severity, Severity::Warning);
        assert_eq!(web.args["host"], "mcp.example.com");
        assert_eq!(web.fix.unwrap().call, FixCall::SetToolsAsk { connector: "web".into(), tools: vec!["search".into()] });
        let local = finding(&o, "addon_auto_run:local");
        assert_eq!(local.severity, Severity::Notice);
        assert_eq!(local.args["tools"], "write_file, move_file");
    }

    #[test]
    fn addons_that_ask_or_are_off_raise_nothing() {
        let mut o = tidy();
        let mut off = addon("off", Some("mcp.example.com"), &["search"]);
        off.enabled = false;
        o.addons = vec![addon("asks", Some("mcp.example.com"), &[]), off];
        assert!(checks(&o).is_empty());
    }

    fn key(kind: &str, label: &str, host: Option<&str>, age_days: u64) -> KeyInfo {
        KeyInfo { kind: kind.into(), label: label.into(), host: host.map(str::to_string), age_days, fingerprint: String::new() }
    }

    #[test]
    fn keys_older_than_a_year_can_be_replaced_or_removed() {
        let mut o = tidy();
        o.keys = vec![key("api_server", "Local API server", None, 365), key("site", "Finnhub", Some("finnhub.io"), 400), key("site", "Fresh", Some("fresh.io"), 364)];
        let api = finding(&o, "key_old:api_server");
        assert_eq!(api.fix.unwrap().call, FixCall::NewApiKey);
        let site = finding(&o, "key_old:site:finnhub.io");
        assert_eq!(site.args["days"], "400");
        assert_eq!(site.fix.unwrap().call, FixCall::ForgetSiteKey { host: "finnhub.io".into() });
        assert!(!ids(&o).contains(&"key_old:site:fresh.io".to_string()));
    }

    #[test]
    fn findings_come_worst_first() {
        let mut o = tidy();
        o.protection.lock_enabled = false;
        o.protection.encryption = false;
        o.listeners[0] = Listener { id: "api".into(), name: "Local API server".into(), port: Some(7340), enabled: true, running: true, local_only: true, problem: None };
        let severities: Vec<Severity> = checks(&o).iter().map(|f| f.severity).collect();
        let mut sorted = severities.clone();
        sorted.sort();
        assert_eq!(severities, sorted);
        assert_eq!(severities[0], Severity::Critical);
    }

    #[test]
    fn every_fix_has_a_label_and_a_question_and_ids_are_unique() {
        let mut o = tidy();
        o.protection.auto_lock_minutes = 0;
        o.phones = Phones { page_on: true, anywhere: true, while_locked: true, devices: vec![device("a"), Device { has_password: false, last_seen: None, added: 0, ..device("b") }], open_sessions: 1 };
        o.listeners[0].enabled = true;
        o.listeners[0].running = true;
        o.addons = vec![addon("web", Some("h.example"), &["t"])];
        o.keys = vec![key("api_server", "Local API server", None, 999)];
        let found = checks(&o);
        assert!(found.len() >= 6);
        let mut seen = std::collections::BTreeSet::new();
        for f in &found {
            assert!(seen.insert(f.id.clone()), "duplicate id {}", f.id);
            assert!(!f.title.contains('\u{2014}') && !f.detail.contains('\u{2014}'));
            if let Some(fix) = &f.fix {
                assert!(!fix.label.is_empty() && fix.confirm.contains('?'), "{}", f.id);
            }
        }
    }

    #[test]
    fn a_fix_is_found_only_for_an_existing_finding_that_has_one() {
        let mut o = tidy();
        o.protection.lock_enabled = false;
        o.protection.encryption = false;
        o.protection.auto_lock_minutes = 0;
        let found = checks(&o);
        assert!(find_fix(&found, "lock_off").is_none(), "no fix to apply");
        assert!(find_fix(&found, "invented").is_none());
        o.protection.lock_enabled = true;
        let found = checks(&o);
        assert!(find_fix(&found, "auto_lock_long").is_some());
    }

    #[test]
    fn a_fix_call_serializes_as_data_the_window_can_show() {
        let v = serde_json::to_value(FixCall::SetApiServer { enabled: false, port: 7340 }).unwrap();
        assert_eq!(v, json!({ "kind": "set_api_server", "enabled": false, "port": 7340 }));
        let back: FixCall = serde_json::from_value(v).unwrap();
        assert_eq!(back, FixCall::SetApiServer { enabled: false, port: 7340 });
    }

    #[test]
    fn key_ages_count_from_first_sight_and_forget_removed_keys() {
        let mut seen = BTreeMap::new();
        assert!(update_seen(&mut seen, &["a", "b"], NOW - 10 * DAY_MS));
        assert!(!update_seen(&mut seen, &["a", "b"], NOW));
        assert_eq!(age_days(&seen, "a", NOW), 10);
        assert_eq!(age_days(&seen, "unknown", NOW), 0);
        assert!(update_seen(&mut seen, &["b", "c"], NOW));
        assert!(!seen.contains_key("a"));
        assert_eq!(age_days(&seen, "c", NOW), 0);
        assert_eq!(age_days(&seen, "b", NOW), 10);
    }

    #[test]
    fn ports_are_read_from_addresses() {
        assert_eq!(port_of("http://192.168.1.4:47818"), Some(47818));
        assert_eq!(port_of("http://192.168.1.4:47818/"), Some(47818));
        assert_eq!(port_of("http://example.com"), None);
    }

    fn scratch() -> std::sync::Arc<AppState> {
        crate::testutil::scratch_state()
    }

    #[test]
    fn the_overview_reads_protection_phones_listeners_add_ons_and_keys() {
        use crate::connectors::{Connector, ToolMode};
        use crate::mcp::{McpTool, ServerSpec};
        use std::collections::HashMap;

        let state = scratch();
        let c = state.cipher().unwrap();
        let tool = |name: &str, read_only: bool| McpTool { name: name.into(), description: String::new(), input_schema: json!({}), read_only };
        {
            let conn = state.db.lock().unwrap();
            let local = Connector {
                id: "files".into(),
                name: "Files".into(),
                spec: ServerSpec { command: "node".into(), env: HashMap::from([("TOKEN".to_string(), "x".to_string())]), ..Default::default() },
                enabled: true,
                plugin: None,
                tools: vec![tool("read", true), tool("write", false), tool("delete", false)],
                tool_modes: HashMap::from([("delete".to_string(), ToolMode::Off), ("write".to_string(), ToolMode::Allow)]),
                auth: None,
            };
            let web = Connector {
                id: "notes".into(),
                name: "Notes".into(),
                spec: ServerSpec { url: "https://mcp.example.com/mcp".into(), ..Default::default() },
                enabled: true,
                plugin: None,
                tools: vec![tool("search", true), tool("save", false)],
                tool_modes: HashMap::from([("search".to_string(), ToolMode::Allow)]),
                auth: None,
            };
            crate::connectors::store(&conn, &c, "connectors", "files", &local).unwrap();
            crate::connectors::store(&conn, &c, "connectors", "notes", &web).unwrap();
            sitekeys::approve(
                &conn,
                &c,
                "chat1",
                sitekeys::SiteKey { host: "finnhub.io".into(), platform: "Finnhub".into(), param: "token".into(), header: String::new(), key: "k-123".into(), chats: vec![] },
                Some("k-123".to_string()),
            )
            .unwrap();
        }
        let now = db::now_ms();
        let api = json!({ "enabled": true, "port": 7340, "running": true, "key": "sk-sulcus-abc", "ollama": true, "ollama_port": 11434, "ollama_running": false, "ollama_problem": "Port 11434 is in use." });
        let phone = json!({
            "enabled": true, "running": true, "address": "http://192.168.1.4:47818", "anywhere": true, "while_locked": true,
            "phones": [
                { "id": "p1", "name": "Pixel", "added": now - 5 * DAY_MS, "last_seen": now - 1000, "has_password": true },
                { "id": "p2", "name": "Old", "added": now - 200 * DAY_MS, "last_seen": null, "has_password": false }
            ]
        });
        let o = build(&state, &api, &phone).unwrap();

        assert!(o.protection.encryption && !o.protection.lock_enabled);
        assert_eq!(o.phones.devices.len(), 2);
        assert_eq!(o.phones.open_sessions, 1);
        assert!(o.phones.page_on && o.phones.anywhere && o.phones.while_locked);
        let by = |id: &str| o.listeners.iter().find(|l| l.id == id).unwrap().clone();
        assert_eq!((by("api").port, by("api").running, by("api").local_only), (Some(7340), true, true));
        assert_eq!((by("ollama").running, by("ollama").problem.as_deref()), (false, Some("Port 11434 is in use.")));
        assert_eq!((by("phone").port, by("phone").local_only), (Some(47818), false));

        let files = o.addons.iter().find(|a| a.id == "files").unwrap();
        assert_eq!(files.host, None);
        assert_eq!(files.auto_run, vec!["write".to_string()], "read-only runs freely and delete is off");
        assert_eq!((files.off, files.ask, files.has_secrets), (1, 0, true));
        let notes = o.addons.iter().find(|a| a.id == "notes").unwrap();
        assert_eq!(notes.host.as_deref(), Some("mcp.example.com"));
        assert_eq!(notes.auto_run, vec!["search".to_string()], "on the web even a read leaves the PC");
        assert_eq!(notes.ask, 1);

        assert_eq!(o.keys.len(), 2);
        assert!(o.keys.iter().any(|k| k.kind == "api_server") && o.keys.iter().any(|k| k.host.as_deref() == Some("finnhub.io")));
        assert!(o.keys.iter().all(|k| k.age_days == 0 && !k.fingerprint.is_empty()));
        let json = serde_json::to_string(&o).unwrap();
        assert!(!json.contains("sk-sulcus-abc") && !json.contains("k-123"), "no key leaves the core");
        assert!(!json.contains("fingerprint"));

        remember_keys(&state, &o.keys);
        let seen: BTreeMap<String, i64> = db::get(&state.db.lock().unwrap(), SEEN_KEYS).unwrap();
        assert_eq!(seen.len(), 2);
    }

    #[test]
    fn a_disabled_api_server_has_no_key_listed() {
        let state = scratch();
        let api = json!({ "enabled": false, "port": 7340, "key": "sk-sulcus-abc", "ollama": false, "ollama_port": 11434 });
        let o = build(&state, &api, &json!({ "phones": [] })).unwrap();
        assert!(o.keys.is_empty());
        assert!(checks(&o).iter().all(|f| f.severity != Severity::Critical));
    }
}
