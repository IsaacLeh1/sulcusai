// SPDX-License-Identifier: AGPL-3.0-only
//! SulcusAI's own logs on this PC, for finding out what went wrong:
//! `logs\app.log` (what it did: startup, models loaded, pictures made and on
//! which card, downloads, updates, errors) and `logs\crashes.log` (every
//! crash, whether or not crash reports are sent). They never hold what's
//! said in chats, prompts, notes, mail or file contents. The engines keep
//! their own output beside them (engine.log, media.log, speech-*.log).
//! Settings → Logs shows them; nothing here leaves the PC.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::Serialize;

use crate::AppStateRef;

static DIR: OnceLock<PathBuf> = OnceLock::new();
/// One writer at a time, so lines don't interleave.
static WRITING: Mutex<()> = Mutex::new(());
/// A log is started over (the old one kept as `.old.log`) past this size.
const MAX_BYTES: u64 = 2 * 1024 * 1024;

pub const APP_LOG: &str = "app.log";
pub const CRASH_LOG: &str = "crashes.log";

pub fn init(logs: &Path) {
    let _ = DIR.set(logs.to_path_buf());
}

fn append(name: &str, text: &str) {
    let Some(dir) = DIR.get() else { return };
    let _guard = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let path = dir.join(name);
    if path.metadata().is_ok_and(|m| m.len() > MAX_BYTES) {
        let old = dir.join(name.replace(".log", ".old.log"));
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(&path, &old);
    }
    let _ = std::fs::create_dir_all(dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(text.as_bytes());
    }
}

/// One line in app.log: "2026-10-17 10:22:05 [media] Made a picture…".
pub fn line(category: &str, message: &str) {
    let when = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let message = message.replace(['\r', '\n'], " ⏎ ");
    append(APP_LOG, &format!("{when} [{category}] {message}\n"));
}

/// A crash or failure, in crashes.log (and a line in app.log).
pub fn crash(what: &str, details: &str) {
    let when = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    append(CRASH_LOG, &format!("===== {when} · SulcusAI {} =====\n{what}\n{}\n\n", env!("CARGO_PKG_VERSION"), details.trim_end()));
    line("crash", what);
}

// ---------- what log lines mean ----------

/// Problems the engines report, by words in their log lines (lowercase),
/// most telling first, each with what it means and what to do.
const MEANINGS: &[(&[&str], &str)] = &[
    (
        &["erroroutofdevicememory", "out of device memory", "cudamalloc failed", "cuda error: out of memory", "out of memory", "failed to allocate", "unable to allocate", "allocation failed"],
        "Out of memory: the graphics card (or the PC) didn't have room for the model with its context window. A smaller version of the model (a lower quant such as Q4_K_M), a smaller context window, or closing apps that use the card (games, browsers, video) fixes it.",
    ),
    (
        &["bad_alloc", "not enough memory resources", "cannot allocate memory", "failed to mmap", "virtualalloc failed", "the paging file is too small"],
        "The PC ran out of memory (RAM): the model is too big for the memory that's free. Close other apps, pick a smaller version of the model, or a smaller context window.",
    ),
    (
        &["unknown model architecture", "unknown architecture", "unsupported model architecture"],
        "This engine doesn't know this kind of model yet. A newer engine may (SulcusAI updates it with the app); until then pick another model.",
    ),
    (
        &["invalid magic", "not within the file bounds", "failed to read tensor", "unexpected end of file", "corrupted", "tensor data is not"],
        "The model file couldn't be read: it's damaged, unfinished, or not a model file this engine reads. Download it again (Models, then the model, then Download again).",
    ),
    (
        &["invalid ggml type", "unsupported tensor type", "unknown ggml type"],
        "The model file uses a number format this engine can't read. Pick another version of the model (Q4_K_M or Q8_0 are read everywhere).",
    ),
    (&["split", "missing"], "The model comes in parts and one isn't there. Download it again so every part is."),
    (
        &["errordevicelost", "device lost", "vk_error_device_lost", "unspecified launch failure", "illegal memory access", "nvlddmkm"],
        "The graphics driver stopped responding while loading. Updating the graphics driver usually fixes it; restarting the PC clears it for now. Lowering the GPU limit at the top of the chat avoids the card.",
    ),
    (
        &["no vulkan devices", "ggml_vulkan: no devices", "failed to initialize vulkan", "vkcreateinstance", "no cuda-capable device", "cuda driver version is insufficient"],
        "The engine couldn't use the graphics card: its driver is missing or too old for it. Update the graphics driver (from NVIDIA, AMD or Intel), then try again; until then the model runs on the processor.",
    ),
    (&["access is denied", "permission denied"], "Windows (or an antivirus) blocked the engine from a file it needs. Allow SulcusAI in the antivirus, then try again."),
    (
        &["couldn't bind", "address already in use", "only one usage of each socket address"],
        "Another program is using the address the engine listens on. Try again; if it keeps happening, restart the PC.",
    ),
    (
        &["n_ctx_train", "exceeds the model", "context size exceeds", "context length exceeds"],
        "The context window asked for is bigger than this model takes. Choose a smaller one (click the meter at the top of the chat).",
    ),
    (&["cannot find the file", "no such file or directory", "failed to open"], "A file the engine needs isn't where it should be (moved, deleted, or on a drive that's not connected)."),
];

