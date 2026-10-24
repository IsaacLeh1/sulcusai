// SPDX-License-Identifier: AGPL-3.0-only
//! Big files and folders in People. A file over the 16 MB a message carries,
//! or a folder (as a zip), goes in parts of 512 KB over the live link: the
//! message says what's coming, the other app asks for the parts (from where
//! it got to, so a dropped link or a restart carries on), and confirms each
//! one, with at most a few on their way at a time. The whole is checked
//! against its SHA-256 at the end.
//!
//! - Both sides keep the file sealed with their data key, a part at a time
//!   (`MAGIC`, then each part's length and sealed bytes), so a file of many
//!   gigabytes never sits in memory.
//! - Only the live link carries parts (not the relay's mailbox); the message
//!   itself can go by mail, and the parts follow when they're connected.
//! - An app too old to take parts never gets such a message: it waits.
//! - A folder is saved by unpacking it where the person picks; names that
//!   would land outside it are skipped.

use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;

use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

use super::link::{self, Frame};
use super::*;

/// The plain bytes in each part.
pub const PART: usize = 512 * 1024;
/// The biggest file or folder sent this way.
pub const MAX_BIG: u64 = 50 << 30;
/// Parts on their way, not yet confirmed.
const WINDOW: u64 = 8;
const MAGIC: &[u8; 8] = b"SULPART1";

/// People whose app takes files in parts (it said so on connecting).
static CAPABLE: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

pub(super) fn set_capable(contact: &str, yes: bool) {
    let mut c = CAPABLE.lock().unwrap();
    if yes {
        c.insert(contact.to_string());
    } else {
        c.remove(contact);
    }
}

pub(super) fn capable(contact: &str) -> bool {
    CAPABLE.lock().unwrap().contains(contact)
}

/// A file going out: how far the other side has confirmed.
struct Out {
    acked: AtomicU64,
    size: u64,
    moved: Notify,
}

static OUT: LazyLock<Mutex<HashMap<(String, String), Arc<Out>>>> = LazyLock::new(Default::default);

/// How far a file going out has got (while it's being sent).
pub(super) fn sending(contact: &str, id: &str) -> Option<u64> {
    OUT.lock().unwrap().get(&(contact.to_string(), id.to_string())).map(|o| o.acked.load(Ordering::Relaxed).min(o.size))
}

// ---------- the sealed-in-parts file ----------

/// Writes plain bytes as a file sealed in parts, hashing them as they go.
pub(super) struct PartWriter<'a> {
    file: File,
    c: &'a Cipher,
    buf: Vec<u8>,
    hash: Sha256,
    pub size: u64,
}

impl<'a> PartWriter<'a> {
    pub fn create(path: &Path, c: &'a Cipher) -> Result<Self, String> {
        let mut file = File::create(path).map_err(err)?;
        file.write_all(MAGIC).map_err(err)?;
        Ok(PartWriter { file, c, buf: Vec::with_capacity(PART), hash: Sha256::new(), size: 0 })
    }

    /// Appends one sealed part as it is (the receiving side: one part per message).
    fn record(&mut self, plain: &[u8]) -> std::io::Result<()> {
        let sealed = self.c.seal_bytes(plain);
        self.file.write_all(&(sealed.len() as u32).to_le_bytes())?;
        self.file.write_all(&sealed)
    }

    /// The rest, and the SHA-256 of everything written (hex).
    pub fn finish(mut self) -> Result<(u64, String), String> {
        if !self.buf.is_empty() {
            let rest = std::mem::take(&mut self.buf);
            self.record(&rest).map_err(err)?;
        }
        self.file.flush().map_err(err)?;
        Ok((self.size, hex::encode(self.hash.finalize())))
    }
}

