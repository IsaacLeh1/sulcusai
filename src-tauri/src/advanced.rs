// SPDX-License-Identifier: AGPL-3.0-only
//! Advanced mode (off by default): model tuning, guardrails and how the
//! assistant processes a chat. Nothing here applies until it's turned on,
//! and a separate PIN can lock it (parental controls).

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::db;
use crate::tools::Risk;
use crate::{AppState, AppStateRef};

const KEY: &str = "advanced";
const LOCK_KEY: &str = "advanced_lock";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Advanced {
    pub enabled: bool,
    pub sampling: Sampling,
    pub engine: EngineTuning,
    pub guardrails: Guardrails,
    pub processing: Processing,
}

/// How the next word is picked. Empty means the app's default.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Sampling {
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub top_k: Option<u32>,
    pub min_p: Option<f64>,
    pub repeat_penalty: Option<f64>,
    pub presence_penalty: Option<f64>,
    pub frequency_penalty: Option<f64>,
    pub seed: Option<i64>,
}

/// How the engine runs the model. Empty means worked out for this PC.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EngineTuning {
    /// Context window in tokens.
    pub ctx: Option<u32>,
    /// Layers on the graphics card.
    pub gpu_layers: Option<u32>,
    pub threads: Option<u32>,
    /// Tokens read per batch while reading the prompt.
    pub batch: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Guardrails {
    /// Replaces the app's own instructions (its role, honesty and tool rules).
    pub instructions: Option<String>,
    /// Added after them.
    pub extra: String,
    /// Kinds of action the assistant may never take: "write", "execute",
    /// "connector".
    pub never: Vec<String>,
    /// Ask before every change or command, even in Auto and Bypass.
    pub always_ask: bool,
    /// Most model replies in one turn before it pauses.
    pub max_steps: Option<u32>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Thinking {
    /// The model's own default.
    #[default]
    Auto,
    On,
    Off,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Processing {
    pub thinking: Thinking,
    /// Continue in a new chat with a summary once the window is this full
    /// (0.5–0.95). Off: the oldest messages are left out instead.
    pub handoff_at: Option<f64>,
    pub handoff_off: bool,
    /// Most replies a helper agent gets for its task.
    pub helper_steps: Option<u32>,
}

pub const DEFAULT_MAX_STEPS: usize = 30;
pub const DEFAULT_HELPER_STEPS: usize = 12;
pub const DEFAULT_HANDOFF_AT: f64 = 0.8;

/// The stored settings, as the Settings page edits them.
pub fn stored(conn: &Connection) -> Advanced {
    db::get(conn, KEY).unwrap_or_default()
}

/// What applies now: the stored settings when advanced mode is on,
/// otherwise the defaults.
pub fn active(conn: &Connection) -> Advanced {
    let a = stored(conn);
    if a.enabled {
        a
    } else {
        Advanced::default()
    }
}

pub fn risk_key(r: Risk) -> &'static str {
    match r {
        Risk::Read => "read",
        Risk::Write => "write",
        Risk::Execute => "execute",
        Risk::Memory => "memory",
        Risk::Connector => "connector",
        Risk::Submit => "submit",
    }
}

impl Advanced {
    /// Fields merged into each chat request.
    pub fn request_extra(&self) -> Value {
        let s = &self.sampling;
        let mut m = Map::new();
        let mut put = |k: &str, v: Option<Value>| {
            if let Some(v) = v {
                m.insert(k.into(), v);
            }
        };
        put("temperature", s.temperature.map(|v| json!(v.clamp(0.0, 2.0))));
        put("top_p", s.top_p.map(|v| json!(v.clamp(0.0, 1.0))));
        put("top_k", s.top_k.map(|v| json!(v)));
        put("min_p", s.min_p.map(|v| json!(v.clamp(0.0, 1.0))));
        put("repeat_penalty", s.repeat_penalty.map(|v| json!(v.clamp(0.5, 2.0))));
        put("presence_penalty", s.presence_penalty.map(|v| json!(v.clamp(-2.0, 2.0))));
        put("frequency_penalty", s.frequency_penalty.map(|v| json!(v.clamp(-2.0, 2.0))));
        put("seed", s.seed.map(|v| json!(v)));
        match self.processing.thinking {
            Thinking::Auto => {}
            Thinking::On => put("chat_template_kwargs", Some(json!({ "enable_thinking": true }))),
            Thinking::Off => put("chat_template_kwargs", Some(json!({ "enable_thinking": false }))),
        }
        Value::Object(m)
    }

    /// The instructions every chat starts with.
    pub fn base_prompt(&self, app_default: String) -> String {
        let g = &self.guardrails;
        let mut base = g.instructions.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()).map(str::to_string).unwrap_or(app_default);
        if !g.extra.trim().is_empty() {
            base.push_str("\n\n");
            base.push_str(g.extra.trim());
        }
        base
    }

    pub fn max_steps(&self) -> usize {
        self.guardrails.max_steps.map_or(DEFAULT_MAX_STEPS, |n| n.clamp(1, 200) as usize)
    }

    pub fn helper_steps(&self) -> usize {
        self.processing.helper_steps.map_or(DEFAULT_HELPER_STEPS, |n| n.clamp(1, 100) as usize)
    }

    /// How full the window gets before a handoff, or None for never.
    pub fn handoff_at(&self) -> Option<f64> {
        (!self.processing.handoff_off).then(|| self.processing.handoff_at.unwrap_or(DEFAULT_HANDOFF_AT).clamp(0.5, 0.95))
    }

    pub fn forbids(&self, r: Risk) -> bool {
        self.guardrails.never.iter().any(|k| k == risk_key(r))
    }

    /// The tool may run without asking? (Read and memory always may.)
    pub fn must_ask(&self, r: Risk) -> bool {
        self.guardrails.always_ask && matches!(r, Risk::Write | Risk::Execute | Risk::Connector)
    }
}

