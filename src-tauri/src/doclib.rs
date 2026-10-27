// SPDX-License-Identifier: AGPL-3.0-only
//! The documents library: folders and files the person adds are read, cut
//! into passages and indexed on this PC, so the assistant can answer
//! questions across all of them and say where each answer came from (the
//! file, and its page, slide, sheet or section).
//!
//! - Reading: PDFs page by page; Word, PowerPoint, Excel, Markdown, text,
//!   CSV and web pages through `docs::read_all`.
//! - Passages: about 1,200 characters with a little overlap, never across
//!   pages or slides, each knowing its place.
//! - Search: what each passage means, as numbers (Qwen3-Embedding 0.6B on a
//!   small engine of its own beside the chat model; the first 512 of its
//!   1,024, which it's trained to allow), and its words (BM25). The two
//!   rankings are merged. Questions get Qwen's search instruction, passages
//!   don't. Without the model (not downloaded yet), words alone.
//! - Everything but the ids is encrypted like the rest of SulcusAI's data,
//!   the numbers too. The index is opened into memory on the first search
//!   and kept while the app runs (about 4 KB a passage).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::Emitter;

use crate::crypto::Cipher;
use crate::{engine, AppState, AppStateRef};

/// Kinds of files the library reads.
pub const KINDS: &[&str] = &["pdf", "docx", "pptx", "xlsx", "xlsm", "xls", "ods", "md", "markdown", "txt", "csv", "tsv", "html", "htm", "json", "log"];
/// Numbers kept for each passage.
const DIMS: usize = 512;
/// A passage's length, and how much of the one before it starts with.
const PASSAGE: usize = 1_200;
const OVERLAP: usize = 200;
/// Files bigger than this are left out.
const MAX_FILE: u64 = 64 * 1024 * 1024;
/// The most files a folder adds, and passages a file keeps.
const MAX_FILES: usize = 5_000;
const MAX_PASSAGES: usize = 3_000;
/// Passages sent to the model at once.
const BATCH: usize = 16;
/// What a question is introduced with (Qwen3-Embedding's instruction).
const ASK: &str = "Instruct: Given a question, retrieve passages from the person's own documents that answer it\nQuery: ";

// ---------- what's kept ----------

/// A folder or file the person added.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Source {
    pub id: String,
    pub path: String,
    pub folder: bool,
    pub added_at: i64,
    pub scanned_at: Option<i64>,
}

/// A file found in a source.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DocFile {
    pub id: String,
    pub source_id: String,
    pub path: String,
    pub name: String,
    /// When it was changed and how big it was when read (to read it again only when it changes).
    pub modified: i64,
    pub size: u64,
    pub passages: usize,
    /// Its passages have their numbers (else it's found by its words only).
    pub embedded: bool,
    pub indexed_at: i64,
    pub error: Option<String>,
}

/// A passage: its words and where in the file it is ("page 3", "slide 2").
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Passage {
    pub text: String,
    pub place: Option<String>,
}

fn file_id(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let key = path.to_string_lossy().to_lowercase().replace('/', "\\");
    hex::encode(&Sha256::digest(key.as_bytes())[..12])
}

pub fn sources(conn: &Connection, c: &Cipher) -> Vec<Source> {
    let mut out: Vec<Source> = rows(conn, "SELECT data FROM doc_sources ORDER BY created_at", []).iter().filter_map(|d| c.decrypt(d).ok()).filter_map(|j| serde_json::from_str(&j).ok()).collect();
    out.sort_by_key(|s| s.added_at);
    out
}

pub fn files(conn: &Connection, c: &Cipher, source_id: Option<&str>) -> Vec<DocFile> {
    let all = match source_id {
        Some(s) => rows(conn, "SELECT data FROM doc_files WHERE source_id = ?1", [s]),
        None => rows(conn, "SELECT data FROM doc_files", []),
    };
    let mut out: Vec<DocFile> = all.iter().filter_map(|d| c.decrypt(d).ok()).filter_map(|j| serde_json::from_str(&j).ok()).collect();
    out.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));
    out
}

fn rows<P: rusqlite::Params>(conn: &Connection, sql: &str, p: P) -> Vec<String> {
    let Ok(mut st) = conn.prepare(sql) else { return Vec::new() };
    st.query_map(p, |r| r.get::<_, String>(0)).map(|r| r.flatten().collect()).unwrap_or_default()
}

