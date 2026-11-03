// SPDX-License-Identifier: AGPL-3.0-only
//! Checks at every start that the programs SulcusAI downloaded (the engines
//! and helpers in `engines\`) are still the ones it installed.
//!
//! The catalog vouches for each download by the sha256 of its archive, and
//! the archive is deleted once unpacked, so its hash alone can't check the
//! files later. When an archive that matched the catalog is unpacked,
//! `record_unpacked` writes down the sha256 of every file that came out of
//! it (a manifest, in `integrity\manifests\`). At start `check_all` compares
//! the files with their manifest, and trusts a manifest only while the
//! catalog still lists the archive it was made from.
//!
//! Fail safe: a folder with no manifest, or one whose archive the catalog no
//! longer lists, is "unchecked", never "ok". A changed or missing file is
//! reported with what it is and where; nothing is ever deleted or changed.
//!
//! Hashing a few GB at every start would be slow, so a hash is kept per
//! (path, size, modified time) in `integrity\cache.json`. A program that
//! changes a file and puts its old size and time back would get past the
//! cache, so a kept hash is also redone once it's a week old.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::catalog::{Asset, Catalog};
use crate::paths::Paths;

const FOLDER: &str = "integrity";
const MANIFESTS: &str = "manifests";
const CACHE_FILE: &str = "cache.json";
const REPORT_FILE: &str = "last-report.json";
const REHASH_AFTER_MS: i64 = 7 * 24 * 3600 * 1000;
/// A Python environment beside a helper can hold tens of thousands of
/// files: the look for programs that weren't in the archive stops here.
const MAX_WALK: usize = 20_000;
/// Unrecorded programs listed per folder; the rest are only counted.
const MAX_EXTRA_LISTED: usize = 20;
/// Kinds of file Windows (or Python, or Node) will run or load as code.
const RUNNABLE: &[&str] = &["exe", "dll", "sys", "com", "scr", "so", "dylib", "pyd", "node", "bat", "cmd", "ps1"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Mismatch,
    Missing,
    Unchecked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    /// The download it came from (the archive's name) and the file in it.
    pub what: String,
    pub path: String,
    pub status: Status,
    pub expected: Option<String>,
    pub found: Option<String>,
    /// Why it has this status, in words for the Security page.
    pub note: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Report {
    pub at: i64,
    pub items: Vec<Item>,
    pub ok: usize,
    pub mismatched: usize,
    pub missing: usize,
    pub unchecked: usize,
    /// Files read and hashed this time (the rest came from the cache).
    pub hashed: usize,
}

impl Report {
    fn push(&mut self, item: Item) {
        match item.status {
            Status::Ok => self.ok += 1,
            Status::Mismatch => self.mismatched += 1,
            Status::Missing => self.missing += 1,
            Status::Unchecked => self.unchecked += 1,
        }
        self.items.push(item);
    }

    pub fn problems(&self) -> impl Iterator<Item = &Item> {
        self.items.iter().filter(|i| matches!(i.status, Status::Mismatch | Status::Missing))
    }

    pub fn has_problems(&self) -> bool {
        self.mismatched + self.missing > 0
    }

    /// One line for app.log and the top of the Security page.
    pub fn summary(&self) -> String {
        let mut s = format!("{} program files match what was installed", self.ok);
        if self.mismatched > 0 {
            s.push_str(&format!(", {} changed since", self.mismatched));
        }
        if self.missing > 0 {
            s.push_str(&format!(", {} missing", self.missing));
        }
        if self.unchecked > 0 {
            s.push_str(&format!(", {} not checked (no recorded hashes)", self.unchecked));
        }
        s
    }
}

/// What came out of one verified archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub dir: String,
    /// The archive's sha256, as the catalog gave it: the link back to the catalog.
    pub source_sha256: String,
    pub source_url: String,
    pub recorded: i64,
    /// Path inside the folder ('/' between parts) and its sha256.
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    size: u64,
    modified_ns: u64,
    sha256: String,
    hashed_at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Cache {
    entries: HashMap<String, CacheEntry>,
    #[serde(skip)]
    seen: HashSet<String>,
}

impl Cache {
    pub fn load(path: &Path) -> Cache {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    /// Saves only the files looked at this time, so removed engines drop out.
    pub fn save(&mut self, path: &Path) -> std::io::Result<()> {
        let seen = std::mem::take(&mut self.seen);
        self.entries.retain(|k, _| seen.contains(k));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        crate::storage::write_safely(path, serde_json::to_vec(self).map_err(std::io::Error::other)?)
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn modified_ns(m: &std::fs::Metadata) -> u64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64)
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(hex::encode(hasher.finalize()));
        }
        hasher.update(&buf[..n]);
    }
}