impl Write for PartWriter<'_> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.hash.update(data);
        self.size += data.len() as u64;
        let mut rest = data;
        while !rest.is_empty() {
            let take = (PART - self.buf.len()).min(rest.len());
            self.buf.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.buf.len() == PART {
                let full = std::mem::replace(&mut self.buf, Vec::with_capacity(PART));
                self.record(&full)?;
            }
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// Whether a stored file is sealed in parts (else it's one sealed blob).
pub(super) fn in_parts(path: &Path) -> bool {
    let mut head = [0u8; 8];
    File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && &head == MAGIC
}

/// Reads a file sealed in parts from part `first` on: each part, opened.
fn parts(path: &Path, c: &Cipher, first: u64, mut each: impl FnMut(Vec<u8>) -> Result<bool, String>) -> Result<(), String> {
    let mut f = BufReader::new(File::open(path).map_err(|_| "That file isn't on this PC anymore.".to_string())?);
    let mut head = [0u8; 8];
    f.read_exact(&mut head).map_err(err)?;
    if &head != MAGIC {
        return Err("That file isn't stored in parts.".into());
    }
    let mut n = 0u64;
    loop {
        let mut len = [0u8; 4];
        match f.read_exact(&mut len) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(err(e)),
        }
        let len = u32::from_le_bytes(len) as usize;
        if len > PART + 1024 {
            return Err("That file is damaged.".into());
        }
        if n < first {
            f.seek_relative(len as i64).map_err(err)?;
        } else {
            let mut sealed = vec![0u8; len];
            f.read_exact(&mut sealed).map_err(err)?;
            if !each(c.open_bytes(&sealed)?)? {
                return Ok(());
            }
        }
        n += 1;
    }
}

/// Seals a file on this PC into `dst`, in parts. Returns its size and SHA-256.
pub(super) fn seal_file(c: &Cipher, src: &Path, dst: &Path) -> Result<(u64, String), String> {
    let mut from = BufReader::with_capacity(PART, File::open(src).map_err(|_| "That file couldn't be read.".to_string())?);
    let mut w = PartWriter::create(dst, c)?;
    std::io::copy(&mut from, &mut w).map_err(|_| "That file couldn't be read.".to_string())?;
    w.finish()
}

/// Zips a folder into `dst`, sealed in parts. Returns the size, SHA-256 and
/// how many files went in. The zip is made beside it first (a zip's
/// directory is written at its end), then sealed, then removed.
pub(super) fn seal_folder(c: &Cipher, src: &Path, dst: &Path) -> Result<(u64, String, usize), String> {
    let temp = dst.with_extension("zip-making");
    let made = zip_folder(src, &temp);
    let sealed = made.and_then(|files| seal_file(c, &temp, dst).map(|(size, sha)| (size, sha, files)));
    let _ = std::fs::remove_file(&temp);
    sealed
}

fn zip_folder(src: &Path, to: &Path) -> Result<usize, String> {
    use zip::write::SimpleFileOptions;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(File::create(to).map_err(err)?));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(true);
    let mut files = 0;
    let mut total = 0u64;
    let mut stack = vec![src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(&dir).map_err(|_| "That folder couldn't be read.".to_string())?.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let Ok(kind) = e.file_type() else { continue };
            // Links are left out (they could lead anywhere, or round in circles).
            if kind.is_symlink() {
                continue;
            }
            let path = e.path();
            let rel = path.strip_prefix(src).map_err(err)?.to_string_lossy().replace('\\', "/");
            if kind.is_dir() {
                zip.add_directory(format!("{rel}/"), options).map_err(err)?;
                stack.push(path);
            } else if kind.is_file() {
                zip.start_file(rel, options).map_err(err)?;
                total += e.metadata().map(|m| m.len()).unwrap_or(0);
                if total > MAX_BIG {
                    return Err(format!("That folder is over {} GB. Send a smaller one.", MAX_BIG >> 30));
                }
                let mut f = File::open(&path).map_err(|_| format!("{} couldn't be read.", path.display()))?;
                std::io::copy(&mut f, &mut zip).map_err(err)?;
                files += 1;
            }
        }
    }
    zip.finish().map_err(err)?.flush().map_err(err)?;
    Ok(files)
}