fn put_source(conn: &Connection, c: &Cipher, s: &Source) -> Result<(), String> {
    let data = c.encrypt(&serde_json::to_string(s).map_err(|e| e.to_string())?);
    conn.execute("INSERT INTO doc_sources (id, data, created_at) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET data = excluded.data", params![s.id, data, s.added_at])
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn put_file(conn: &Connection, c: &Cipher, f: &DocFile) -> Result<(), String> {
    let data = c.encrypt(&serde_json::to_string(f).map_err(|e| e.to_string())?);
    conn.execute("INSERT INTO doc_files (id, source_id, data) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET data = excluded.data, source_id = excluded.source_id", params![f.id, f.source_id, data])
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn forget_file(conn: &Connection, id: &str) {
    let _ = conn.execute("DELETE FROM doc_passages WHERE file_id = ?1", [id]);
    let _ = conn.execute("DELETE FROM doc_files WHERE id = ?1", [id]);
}

/// A passage's numbers as bytes (sealed with the rest).
fn vec_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn bytes_vec(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

fn put_passages(conn: &Connection, c: &Cipher, file: &str, passages: &[Passage], vecs: Option<&[Vec<f32>]>) -> Result<(), String> {
    conn.execute("DELETE FROM doc_passages WHERE file_id = ?1", [file]).map_err(|e| e.to_string())?;
    let mut st = conn.prepare("INSERT INTO doc_passages (file_id, ord, data, vec) VALUES (?1, ?2, ?3, ?4)").map_err(|e| e.to_string())?;
    for (i, p) in passages.iter().enumerate() {
        let data = c.encrypt(&serde_json::to_string(p).map_err(|e| e.to_string())?);
        let v = vecs.and_then(|v| v.get(i)).map(|v| c.seal_bytes(&vec_bytes(v)));
        st.execute(params![file, i as i64, data, v]).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---------- reading files into passages ----------

/// Text cut into passages of about `PASSAGE` characters at paragraph or
/// sentence ends, each starting with the end of the one before.
pub fn cut(text: &str, place: Option<&str>) -> Vec<Passage> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let push = |cur: &mut String, out: &mut Vec<Passage>| {
        let t = cur.split_whitespace().collect::<Vec<_>>().join(" ");
        if t.chars().count() >= 20 {
            out.push(Passage { text: t, place: place.map(str::to_string) });
        }
        // The next starts with this one's last sentences.
        let tail: String = cur.chars().rev().take(OVERLAP).collect::<Vec<_>>().into_iter().rev().collect();
        let start = tail.find(|c: char| c == '.' || c == '\n').map(|i| i + 1).unwrap_or(0);
        *cur = tail[start..].trim_start().to_string();
    };
    for para in text.split("\n\n").flat_map(|p| p.split('\n')) {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }
        // A paragraph longer than a passage goes in by sentences.
        let pieces: Vec<&str> = if para.chars().count() > PASSAGE { para.split_inclusive(['.', '!', '?', ';']).collect() } else { vec![para] };
        for piece in pieces {
            if cur.chars().count() + piece.chars().count() > PASSAGE && cur.chars().count() > OVERLAP {
                push(&mut cur, &mut out);
            }
            if !cur.is_empty() {
                cur.push('\n');
            }
            // A sentence longer than a passage is cut where it must be.
            let mut rest = piece;
            while rest.chars().count() > PASSAGE * 2 {
                let at = rest.char_indices().nth(PASSAGE).map(|(i, _)| i).unwrap_or(rest.len());
                cur.push_str(&rest[..at]);
                push(&mut cur, &mut out);
                rest = &rest[at..];
            }
            cur.push_str(rest);
        }
    }
    let before = out.len();
    push(&mut cur, &mut out);
    // The last push may repeat only the overlap: drop it if it adds nothing.
    if out.len() > before && before > 0 && out[before].text.chars().count() <= OVERLAP && out[before - 1].text.contains(&out[before].text) {
        out.pop();
    }
    out
}

/// Text with "## Slide n", "## Sheet: x" and headings, cut by its sections:
/// each passage knows its slide, sheet or heading.
pub fn by_sections(text: &str) -> Vec<Passage> {
    let mut out = Vec::new();
    let mut place: Option<String> = None;
    let mut body = String::new();
    for line in text.lines() {
        let heading = line.trim_start().strip_prefix('#').map(|h| h.trim_start_matches('#').trim());
        match heading.filter(|h| !h.is_empty()) {
            Some(h) => {
                out.extend(cut(&body, place.as_deref()));
                body.clear();
                place = Some(match h.strip_prefix("Slide ") {
                    Some(n) if n.chars().all(|c| c.is_ascii_digit()) => format!("slide {n}"),
                    _ => match h.strip_prefix("Sheet: ") {
                        Some(s) => format!("sheet “{s}”"),
                        None => format!("“{}”", h.chars().take(80).collect::<String>()),
                    },
                });
                // The heading's words help find it.
                body.push_str(h);
                body.push('\n');
            }
            None => {
                body.push_str(line);
                body.push('\n');
            }
        }
    }
    out.extend(cut(&body, place.as_deref()));
    out
}

/// A file's passages.
pub fn passages_of(path: &Path) -> Result<Vec<Passage>, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let mut out = if ext == "pdf" {
        let pages = crate::docs::pdf_pages(path)?;
        pages.iter().enumerate().flat_map(|(i, t)| cut(t, Some(&format!("page {}", i + 1)))).collect()
    } else {
        by_sections(&crate::docs::read_all(path)?)
    };
    out.truncate(MAX_PASSAGES);
    if out.is_empty() {
        return Err("No text was found in it (a scanned PDF has pictures of pages, not text).".into());
    }
    Ok(out)
}

/// The files a source holds now: itself, or a folder's readable files
/// (hidden ones and those Git ignores left out), at most `MAX_FILES`.
fn found(source: &Source) -> Vec<PathBuf> {
    let root = PathBuf::from(&source.path);
    let readable = |p: &Path| p.extension().and_then(|e| e.to_str()).is_some_and(|e| KINDS.contains(&e.to_ascii_lowercase().as_str())) && p.metadata().is_ok_and(|m| m.len() <= MAX_FILE);
    if !source.folder {
        return if root.is_file() && readable(&root) { vec![root] } else { Vec::new() };
    }
    ignore::WalkBuilder::new(&root)
        .max_depth(Some(12))
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(|e| e.into_path())
        .filter(|p| readable(p) && !p.components().any(|c| matches!(c.as_os_str().to_str(), Some("node_modules" | "target" | "AppData"))))
        .take(MAX_FILES)
        .collect()
}

fn modified_ms(p: &Path) -> i64 {
    p.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64)
}

// ---------- the embedding model ----------

struct Embedder {
    eng: engine::Engine,
    ep: engine::Endpoint,
    used: Instant,
}

static EMBEDDER: tokio::sync::Mutex<Option<Embedder>> = tokio::sync::Mutex::const_new(None);

/// Where the model is kept.
fn model_path(state: &AppState) -> Option<PathBuf> {
    let f = state.catalog.embedding.as_ref()?;
    Some(state.paths.models.join("embedding").join(&f.file))
}

/// Whether the model is here.
pub fn model_ready(state: &AppState) -> bool {
    let size = state.catalog.embedding.as_ref().map_or(0, |f| f.size);
    model_path(state).is_some_and(|p| p.metadata().is_ok_and(|m| m.len() == size))
}

/// Downloads the model (once).
async fn fetch_model(state: &Arc<AppState>, cancel: &AtomicBool, progress: &(dyn Fn(f64) + Sync)) -> Result<PathBuf, String> {
    let f = state.catalog.embedding.clone().ok_or("The documents library isn't available on this system yet.")?;
    let dest = model_path(state).ok_or("No model.")?;
    if dest.metadata().is_ok_and(|m| m.len() == f.size) {
        return Ok(dest);
    }
    state.log("network", &format!("Downloading {} ({}) from {}", f.file, crate::size_label(f.size), crate::host_of(&f.url)));
    let client = crate::net::external_client(state.settings().connectivity, crate::net::Purpose::ModelDownload, false)?;
    crate::download::fetch_verified(&client, &f.url, &dest, f.size, &f.sha256, cancel, |r, t| progress(r as f64 / t.max(1) as f64)).await?;
    Ok(dest)
}

/// The model's engine, started when needed: on the graphics card where it
/// fits beside what's there (it's small), else on the processor.
async fn embedder(state: &Arc<AppState>) -> Result<engine::Endpoint, String> {
    let mut slot = EMBEDDER.lock().await;
    if let Some(e) = slot.as_mut() {
        if e.eng.status().model_id.is_some() {
            e.used = Instant::now();
            return Ok(e.ep.clone());
        }
    }
    let model = model_path(state).filter(|_| model_ready(state)).ok_or("The library's model isn't downloaded yet.")?;
    let backend = engine::backend_for(&state.budget())?;
    let exe = engine::installed_server(&state.paths, &state.catalog.engine, backend).ok_or("The AI engine isn't installed yet: send a chat message once first.")?;
    let need = std::fs::metadata(&model).map_or(0, |m| m.len()) + crate::catalog::GIB / 2;
    let vram = state.budget().vram_card;
    let reserved = crate::media::chat_model_vram(state).await;
    let on_card = backend != engine::cpu_key() && vram.saturating_sub(reserved) >= need;
    let limits = state.limits();
    let log = state.paths.data.join("logs").join("library.log");
    let mut eng = engine::Engine::default();
    let spec = engine::LaunchSpec {
        exe: &exe,
        model_path: &model,
        model_id: "library",
        quant: "Q8_0",
        ctx: 4096,
        gpu_layers: if on_card { 999 } else { 0 },
        threads: limits.threads,
        low_priority: true,
        log: &log,
        mmproj: None,
        batch: 0,
        lora: None,
    };
    let ep = eng.ensure_tuned(spec, engine::Tuning { embedding: true, ..Default::default() }, state.job()).await?;
    crate::applog::line("library", &format!("Started the library's model on the {}", if on_card { "graphics card" } else { "processor" }));
    *slot = Some(Embedder { eng, ep: ep.clone(), used: Instant::now() });
    Ok(ep)
}

/// The library's embedding model for other programs (the local API's
/// `/v1/embeddings`): started when needed, once it's downloaded.
pub async fn embeddings_endpoint(state: &Arc<AppState>) -> Result<engine::Endpoint, String> {
    embedder(state).await
}

/// Stops the model when nothing has used it for ten minutes.
pub async fn unload_if_idle() {
    let mut slot = EMBEDDER.lock().await;
    if slot.as_ref().is_some_and(|e| e.used.elapsed().as_secs() > 600) {
        if let Some(mut e) = slot.take() {
            e.eng.stop().await;
        }
    }
}

/// The first `DIMS` numbers, scaled to length 1.
fn shortened(v: &[f32]) -> Vec<f32> {
    let mut out: Vec<f32> = v.iter().take(DIMS).copied().collect();
    let n = out.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    out.iter_mut().for_each(|x| *x /= n);
    out
}

/// Each text's numbers.
async fn embed(ep: &engine::Endpoint, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
    let body = json!({ "input": texts });
    let resp = crate::net::local_client()
        .post(ep.url("/v1/embeddings"))
        .bearer_auth(&ep.key)
        .json(&body)
        .timeout(std::time::Duration::from_secs(300))
        .send()
        .await
        .map_err(|e| format!("The library's model didn't answer: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("The library's model said: {}", resp.text().await.unwrap_or_default().chars().take(200).collect::<String>()));
    }
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    let mut data: Vec<(usize, Vec<f32>)> = v["data"]
        .as_array()
        .ok_or("The library's model gave no numbers.")?
        .iter()
        .map(|d| (d["index"].as_u64().unwrap_or(0) as usize, d["embedding"].as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect()).unwrap_or_default()))
        .collect();
    data.sort_by_key(|d| d.0);
    if data.len() != texts.len() || data.iter().any(|d| d.1.len() < DIMS) {
        return Err("The library's model gave the wrong numbers.".into());
    }
    Ok(data.into_iter().map(|d| shortened(&d.1)).collect())
}

/// What's embedded for a passage: its file's name and place help it be found.
fn embed_text(name: &str, p: &Passage) -> String {
    match &p.place {
        Some(place) => format!("{name}, {place}\n{}", p.text),
        None => format!("{name}\n{}", p.text),
    }
}

// ---------- indexing ----------

/// What indexing is doing, for the window.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Progress {
    pub running: bool,
    pub stage: String,
    pub done: usize,
    pub total: usize,
    pub file: Option<String>,
}