/// The file's sha256, from the cache while its size and modified time are
/// unchanged and the kept hash is under a week old. The bool: it was hashed.
pub fn hash_cached(path: &Path, cache: &mut Cache, now: i64) -> std::io::Result<(String, bool)> {
    let meta = std::fs::metadata(path)?;
    let key = path.to_string_lossy().into_owned();
    cache.seen.insert(key.clone());
    let (size, modified) = (meta.len(), modified_ns(&meta));
    if let Some(e) = cache.entries.get(&key) {
        if e.size == size && e.modified_ns == modified && now - e.hashed_at < REHASH_AFTER_MS {
            return Ok((e.sha256.clone(), false));
        }
    }
    let sha = sha256_file(path)?;
    cache.entries.insert(key, CacheEntry { size, modified_ns: modified, sha256: sha.clone(), hashed_at: now });
    Ok((sha, true))
}

/// One file against the hash it should have. No hash known: unchecked.
pub fn check_file(what: &str, path: &Path, expected: Option<&str>, cache: &mut Cache, now: i64, report: &mut Report) {
    let shown = path.display().to_string();
    let Some(expected) = expected.map(str::to_ascii_lowercase).filter(|e| e.len() == 64) else {
        report.push(Item { what: what.into(), path: shown, status: Status::Unchecked, expected: None, found: None, note: "No recorded hash for this file, so it wasn't checked.".into() });
        return;
    };
    if !path.is_file() {
        report.push(Item { what: what.into(), path: shown, status: Status::Missing, expected: Some(expected), found: None, note: "This file was installed but isn't there now.".into() });
        return;
    }
    match hash_cached(path, cache, now) {
        Ok((found, hashed)) => {
            report.hashed += usize::from(hashed);
            let (status, note) = if found == expected {
                (Status::Ok, "Matches what was installed.")
            } else {
                (Status::Mismatch, "This file changed after it was installed: it isn't the one that was downloaded.")
            };
            report.push(Item { what: what.into(), path: shown, status, expected: Some(expected), found: Some(found), note: note.into() });
        }
        Err(e) => report.push(Item {
            what: what.into(),
            path: shown,
            status: Status::Unchecked,
            expected: Some(expected),
            found: None,
            note: format!("Couldn't read this file to check it ({e})."),
        }),
    }
}

fn is_runnable(path: &Path, _meta: &std::fs::Metadata) -> bool {
    if path.extension().and_then(|x| x.to_str()).is_some_and(|x| RUNNABLE.contains(&x.to_ascii_lowercase().as_str())) {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if _meta.permissions().mode() & 0o111 != 0 {
            return true;
        }
    }
    false
}

/// Every file under `dir` ('/'-joined relative paths), links not followed,
/// at most `limit` entries looked at.
fn walk(dir: &Path, limit: usize, mut keep: impl FnMut(&Path, &std::fs::Metadata) -> bool) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    let mut looked = 0usize;
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            looked += 1;
            if looked > limit {
                return out;
            }
            let Ok(ft) = e.file_type() else { continue };
            let path = e.path();
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                if e.file_name() != "__pycache__" {
                    stack.push(path);
                }
            } else if ft.is_file() {
                let Ok(meta) = e.metadata() else { continue };
                if keep(&path, &meta) {
                    let rel = path.strip_prefix(dir).unwrap_or(&path).components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
                    out.push((rel, path));
                }
            }
        }
    }
    out
}