/// Saves a file stored here where the person picked: a file as it is, a
/// folder unpacked into `to` (a folder).
pub(super) fn save(c: &Cipher, stored: &Path, to: &Path, folder: bool) -> Result<(), String> {
    if !folder {
        let mut out = File::create(to).map_err(|e| format!("Couldn't save it there ({e})."))?;
        return parts(stored, c, 0, |p| out.write_all(&p).map(|_| true).map_err(|e| format!("Couldn't save it there ({e}).")));
    }
    std::fs::create_dir_all(to).map_err(|e| format!("Couldn't make that folder ({e})."))?;
    // Unpacking reads the zip's directory from its end: it's opened into a
    // file beside the folder first, then removed.
    let temp = to.with_extension("sulcus-zip");
    {
        let mut out = File::create(&temp).map_err(|e| format!("Couldn't save it there ({e})."))?;
        parts(stored, c, 0, |p| out.write_all(&p).map(|_| true).map_err(err))?;
    }
    let result = (|| {
        let mut zip = zip::ZipArchive::new(File::open(&temp).map_err(err)?).map_err(|_| "That folder's zip is damaged.".to_string())?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(err)?;
            // Never outside the folder picked.
            let Some(rel) = entry.enclosed_name() else { continue };
            let path = to.join(rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&path).map_err(err)?;
            } else {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(err)?;
                }
                let mut out = File::create(&path).map_err(|e| format!("Couldn't write {} ({e}).", path.display()))?;
                std::io::copy(&mut entry, &mut out).map_err(err)?;
            }
        }
        Ok::<_, String>(())
    })();
    let _ = std::fs::remove_file(&temp);
    result
}

// ---------- sending ----------

/// They asked for a file's parts from `from` on: they go, a few at a time.
pub(super) fn want(state: &Arc<AppState>, contact: &str, id: &str, from: u64) {
    if !safe_id(id) {
        return;
    }
    let known: Option<i64> = state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT file_size FROM people_messages WHERE contact_id = ?1 AND id = ?2 AND from_me = 1 AND file_parts = 1 AND deleted = 0", params![contact, id], |r| r.get(0))
        .optional()
        .ok()
        .flatten();
    let path = files_dir(state, contact).join(id);
    let Some(size) = known.filter(|_| in_parts(&path)) else {
        link::send(contact, Frame::NoFile { id: id.into() });
        return;
    };
    let key = (contact.to_string(), id.to_string());
    let out = {
        let mut all = OUT.lock().unwrap();
        if let Some(o) = all.get(&key) {
            // Already going: from where they are now.
            o.acked.store(from, Ordering::Relaxed);
            o.moved.notify_one();
            return;
        }
        let o = Arc::new(Out { acked: AtomicU64::new(from), size: size.max(0) as u64, moved: Notify::new() });
        all.insert(key.clone(), o.clone());
        o
    };
    let (state, contact, id) = (state.clone(), contact.to_string(), id.to_string());
    tauri::async_runtime::spawn(async move {
        let c = match state.work_cipher() {
            Ok(c) => c,
            Err(_) => {
                OUT.lock().unwrap().remove(&key);
                return;
            }
        };
        let first = from / PART as u64;
        let mut at = first * PART as u64;
        // Parts are read on a thread of their own and handed over one at a time.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(2);
        let reader = {
            let path = path.clone();
            std::thread::spawn(move || {
                let _ = parts(&path, &c, first, |p| Ok(tx.blocking_send(p).is_ok()));
            })
        };
        let mut quiet = 0;
        while let Some(part) = rx.recv().await {
            // No more than WINDOW parts ahead of what they've confirmed.
            while at.saturating_sub(out.acked.load(Ordering::Relaxed)) >= WINDOW * PART as u64 {
                if tokio::time::timeout(Duration::from_secs(30), out.moved.notified()).await.is_err() {
                    quiet += 1;
                    if quiet >= 4 || !link::connected(&contact) {
                        // They'll ask again from where they got to.
                        drop(rx);
                        let _ = reader.join();
                        OUT.lock().unwrap().remove(&key);
                        return;
                    }
                } else {
                    quiet = 0;
                }
            }
            let len = part.len() as u64;
            if !link::send(&contact, Frame::Part { id: id.clone(), at, data: B64.encode(&part) }) {
                break;
            }
            at += len;
        }
        // Everything sent: wait (a while) for the last confirmations.
        for _ in 0..20 {
            if out.acked.load(Ordering::Relaxed) >= out.size {
                break;
            }
            let _ = tokio::time::timeout(Duration::from_secs(15), out.moved.notified()).await;
        }
        let _ = reader.join();
        OUT.lock().unwrap().remove(&key);
        emit(&state, "people:file", json!({ "contact_id": contact, "id": id, "got": out.acked.load(Ordering::Relaxed).min(out.size), "size": out.size }));
    });
}