static PROGRESS: Mutex<Option<Progress>> = Mutex::new(None);
static STOP: AtomicBool = AtomicBool::new(false);
/// Changes when the index changes, so the copy in memory is opened again.
static VERSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn say(state: &AppState, p: Progress) {
    *PROGRESS.lock().unwrap() = Some(p.clone()).filter(|p| p.running);
    if let Some(app) = state.app.get() {
        let _ = app.emit("docs:progress", json!(p));
    }
}

/// Reads every source again: new and changed files are read and indexed,
/// files gone are forgotten. One run at a time.
pub async fn index(state: &Arc<AppState>) -> Result<(), String> {
    {
        let mut p = PROGRESS.lock().unwrap();
        if p.as_ref().is_some_and(|p| p.running) {
            return Ok(());
        }
        *p = Some(Progress { running: true, stage: "Looking for files".into(), ..Default::default() });
    }
    STOP.store(false, Ordering::Relaxed);
    let result = index_all(state).await;
    VERSION.fetch_add(1, Ordering::Relaxed);
    say(state, Progress { running: false, stage: result.as_ref().err().cloned().unwrap_or_else(|| "Up to date".into()), ..Default::default() });
    if let Some(app) = state.app.get() {
        let _ = app.emit("docs:changed", json!({}));
    }
    result
}

