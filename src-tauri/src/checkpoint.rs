// SPDX-License-Identifier: AGPL-3.0-only
//! Undo for the agent's file changes. Before a tool changes a file, the
//! original is backed up (encrypted) and the change recorded against the
//! turn, so "Undo" can put everything back. Deleting uses the Recycle Bin.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::Connection;

use crate::crypto::Cipher;
use crate::db;

/// Files bigger than this aren't backed up (deletes still go to the Recycle Bin).
const MAX_BACKUP: u64 = 50 * 1024 * 1024;
/// Backups older than this are removed at startup.
pub const KEEP_DAYS: i64 = 30;

pub struct Recorder<'a> {
    pub db: &'a Mutex<Connection>,
    pub cipher: &'a Cipher,
    pub dir: PathBuf,
    pub chat_id: &'a str,
    pub turn_id: &'a str,
}

impl Recorder<'_> {
    fn backup(&self, path: &Path) -> Result<Option<String>, String> {
        let meta = match std::fs::metadata(path) {
            Ok(m) if m.is_file() => m,
            _ => return Ok(None),
        };
        if meta.len() > MAX_BACKUP {
            return Ok(None);
        }
        let bytes = std::fs::read(path).map_err(|e| format!("Couldn't back up {}: {e}", path.display()))?;
        let dir = self.dir.join(self.turn_id);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let file = dir.join(format!("{}.bak", uuid::Uuid::new_v4().simple()));
        std::fs::write(&file, self.cipher.seal_bytes(&bytes)).map_err(|e| e.to_string())?;
        Ok(Some(file.display().to_string()))
    }

    fn record(&self, action: &str, path: &Path, other: Option<&Path>, backup: Option<String>) -> Result<(), String> {
        let conn = self.db.lock().unwrap();
        db::add_checkpoint(
            &conn,
            self.cipher,
            self.chat_id,
            self.turn_id,
            action,
            &path.display().to_string(),
            other.map(|o| o.display().to_string()).as_deref(),
            backup.as_deref(),
        )
    }

    /// Call before writing to `path`.
    pub fn before_write(&self, path: &Path) -> Result<(), String> {
        if path.exists() {
            let backup = self.backup(path)?;
            self.record("modify", path, None, backup)
        } else {
            self.record("create", path, None, None)
        }
    }

    pub fn before_delete(&self, path: &Path) -> Result<(), String> {
        let backup = if path.is_file() { self.backup(path)? } else { None };
        self.record("delete", path, None, backup)
    }

    pub fn after_move(&self, from: &Path, to: &Path) -> Result<(), String> {
        self.record("move", from, Some(to), None)
    }

    pub fn after_mkdir(&self, path: &Path) -> Result<(), String> {
        self.record("mkdir", path, None, None)
    }
}

fn restore(cipher: &Cipher, backup: &str, to: &Path) -> Result<(), String> {
    let blob = std::fs::read(backup).map_err(|e| format!("The backup is missing: {e}"))?;
    let bytes = cipher.open_bytes(&blob)?;
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(to, bytes).map_err(|e| e.to_string())
}