/// They confirmed parts up to `upto`.
pub(super) fn have(state: &AppState, contact: &str, id: &str, upto: u64) {
    let o = OUT.lock().unwrap().get(&(contact.to_string(), id.to_string())).cloned();
    if let Some(o) = o {
        let before = o.acked.fetch_max(upto, Ordering::Relaxed);
        o.moved.notify_one();
        // Every 4 MB (and the end), the window hears how far it is.
        if upto / (4 << 20) != before / (4 << 20) || upto >= o.size {
            emit(state, "people:file", json!({ "contact_id": contact, "id": id, "got": upto.min(o.size), "size": o.size }));
        }
    }
}

// ---------- receiving ----------

/// A message with a file in parts arrived: its stored file starts empty.
pub(super) fn begin(state: &AppState, c: &Cipher, contact: &str, id: &str) -> Result<(), String> {
    let dir = files_dir(state, contact);
    std::fs::create_dir_all(&dir).map_err(err)?;
    PartWriter::create(&dir.join(id), c).map(|_| ())
}

/// Asks for every file from them that hasn't all arrived (on connecting).
pub(super) fn ask_for_missing(state: &AppState, contact: &str) {
    let rows: Vec<(String, i64)> = {
        let conn = state.db.lock().unwrap();
        let Ok(mut stmt) = conn.prepare("SELECT id, file_got FROM people_messages WHERE contact_id = ?1 AND from_me = 0 AND file_parts = 1 AND file_got >= 0 AND deleted = 0") else { return };
        stmt.query_map([contact], |r| Ok((r.get(0)?, r.get(1)?))).map(|it| it.filter_map(Result::ok).collect()).unwrap_or_default()
    };
    for (id, got) in rows {
        link::send(contact, Frame::Want { id, from: got.max(0) as u64 });
    }
}

/// A part of a file they're sending.
pub(super) fn part(state: &AppState, contact: &str, id: &str, at: u64, data: &str) {
    let Ok(c) = state.work_cipher() else { return };
    if !safe_id(id) {
        return;
    }
    let row: Option<(i64, i64, Option<String>)> = state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT file_got, file_size, file_sha FROM people_messages WHERE contact_id = ?1 AND id = ?2 AND from_me = 0 AND file_parts = 1 AND deleted = 0", params![contact, id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()
        .ok()
        .flatten();
    // Done already, or failed, or not asked for.
    let Some((got, size, sha)) = row.filter(|r| r.0 >= 0) else { return };
    let (got, size) = (got as u64, size.max(0) as u64);
    if at != got {
        // Out of step (a part was lost): from where this side got to.
        if at > got {
            link::send(contact, Frame::Want { id: id.into(), from: got });
        }
        return;
    }
    let Ok(plain) = B64.decode(data.as_bytes()) else { return };
    if plain.is_empty() || plain.len() > PART || got + plain.len() as u64 > size {
        return;
    }
    let path = files_dir(state, contact).join(id);
    let stored = (|| {
        let file = std::fs::OpenOptions::new().append(true).open(&path)?;
        let mut w = PartWriter { file, c: &c, buf: Vec::new(), hash: Sha256::new(), size: 0 };
        w.record(&plain)?;
        w.file.flush()
    })();
    if stored.is_err() {
        return;
    }
    let now = got + plain.len() as u64;
    let done = now >= size;
    let _ = state.db.lock().unwrap().execute("UPDATE people_messages SET file_got = ?3 WHERE contact_id = ?1 AND id = ?2", params![contact, id, now as i64]);
    link::send(contact, Frame::Have { id: id.into(), upto: now });
    if now / (4 << 20) != got / (4 << 20) || done {
        emit(state, "people:file", json!({ "contact_id": contact, "id": id, "got": now, "size": size }));
    }
    if done {
        finish(state, &c, contact, id, &path, sha.as_deref());
    }
}