/// Hashes everything that came out of a verified archive.
pub fn make_manifest(dir_name: &str, source_sha256: &str, source_url: &str, unpacked: &Path) -> std::io::Result<Manifest> {
    let mut files = BTreeMap::new();
    for (rel, path) in walk(unpacked, usize::MAX, |_, _| true) {
        files.insert(rel, sha256_file(&path)?);
    }
    Ok(Manifest { dir: dir_name.into(), source_sha256: source_sha256.to_ascii_lowercase(), source_url: source_url.into(), recorded: now_ms(), files })
}

fn folder(paths: &Paths) -> PathBuf {
    paths.data.join(FOLDER)
}

fn manifest_file(integrity: &Path, dir_name: &str) -> PathBuf {
    let safe: String = dir_name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect();
    integrity.join(MANIFESTS).join(format!("{safe}.json"))
}

pub fn save_manifest(integrity: &Path, m: &Manifest) -> std::io::Result<()> {
    let path = manifest_file(integrity, &m.dir);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::storage::write_safely(&path, serde_json::to_vec_pretty(m).map_err(std::io::Error::other)?)
}

fn load_manifest(integrity: &Path, dir_name: &str) -> Option<Manifest> {
    serde_json::from_str(&std::fs::read_to_string(manifest_file(integrity, dir_name)).ok()?).ok()
}

/// Called right after a download that matched the catalog is unpacked
/// (before anything runs from it): what to compare with at every start.
pub fn record_unpacked(paths: &Paths, dir_name: &str, asset: &Asset, unpacked: &Path) -> Result<(), String> {
    let m = make_manifest(dir_name, &asset.sha256, &asset.url, unpacked).map_err(|e| format!("Couldn't record the installed files' hashes: {e}"))?;
    save_manifest(&folder(paths), &m).map_err(|e| format!("Couldn't save the installed files' hashes: {e}"))
}

/// Every sha256 the catalog vouches for, with its download's address. Read
/// from the whole catalog so a helper added to it later is covered too.
pub fn known_downloads(catalog: &serde_json::Value) -> HashMap<String, String> {
    fn visit(v: &serde_json::Value, out: &mut HashMap<String, String>) {
        match v {
            serde_json::Value::Object(o) => {
                if let (Some(sha), Some(url)) = (o.get("sha256").and_then(|s| s.as_str()), o.get("url").and_then(|u| u.as_str())) {
                    if sha.len() == 64 {
                        out.insert(sha.to_ascii_lowercase(), url.to_string());
                    }
                }
                o.values().for_each(|x| visit(x, out));
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| visit(x, out)),
            _ => {}
        }
    }
    let mut out = HashMap::new();
    visit(catalog, &mut out);
    out
}

fn archive_name(url: &str) -> &str {
    url.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or(url)
}

/// One folder under `engines\`.
pub fn check_dir(dir: &Path, manifest: Option<&Manifest>, known: &HashMap<String, String>, cache: &mut Cache, now: i64, report: &mut Report) {
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let unchecked = |note: &str| Item { what: name.clone(), path: dir.display().to_string(), status: Status::Unchecked, expected: None, found: None, note: note.into() };
    let Some(m) = manifest else {
        report.push(unchecked("No hashes were recorded when this was installed (installed by an earlier version, or made on this PC), so it wasn't checked."));
        return;
    };
    if !known.contains_key(&m.source_sha256.to_ascii_lowercase()) {
        report.push(unchecked("The catalog no longer lists the download this came from, so its recorded hashes aren't trusted."));
        return;
    }
    let source = archive_name(&m.source_url);
    for (rel, sha) in &m.files {
        let path = rel.split('/').fold(dir.to_path_buf(), |p, part| p.join(part));
        check_file(&format!("{source}: {rel}"), &path, Some(sha), cache, now, report);
    }
    // A program dropped beside a checked one can be loaded in its place
    // (a DLL beside an .exe is found first), so new ones are named.
    let extra: Vec<(String, PathBuf)> = walk(dir, MAX_WALK, is_runnable).into_iter().filter(|(rel, _)| !m.files.contains_key(rel)).collect();
    for (rel, path) in extra.iter().take(MAX_EXTRA_LISTED) {
        report.push(Item {
            what: format!("{source}: {rel}"),
            path: path.display().to_string(),
            status: Status::Unchecked,
            expected: None,
            found: None,
            note: "A program that wasn't in the download sits beside it (added later, by SulcusAI or something else).".into(),
        });
    }
    if extra.len() > MAX_EXTRA_LISTED {
        report.push(unchecked(&format!("{} more programs that weren't in the download sit in this folder.", extra.len() - MAX_EXTRA_LISTED)));
    }
}