async fn index_all(state: &Arc<AppState>) -> Result<(), String> {
    let c = state.work_cipher()?;
    let list = sources(&state.db.lock().unwrap(), &c);
    // Every file now, by source.
    let mut todo: Vec<(Source, PathBuf)> = Vec::new();
    let mut here: HashSet<String> = HashSet::new();
    for s in &list {
        let (src, paths) = (s.clone(), tokio::task::spawn_blocking({
            let s = s.clone();
            move || found(&s)
        })
        .await
        .unwrap_or_default());
        for p in paths {
            here.insert(file_id(&p));
            todo.push((src.clone(), p));
        }
    }
    // Files gone (or their source removed) are forgotten.
    let known = files(&state.db.lock().unwrap(), &c, None);
    {
        let conn = state.db.lock().unwrap();
        for f in known.iter().filter(|f| !here.contains(&f.id)) {
            forget_file(&conn, &f.id);
        }
    }
    let known: HashMap<String, DocFile> = known.into_iter().map(|f| (f.id.clone(), f)).collect();
    // The model, downloaded the first time: words alone until then.
    let model = if model_ready(state) {
        true
    } else if crate::net::allowed(state.settings().connectivity, crate::net::Purpose::ModelDownload, false) {
        let total = state.catalog.embedding.as_ref().map_or(0, |f| f.size);
        let st = state.clone();
        let progress = move |f: f64| say(&st, Progress { running: true, stage: format!("Getting the library's model (once, {})", crate::size_label(total)), done: (f * 100.0) as usize, total: 100, file: None });
        match fetch_model(state, &STOP, &progress).await {
            Ok(_) => true,
            Err(e) => {
                crate::applog::line("library", &format!("The library's model didn't download ({e}); files are found by their words for now"));
                false
            }
        }
    } else {
        false
    };
    let changed: Vec<&(Source, PathBuf)> = todo
        .iter()
        .filter(|(_, p)| match known.get(&file_id(p)) {
            Some(f) => f.modified != modified_ms(p) || f.size != p.metadata().map_or(0, |m| m.len()) || (model && !f.embedded && f.error.is_none()),
            None => true,
        })
        .collect();
    let total = changed.len();
    let mut ep = None;
    for (n, (src, path)) in changed.into_iter().enumerate() {
        if STOP.load(Ordering::Relaxed) {
            return Err("Stopped".into());
        }
        let name = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        say(state, Progress { running: true, stage: "Reading".into(), done: n, total, file: Some(name.clone()) });
        let id = file_id(path);
        let mut f = DocFile { id: id.clone(), source_id: src.id.clone(), path: path.to_string_lossy().to_string(), name: name.clone(), modified: modified_ms(path), size: path.metadata().map_or(0, |m| m.len()), indexed_at: crate::db::now_ms(), ..Default::default() };
        let p = path.clone();
        let read = tokio::task::spawn_blocking(move || passages_of(&p)).await.map_err(|e| e.to_string())?;
        let passages = match read {
            Ok(p) => p,
            Err(e) => {
                f.error = Some(e);
                let conn = state.db.lock().unwrap();
                let _ = conn.execute("DELETE FROM doc_passages WHERE file_id = ?1", [&id]);
                put_file(&conn, &c, &f)?;
                continue;
            }
        };
        let mut vecs = None;
        if model {
            if ep.is_none() {
                ep = embedder(state).await.map_err(|e| crate::applog::line("library", &format!("The library's model didn't start: {e}"))).ok();
            }
            if let Some(e) = &ep {
                let mut all = Vec::with_capacity(passages.len());
                for (b, batch) in passages.chunks(BATCH).enumerate() {
                    if STOP.load(Ordering::Relaxed) {
                        return Err("Stopped".into());
                    }
                    say(state, Progress { running: true, stage: format!("Indexing ({} of {} passages)", b * BATCH, passages.len()), done: n, total, file: Some(name.clone()) });
                    let texts: Vec<String> = batch.iter().map(|p| embed_text(&name, p)).collect();
                    match embed(e, &texts).await {
                        Ok(v) => all.extend(v),
                        Err(err) => {
                            crate::applog::line("library", &format!("{name} wasn't indexed by meaning: {err}"));
                            break;
                        }
                    }
                }
                if all.len() == passages.len() {
                    vecs = Some(all);
                }
            }
        }
        f.passages = passages.len();
        f.embedded = vecs.is_some();
        let conn = state.db.lock().unwrap();
        put_passages(&conn, &c, &id, &passages, vecs.as_deref())?;
        put_file(&conn, &c, &f)?;
    }
    let now = crate::db::now_ms();
    let conn = state.db.lock().unwrap();
    for mut s in list {
        s.scanned_at = Some(now);
        put_source(&conn, &c, &s)?;
    }
    if total > 0 {
        crate::db::log_action(&conn, "library", &format!("Indexed {total} document(s) for the library"));
    }
    Ok(())
}

