// SPDX-License-Identifier: AGPL-3.0-only
//! Files dropped on a chat from Explorer. The window can read a dropped file
//! but not where it is, so it's written here in pieces and then sent from
//! this copy like any picked file. Copies older than a day are cleared.

use std::io::Write;
use std::path::PathBuf;

use crate::AppStateRef;

fn dir(state: &crate::AppState) -> PathBuf {
    state.paths.data.join("drops")
}

/// Appends one piece (base64) of dropped file `id` and returns where it is.
/// `first` starts it, and clears copies from earlier days.
#[tauri::command]
pub fn drop_piece(state: AppStateRef, id: String, name: String, data: String, first: bool) -> Result<String, String> {
    use base64::Engine;
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("That drop isn't valid.".into());
    }
    let clean: String = name.chars().filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') && !c.is_control()).take(150).collect();
    let clean = if clean.trim_matches('.').is_empty() { "file".to_string() } else { clean };
    let folder = dir(&state).join(&id);
    if first {
        clear_old(&dir(&state));
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    }
    let path = folder.join(clean);
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| "A piece of the file was damaged.".to_string())?;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path).map_err(|e| e.to_string())?;
    f.write_all(&bytes).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

fn clear_old(root: &std::path::Path) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).map(|t| t.elapsed().map(|d| d.as_secs() > 86_400).unwrap_or(false)).unwrap_or(false);
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}