/// What a log line means, if it reports a known problem.
pub fn meaning(line: &str) -> Option<&'static str> {
    let low = line.to_lowercase();
    MEANINGS.iter().find(|(words, _)| if words == &["split", "missing"] { words.iter().all(|w| low.contains(w)) } else { words.iter().any(|w| low.contains(w)) }).map(|(_, m)| *m)
}

/// The most telling problem in a log's text: the first in `MEANINGS`'s
/// order found on any line.
pub fn diagnose(text: &str) -> Option<&'static str> {
    let found: Vec<&'static str> = text.lines().filter_map(meaning).collect();
    MEANINGS.iter().map(|(_, m)| *m).find(|m| found.contains(m))
}

/// Lines with what the known ones mean underneath.
pub fn annotated(text: &str) -> String {
    let mut out = String::new();
    let mut said: Vec<&str> = Vec::new();
    for l in text.lines() {
        out.push_str(l);
        out.push('\n');
        if let Some(m) = meaning(l).filter(|m| !said.contains(m)) {
            said.push(m);
            out.push_str(&format!("    -> means: {m}\n"));
        }
    }
    out
}

/// The end of a file (at most `bytes`), for a big engine log.
pub fn tail(path: &Path, bytes: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let len = f.metadata().map_or(0, |m| m.len());
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(bytes)));
    let mut b = Vec::new();
    let _ = f.read_to_end(&mut b);
    String::from_utf8_lossy(&b).into_owned()
}

// ---------- Settings → Logs ----------

#[derive(Serialize)]
pub struct LogFile {
    name: String,
    size: u64,
    modified: i64,
}

#[derive(Serialize)]
pub struct LogsView {
    /// The logs folder and the app's data folder.
    folder: String,
    data_folder: String,
    files: Vec<LogFile>,
}

fn files(dir: &Path) -> Vec<LogFile> {
    let mut out: Vec<LogFile> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            let modified = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as i64;
            Some(LogFile { name: e.file_name().to_string_lossy().into_owned(), size: m.len(), modified })
        })
        .collect();
    // The app's own logs first, then the newest.
    out.sort_by_key(|f| (f.name != APP_LOG, f.name != CRASH_LOG, -f.modified));
    out
}

#[tauri::command(async)]
pub fn logs_view(state: AppStateRef) -> LogsView {
    LogsView { folder: state.paths.logs.display().to_string(), data_folder: state.paths.data.display().to_string(), files: files(&state.paths.logs) }
}

/// The last `lines` lines of a log in the logs folder (nothing outside it).
#[tauri::command(async)]
pub fn read_log(state: AppStateRef, name: String, lines: Option<usize>) -> Result<String, String> {
    let file = Path::new(&name).file_name().filter(|f| f.to_string_lossy() == name && name.ends_with(".log")).ok_or("That isn't one of SulcusAI's logs.")?;
    let path = state.paths.logs.join(file);
    // The end of a big log is what matters: at most the last 400 KB, read
    // from there (an engine log can be 64 MB).
    let tail = {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = std::fs::File::open(&path).map_err(|_| "That log is empty or gone.".to_string())?;
        let len = f.metadata().map_or(0, |m| m.len());
        f.seek(SeekFrom::Start(len.saturating_sub(400 * 1024))).map_err(|e| e.to_string())?;
        let mut b = Vec::new();
        f.read_to_end(&mut b).map_err(|e| e.to_string())?;
        b
    };
    let text = String::from_utf8_lossy(&tail);
    let all: Vec<&str> = text.lines().collect();
    let keep = lines.unwrap_or(400).clamp(10, 5000);
    Ok(all[all.len().saturating_sub(keep)..].join("\n"))
}