// ---------- searching ----------

struct Entry {
    file: String,
    passage: Passage,
    vec: Option<Vec<f32>>,
    /// Each word's count, for BM25.
    words: HashMap<String, u16>,
    len: usize,
}

struct Index {
    version: u64,
    entries: Vec<Entry>,
    /// How many passages each word is in.
    df: HashMap<String, usize>,
    avg_len: f32,
    names: HashMap<String, (String, String)>,
}

static INDEX: Mutex<Option<Arc<Index>>> = Mutex::new(None);

/// English words too common to tell passages apart.
const COMMON: &[&str] = &[
    "the", "and", "for", "are", "was", "were", "is", "be", "to", "of", "in", "on", "at", "by", "an", "or", "it", "its", "as", "with", "that", "this", "from", "what",
    "when", "where", "which", "who", "how", "does", "do", "did", "my", "our", "your", "me", "we", "you", "i", "can", "will", "would", "there", "their", "has", "have", "had",
];

/// A word without its ending ("ends", "ended", "ending" → "end").
fn stem(w: &str) -> String {
    for end in ["ing", "ed", "es", "s"] {
        if w.len() > end.len() + 2 && w.ends_with(end) && w.is_ascii() {
            return w[..w.len() - end.len()].to_string();
        }
    }
    w.to_string()
}

/// Lowercase words of two letters or more, without their endings and the
/// commonest ones (CJK characters one by one).
fn words_of(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for w in text.split(|c: char| !c.is_alphanumeric()) {
        if w.chars().any(|c| ('\u{3040}'..='\u{9fff}').contains(&c) || ('\u{ac00}'..='\u{d7af}').contains(&c)) {
            out.extend(w.chars().map(|c| c.to_string()));
        } else if w.chars().count() >= 2 {
            let low = w.to_lowercase();
            if !COMMON.contains(&low.as_str()) {
                out.push(stem(&low));
            }
        }
    }
    out
}

fn open_index(conn: &Connection, c: &Cipher) -> Index {
    let version = VERSION.load(Ordering::Relaxed);
    let names: HashMap<String, (String, String)> = files(conn, c, None).into_iter().map(|f| (f.id, (f.name, f.path))).collect();
    let mut entries = Vec::new();
    if let Ok(mut st) = conn.prepare("SELECT file_id, data, vec FROM doc_passages ORDER BY file_id, ord") {
        let it = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<Vec<u8>>>(2)?)));
        if let Ok(it) = it {
            for (file, data, vec) in it.flatten() {
                let Some(passage) = c.decrypt(&data).ok().and_then(|j| serde_json::from_str::<Passage>(&j).ok()) else { continue };
                let vec = vec.and_then(|v| c.open_bytes(&v).ok()).map(|b| bytes_vec(&b)).filter(|v| v.len() == DIMS);
                let list = words_of(&format!("{} {}", names.get(&file).map_or("", |n| n.0.as_str()), passage.text));
                let mut words: HashMap<String, u16> = HashMap::new();
                for w in &list {
                    *words.entry(w.clone()).or_default() += 1;
                }
                entries.push(Entry { file, passage, vec, words, len: list.len() });
            }
        }
    }
    let mut df: HashMap<String, usize> = HashMap::new();
    for e in &entries {
        for w in e.words.keys() {
            *df.entry(w.clone()).or_default() += 1;
        }
    }
    let avg_len = entries.iter().map(|e| e.len).sum::<usize>() as f32 / entries.len().max(1) as f32;
    Index { version, entries, df, avg_len, names }
}