/// Reverses every recorded change in a turn, newest first. Returns notes
/// for anything that couldn't be fully undone.
pub fn undo_turn(conn: &Connection, cipher: &Cipher, turn_id: &str) -> Vec<String> {
    let mut notes = Vec::new();
    for cp in db::turn_checkpoints(conn, cipher, turn_id) {
        let path = PathBuf::from(&cp.path);
        let result: Result<(), String> = match cp.action.as_str() {
            "create" => {
                if path.exists() {
                    recycle(&path)
                } else {
                    Ok(())
                }
            }
            "modify" => match &cp.backup {
                Some(b) => restore(cipher, b, &path),
                None => Err(format!("{} was too large to back up, so it wasn't restored.", cp.path)),
            },
            "delete" => match &cp.backup {
                Some(b) => restore(cipher, b, &path),
                None => Err(format!("{} wasn't backed up; you can restore it from the Recycle Bin.", cp.path)),
            },
            "move" => match cp.other.as_deref().map(PathBuf::from) {
                Some(to) if to.exists() && !path.exists() => std::fs::rename(&to, &path).map_err(|e| e.to_string()),
                Some(_) => Err(format!("{} couldn't be moved back because something has changed since.", cp.path)),
                None => Ok(()),
            },
            "mkdir" => {
                // Only removes the folder if it's empty again.
                let _ = std::fs::remove_dir(&path);
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            notes.push(e);
        }
        let _ = db::mark_undone(conn, cp.id);
    }
    notes
}

/// Removes backups past the retention period.
pub fn prune(conn: &Connection) {
    for (id, backup) in db::old_checkpoints(conn, KEEP_DAYS) {
        if let Some(b) = backup {
            let _ = std::fs::remove_file(b);
        }
        let _ = db::delete_checkpoint(conn, id);
    }
}

/// Moves a file or folder to the Recycle Bin, so deletes can be recovered.
#[cfg(windows)]
pub fn recycle(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW,
    };

    // pFrom is a list ending in two NULs.
    let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
    from.extend([0, 0]);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT).0 as u16,
        ..Default::default()
    };
    // SAFETY: `from` outlives the call; no window handle, so no UI is shown.
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 || op.fAnyOperationsAborted.as_bool() {
        return Err(format!("Couldn't move {} to the Recycle Bin (code {code}).", path.display()));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn recycle(path: &Path) -> Result<(), String> {
    Err(format!("Moving {} to the trash isn't supported on this system yet.", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Protector, Vault};

    struct Plain;
    impl Protector for Plain {
        fn protect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
            Ok(d.to_vec())
        }
        fn unprotect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
            Ok(d.to_vec())
        }
    }

    fn setup() -> (PathBuf, Mutex<Connection>, Cipher) {
        let d = std::env::temp_dir().join(format!("sulcusai-cp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(d.join("work")).unwrap();
        let conn = db::open(&d.join("t.db")).unwrap();
        let cipher = Vault::open(&d.join("keys.json"), Box::new(Plain)).unwrap().cipher().unwrap();
        (d, Mutex::new(conn), cipher)
    }

    #[test]
    fn undo_restores_modified_created_moved_and_deleted_files() {
        let (d, conn, cipher) = setup();
        let w = d.join("work");
        std::fs::write(w.join("keep.txt"), "original").unwrap();
        std::fs::write(w.join("gone.txt"), "precious").unwrap();
        std::fs::write(w.join("old-name.txt"), "moved").unwrap();
        let rec = Recorder { db: &conn, cipher: &cipher, dir: d.join("checkpoints"), chat_id: "c", turn_id: "t1" };

        rec.before_write(&w.join("keep.txt")).unwrap();
        std::fs::write(w.join("keep.txt"), "changed").unwrap();
        rec.before_write(&w.join("new.txt")).unwrap();
        std::fs::write(w.join("new.txt"), "fresh").unwrap();
        std::fs::rename(w.join("old-name.txt"), w.join("new-name.txt")).unwrap();
        rec.after_move(&w.join("old-name.txt"), &w.join("new-name.txt")).unwrap();
        rec.before_delete(&w.join("gone.txt")).unwrap();
        std::fs::remove_file(w.join("gone.txt")).unwrap();

        // The backup on disk is encrypted, not a plain copy.
        let backups: Vec<_> = std::fs::read_dir(d.join("checkpoints/t1")).unwrap().flatten().collect();
        assert!(backups.iter().all(|b| !std::fs::read(b.path()).unwrap().windows(8).any(|w| w == b"original")));

        let notes = undo_turn(&conn.lock().unwrap(), &cipher, "t1");
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(std::fs::read_to_string(w.join("keep.txt")).unwrap(), "original");
        assert_eq!(std::fs::read_to_string(w.join("gone.txt")).unwrap(), "precious");
        assert_eq!(std::fs::read_to_string(w.join("old-name.txt")).unwrap(), "moved");
        assert!(!w.join("new-name.txt").exists());
        if cfg!(windows) {
            assert!(!w.join("new.txt").exists(), "created file went to the Recycle Bin");
        }
        // A second undo has nothing left to do.
        assert!(db::turn_checkpoints(&conn.lock().unwrap(), &cipher, "t1").is_empty());
    }
}
