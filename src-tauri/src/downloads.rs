// SPDX-License-Identifier: AGPL-3.0-only
//! The download manager for chat models: pausing stops a download but keeps
//! its partial file, so resuming continues from there (a range request);
//! cancelling deletes it. Paused downloads are remembered across restarts.

use std::collections::BTreeSet;
use std::sync::Mutex;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::catalog::ModelSpec;
use crate::db;
use crate::paths::Paths;

const KEY: &str = "paused_installs";

/// A download that was paused.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Paused {
    pub model_id: String,
    pub quant: String,
    pub name: String,
    /// Bytes downloaded so far, and the whole size (the step that was running).
    pub received: u64,
    pub total: u64,
    pub at: i64,
}

pub fn list(conn: &Connection) -> Vec<Paused> {
    db::get(conn, KEY).unwrap_or_default()
}

pub fn remember(conn: &Connection, p: Paused) -> Result<(), String> {
    let mut all = list(conn);
    all.retain(|x| x.model_id != p.model_id);
    all.push(p);
    db::set(conn, KEY, &all)
}

pub fn forget(conn: &Connection, model_id: &str) -> Result<(), String> {
    let mut all = list(conn);
    let before = all.len();
    all.retain(|x| x.model_id != model_id);
    if all.len() != before {
        db::set(conn, KEY, &all)?;
    }
    Ok(())
}

/// Downloads asked to pause (rather than cancel) and not stopped yet.
static PAUSING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

pub fn mark_pausing(model_id: &str) {
    PAUSING.lock().unwrap().insert(model_id.to_string());
}

/// Whether the download that just stopped was paused (and forgets it).
pub fn take_pausing(model_id: &str) -> bool {
    PAUSING.lock().unwrap().remove(model_id)
}

/// Deletes the partial files a model's download left (the model and its
/// picture reader).
pub fn delete_parts(paths: &Paths, spec: &ModelSpec) {
    let dir = paths.models.join(&spec.id);
    let files = spec.variants.iter().map(|v| v.file.as_str()).chain(spec.vision.as_ref().map(|v| v.file.as_str()));
    for f in files {
        std::fs::remove_file(crate::download::part_path(&dir.join(f))).ok();
    }
    // An empty folder left behind.
    std::fs::remove_dir(&dir).ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paused_downloads_are_remembered_once_each() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_for_test(&conn);
        let p = |id: &str, received: u64| Paused { model_id: id.into(), quant: "Q4_K_M".into(), name: id.into(), received, total: 100, at: 0 };
        remember(&conn, p("a", 10)).unwrap();
        remember(&conn, p("b", 20)).unwrap();
        remember(&conn, p("a", 40)).unwrap();
        let all = list(&conn);
        assert_eq!(all.len(), 2);
        assert_eq!(all.iter().find(|x| x.model_id == "a").unwrap().received, 40);
        forget(&conn, "a").unwrap();
        assert_eq!(list(&conn).len(), 1);
        mark_pausing("b");
        assert!(take_pausing("b") && !take_pausing("b"), "a pause is taken once");
    }
}