/// Every folder in `engines` against its manifest in `integrity`.
pub fn check_with(engines: &Path, integrity: &Path, known: &HashMap<String, String>, cache: &mut Cache, now: i64) -> Report {
    let mut report = Report { at: now, ..Default::default() };
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(engines)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        // Half-unpacked downloads, not installed yet.
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    for dir in dirs {
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut one = Report::default();
        check_dir(&dir, load_manifest(integrity, &name).as_ref(), known, cache, now, &mut one);
        // Cleanup can remove an old engine while this runs: a folder gone
        // since isn't "missing files", it's uninstalled.
        if dir.is_dir() {
            report.hashed += one.hashed;
            one.items.into_iter().for_each(|i| report.push(i));
        }
    }
    report
}

/// At every start, off the window's thread: the installed engines and
/// helpers against the catalog. Saves the cache and the report.
pub fn check_all(paths: &Paths, catalog: &Catalog) -> Report {
    let known = serde_json::to_value(catalog).map(|v| known_downloads(&v)).unwrap_or_default();
    let integrity = folder(paths);
    let cache_path = integrity.join(CACHE_FILE);
    let mut cache = Cache::load(&cache_path);
    let report = check_with(&paths.engines, &integrity, &known, &mut cache, now_ms());
    let _ = cache.save(&cache_path);
    if let Ok(json) = serde_json::to_vec(&report) {
        let _ = crate::storage::write_safely(&integrity.join(REPORT_FILE), json);
    }
    report
}