fn loaded(state: &AppState, c: &Cipher) -> Arc<Index> {
    let now = VERSION.load(Ordering::Relaxed);
    let mut slot = INDEX.lock().unwrap();
    if let Some(i) = slot.as_ref().filter(|i| i.version == now) {
        return i.clone();
    }
    let index = Arc::new(open_index(&state.db.lock().unwrap(), c));
    *slot = Some(index.clone());
    index
}

/// A passage found for a question.
#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub file_id: String,
    pub name: String,
    pub path: String,
    pub place: Option<String>,
    pub text: String,
    pub score: f32,
}

/// The ranking by words (BM25).
fn bm25(index: &Index, query: &str) -> Vec<(usize, f32)> {
    let q: HashSet<String> = words_of(query).into_iter().collect();
    let n = index.entries.len() as f32;
    let (k1, b) = (1.2f32, 0.75f32);
    let mut out: Vec<(usize, f32)> = index
        .entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let s: f32 = q
                .iter()
                .filter_map(|w| {
                    let tf = *e.words.get(w)? as f32;
                    let df = *index.df.get(w).unwrap_or(&0) as f32;
                    let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                    Some(idf * tf * (k1 + 1.0) / (tf + k1 * (1.0 - b + b * e.len as f32 / index.avg_len.max(1.0))))
                })
                .sum();
            (s > 0.0).then_some((i, s))
        })
        .collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

/// Both rankings merged (reciprocal rank fusion), at most `per_file`
/// passages from one file, the best `limit`.
fn merged(index: &Index, by_meaning: &[(usize, f32)], by_words: &[(usize, f32)], limit: usize, per_file: usize) -> Vec<(usize, f32)> {
    let mut score: HashMap<usize, f32> = HashMap::new();
    for (rank, (i, _)) in by_meaning.iter().take(60).enumerate() {
        *score.entry(*i).or_default() += 1.0 / (60.0 + rank as f32);
    }
    for (rank, (i, _)) in by_words.iter().take(60).enumerate() {
        *score.entry(*i).or_default() += 1.0 / (60.0 + rank as f32);
    }
    let mut all: Vec<(usize, f32)> = score.into_iter().collect();
    all.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut per: HashMap<&str, usize> = HashMap::new();
    all.into_iter()
        .filter(|(i, _)| {
            let n = per.entry(index.entries[*i].file.as_str()).or_default();
            *n += 1;
            *n <= per_file
        })
        .take(limit)
        .collect()
}

/// The passages that best answer `query`.
pub async fn search(state: &Arc<AppState>, query: &str, limit: usize) -> Result<Vec<Hit>, String> {
    let c = state.work_cipher()?;
    let index = loaded(state, &c);
    if index.entries.is_empty() {
        return Ok(Vec::new());
    }
    let by_words = bm25(&index, query);
    let mut by_meaning = Vec::new();
    if model_ready(state) && index.entries.iter().any(|e| e.vec.is_some()) {
        match embedder(state).await {
            Ok(ep) => match embed(&ep, &[format!("{ASK}{query}")]).await {
                Ok(q) => {
                    by_meaning = index.entries.iter().enumerate().filter_map(|(i, e)| Some((i, e.vec.as_ref()?.iter().zip(&q[0]).map(|(a, b)| a * b).sum::<f32>()))).collect();
                    by_meaning.sort_by(|a, b| b.1.total_cmp(&a.1));
                }
                Err(e) => crate::applog::line("library", &format!("Searching by words only: {e}")),
            },
            Err(e) => crate::applog::line("library", &format!("Searching by words only: {e}")),
        }
    }
    Ok(merged(&index, &by_meaning, &by_words, limit.clamp(1, 20), 3)
        .into_iter()
        .map(|(i, s)| {
            let e = &index.entries[i];
            let (name, path) = index.names.get(&e.file).cloned().unwrap_or_default();
            Hit { file_id: e.file.clone(), name, path, place: e.passage.place.clone(), text: e.passage.text.clone(), score: s }
        })
        .collect())
}

/// A library file's name and whole text (read again from the file, else
/// from its passages), for study material.
pub fn file_text(state: &AppState, id: &str) -> Option<(String, String)> {
    let c = state.work_cipher().ok()?;
    let f = files(&state.db.lock().unwrap(), &c, None).into_iter().find(|f| f.id == id)?;
    let path = Path::new(&f.path);
    let text = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
        crate::docs::pdf_pages(path).ok().map(|p| p.join("\n\n"))
    } else {
        crate::docs::read_all(path).ok()
    };
    let text = text.filter(|t| !t.trim().is_empty()).or_else(|| {
        let conn = state.db.lock().unwrap();
        let mut st = conn.prepare("SELECT data FROM doc_passages WHERE file_id = ?1 ORDER BY ord").ok()?;
        let parts: Vec<String> = st.query_map([id], |r| r.get::<_, String>(0)).ok()?.flatten().filter_map(|d| c.decrypt(&d).ok()).filter_map(|j| serde_json::from_str::<Passage>(&j).ok()).map(|p| p.text).collect();
        Some(parts.join("\n\n"))
    })?;
    Some((f.name, text))
}

/// Whether there's anything in the library.
pub fn has_documents(state: &AppState) -> bool {
    state.db.lock().unwrap().query_row("SELECT EXISTS(SELECT 1 FROM doc_passages)", [], |r| r.get::<_, bool>(0)).unwrap_or(false)
}

