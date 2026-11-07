// SPDX-License-Identifier: AGPL-3.0-only
//! A diagnostics bundle: one zip the person saves and may attach to a
//! report. It holds the PC summary, the ends of the logs and any crash
//! reports waiting, all with personal data taken out (the user's folder and
//! name, keys and passwords, e-mail addresses, network addresses). Nothing
//! is sent: the person picks where it goes and reads it first if they like.

use std::io::Write;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::AppStateRef;

/// How much of each log goes in: the end is what explains a failure.
const LOG_TAIL: u64 = 256 * 1024;

static EMAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}").unwrap());
static IPV4: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}\b").unwrap());
static IPV6: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:[0-9a-fA-F]{1,4}:){3,7}[0-9a-fA-F]{1,4}\b").unwrap());
static URL_QUERY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(https?://[^\s?#]+)[?#][^\s]*").unwrap());

/// Personal data out of a text, on top of what crash reports remove.
pub fn clean(s: &str) -> String {
    let s = crate::crash::tidy(s);
    let s = EMAIL.replace_all(&s, "[e-mail]");
    let s = URL_QUERY.replace_all(&s, "$1?[removed]");
    // Loopback stays: it says something without saying who.
    let s = IPV4.replace_all(&s, |c: &regex::Captures| if c[0].starts_with("127.") { c[0].to_string() } else { "[address]".to_string() });
    IPV6.replace_all(&s, "[address]").into_owned()
}

fn add(zip: &mut zip::ZipWriter<std::fs::File>, name: &str, text: &str) -> Result<(), String> {
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file(name, opts).map_err(|e| e.to_string())?;
    zip.write_all(clean(text).as_bytes()).map_err(|e| e.to_string())
}

pub fn make(state: &crate::AppState, dest: &Path) -> Result<usize, String> {
    let out = std::fs::File::create(dest).map_err(|e| format!("Couldn't create {}: {e}", dest.display()))?;
    let mut zip = zip::ZipWriter::new(out);
    let mut n = 0;
    let summary = format!(
        "{}\nSafe mode: {}\nLow-end mode: {}\nMade: {}\n",
        crate::crash::about(state),
        crate::startup::safe_mode(),
        crate::startup::low_end_mode(state),
        chrono::Local::now().format("%Y-%m-%d %H:%M"),
    );
    add(&mut zip, "about.txt", &summary)?;
    n += 1;
    for dir in [state.paths.logs.clone(), state.paths.data.join("crashes")] {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
            if !p.is_file() || !matches!(ext, "log" | "txt" | "json") {
                continue;
            }
            let folder = dir.file_name().and_then(|x| x.to_str()).unwrap_or("logs");
            let name = format!("{folder}/{}", e.file_name().to_string_lossy());
            add(&mut zip, &name, &crate::applog::tail(&p, LOG_TAIL))?;
            n += 1;
        }
    }
    add(&mut zip, "engine.log", &crate::applog::tail(&state.paths.engine_log(), LOG_TAIL))?;
    zip.finish().map_err(|e| e.to_string())?;
    state.log("privacy", &format!("Saved a diagnostics bundle ({n} files, personal data removed) at {}", dest.display()));
    Ok(n + 1)
}

/// Saves the bundle where the person chose. Returns how many files it holds.
#[tauri::command]
pub async fn save_diagnostics(state: AppStateRef<'_>, path: String) -> Result<usize, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || make(&state, Path::new(&path))).await.map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personal_data_leaves_the_bundle() {
        let out = clean("mail a.b@example.com from 192.168.1.20 via https://x.test/p?token=abc, local 127.0.0.1");
        assert!(!out.contains("example.com") && out.contains("[e-mail]"), "{out}");
        assert!(!out.contains("192.168") && out.contains("127.0.0.1"), "{out}");
        assert!(!out.contains("token=abc"), "{out}");
    }
}