// ---------- parental lock ----------

fn hash_pin(pin: &str) -> Result<String, String> {
    use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
    let salt = SaltString::generate(&mut OsRng);
    argon2::Argon2::default().hash_password(pin.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| e.to_string())
}

fn pin_ok(stored: &str, pin: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    PasswordHash::new(stored).is_ok_and(|h| argon2::Argon2::default().verify_password(pin.as_bytes(), &h).is_ok())
}

fn lock_hash(conn: &Connection) -> Option<String> {
    db::get(conn, LOCK_KEY)
}

fn check_lock(conn: &Connection, pin: Option<&str>) -> Result<(), String> {
    match lock_hash(conn) {
        None => Ok(()),
        Some(h) if pin.is_some_and(|p| pin_ok(&h, p)) => Ok(()),
        Some(_) if pin.is_some() => {
            // Slow down guessing.
            std::thread::sleep(std::time::Duration::from_millis(800));
            Err("That PIN isn't right.".into())
        }
        Some(_) => Err("Advanced settings are locked. Enter the PIN to change them.".into()),
    }
}

#[derive(Serialize)]
pub struct AdvancedView {
    settings: Advanced,
    locked: bool,
}

#[tauri::command]
pub fn advanced_view(state: AppStateRef) -> AdvancedView {
    let conn = state.db.lock().unwrap();
    AdvancedView { settings: stored(&conn), locked: lock_hash(&conn).is_some() }
}

#[tauri::command]
pub fn set_advanced(state: AppStateRef, settings: Advanced, pin: Option<String>) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    check_lock(&conn, pin.as_deref())?;
    let was = stored(&conn).enabled;
    db::set(&conn, KEY, &settings)?;
    drop(conn);
    if was != settings.enabled {
        state.log("settings", if settings.enabled { "Turned on advanced mode" } else { "Turned off advanced mode" });
    } else {
        state.log("settings", "Changed advanced settings");
    }
    Ok(())
}

/// Sets, changes or (with an empty `new_pin`) removes the advanced-mode PIN.
#[tauri::command]
pub fn set_advanced_pin(state: AppStateRef, pin: Option<String>, new_pin: String) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    check_lock(&conn, pin.as_deref())?;
    if new_pin.is_empty() {
        conn.execute("DELETE FROM settings WHERE key = ?1", [LOCK_KEY]).map_err(|e| e.to_string())?;
        drop(conn);
        state.log("settings", "Removed the advanced-settings PIN");
        return Ok(());
    }
    if new_pin.chars().count() < 4 {
        return Err("Use at least 4 characters.".into());
    }
    db::set(&conn, LOCK_KEY, &hash_pin(&new_pin)?)?;
    drop(conn);
    state.log("settings", "Set a PIN for advanced settings");
    Ok(())
}

/// Checks the PIN without changing anything (to open the settings).
#[tauri::command]
pub fn check_advanced_pin(state: AppStateRef, pin: String) -> Result<(), String> {
    check_lock(&state.db.lock().unwrap(), Some(&pin))
}

pub fn get(state: &AppState) -> Advanced {
    active(&state.db.lock().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_applies_until_turned_on() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_for_test(&conn);
        let mut a = Advanced::default();
        a.sampling.temperature = Some(1.2);
        a.guardrails.max_steps = Some(5);
        db::set(&conn, KEY, &a).unwrap();
        assert_eq!(active(&conn).max_steps(), DEFAULT_MAX_STEPS);
        assert_eq!(active(&conn).request_extra(), json!({}));
        a.enabled = true;
        db::set(&conn, KEY, &a).unwrap();
        assert_eq!(active(&conn).max_steps(), 5);
        assert_eq!(active(&conn).request_extra(), json!({ "temperature": 1.2 }));
    }

    #[test]
    fn settings_map_to_the_request_and_limits() {
        let mut a = Advanced { enabled: true, ..Default::default() };
        a.sampling = Sampling { temperature: Some(9.0), top_k: Some(40), min_p: Some(0.05), ..Default::default() };
        a.processing.thinking = Thinking::Off;
        let r = a.request_extra();
        assert_eq!(r["temperature"], json!(2.0), "clamped");
        assert_eq!(r["top_k"], json!(40));
        assert_eq!(r["chat_template_kwargs"]["enable_thinking"], json!(false));

        assert_eq!(a.handoff_at(), Some(DEFAULT_HANDOFF_AT));
        a.processing.handoff_at = Some(0.3);
        assert_eq!(a.handoff_at(), Some(0.5), "clamped");
        a.processing.handoff_off = true;
        assert_eq!(a.handoff_at(), None);

        a.guardrails.instructions = Some("Be a pirate.".into());
        a.guardrails.extra = "Answer in French.".into();
        assert_eq!(a.base_prompt("default".into()), "Be a pirate.\n\nAnswer in French.");
        a.guardrails.instructions = Some("  ".into());
        assert!(a.base_prompt("default".into()).starts_with("default"));

        a.guardrails.never = vec!["execute".into()];
        assert!(a.forbids(Risk::Execute) && !a.forbids(Risk::Write));
        a.guardrails.always_ask = true;
        assert!(a.must_ask(Risk::Write) && !a.must_ask(Risk::Read));
    }

    #[test]
    fn the_pin_locks_changes() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_for_test(&conn);
        assert!(check_lock(&conn, None).is_ok());
        db::set(&conn, LOCK_KEY, &hash_pin("2468").unwrap()).unwrap();
        assert!(check_lock(&conn, None).is_err());
        assert!(check_lock(&conn, Some("2468")).is_ok());
        assert!(check_lock(&conn, Some("1111")).is_err());
    }
}