/// The names of what's in the library, for the assistant's instructions.
pub fn summary(state: &AppState) -> Option<String> {
    let c = state.work_cipher().ok()?;
    let conn = state.db.lock().unwrap();
    let list = sources(&conn, &c);
    if list.is_empty() {
        return None;
    }
    let n: usize = conn.query_row("SELECT COUNT(*) FROM doc_files", [], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize;
    let names: Vec<String> = list.iter().take(8).map(|s| Path::new(&s.path).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| s.path.clone())).collect();
    Some(format!("{n} file(s), from {}", names.join(", ")))
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct SourceView {
    #[serde(flatten)]
    source: Source,
    name: String,
    files: usize,
    passages: usize,
    problems: usize,
    /// Files found by their words only (indexed before the model was here).
    words_only: usize,
}

#[derive(Serialize)]
pub struct LibraryView {
    sources: Vec<SourceView>,
    files: usize,
    passages: usize,
    progress: Option<Progress>,
    model_ready: bool,
    model_size: u64,
    kinds: &'static [&'static str],
}

fn view(state: &AppState) -> Result<LibraryView, String> {
    let c = state.work_cipher()?;
    let conn = state.db.lock().unwrap();
    let all = files(&conn, &c, None);
    let sources = sources(&conn, &c)
        .into_iter()
        .map(|s| {
            let mine: Vec<&DocFile> = all.iter().filter(|f| f.source_id == s.id).collect();
            SourceView {
                name: Path::new(&s.path).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| s.path.clone()),
                files: mine.iter().filter(|f| f.error.is_none()).count(),
                passages: mine.iter().map(|f| f.passages).sum(),
                problems: mine.iter().filter(|f| f.error.is_some()).count(),
                words_only: mine.iter().filter(|f| f.error.is_none() && !f.embedded).count(),
                source: s,
            }
        })
        .collect();
    drop(conn);
    Ok(LibraryView {
        sources,
        files: all.iter().filter(|f| f.error.is_none()).count(),
        passages: all.iter().map(|f| f.passages).sum(),
        progress: PROGRESS.lock().unwrap().clone(),
        model_ready: model_ready(state),
        model_size: state.catalog.embedding.as_ref().map_or(0, |f| f.size),
        kinds: KINDS,
    })
}

/// A drive, or Windows' and the programs' folders: too big, and not documents.
fn too_broad(p: &Path) -> bool {
    if p.parent().is_none() {
        return true;
    }
    let low = p.to_string_lossy().to_lowercase();
    ["WINDIR", "ProgramFiles", "ProgramFiles(x86)", "ProgramData"].iter().filter_map(std::env::var_os).any(|d| low.starts_with(&d.to_string_lossy().to_lowercase()))
}

fn start_index(state: &Arc<AppState>) {
    let st = state.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = index(&st).await {
            crate::applog::line("library", &format!("Indexing stopped: {e}"));
        }
    });
}

#[tauri::command]
pub fn docs_library(state: AppStateRef<'_>) -> Result<LibraryView, String> {
    view(&state)
}

/// Adds folders or files, then indexes them in the background.
#[tauri::command]
pub fn docs_add(state: AppStateRef<'_>, paths: Vec<String>) -> Result<LibraryView, String> {
    crate::features::require(&state, crate::features::Feature::Documents)?;
    let c = state.work_cipher()?;
    {
        let conn = state.db.lock().unwrap();
        let have: HashSet<String> = sources(&conn, &c).into_iter().map(|s| s.path.to_lowercase()).collect();
        for p in paths.iter().filter(|p| !have.contains(&p.to_lowercase())) {
            let path = Path::new(p);
            if !path.exists() {
                return Err(format!("{p} isn't there."));
            }
            let folder = path.is_dir();
            if folder && too_broad(path) {
                return Err(format!("{p} is too big a place to add: pick the folders with your documents in it."));
            }
            put_source(&conn, &c, &Source { id: uuid::Uuid::new_v4().simple().to_string(), path: p.clone(), folder, added_at: crate::db::now_ms(), scanned_at: None })?;
        }
        crate::db::log_action(&conn, "library", &format!("Added {} place(s) to the documents library", paths.len()));
    }
    start_index(state.inner());
    view(&state)
}

/// Takes a folder or file out of the library (the files themselves stay where they are).
#[tauri::command]
pub fn docs_remove(state: AppStateRef<'_>, id: String) -> Result<LibraryView, String> {
    let c = state.work_cipher()?;
    {
        let conn = state.db.lock().unwrap();
        for f in files(&conn, &c, Some(&id)) {
            forget_file(&conn, &f.id);
        }
        conn.execute("DELETE FROM doc_sources WHERE id = ?1", [&id]).map_err(|e| e.to_string())?;
        crate::db::log_action(&conn, "library", "Took a place out of the documents library");
    }
    VERSION.fetch_add(1, Ordering::Relaxed);
    view(&state)
}

/// Reads every place again for new and changed files.
#[tauri::command]
pub fn docs_update(state: AppStateRef<'_>) -> Result<LibraryView, String> {
    start_index(state.inner());
    view(&state)
}

#[tauri::command]
pub fn docs_stop() {
    STOP.store(true, Ordering::Relaxed);
}

/// A place's files and what happened to each.
#[tauri::command]
pub fn docs_files(state: AppStateRef<'_>, source: String) -> Result<Vec<DocFile>, String> {
    let c = state.work_cipher()?;
    Ok(files(&state.db.lock().unwrap(), &c, Some(&source)))
}

