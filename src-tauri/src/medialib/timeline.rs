// SPDX-License-Identifier: AGPL-3.0-only
//! The screen-history timeline's settings. It records nothing until turned
//! on, keeps everything on this PC, and once a PIN is set, changing it or
//! opening the history needs that PIN.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::db;

const KEY: &str = "media.timeline";
const PIN_KEY: &str = "media.timeline.pin";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
    /// Seconds between captures.
    pub every_secs: u32,
    pub keep_days: u32,
    /// Programs never captured (by executable name), on top of private
    /// browser windows and the password vault, which never are.
    pub skip_apps: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { enabled: false, every_secs: 30, keep_days: 14, skip_apps: vec![] }
    }
}

impl Settings {
    fn clamped(mut self) -> Self {
        self.every_secs = self.every_secs.clamp(5, 600);
        self.keep_days = self.keep_days.clamp(1, 365);
        self.skip_apps = self.skip_apps.into_iter().map(|a| a.trim().to_ascii_lowercase()).filter(|a| !a.is_empty()).collect();
        self
    }

    pub fn captures(&self, exe: &str, private_window: bool) -> bool {
        let exe = exe.to_ascii_lowercase();
        self.enabled && !private_window && !exe.contains("sulcusai") && !self.skip_apps.iter().any(|a| *a == exe)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct View {
    pub settings: Settings,
    pub locked: bool,
}

fn hash_pin(pin: &str) -> Result<String, String> {
    use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
    let salt = SaltString::generate(&mut OsRng);
    argon2::Argon2::default().hash_password(pin.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| e.to_string())
}

fn check(conn: &Connection, pin: Option<&str>) -> Result<(), String> {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    let Some(stored) = db::get::<String>(conn, PIN_KEY) else { return Ok(()) };
    let ok = pin.is_some_and(|p| PasswordHash::new(&stored).is_ok_and(|h| argon2::Argon2::default().verify_password(p.as_bytes(), &h).is_ok()));
    if ok {
        Ok(())
    } else {
        std::thread::sleep(std::time::Duration::from_millis(800));
        Err("That PIN isn't right.".into())
    }
}

pub fn view(conn: &Connection) -> View {
    View { settings: db::get(conn, KEY).unwrap_or_default(), locked: db::get::<String>(conn, PIN_KEY).is_some() }
}

pub fn set(conn: &Connection, s: Settings, pin: Option<&str>) -> Result<(), String> {
    check(conn, pin)?;
    db::set(conn, KEY, &s.clamped())
}

/// Opening the history asks for the PIN the same way.
pub fn unlock(conn: &Connection, pin: Option<&str>) -> Result<(), String> {
    check(conn, pin)
}

pub fn set_pin(conn: &Connection, pin: Option<&str>, new_pin: &str) -> Result<(), String> {
    check(conn, pin)?;
    if new_pin.is_empty() {
        conn.execute("DELETE FROM settings WHERE key = ?1", [PIN_KEY]).map_err(|e| e.to_string())?;
        return Ok(());
    }
    if new_pin.chars().count() < 4 {
        return Err("Use at least 4 characters.".into());
    }
    db::set(conn, PIN_KEY, &hash_pin(new_pin)?)
}

/// Captures older than the kept days, to delete.
pub fn expired(taken: &[i64], now: i64, keep_days: u32) -> Vec<i64> {
    let cutoff = now - keep_days as i64 * 86_400;
    taken.iter().copied().filter(|t| *t < cutoff).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)", []).unwrap();
        c
    }

    #[test]
    fn off_by_default_and_skips() {
        let c = conn();
        let v = view(&c);
        assert!(!v.settings.enabled && !v.locked);
        assert!(!v.settings.captures("notepad.exe", false));
        let s = Settings { enabled: true, every_secs: 1, keep_days: 0, skip_apps: vec![" Bank.EXE ".into()] };
        set(&c, s, None).unwrap();
        let s = view(&c).settings;
        assert_eq!((s.every_secs, s.keep_days), (5, 1));
        assert!(s.captures("notepad.exe", false));
        assert!(!s.captures("bank.exe", false));
        assert!(!s.captures("chrome.exe", true));
        assert!(!s.captures("SulcusAI.exe", false));
    }

    #[test]
    fn pin_locks_changes() {
        let c = conn();
        assert!(set_pin(&c, None, "12").is_err());
        set_pin(&c, None, "2468").unwrap();
        assert!(view(&c).locked);
        assert!(set(&c, Settings::default(), None).is_err());
        assert!(unlock(&c, Some("0000")).is_err());
        set(&c, Settings { enabled: true, ..Default::default() }, Some("2468")).unwrap();
        set_pin(&c, Some("2468"), "").unwrap();
        assert!(!view(&c).locked);
    }

    #[test]
    fn old_captures_expire() {
        assert_eq!(expired(&[0, 86_400 * 9, 86_400 * 10], 86_400 * 11, 2), vec![0]);
    }
}
