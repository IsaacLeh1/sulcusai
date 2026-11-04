// SPDX-License-Identifier: AGPL-3.0-only
//! The Find index: file names, document text and picture descriptions in one
//! SQLite full-text table, kept in its own file so a rebuild never touches
//! chats. Only folders the person adds are read; nothing is indexed until then.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// Text beyond this is left out of the index (search still finds the file by name).
const MAX_BODY: usize = 200_000;
/// Files bigger than this are indexed by name only.
const MAX_READ_BYTES: u64 = 50 * 1024 * 1024;

const TEXT_EXTS: &[&str] = &[
    "txt", "md", "markdown", "csv", "tsv", "json", "toml", "yaml", "yml", "xml", "html", "htm", "log", "ini", "rs", "py", "js", "ts", "tsx", "jsx", "c",
    "h", "cpp", "hpp", "cs", "java", "go", "rb", "php", "sh", "ps1", "bat", "sql", "css", "lua", "kt", "swift",
];
const DOC_EXTS: &[&str] = &["pdf", "docx", "pptx", "xlsx", "xlsm", "xls", "ods"];
const PICTURE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "heic", "tif", "tiff"];
/// Folders that are never worth reading (build output, tool caches).
const SKIP_DIRS: &[&str] = &["node_modules", ".git", "target", "__pycache__", ".venv", "$RECYCLE.BIN", "System Volume Information"];

/// The root given to files known only through the Media index.
pub const MEDIA_ROOT: &str = "media-index";