/// All of it arrived: it's checked against its SHA-256.
fn finish(state: &AppState, c: &Cipher, contact: &str, id: &str, path: &Path, sha: Option<&str>) {
    let mut hash = Sha256::new();
    let read = parts(path, c, 0, |p| {
        hash.update(&p);
        Ok(true)
    });
    let ok = read.is_ok() && sha.is_none_or(|s| s.eq_ignore_ascii_case(&hex::encode(hash.finalize())));
    let conn = state.db.lock().unwrap();
    if ok {
        let _ = conn.execute("UPDATE people_messages SET file_got = NULL WHERE contact_id = ?1 AND id = ?2", params![contact, id]);
    } else {
        let _ = conn.execute("UPDATE people_messages SET file_got = -1 WHERE contact_id = ?1 AND id = ?2", params![contact, id]);
        let _ = std::fs::remove_file(path);
    }
    drop(conn);
    emit(state, "people:message", json!({ "contact_id": contact }));
}

/// They don't have the file anymore: it can't arrive.
pub(super) fn gone(state: &AppState, contact: &str, id: &str) {
    let _ = state.db.lock().unwrap().execute(
        "UPDATE people_messages SET file_got = -1 WHERE contact_id = ?1 AND id = ?2 AND from_me = 0 AND file_parts = 1 AND file_got IS NOT NULL",
        params![contact, id],
    );
    let _ = std::fs::remove_file(files_dir(state, contact).join(id));
    emit(state, "people:message", json!({ "contact_id": contact }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_and_folders_go_in_sealed_parts_and_come_back_whole() {
        let dir = std::env::temp_dir().join(format!("sulcus-parts-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(dir.join("trip/photos")).unwrap();
        let c = Cipher::for_test();
        // A file of a little over two parts.
        let big: Vec<u8> = (0..(PART * 2 + 1234)).map(|i| (i % 251) as u8).collect();
        std::fs::write(dir.join("big.bin"), &big).unwrap();
        let stored = dir.join("stored");
        let (size, sha) = seal_file(&c, &dir.join("big.bin"), &stored).unwrap();
        assert_eq!(size, big.len() as u64);
        assert_eq!(sha, hex::encode(Sha256::digest(&big)));
        assert!(in_parts(&stored));
        let raw = std::fs::read(&stored).unwrap();
        assert!(!raw.windows(64).any(|w| w == &big[1000..1064]), "sealed, not plain");
        // From the second part on (a resumed transfer), and all of it.
        let mut from_second = Vec::new();
        parts(&stored, &c, 1, |p| {
            from_second.extend(p);
            Ok(true)
        })
        .unwrap();
        assert_eq!(from_second, big[PART..]);
        save(&c, &stored, &dir.join("back.bin"), false).unwrap();
        assert_eq!(std::fs::read(dir.join("back.bin")).unwrap(), big);

        // A folder: zipped, sealed, and unpacked again where it's saved.
        std::fs::write(dir.join("trip/plan.txt"), "Day 1: Zion").unwrap();
        std::fs::write(dir.join("trip/photos/a.jpg"), &big[..5000]).unwrap();
        let (_, _, files) = seal_folder(&c, &dir.join("trip"), &dir.join("trip-stored")).unwrap();
        assert_eq!(files, 2);
        save(&c, &dir.join("trip-stored"), &dir.join("got/trip"), true).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("got/trip/plan.txt")).unwrap(), "Day 1: Zion");
        assert_eq!(std::fs::read(dir.join("got/trip/photos/a.jpg")).unwrap(), big[..5000]);
        assert!(!dir.join("got/trip.sulcus-zip").exists(), "the zip goes once unpacked");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