#[tauri::command]
pub async fn docs_search(state: AppStateRef<'_>, query: String, limit: Option<usize>) -> Result<Vec<Hit>, String> {
    let state = state.inner().clone();
    search(&state, &query, limit.unwrap_or(8)).await
}

/// Opens a library file in its app (documents only, never programs).
#[tauri::command(async)]
pub fn docs_open(state: AppStateRef<'_>, id: String) -> Result<(), String> {
    let c = state.work_cipher()?;
    let f = files(&state.db.lock().unwrap(), &c, None).into_iter().find(|f| f.id == id).ok_or("That file isn't in the library any more.")?;
    let p = PathBuf::from(&f.path);
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !KINDS.contains(&ext.as_str()) {
        return Err("Only documents can be opened from here.".into());
    }
    crate::open_with_system(p.as_os_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_cut_into_passages_with_places() {
        let para = "The quick brown fox jumps over the lazy dog. ".repeat(20);
        let text = format!("{para}\n\n{para}\n\n{para}");
        let p = cut(&text, Some("page 2"));
        assert!(p.len() >= 2, "{}", p.len());
        assert!(p.iter().all(|x| x.place.as_deref() == Some("page 2")));
        assert!(p.iter().all(|x| x.text.chars().count() <= PASSAGE * 2 + OVERLAP), "{:?}", p.iter().map(|x| x.text.len()).collect::<Vec<_>>());
        // Each starts with the end of the one before.
        assert!(p[1].text.starts_with("The quick brown fox"));
        assert!(cut("tiny", None).is_empty(), "too short to be a passage");
        // A single long run without stops is still cut.
        assert!(cut(&"a".repeat(5_000), None).len() >= 2);
    }

    #[test]
    fn sections_name_their_place() {
        let text = "## Slide 1\nWelcome to the plan\n## Slide 2\nBudget for the year is twelve thousand dollars in total\n## Sheet: Costs\nRent, 1200\n# Risks\nThe main risk is the weather in spring";
        let p = by_sections(text);
        let places: Vec<&str> = p.iter().filter_map(|x| x.place.as_deref()).collect();
        assert_eq!(places, ["slide 1", "slide 2", "sheet “Costs”", "“Risks”"], "{p:?}");
        assert!(p[1].text.contains("twelve thousand"));
    }

    fn index_of(texts: &[(&str, &str)]) -> Index {
        let entries: Vec<Entry> = texts
            .iter()
            .map(|(file, t)| {
                let list = words_of(t);
                let mut words = HashMap::new();
                for w in &list {
                    *words.entry(w.clone()).or_default() += 1;
                }
                Entry { file: file.to_string(), passage: Passage { text: t.to_string(), place: None }, vec: None, words, len: list.len() }
            })
            .collect();
        let mut df: HashMap<String, usize> = HashMap::new();
        for e in &entries {
            for w in e.words.keys() {
                *df.entry(w.clone()).or_default() += 1;
            }
        }
        let avg_len = entries.iter().map(|e| e.len).sum::<usize>() as f32 / entries.len() as f32;
        Index { version: 0, entries, df, avg_len, names: HashMap::new() }
    }

    #[test]
    fn words_find_passages_and_one_file_doesnt_crowd_the_rest() {
        let index = index_of(&[
            ("a", "The lease ends in March and the rent is 1200 dollars"),
            ("a", "The lease can be renewed for one more year"),
            ("a", "Pets are allowed under the lease with a deposit"),
            ("a", "The lease names the landlord as Smith"),
            ("b", "Our lease for the office starts in June"),
            ("c", "A recipe for bread with flour and water"),
        ]);
        let words = bm25(&index, "When does the lease end?");
        assert_eq!(words[0].0, 0, "{words:?}");
        assert!(!words.iter().any(|(i, _)| *i == 5), "the recipe has none of the words");
        let top = merged(&index, &[], &words, 5, 3);
        assert_eq!(top.iter().filter(|(i, _)| index.entries[*i].file == "a").count(), 3, "at most three from one file");
        assert!(top.iter().any(|(i, _)| index.entries[*i].file == "b"));
        // Meaning and words agreeing beat either alone.
        let meaning = vec![(4, 0.9), (0, 0.8)];
        let both = merged(&index, &meaning, &words, 2, 3);
        assert_eq!(both[0].0, 0, "{both:?}");
    }

    #[test]
    fn numbers_are_shortened_and_kept_exactly() {
        let v: Vec<f32> = (0..1024).map(|i| (i as f32).sin()).collect();
        let s = shortened(&v);
        assert_eq!(s.len(), DIMS);
        assert!((s.iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-4);
        assert_eq!(bytes_vec(&vec_bytes(&s)), s);
        assert_eq!(words_of("The rent: 1,200 dollars — 東京"), vec!["rent", "200", "dollar", "東", "京"]);
        assert_eq!(words_of("When does it end? It ends, ended"), vec!["end", "end", "end"]);
    }

    #[test]
    fn files_have_steady_ids() {
        assert_eq!(file_id(Path::new("C:/Docs/A.pdf")), file_id(Path::new("c:\\docs\\a.pdf")));
        assert_ne!(file_id(Path::new("C:/Docs/A.pdf")), file_id(Path::new("C:/Docs/B.pdf")));
    }
}