pub struct FindIndex {
    db: Connection,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Hit {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub modified: i64,
    /// A few words around the match, with the match in [brackets].
    pub snippet: String,
}

#[derive(Serialize, Debug, Default, PartialEq)]
pub struct IndexReport {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub unchanged: usize,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct DupeGroup {
    pub paths: Vec<String>,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Move {
    pub from: String,
    pub to: String,
}

pub fn kind_of(ext: &str) -> &'static str {
    let e = ext.to_ascii_lowercase();
    if PICTURE_EXTS.contains(&e.as_str()) {
        "Pictures"
    } else if DOC_EXTS.contains(&e.as_str()) || matches!(e.as_str(), "txt" | "md" | "doc" | "rtf" | "odt") {
        "Documents"
    } else if matches!(e.as_str(), "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac") {
        "Music"
    } else if matches!(e.as_str(), "mp4" | "mkv" | "mov" | "avi" | "webm") {
        "Videos"
    } else if matches!(e.as_str(), "zip" | "7z" | "rar" | "tar" | "gz") {
        "Archives"
    } else if matches!(e.as_str(), "exe" | "msi") {
        "Installers"
    } else {
        "Other"
    }
}

fn ext_of(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

fn mtime(meta: &std::fs::Metadata) -> i64 {
    meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The text worth searching in a file, or empty when it has none we can read.
pub fn body_of(path: &Path, size: u64) -> String {
    if size > MAX_READ_BYTES {
        return String::new();
    }
    let ext = ext_of(path);
    let text = if TEXT_EXTS.contains(&ext.as_str()) {
        std::fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
    } else if DOC_EXTS.contains(&ext.as_str()) {
        crate::docs::read_all(path).unwrap_or_default()
    } else {
        String::new()
    };
    text.chars().take(MAX_BODY).collect()
}

/// A search box's words as an FTS5 query: each word quoted (so punctuation
/// can't be read as syntax) and matched as a prefix.
pub fn fts_query(q: &str) -> Option<String> {
    let words: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{}\"*", w.replace('"', "")))
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

impl FindIndex {
    pub fn open(path: &Path) -> rusqlite::Result<FindIndex> {
        Self::setup(Connection::open(path)?)
    }

    pub fn in_memory() -> rusqlite::Result<FindIndex> {
        Self::setup(Connection::open_in_memory()?)
    }

    fn setup(db: Connection) -> rusqlite::Result<FindIndex> {
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS roots (path TEXT PRIMARY KEY);
             CREATE TABLE IF NOT EXISTS files (
                 id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL, root TEXT NOT NULL,
                 name TEXT NOT NULL, ext TEXT NOT NULL, size INTEGER NOT NULL, modified INTEGER NOT NULL,
                 hash TEXT, caption TEXT NOT NULL DEFAULT '');
             CREATE INDEX IF NOT EXISTS files_size ON files(size);
             CREATE VIRTUAL TABLE IF NOT EXISTS files_fts USING fts5(name, body, caption, tokenize='unicode61 remove_diacritics 2');",
        )?;
        Ok(FindIndex { db })
    }

    pub fn roots(&self) -> Vec<String> {
        let mut st = self.db.prepare("SELECT path FROM roots ORDER BY path").unwrap();
        st.query_map([], |r| r.get(0)).unwrap().filter_map(Result::ok).collect()
    }

    pub fn add_root(&self, root: &Path) -> Result<IndexReport, String> {
        let root = root.canonicalize().map_err(|e| format!("Couldn't open that folder: {e}"))?;
        if !root.is_dir() {
            return Err("That isn't a folder.".into());
        }
        let r = root.display().to_string();
        self.db.execute("INSERT OR IGNORE INTO roots(path) VALUES (?1)", [&r]).map_err(|e| e.to_string())?;
        self.refresh_root(&r)
    }

    pub fn remove_root(&self, root: &str) -> Result<(), String> {
        let tx = self.db.unchecked_transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM files_fts WHERE rowid IN (SELECT id FROM files WHERE root = ?1)", [root]).map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM files WHERE root = ?1", [root]).map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM roots WHERE path = ?1", [root]).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }

    /// Brings every added folder up to date.
    pub fn refresh(&self) -> Result<IndexReport, String> {
        let mut total = IndexReport::default();
        for r in self.roots() {
            let one = self.refresh_root(&r)?;
            total.added += one.added;
            total.updated += one.updated;
            total.removed += one.removed;
            total.unchanged += one.unchanged;
        }
        Ok(total)
    }

    /// Reads only files whose size or time changed since the last pass.
    pub fn refresh_root(&self, root: &str) -> Result<IndexReport, String> {
        let mut known: HashMap<String, (i64, u64, i64)> = HashMap::new();
        {
            let mut st = self.db.prepare("SELECT path, id, size, modified FROM files WHERE root = ?1").map_err(|e| e.to_string())?;
            let rows = st.query_map([root], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get::<_, i64>(2)? as u64, r.get(3)?)))).map_err(|e| e.to_string())?;
            for row in rows.flatten() {
                known.insert(row.0, row.1);
            }
        }
        let mut report = IndexReport::default();
        let tx = self.db.unchecked_transaction().map_err(|e| e.to_string())?;
        let mut stack = vec![PathBuf::from(root)];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for entry in rd.flatten() {
                let Ok(ft) = entry.file_type() else { continue };
                let p = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if ft.is_symlink() {
                    continue;
                }
                if ft.is_dir() {
                    if !SKIP_DIRS.contains(&name.as_str()) {
                        stack.push(p);
                    }
                    continue;
                }
                let Ok(meta) = entry.metadata() else { continue };
                let (size, modified) = (meta.len(), mtime(&meta));
                let key = p.display().to_string();
                match known.remove(&key) {
                    Some((_, s, m)) if s == size && m == modified => report.unchanged += 1,
                    Some((id, _, _)) => {
                        tx.execute("UPDATE files SET size=?2, modified=?3, hash=NULL WHERE id=?1", params![id, size as i64, modified]).map_err(|e| e.to_string())?;
                        let caption: String = tx.query_row("SELECT caption FROM files WHERE id=?1", [id], |r| r.get(0)).unwrap_or_default();
                        tx.execute("DELETE FROM files_fts WHERE rowid=?1", [id]).map_err(|e| e.to_string())?;
                        tx.execute("INSERT INTO files_fts(rowid, name, body, caption) VALUES (?1, ?2, ?3, ?4)", params![id, name, body_of(&p, size), caption])
                            .map_err(|e| e.to_string())?;
                        report.updated += 1;
                    }
                    None => {
                        tx.execute(
                            "INSERT INTO files(path, root, name, ext, size, modified) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                            params![key, root, name, ext_of(&p), size as i64, modified],
                        )
                        .map_err(|e| e.to_string())?;
                        let id = tx.last_insert_rowid();
                        tx.execute("INSERT INTO files_fts(rowid, name, body, caption) VALUES (?1, ?2, ?3, '')", params![id, name, body_of(&p, size)])
                            .map_err(|e| e.to_string())?;
                        report.added += 1;
                    }
                }
            }
        }
        for (_, (id, _, _)) in known {
            tx.execute("DELETE FROM files_fts WHERE rowid=?1", [id]).map_err(|e| e.to_string())?;
            tx.execute("DELETE FROM files WHERE id=?1", [id]).map_err(|e| e.to_string())?;
            report.removed += 1;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(report)
    }

    /// Stores what a picture shows (from the vision model) so it can be found by words.
    pub fn set_caption(&self, path: &str, caption: &str) -> Result<bool, String> {
        let id: Option<i64> = self.db.query_row("SELECT id FROM files WHERE path=?1", [path], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
        let Some(id) = id else { return Ok(false) };
        self.db.execute("UPDATE files SET caption=?2 WHERE id=?1", params![id, caption]).map_err(|e| e.to_string())?;
        self.db.execute("UPDATE files_fts SET caption=?2 WHERE rowid=?1", params![id, caption]).map_err(|e| e.to_string())?;
        Ok(true)
    }

    /// What's said in a sound or video file (from the Media index), searchable
    /// like a picture's description. Files outside the Find folders get a row of
    /// their own under MEDIA_ROOT, which refreshes never walk.
    pub fn set_transcript(&self, path: &str, text: &str) -> Result<(), String> {
        let text: String = text.chars().take(MAX_BODY).collect();
        if self.set_caption(path, &text)? {
            return Ok(());
        }
        let p = Path::new(path);
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let (size, modified) = std::fs::metadata(p).map(|m| (m.len(), mtime(&m))).unwrap_or((0, 0));
        self.db
            .execute("INSERT INTO files(path, root, name, ext, size, modified, caption) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![path, MEDIA_ROOT, name, ext_of(p), size as i64, modified, text])
            .map_err(|e| e.to_string())?;
        let id = self.db.last_insert_rowid();
        self.db.execute("INSERT INTO files_fts(rowid, name, body, caption) VALUES (?1, ?2, '', ?3)", params![id, name, text]).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Takes a transcript back out: its own row goes, a Find folder's file keeps its name.
    pub fn clear_transcript(&self, path: &str) -> Result<(), String> {
        let row: Option<(i64, String)> = self.db.query_row("SELECT id, root FROM files WHERE path=?1", [path], |r| Ok((r.get(0)?, r.get(1)?))).optional().map_err(|e| e.to_string())?;
        match row {
            Some((id, root)) if root == MEDIA_ROOT => {
                self.db.execute("DELETE FROM files_fts WHERE rowid=?1", [id]).map_err(|e| e.to_string())?;
                self.db.execute("DELETE FROM files WHERE id=?1", [id]).map_err(|e| e.to_string())?;
            }
            Some(_) => {
                self.set_caption(path, "")?;
            }
            None => {}
        }
        Ok(())
    }

    /// Pictures that have no description yet, for the vision model to fill in.
    pub fn uncaptioned_pictures(&self, limit: usize) -> Vec<String> {
        let marks = PICTURE_EXTS.iter().map(|e| format!("'{e}'")).collect::<Vec<_>>().join(",");
        let sql = format!("SELECT path FROM files WHERE caption = '' AND ext IN ({marks}) ORDER BY modified DESC LIMIT ?1");
        let mut st = self.db.prepare(&sql).unwrap();
        st.query_map([limit as i64], |r| r.get(0)).unwrap().filter_map(Result::ok).collect()
    }

    /// Names, text and picture descriptions, best match first. `kind` narrows
    /// to one of the tidy groups ("Pictures", "Documents", ...).
    pub fn search(&self, q: &str, kind: Option<&str>, limit: usize) -> Vec<Hit> {
        let Some(fq) = fts_query(q) else { return vec![] };
        let mut st = self
            .db
            .prepare(
                "SELECT f.path, f.name, f.size, f.modified, f.ext,
                        snippet(files_fts, -1, '[', ']', '...', 12)
                 FROM files_fts JOIN files f ON f.id = files_fts.rowid
                 WHERE files_fts MATCH ?1
                 ORDER BY bm25(files_fts, 10.0, 1.0, 3.0) LIMIT ?2",
            )
            .unwrap();
        let rows = st
            .query_map(params![fq, (limit * 4) as i64], |r| {
                Ok((
                    Hit { path: r.get(0)?, name: r.get(1)?, size: r.get::<_, i64>(2)? as u64, modified: r.get(3)?, snippet: r.get(5)? },
                    r.get::<_, String>(4)?,
                ))
            })
            .unwrap();
        rows.filter_map(Result::ok).filter(|(_, ext)| kind.map_or(true, |k| kind_of(ext) == k)).map(|(h, _)| h).take(limit).collect()
    }

    /// Files with the same contents: same size first, then the same full hash.
    pub fn duplicates(&self, min_size: u64) -> Vec<DupeGroup> {
        let same_size: Vec<(i64, String, u64, Option<String>)> = {
            let mut st = self
                .db
                .prepare(
                    "SELECT id, path, size, hash FROM files WHERE size >= ?1 AND size IN
                     (SELECT size FROM files WHERE size >= ?1 GROUP BY size HAVING COUNT(*) > 1)",
                )
                .unwrap();
            st.query_map([min_size.max(1) as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u64, r.get(3)?))).unwrap().filter_map(Result::ok).collect()
        };
        let mut by: HashMap<(u64, String), Vec<String>> = HashMap::new();
        for (id, path, size, hash) in same_size {
            let hash = match hash {
                Some(h) => h,
                None => match full_hash(Path::new(&path)) {
                    Ok(h) => {
                        let _ = self.db.execute("UPDATE files SET hash=?2 WHERE id=?1", params![id, h]);
                        h
                    }
                    Err(_) => continue,
                },
            };
            by.entry((size, hash)).or_default().push(path);
        }
        let mut out: Vec<DupeGroup> = by
            .into_iter()
            .filter(|(_, v)| v.len() > 1)
            .map(|((size, _), mut paths)| {
                paths.sort();
                DupeGroup { paths, size }
            })
            .collect();
        out.sort_by(|a, b| (b.size * b.paths.len() as u64).cmp(&(a.size * a.paths.len() as u64)).then(a.paths.cmp(&b.paths)));
        out
    }

    pub fn count(&self) -> usize {
        self.db.query_row("SELECT COUNT(*) FROM files", [], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize
    }
}

pub fn full_hash(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

/// A tidy plan for one folder: loose files moved into a subfolder per kind.
/// Only a plan; nothing moves until `apply_tidy` runs it after the person agrees.
pub fn tidy_plan(folder: &Path) -> Result<Vec<Move>, String> {
    let mut moves = vec![];
    for entry in std::fs::read_dir(folder).map_err(|e| e.to_string())?.flatten() {
        let p = entry.path();
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.eq_ignore_ascii_case("desktop.ini") {
            continue;
        }
        let kind = kind_of(&ext_of(&p));
        let mut to = folder.join(kind).join(&name);
        let mut n = 2;
        while to.exists() {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
            let ext = ext_of(&p);
            to = folder.join(kind).join(if ext.is_empty() { format!("{stem} ({n})") } else { format!("{stem} ({n}).{ext}") });
            n += 1;
        }
        moves.push(Move { from: p.display().to_string(), to: to.display().to_string() });
    }
    moves.sort_by(|a, b| a.from.cmp(&b.from));
    Ok(moves)
}

/// Runs an agreed plan. A move whose source changed or whose target appeared
/// since the plan was made is skipped, never overwritten.
pub fn apply_tidy(moves: &[Move]) -> (usize, Vec<String>) {
    let mut done = 0;
    let mut skipped = vec![];
    for m in moves {
        let (from, to) = (Path::new(&m.from), Path::new(&m.to));
        if !from.is_file() || to.exists() {
            skipped.push(m.from.clone());
            continue;
        }
        if let Some(parent) = to.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::rename(from, to) {
            Ok(()) => done += 1,
            Err(_) => skipped.push(m.from.clone()),
        }
    }
    (done, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let d = std::env::temp_dir().join(format!("sulcusai-find-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn finds_by_name_and_contents_and_follows_changes() {
        let d = scratch();
        std::fs::write(d.join("taxes 2025.txt"), "refund from the county assessor").unwrap();
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("sub").join("notes.md"), "grocery list: apples").unwrap();
        std::fs::create_dir_all(d.join("node_modules")).unwrap();
        std::fs::write(d.join("node_modules").join("assessor.txt"), "assessor").unwrap();
        let idx = FindIndex::in_memory().unwrap();
        let r = idx.add_root(&d).unwrap();
        assert_eq!(r.added, 2, "skipped folders stay out");
        assert_eq!(idx.search("taxes", None, 10).len(), 1);
        let hits = idx.search("assess", None, 10);
        assert_eq!(hits.len(), 1, "prefix match on contents");
        assert!(hits[0].snippet.contains("[assessor]"));
        assert_eq!(idx.search("apples", Some("Pictures"), 10).len(), 0);
        assert_eq!(idx.search("apples", Some("Documents"), 10).len(), 1);
        assert!(idx.search("\"bad) OR (", None, 10).is_empty(), "odd input never errors");

        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(d.join("sub").join("notes.md"), "grocery list: pears and more").unwrap();
        std::fs::remove_file(d.join("taxes 2025.txt")).unwrap();
        let r = idx.refresh().unwrap();
        assert_eq!((r.added, r.updated, r.removed), (0, 1, 1));
        assert!(idx.search("apples", None, 10).is_empty());
        assert_eq!(idx.search("pears", None, 10).len(), 1);
        assert_eq!(idx.refresh().unwrap().unchanged, 1);

        idx.remove_root(&idx.roots()[0]).unwrap();
        assert_eq!(idx.count(), 0);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn pictures_are_found_by_their_description() {
        let d = scratch();
        std::fs::write(d.join("IMG_0042.jpg"), [0xFFu8, 0xD8, 0xFF]).unwrap();
        let idx = FindIndex::in_memory().unwrap();
        idx.add_root(&d).unwrap();
        let pics = idx.uncaptioned_pictures(10);
        assert_eq!(pics.len(), 1);
        assert!(idx.set_caption(&pics[0], "a red bicycle by a lake").unwrap());
        assert!(idx.uncaptioned_pictures(10).is_empty());
        assert_eq!(idx.search("bicycle", Some("Pictures"), 10).len(), 1);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn duplicates_need_equal_contents() {
        let d = scratch();
        std::fs::write(d.join("a.bin"), b"same bytes here").unwrap();
        std::fs::write(d.join("b.bin"), b"same bytes here").unwrap();
        std::fs::write(d.join("c.bin"), b"diff bytes here").unwrap();
        let idx = FindIndex::in_memory().unwrap();
        idx.add_root(&d).unwrap();
        let g = idx.duplicates(1);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].paths.len(), 2);
        assert!(g[0].paths.iter().all(|p| !p.ends_with("c.bin")));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn tidy_is_a_plan_until_applied_and_never_overwrites() {
        let d = scratch();
        std::fs::write(d.join("photo.jpg"), b"x").unwrap();
        std::fs::write(d.join("song.mp3"), b"x").unwrap();
        std::fs::create_dir_all(d.join("Pictures")).unwrap();
        std::fs::write(d.join("Pictures").join("photo.jpg"), b"older").unwrap();
        let plan = tidy_plan(&d).unwrap();
        assert_eq!(plan.len(), 2);
        assert!(d.join("photo.jpg").exists(), "planning moves nothing");
        assert!(plan.iter().any(|m| m.to.ends_with("photo (2).jpg")));
        let (done, skipped) = apply_tidy(&plan);
        assert_eq!((done, skipped.len()), (2, 0));
        assert_eq!(std::fs::read(d.join("Pictures").join("photo.jpg")).unwrap(), b"older");
        assert!(d.join("Music").join("song.mp3").exists());
        assert_eq!(apply_tidy(&plan).1.len(), 2, "a second run skips");
        std::fs::remove_dir_all(&d).ok();
    }
}

#[cfg(test)]
mod media_transcript_tests {
    use super::*;

    #[test]
    fn transcripts_are_found_and_taken_back() {
        let idx = FindIndex::in_memory().unwrap();
        idx.set_transcript("D:\talks\budget.mp4", "we agreed the garden budget").unwrap();
        let hits = idx.search("garden", None, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "budget.mp4");
        idx.set_transcript("D:\talks\budget.mp4", "now about engines").unwrap();
        assert!(idx.search("garden", None, 10).is_empty());
        assert_eq!(idx.search("engines", Some("Videos"), 10).len(), 1);
        idx.clear_transcript("D:\talks\budget.mp4").unwrap();
        assert!(idx.search("engines", None, 10).is_empty());
        assert!(idx.search("budget", None, 10).is_empty());
    }
}