/// The report from the last start, for the Security page.
pub fn last_report(paths: &Paths) -> Option<Report> {
    serde_json::from_str(&std::fs::read_to_string(folder(paths).join(REPORT_FILE)).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 24 * 3600 * 1000;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sulcus-integrity-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sha(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    /// An engine folder as an archive would leave it, its manifest, and the
    /// catalog vouching for that archive.
    fn installed(root: &Path) -> (PathBuf, PathBuf, HashMap<String, String>) {
        let engines = root.join("engines");
        let integrity = root.join("integrity");
        let dir = engines.join("b11450-vulkan-x64");
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(dir.join("llama-server.exe"), b"server program").unwrap();
        std::fs::write(dir.join("lib").join("ggml-vulkan.dll"), b"vulkan library").unwrap();
        let zip_sha = sha(b"the archive");
        let m = make_manifest("b11450-vulkan-x64", &zip_sha, "https://example.org/llama-b11450-bin-win-vulkan-x64.zip", &dir).unwrap();
        save_manifest(&integrity, &m).unwrap();
        let known = HashMap::from([(zip_sha, m.source_url.clone())]);
        (engines, integrity, known)
    }

    fn status_of<'a>(r: &'a Report, ends: &str) -> Option<&'a Item> {
        r.items.iter().find(|i| i.what.ends_with(ends))
    }

    #[test]
    fn unchanged_files_match() {
        let root = temp();
        let (engines, integrity, known) = installed(&root);
        let r = check_with(&engines, &integrity, &known, &mut Cache::default(), 0);
        assert_eq!((r.ok, r.mismatched, r.missing, r.unchecked), (2, 0, 0, 0), "{r:#?}");
        assert!(!r.has_problems());
        let server = status_of(&r, "llama-server.exe").unwrap();
        assert_eq!(server.what, "llama-b11450-bin-win-vulkan-x64.zip: llama-server.exe");
        assert_eq!(server.found.as_deref(), Some(sha(b"server program").as_str()));
        assert!(status_of(&r, "lib/ggml-vulkan.dll").is_some_and(|i| i.status == Status::Ok));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_changed_file_is_reported_with_what_and_where_and_left_alone() {
        let root = temp();
        let (engines, integrity, known) = installed(&root);
        let exe = engines.join("b11450-vulkan-x64").join("llama-server.exe");
        std::fs::write(&exe, b"something else").unwrap();
        let r = check_with(&engines, &integrity, &known, &mut Cache::default(), 0);
        assert_eq!(r.mismatched, 1, "{r:#?}");
        let bad = r.problems().next().unwrap();
        assert_eq!(bad.status, Status::Mismatch);
        assert_eq!(bad.path, exe.display().to_string());
        assert!(bad.what.contains("llama-b11450-bin-win-vulkan-x64.zip"));
        assert_eq!(bad.expected.as_deref(), Some(sha(b"server program").as_str()));
        assert_eq!(bad.found.as_deref(), Some(sha(b"something else").as_str()));
        assert_eq!(std::fs::read(&exe).unwrap(), b"something else", "never changed or deleted");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_file_is_reported() {
        let root = temp();
        let (engines, integrity, known) = installed(&root);
        std::fs::remove_file(engines.join("b11450-vulkan-x64").join("lib").join("ggml-vulkan.dll")).unwrap();
        let r = check_with(&engines, &integrity, &known, &mut Cache::default(), 0);
        assert_eq!((r.ok, r.missing), (1, 1), "{r:#?}");
        assert!(status_of(&r, "lib/ggml-vulkan.dll").is_some_and(|i| i.status == Status::Missing && i.found.is_none()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn what_has_no_trusted_hash_is_unchecked_never_ok() {
        let root = temp();
        let mut report = Report::default();
        let f = root.join("helper.exe");
        std::fs::write(&f, b"x").unwrap();
        check_file("helper", &f, None, &mut Cache::default(), 0, &mut report);
        check_file("helper", &f, Some("not a hash"), &mut Cache::default(), 0, &mut report);
        assert!(report.items.iter().all(|i| i.status == Status::Unchecked));
        assert_eq!(report.ok, 0);

        // A folder with no manifest, and one whose archive the catalog dropped.
        let (engines, integrity, _) = installed(&root);
        std::fs::create_dir_all(engines.join("trainer-uv")).unwrap();
        std::fs::write(engines.join("trainer-uv").join("python.exe"), b"py").unwrap();
        let r = check_with(&engines, &integrity, &HashMap::new(), &mut Cache::default(), 0);
        assert_eq!((r.ok, r.unchecked), (0, 2), "{r:#?}");
        assert!(r.items.iter().any(|i| i.what == "trainer-uv" && i.note.contains("No hashes were recorded")));
        assert!(r.items.iter().any(|i| i.what == "b11450-vulkan-x64" && i.note.contains("no longer lists")));

        // Half-unpacked downloads aren't installed yet.
        std::fs::create_dir_all(engines.join(".staging-x")).unwrap();
        assert!(!check_with(&engines, &integrity, &HashMap::new(), &mut Cache::default(), 0).items.iter().any(|i| i.what.contains("staging")));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_program_added_beside_a_checked_one_is_named() {
        let root = temp();
        let (engines, integrity, known) = installed(&root);
        let dir = engines.join("b11450-vulkan-x64");
        std::fs::write(dir.join("version.dll"), b"planted").unwrap();
        std::fs::write(dir.join("notes.txt"), b"not a program").unwrap();
        let r = check_with(&engines, &integrity, &known, &mut Cache::default(), 0);
        assert_eq!((r.ok, r.unchecked), (2, 1), "{r:#?}");
        assert!(status_of(&r, "version.dll").is_some_and(|i| i.status == Status::Unchecked && i.note.contains("wasn't in the download")));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_cache_skips_unchanged_files() {
        let root = temp();
        let f = root.join("llama-server.exe");
        std::fs::write(&f, b"server program").unwrap();
        let mut cache = Cache::default();
        assert_eq!(hash_cached(&f, &mut cache, 0).unwrap(), (sha(b"server program"), true));
        assert_eq!(hash_cached(&f, &mut cache, DAY).unwrap(), (sha(b"server program"), false));
        // Proof it wasn't read: a kept hash that the file doesn't have comes back as is.
        let key = f.to_string_lossy().into_owned();
        cache.entries.get_mut(&key).unwrap().sha256 = "0".repeat(64);
        assert_eq!(hash_cached(&f, &mut cache, DAY).unwrap(), ("0".repeat(64), false));

        // The cache survives a save and a load, keeping only what was looked at.
        let saved = root.join("cache.json");
        cache.entries.insert("gone.exe".into(), CacheEntry { size: 1, modified_ns: 1, sha256: "1".repeat(64), hashed_at: 0 });
        cache.save(&saved).unwrap();
        let mut loaded = Cache::load(&saved);
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(hash_cached(&f, &mut loaded, DAY).unwrap(), ("0".repeat(64), false));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_cache_is_dropped_when_size_or_time_changes_or_it_is_old() {
        let root = temp();
        let f = root.join("llama-server.exe");
        std::fs::write(&f, b"server program").unwrap();
        let mut cache = Cache::default();
        hash_cached(&f, &mut cache, 0).unwrap();

        // A different size.
        std::fs::write(&f, b"server program, longer").unwrap();
        assert_eq!(hash_cached(&f, &mut cache, 0).unwrap(), (sha(b"server program, longer"), true));

        // The same size, a different modified time.
        std::fs::write(&f, b"server program, LONGER").unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
        std::fs::File::options().write(true).open(&f).unwrap().set_modified(later).unwrap();
        assert_eq!(hash_cached(&f, &mut cache, 0).unwrap(), (sha(b"server program, LONGER"), true));
        assert!(!hash_cached(&f, &mut cache, 0).unwrap().1);

        // A week on, the kept hash is redone even though nothing seems changed.
        assert!(hash_cached(&f, &mut cache, 8 * DAY).unwrap().1);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_second_start_reads_nothing_unchanged() {
        let root = temp();
        let (engines, integrity, known) = installed(&root);
        let mut cache = Cache::default();
        assert_eq!(check_with(&engines, &integrity, &known, &mut cache, 0).hashed, 2);
        let again = check_with(&engines, &integrity, &known, &mut cache, DAY);
        assert_eq!((again.hashed, again.ok), (0, 2));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_catalog_vouches_for_every_download_it_lists() {
        let cat = Catalog::bundled();
        let known = known_downloads(&serde_json::to_value(&cat).unwrap());
        for asset in cat.engine.assets.values() {
            assert_eq!(known.get(&asset.sha256.to_ascii_lowercase()), Some(&asset.url));
        }
        if let Some(addons) = serde_json::to_value(&cat).unwrap().get("addons").and_then(|a| a.get("node")) {
            assert!(known.contains_key(addons["sha256"].as_str().unwrap()));
        }

        // check_all end to end: an engine recorded from the catalog's own archive.
        let root = temp();
        let paths = Paths::new(root.clone()).unwrap();
        let asset = cat.engine.assets.get("vulkan-x64").unwrap();
        let dir = paths.engines.join(format!("{}-vulkan-x64", cat.engine.build));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("llama-server.exe"), b"server program").unwrap();
        record_unpacked(&paths, &format!("{}-vulkan-x64", cat.engine.build), asset, &dir).unwrap();
        let r = check_all(&paths, &cat);
        assert_eq!((r.ok, r.mismatched, r.unchecked), (1, 0, 0), "{r:#?}");
        assert_eq!(last_report(&paths).map(|l| l.ok), Some(1));
        assert!(r.summary().starts_with("1 program files match"));
        std::fs::remove_dir_all(&root).ok();
    }
}
