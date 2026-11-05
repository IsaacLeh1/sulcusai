// SPDX-License-Identifier: AGPL-3.0-only
//! Who may write which file when several sessions share a tree. A session
//! claims a file or folder (its scope); another session's write there is
//! refused, so two sessions never edit the same file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static CLAIMS: Mutex<Option<HashMap<PathBuf, String>>> = Mutex::new(None);

fn key(p: &Path) -> PathBuf {
    PathBuf::from(p.to_string_lossy().to_lowercase().replace('\\', "/").trim_end_matches('/'))
}

/// Claims `path` (a file, or a folder and all inside it) for `owner`.
/// Err names the session already holding it or something overlapping it.
pub fn claim(owner: &str, path: &Path) -> Result<(), String> {
    let k = key(path);
    let mut g = CLAIMS.lock().unwrap();
    let m = g.get_or_insert_with(HashMap::new);
    if let Some((held, who)) = m.iter().find(|(p, who)| who.as_str() != owner && (k.starts_with(p) || p.starts_with(&k))) {
        return Err(format!("{} is being changed by another session ({who}); pick other files, or wait for it.", held.display()));
    }
    m.insert(k, owner.to_string());
    Ok(())
}

/// Lets go of everything `owner` claimed.
pub fn release(owner: &str) {
    if let Some(m) = CLAIMS.lock().unwrap().as_mut() {
        m.retain(|_, who| who != owner);
    }
}

/// Err when another session holds `path`.
pub fn check(owner: &str, path: &Path) -> Result<(), String> {
    let k = key(path);
    let g = CLAIMS.lock().unwrap();
    match g.as_ref().and_then(|m| m.iter().find(|(p, who)| who.as_str() != owner && k.starts_with(p))) {
        Some((_, who)) => Err(format!("{} belongs to another session's work ({who}), so this session can't change it.", path.display())),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_sessions_cannot_hold_the_same_file() {
        let dir = std::env::temp_dir().join("sulcus-locks-test").join("src");
        claim("lock-a", &dir.join("a.rs")).unwrap();
        assert!(claim("lock-b", &dir.join("a.rs")).is_err());
        assert!(claim("lock-b", &dir).is_err(), "a folder over a held file");
        assert!(check("lock-b", &dir.join("a.rs")).is_err());
        assert!(check("lock-a", &dir.join("a.rs")).is_ok());
        claim("lock-b", &dir.join("b.rs")).unwrap();
        release("lock-a");
        assert!(check("lock-b", &dir.join("a.rs")).is_ok());
        release("lock-b");
    }
}