/// A log line that reports a known problem, and what it means.
#[derive(Serialize)]
pub struct Meaning {
    line: String,
    meaning: &'static str,
}

/// What the problems in a log's last lines mean (each problem once, the
/// latest line that shows it), for Settings → Logs.
#[tauri::command(async)]
pub fn log_meanings(state: AppStateRef, name: String, lines: Option<usize>) -> Result<Vec<Meaning>, String> {
    let text = read_log(state, name, lines)?;
    let mut out: Vec<Meaning> = Vec::new();
    for l in text.lines().rev() {
        if let Some(m) = meaning(l) {
            if !out.iter().any(|x| x.meaning == m) {
                out.push(Meaning { line: l.chars().take(300).collect(), meaning: m });
            }
        }
    }
    Ok(out)
}

/// Opens the logs folder (or, with `data`, the app's data folder) in File Explorer.
#[tauri::command(async)]
pub fn open_app_folder(state: AppStateRef, data: Option<bool>) -> Result<(), String> {
    let dir = if data == Some(true) { state.paths.data.clone() } else { state.paths.logs.clone() };
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    crate::open_with_system(dir.as_os_str())
}

/// Removes the logs (one an engine still has open is emptied when it's done).
#[tauri::command(async)]
pub fn clear_logs(state: AppStateRef) -> LogsView {
    for f in files(&state.paths.logs) {
        let path = state.paths.logs.join(&f.name);
        if std::fs::remove_file(&path).is_err() {
            let _ = std::fs::OpenOptions::new().write(true).truncate(true).open(&path);
        }
    }
    line("app", "Cleared the logs");
    logs_view(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_lines_are_explained() {
        // A 122B model on a 16 GB card: llama.cpp's own words.
        let log = "llama_model_loader: loaded meta data with 44 key-value pairs\n\
                   ggml_vulkan: Device memory allocation of size 9437184000 failed.\n\
                   ggml_vulkan: Requested buffer size exceeds device memory allocation limit: ErrorOutOfDeviceMemory\n\
                   llama_model_load: error loading model: unable to allocate Vulkan0 buffer\n\
                   llama_model_load_from_file_impl: failed to load model";
        assert!(diagnose(log).is_some_and(|m| m.starts_with("Out of memory")), "{:?}", diagnose(log));
        assert!(diagnose("llama_model_load: error loading model: error loading model architecture: unknown model architecture: 'qwen9'").is_some_and(|m| m.contains("doesn't know this kind of model")));
        assert!(diagnose("gguf_init_from_file_impl: invalid magic characters: 'PK\u{3}\u{4}'").is_some_and(|m| m.contains("couldn't be read")));
        assert!(diagnose("llama_model_load: error loading model: missing split file model-00002-of-00003.gguf").is_some_and(|m| m.contains("in parts")));
        assert_eq!(diagnose("main: server is listening on http://127.0.0.1:50000\nload_tensors: offloaded 49/49 layers to GPU"), None);
        // Each known line gets its meaning underneath, once.
        let a = annotated(log);
        assert_eq!(a.matches("-> means:").count(), 1, "{a}");
    }

    #[test]
    fn lines_go_to_app_log_and_crashes_to_their_own() {
        let dir = std::env::temp_dir().join(format!("sulcus-logs-{}", uuid::Uuid::new_v4().simple()));
        init(&dir);
        if DIR.get() != Some(&dir) {
            return; // another test set the folder first
        }
        line("media", "Made a picture\nin 28 s");
        crash("Part of SulcusAI failed (a panic).", "index out of bounds");
        let app = std::fs::read_to_string(dir.join(APP_LOG)).unwrap();
        assert!(app.contains("[media] Made a picture ⏎ in 28 s"), "one line per entry: {app}");
        assert!(app.contains("[crash] Part of SulcusAI failed"));
        assert!(std::fs::read_to_string(dir.join(CRASH_LOG)).unwrap().contains("index out of bounds"));
        let listed: Vec<String> = files(&dir).into_iter().map(|f| f.name).collect();
        assert_eq!(listed, vec![APP_LOG.to_string(), CRASH_LOG.to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
