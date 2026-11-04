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

// ---------- the captures ----------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shot {
    pub at: i64,
    pub exe: String,
    pub title: String,
}

/// Browsers' private windows say so in their titles.
pub fn private_title(title: &str) -> bool {
    let t = title.to_lowercase();
    ["inprivate", "incognito", "private browsing", "private window"].iter().any(|w| t.contains(w))
}

fn table(conn: &Connection) -> Result<(), String> {
    conn.execute("CREATE TABLE IF NOT EXISTS timeline (at INTEGER PRIMARY KEY, data TEXT NOT NULL)", []).map(|_| ()).map_err(|e| e.to_string())
}

fn dir(state: &crate::AppState) -> std::path::PathBuf {
    state.paths.data.join("timeline")
}

pub fn shots(conn: &Connection, c: &crate::crypto::Cipher, from: i64, to: i64) -> Vec<Shot> {
    if table(conn).is_err() {
        return vec![];
    }
    let Ok(mut st) = conn.prepare("SELECT data FROM timeline WHERE at >= ?1 AND at < ?2 ORDER BY at") else { return vec![] };
    let rows: Vec<String> = st.query_map([from, to], |r| r.get(0)).map(|r| r.flatten().collect()).unwrap_or_default();
    rows.iter().filter_map(|d| c.decrypt(d).ok()).filter_map(|s| serde_json::from_str(&s).ok()).collect()
}

/// The words of a capture's window, searched across days.
pub fn search(conn: &Connection, c: &crate::crypto::Cipher, query: &str) -> Vec<Shot> {
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_string).collect();
    shots(conn, c, 0, i64::MAX).into_iter().filter(|s| {
        let hay = format!("{} {}", s.exe, s.title).to_lowercase();
        !words.is_empty() && words.iter().all(|w| hay.contains(w.as_str()))
    }).collect()
}

pub fn image(state: &crate::AppState, c: &crate::crypto::Cipher, at: i64) -> Result<Vec<u8>, String> {
    let blob = std::fs::read(dir(state).join(format!("{at}.bin"))).map_err(|_| "That capture is gone.".to_string())?;
    c.open_bytes(&blob)
}

pub fn clear(state: &crate::AppState) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    table(&conn)?;
    conn.execute("DELETE FROM timeline", []).map_err(|e| e.to_string())?;
    drop(conn);
    let _ = std::fs::remove_dir_all(dir(state));
    Ok(())
}

fn prune(state: &crate::AppState, keep_days: u32, now: i64) {
    let conn = state.db.lock().unwrap();
    if table(&conn).is_err() {
        return;
    }
    let taken: Vec<i64> = conn.prepare("SELECT at FROM timeline").and_then(|mut st| st.query_map([], |r| r.get(0)).map(|r| r.flatten().collect())).unwrap_or_default();
    for at in expired(&taken, now, keep_days) {
        let _ = conn.execute("DELETE FROM timeline WHERE at = ?1", [at]);
        let _ = std::fs::remove_file(dir(state).join(format!("{at}.bin")));
    }
}

/// One capture, when everything allows it: the feature and switch on, the
/// app unlocked, the window in front not excluded and the screen changed.
fn take(state: &crate::AppState, last: &mut u64) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    let s: Settings = db::get(&conn, KEY).unwrap_or_default();
    let on = crate::features::is_on(&conn, crate::features::Feature::Library);
    drop(conn);
    if !on || !s.enabled {
        return Ok(());
    }
    let c = state.cipher()?;
    let (exe, title) = crate::programs::front_window();
    if !s.captures(&exe, private_title(&title)) {
        return Ok(());
    }
    let img = crate::media::screen::capture()?;
    let small = image::imageops::thumbnail(&img, 1280.min(img.width()), (img.height() as u64 * 1280.min(img.width()) as u64 / img.width().max(1) as u64) as u32);
    let rgb = image::DynamicImage::ImageRgba8(small).to_rgb8();
    let mut jpg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 70).encode_image(&rgb).map_err(|e| e.to_string())?;
    let sum = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        rgb.as_raw().iter().step_by(97).collect::<Vec<_>>().hash(&mut h);
        h.finish()
    };
    if sum == *last {
        return Ok(());
    }
    *last = sum;
    let at = chrono::Utc::now().timestamp();
    std::fs::create_dir_all(dir(state)).map_err(|e| e.to_string())?;
    std::fs::write(dir(state).join(format!("{at}.bin")), c.seal_bytes(&jpg)).map_err(|e| e.to_string())?;
    let shot = Shot { at, exe, title: title.chars().take(300).collect() };
    let conn = state.db.lock().unwrap();
    table(&conn)?;
    conn.execute("INSERT OR REPLACE INTO timeline (at, data) VALUES (?1, ?2)", rusqlite::params![at, c.encrypt(&serde_json::to_string(&shot).map_err(|e| e.to_string())?)]).map_err(|e| e.to_string())?;
    drop(conn);
    prune(state, s.keep_days, at);
    Ok(())
}

/// The capture loop. It wakes every few seconds and does nothing until
/// the switch is on.
pub fn start(state: std::sync::Arc<crate::AppState>) {
    std::thread::spawn(move || {
        let mut last = 0u64;
        let mut next = 0i64;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let now = chrono::Utc::now().timestamp();
            if now < next {
                continue;
            }
            let every: Settings = db::get(&state.db.lock().unwrap(), KEY).unwrap_or_default();
            next = now + every.clamped().every_secs as i64;
            let _ = take(&state, &mut last);
        }
    });
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
    fn private_windows_are_spotted() {
        assert!(private_title("New tab - InPrivate - Microsoft Edge"));
        assert!(private_title("New Incognito tab - Google Chrome"));
        assert!(!private_title("Inbox - Mail"));
    }

    #[test]
    fn shots_stored_encrypted() {
        let c = conn();
        let ci = crate::crypto::Cipher::for_test();
        table(&c).unwrap();
        let s = Shot { at: 100, exe: "code.exe".into(), title: "plan.md - Visual Studio Code".into() };
        c.execute("INSERT INTO timeline (at, data) VALUES (?1, ?2)", rusqlite::params![100, ci.encrypt(&serde_json::to_string(&s).unwrap())]).unwrap();
        assert_eq!(shots(&c, &ci, 0, 200), vec![s.clone()]);
        assert!(shots(&c, &ci, 101, 200).is_empty());
        assert_eq!(search(&c, &ci, "PLAN visual").len(), 1);
        assert!(search(&c, &ci, "mail").is_empty());
    }

    #[test]
    fn old_captures_expire() {
        assert_eq!(expired(&[0, 86_400 * 9, 86_400 * 10], 86_400 * 11, 2), vec![0]);
    }
}
