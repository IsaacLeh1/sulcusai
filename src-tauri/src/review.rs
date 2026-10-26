// SPDX-License-Identifier: AGPL-3.0-only
//! Review my code: a file, a folder or a GitHub repository is read and
//! checked for what the person picks (bugs, security, cleanliness, style,
//! speed, tests, documentation, accessibility), by the model they pick or
//! the one best at code on this PC. Nothing is changed: the code is only
//! read, and the result is a report of findings with where each is.
//!
//! The code goes to the model in batches that fit it, every line numbered so
//! findings can say where. Serious findings are then checked once more
//! against their own lines and dropped when they don't hold up (small
//! models flag things that aren't there). A summary closes the report.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::Emitter;

use crate::{AppState, AppStateRef};

/// What a review can check, and what each means to the model.
pub const CHECKS: &[(&str, &str)] = &[
    ("bugs", "Functionality: bugs, wrong logic, unhandled errors and edge cases, things that will crash or give wrong results"),
    ("security", "Security: injection, unsafe input handling, secrets in code, weak authentication or crypto, path traversal, unsafe deserialization"),
    ("clean", "Cleanliness: readability, naming, duplication, dead code, overly long or tangled functions"),
    ("style", "Professionalism: consistent style and structure, conventions of the language, clear interfaces"),
    ("speed", "Performance: needless work, slow algorithms, repeated I/O or queries, memory waste"),
    ("tests", "Tests: important behavior without tests, tests that don't check anything"),
    ("docs", "Documentation: missing or misleading comments and docs on public parts"),
    ("access", "Accessibility (user interfaces): missing labels, keyboard use, contrast, screen readers"),
];

/// Kinds of files read as code.
const CODE: &[&str] = &[
    "rs", "py", "js", "jsx", "ts", "tsx", "mjs", "cjs", "java", "kt", "kts", "cs", "c", "h", "cpp", "cc", "hpp", "go", "rb", "php", "swift", "scala", "lua", "dart", "sh", "bash", "ps1", "sql",
    "html", "htm", "css", "scss", "vue", "svelte", "json", "yaml", "yml", "toml", "xml", "gradle", "r", "m", "pl", "ex", "exs", "erl", "hs", "fs", "clj", "zig", "nim", "md",
];
/// The most files and characters a review reads.
const MAX_FILES: usize = 400;
const MAX_CHARS: usize = 3_000_000;
/// The most serious findings checked twice.
const MAX_VERIFY: usize = 40;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Finding {
    pub check: String,
    /// "high", "medium" or "low".
    pub severity: String,
    pub file: String,
    pub line: Option<u32>,
    pub title: String,
    pub why: String,
    pub fix: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Report {
    pub id: String,
    pub created_at: i64,
    /// What was reviewed: a path or a repository's address.
    pub source: String,
    pub checks: Vec<String>,
    pub model: String,
    pub files: usize,
    pub lines: usize,
    pub findings: Vec<Finding>,
    /// Findings dropped when checked again.
    pub dropped: usize,
    pub summary: String,
    pub took_ms: u64,
    /// Files left out (too many, too big, or minified).
    pub skipped: usize,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ReviewRequest {
    /// A file or folder on this PC, or a GitHub address.
    pub source: String,
    pub checks: Vec<String>,
    /// Anything else to look at.
    pub notes: String,
    /// The model (None: the best at code on this PC).
    pub model: Option<String>,
}

static STOP: AtomicBool = AtomicBool::new(false);

fn say(state: &AppState, stage: &str, fraction: f64) {
    if let Some(app) = state.app.get() {
        let _ = app.emit("review:progress", json!({ "stage": stage, "fraction": fraction }));
    }
}

// ---------- reading the code ----------

/// "owner/repo" and the branch or folder from a GitHub address.
pub fn github(source: &str) -> Option<(String, String, Option<String>, Option<String>)> {
    let s = source.trim().trim_end_matches('/').trim_end_matches(".git");
    let rest = s.strip_prefix("https://github.com/").or_else(|| s.strip_prefix("http://github.com/")).or_else(|| s.strip_prefix("github.com/"))?;
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
        return None;
    }
    let ok = |p: &str| p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok(parts[0]) || !ok(parts[1]) {
        return None;
    }
    // ".../tree/<branch>/<folder…>"
    let (branch, folder) = if parts.get(2) == Some(&"tree") {
        (parts.get(3).map(|b| b.to_string()), (parts.len() > 4).then(|| parts[4..].join("/")))
    } else {
        (None, None)
    };
    Some((parts[0].to_string(), parts[1].to_string(), branch, folder))
}

/// A GitHub repository downloaded (its zip, public repositories only) and
/// unpacked into `into`: the folder its files are in.
async fn download_repo(state: &Arc<AppState>, source: &str, into: &Path) -> Result<PathBuf, String> {
    let (owner, repo, branch, folder) = github(source).ok_or("That isn't a GitHub repository's address.")?;
    let url = format!("https://github.com/{owner}/{repo}/archive/{}.zip", branch.as_deref().map(|b| format!("refs/heads/{b}")).unwrap_or_else(|| "HEAD".into()));
    let c = state.work_cipher()?;
    let cx = crate::outgoing::context(state, &c);
    if let Some(r) = crate::outgoing::review("fetch_page", &json!({ "url": url }), &cx) {
        crate::outgoing::record(state, &c, "", "fetch_page", &r, true);
    }
    let client = crate::net::external_client(state.settings().connectivity, crate::net::Purpose::Web, false)?;
    say(state, &format!("Downloading {owner}/{repo}"), 0.02);
    let resp = client.get(&url).send().await.map_err(|e| format!("Couldn't download it: {e}"))?;
    if resp.status().as_u16() == 404 {
        return Err("GitHub didn't find it. Private repositories can't be downloaded: clone it and pick its folder instead.".into());
    }
    if !resp.status().is_success() {
        return Err(format!("GitHub said {}.", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() > 300 * 1024 * 1024 {
        return Err("It's bigger than 300 MB: clone it and pick the folders to review.".into());
    }
    let dest = into.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
        for i in 0..z.len() {
            let mut f = z.by_index(i).map_err(|e| e.to_string())?;
            // Only paths inside the folder (enclosed_name refuses "../").
            let Some(rel) = f.enclosed_name() else { continue };
            let out = dest.join(rel);
            if f.is_dir() {
                std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            } else if f.size() < 5 * 1024 * 1024 {
                if let Some(p) = out.parent() {
                    std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
                }
                let mut w = std::fs::File::create(&out).map_err(|e| e.to_string())?;
                std::io::copy(&mut f, &mut w).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())??;
    // The zip holds one folder, "<repo>-<branch>".
    let top = std::fs::read_dir(into).map_err(|e| e.to_string())?.flatten().find(|e| e.path().is_dir()).map(|e| e.path()).ok_or("The download was empty.")?;
    Ok(match folder {
        Some(f) => top.join(f),
        None => top,
    })
}

/// A file that's code a person wrote: not minified or generated.
fn is_written(text: &str) -> bool {
    let lines = text.lines().count().max(1);
    let longest = text.lines().map(str::len).max().unwrap_or(0);
    longest < 1_000 && text.len() / lines < 200
}

/// The code files under `root` (or `root` itself), as (path shown, text),
/// and how many were left out.
fn gather(root: &Path) -> (Vec<(String, String)>, usize) {
    let is_code = |p: &Path| p.extension().and_then(|e| e.to_str()).is_some_and(|e| CODE.contains(&e.to_ascii_lowercase().as_str()));
    let paths: Vec<PathBuf> = if root.is_file() {
        vec![root.to_path_buf()]
    } else {
        ignore::WalkBuilder::new(root)
            .max_depth(Some(20))
            .build()
            .flatten()
            .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
            .map(|e| e.into_path())
            .filter(|p| is_code(p) && !p.components().any(|c| matches!(c.as_os_str().to_str(), Some("node_modules" | "target" | "dist" | "build" | "vendor" | ".venv" | "venv" | "__pycache__"))))
            .filter(|p| !p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".lock") || n.contains(".min.") || n == "package-lock.json"))
            .collect()
    };
    let mut out = Vec::new();
    let mut skipped = 0;
    let mut total = 0;
    for p in paths {
        let Ok(bytes) = std::fs::read(&p) else { continue };
        if bytes.len() > 512 * 1024 || bytes.contains(&0) {
            skipped += 1;
            continue;
        }
        let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
        if !is_written(&text) || out.len() >= MAX_FILES || total + text.len() > MAX_CHARS {
            skipped += 1;
            continue;
        }
        total += text.len();
        let shown = p.strip_prefix(root).ok().filter(|r| !r.as_os_str().is_empty()).unwrap_or(&p).to_string_lossy().replace('\\', "/");
        out.push((shown, text));
    }
    (out, skipped)
}

/// A file's lines numbered ("12| …"), from `from` (1-based).
fn numbered(text: &str, from: usize) -> String {
    text.lines().enumerate().map(|(i, l)| format!("{}| {l}", from + i)).collect::<Vec<_>>().join("\n")
}

/// The files in batches of at most `room` characters; a file bigger than a
/// batch is split by lines. Each piece: (file, first line, numbered text).
pub fn batches(files: &[(String, String)], room: usize) -> Vec<Vec<(String, usize, String)>> {
    let mut out: Vec<Vec<(String, usize, String)>> = Vec::new();
    let mut cur: Vec<(String, usize, String)> = Vec::new();
    let mut size = 0;
    for (name, text) in files {
        let lines: Vec<&str> = text.lines().collect();
        let mut start = 0;
        while start < lines.len().max(1) {
            let mut end = start;
            let mut len = 0;
            while end < lines.len() && len + lines[end].len() + 8 < room {
                len += lines[end].len() + 8;
                end += 1;
            }
            end = end.max(start + 1).min(lines.len().max(1));
            let piece = numbered(&lines[start.min(lines.len())..end.min(lines.len())].join("\n"), start + 1);
            if size + piece.len() > room && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
                size = 0;
            }
            size += piece.len();
            cur.push((name.clone(), start + 1, piece));
            start = end;
            if lines.is_empty() {
                break;
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Findings from a reply: `[{check, severity, file, line, title, why, fix}]`.
pub fn findings_from(reply: &str, files: &[String]) -> Vec<Finding> {
    let start = reply.find('[');
    let end = reply.rfind(']');
    let v: Value = match (start, end) {
        (Some(a), Some(b)) if b > a => serde_json::from_str(&reply[a..=b]).unwrap_or(Value::Null),
        _ => Value::Null,
    };
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    let s = |k: &str| f.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
                    let title = s("title");
                    if title.is_empty() {
                        return None;
                    }
                    let severity = match s("severity").to_lowercase().as_str() {
                        "high" | "critical" => "high",
                        "low" | "info" | "minor" => "low",
                        _ => "medium",
                    };
                    // The file as the batch named it (models shorten paths).
                    let named = s("file");
                    let file = files.iter().find(|x| **x == named).or_else(|| files.iter().find(|x| !named.is_empty() && (x.ends_with(&named) || named.ends_with(x.as_str())))).cloned().unwrap_or(named);
                    let line = f.get("line").and_then(|l| l.as_u64().or_else(|| l.as_str().and_then(|s| s.trim().parse().ok()))).map(|l| l as u32).filter(|l| *l > 0);
                    Some(Finding { check: s("check"), severity: severity.into(), file, line, title, why: s("why"), fix: s("fix") })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A check's name in a sentence ("security").
fn check_word(id: &str) -> &'static str {
    match id {
        "bugs" => "bugs",
        "security" => "security",
        "clean" => "cleanliness",
        "style" => "style",
        "speed" => "speed",
        "tests" => "tests",
        "docs" => "documentation",
        _ => "accessibility",
    }
}

/// The lines around a finding, numbered, to check it again.
fn around(files: &[(String, String)], f: &Finding) -> Option<String> {
    let (_, text) = files.iter().find(|(n, _)| *n == f.file)?;
    let at = f.line? as usize;
    let lines: Vec<&str> = text.lines().collect();
    let from = at.saturating_sub(25).max(1);
    let to = (at + 25).min(lines.len());
    (from <= to).then(|| numbered(&lines[from - 1..to].join("\n"), from))
}

// ---------- the review ----------

pub async fn review(state: &Arc<AppState>, req: &ReviewRequest) -> Result<Report, String> {
    let started = std::time::Instant::now();
    STOP.store(false, Ordering::Relaxed);
    let checks: Vec<&(&str, &str)> = CHECKS.iter().filter(|(id, _)| req.checks.iter().any(|c| c == id)).collect();
    if checks.is_empty() {
        return Err("Pick what to check.".into());
    }
    // The code: from GitHub into a scratch folder, or where it is.
    let scratch = std::env::temp_dir().join(format!("sulcusai-review-{}", uuid::Uuid::new_v4().simple()));
    let root = if github(&req.source).is_some() {
        std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
        download_repo(state, &req.source, &scratch).await?
    } else {
        let p = PathBuf::from(req.source.trim());
        if !p.exists() {
            return Err("Pick a file or folder, or paste a GitHub repository's address.".into());
        }
        p
    };
    say(state, "Reading the code", 0.05);
    let r = root.clone();
    let (files, skipped) = tokio::task::spawn_blocking(move || gather(&r)).await.map_err(|e| e.to_string())?;
    let result = async {
        if files.is_empty() {
            return Err("No code was found there.".to_string());
        }
        say(state, "Getting the model ready", 0.08);
        let model = req.model.clone().filter(|m| !m.is_empty()).or_else(|| crate::router::best_for(state, crate::router::Task::Coding).map(|b| b.0));
        let (ep, spec, _hold) = crate::endpoint_for(state, model.as_deref()).await?;
        if ep.cloud.is_some() {
            if let Ok(c) = state.cipher() {
                let me = crate::outgoing::context(state, &c).me;
                let r = crate::outgoing::review_cloud(&spec.name, &format!("Code to review: {} file(s) from {}", files.len(), req.source), "", "", "", &[], 0, false, &me);
                crate::outgoing::record(state, &c, "", "cloud_chat", &r, true);
            }
        }
        review_files(state, req, &checks, &files, skipped, &ep, &spec.name).await
    }
    .await;
    let _ = std::fs::remove_dir_all(&scratch);
    let mut report = result?;
    report.took_ms = started.elapsed().as_millis() as u64;
    Ok(report)
}

/// The review itself, with the model at `ep` (`name`, for the report).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn review_files(state: &Arc<AppState>, req: &ReviewRequest, checks: &[&(&str, &str)], files: &[(String, String)], skipped: usize, ep: &crate::engine::Endpoint, name: &str) -> Result<Report, String> {
    let room = ((ep.ctx as usize).saturating_sub(4_000) * 3).clamp(6_000, 120_000);
    let list = batches(files, room);
    let names: Vec<String> = files.iter().map(|f| f.0.clone()).collect();
    let extra = if req.notes.trim().is_empty() { String::new() } else { format!(" Also look at: {}.", req.notes.trim()) };
    let mut findings: Vec<Finding> = Vec::new();
    let n = list.len();
    // Each batch is read once for each check: one thing at a time, small
    // models find far more (with all at once, a 4B model missed a loop past
    // an array's end beside an injection).
    let steps = n * checks.len();
    for (i, batch) in list.iter().enumerate() {
        let shown: Vec<&str> = batch.iter().map(|b| b.0.as_str()).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        let code = batch.iter().map(|(f, from, text)| format!("=== {f} (from line {from}) ===\n{text}")).collect::<Vec<_>>().join("\n\n");
        for (k, (check, about)) in checks.iter().enumerate() {
            if STOP.load(Ordering::Relaxed) {
                return Err(crate::download::CANCELLED.into());
            }
            let step = i * checks.len() + k;
            say(state, &format!("Checking {} for {} ({} of {steps})", shown.join(", ").chars().take(60).collect::<String>(), check_word(check), step + 1), 0.1 + 0.65 * step as f64 / steps as f64);
            let ask = format!(
                "Review this code for one thing only: {about}.{extra}\n\nReport only real, specific problems you can point to in these lines; don't invent problems and say nothing about code you can't see. \
                 For each give its severity (high, medium or low), file (as named above), line number, a short title, why it's a problem, and how to fix it. \
                 Reply with only a JSON array (empty if there's nothing): [{{\"severity\": \"…\", \"file\": \"…\", \"line\": 0, \"title\": \"…\", \"why\": \"…\", \"fix\": \"…\"}}]\n\n{code}"
            );
            let msgs = vec![json!({"role": "system", "content": "You are a careful senior code reviewer. You read code and report problems; you never rewrite it."}), json!({"role": "user", "content": ask})];
            match crate::chat::complete(ep, msgs, json!({"temperature": 0.1}), 2_500).await {
                Ok(reply) => findings.extend(findings_from(&reply, &names).into_iter().map(|f| Finding { check: check.to_string(), ..f })),
                Err(e) => crate::applog::line("review", &format!("A batch wasn't reviewed for {check}: {e}")),
            }
        }
    }
    // The same line found by two checks is one problem: the more serious
    // finding is kept (the first on a tie).
    let rank = |s: &str| match s {
        "high" => 0,
        "medium" => 1,
        _ => 2,
    };
    let mut best: HashMap<(String, u32), usize> = HashMap::new();
    for (i, f) in findings.iter().enumerate() {
        if let Some(line) = f.line {
            let slot = best.entry((f.file.clone(), line)).or_insert(i);
            if rank(&f.severity) < rank(&findings[*slot].severity) {
                *slot = i;
            }
        }
    }
    let keep: std::collections::HashSet<usize> = best.values().copied().collect();
    let mut i = 0;
    findings.retain(|f| {
        let k = f.line.is_none() || keep.contains(&i);
        i += 1;
        k
    });
    // The serious ones checked again against their own lines.
    let mut dropped = 0;
    let mut kept = Vec::new();
    let to_check: Vec<usize> = (0..findings.len()).filter(|i| findings[*i].severity != "low").take(MAX_VERIFY).collect();
    for (k, f) in findings.iter().enumerate() {
        if !to_check.contains(&k) {
            kept.push(f.clone());
            continue;
        }
        if STOP.load(Ordering::Relaxed) {
            return Err(crate::download::CANCELLED.into());
        }
        say(state, &format!("Checking the findings ({} of {})", to_check.iter().position(|x| *x == k).unwrap_or(0) + 1, to_check.len()), 0.78 + 0.15 * k as f64 / findings.len().max(1) as f64);
        let Some(lines) = around(files, f) else {
            kept.push(f.clone());
            continue;
        };
        let ask = format!(
            "A reviewer says this about {} line {}: “{}”: {}\n\nLooking only at the code below, is it a real problem? Reply with only JSON: {{\"real\": true or false, \"reason\": \"one sentence\"}}\n\n{lines}",
            f.file,
            f.line.unwrap_or(0),
            f.title,
            f.why
        );
        let real = crate::chat::complete(ep, vec![json!({"role": "user", "content": ask})], json!({"temperature": 0}), 200)
            .await
            .ok()
            .and_then(|r| {
                let a = r.find('{')?;
                let b = r.rfind('}')?;
                serde_json::from_str::<Value>(&r[a..=b]).ok()
            })
            .and_then(|v| v.get("real").and_then(Value::as_bool))
            .unwrap_or(true);
        if real {
            kept.push(f.clone());
        } else {
            dropped += 1;
        }
    }
    let order = |s: &str| match s {
        "high" => 0,
        "medium" => 1,
        _ => 2,
    };
    kept.sort_by(|a, b| order(&a.severity).cmp(&order(&b.severity)).then(a.file.cmp(&b.file)).then(a.line.cmp(&b.line)));
    say(state, "Writing the summary", 0.95);
    let counts: HashMap<&str, usize> = kept.iter().fold(HashMap::new(), |mut m, f| {
        *m.entry(f.severity.as_str()).or_default() += 1;
        m
    });
    let summary = if kept.is_empty() {
        "Nothing that needs fixing was found for what was checked.".to_string()
    } else {
        let top = kept.iter().take(15).map(|f| format!("- [{}] {} ({}): {}", f.severity, f.title, f.file, f.why)).collect::<Vec<_>>().join("\n");
        let ask = format!("Write a short summary (3 to 5 sentences) of this code review for the code's author: what's most important to fix first, and what's done well if anything. Plain text, no lists.\n\nFindings:\n{top}");
        crate::chat::complete(ep, vec![json!({"role": "user", "content": ask})], json!({"temperature": 0.3}), 400).await.unwrap_or_default()
    };
    let lines = files.iter().map(|f| f.1.lines().count()).sum();
    state.log("review", &format!("Reviewed {} file(s) with {name}: {} high, {} medium, {} low, {dropped} dropped when checked again", files.len(), counts.get("high").unwrap_or(&0), counts.get("medium").unwrap_or(&0), counts.get("low").unwrap_or(&0)));
    Ok(Report {
        id: uuid::Uuid::new_v4().simple().to_string(),
        created_at: crate::db::now_ms(),
        source: req.source.trim().to_string(),
        checks: checks.iter().map(|c| c.0.to_string()).collect(),
        model: name.to_string(),
        files: files.len(),
        lines,
        findings: kept,
        dropped,
        summary: summary.trim().to_string(),
        took_ms: 0,
        skipped,
    })
}

/// The report as Markdown, to save or share.
pub fn markdown(r: &Report) -> String {
    let mut md = format!("# Code review: {}\n\n{} files, {} lines, checked by {}.\n\n{}\n\n", r.source, r.files, r.lines, r.model, r.summary);
    for sev in ["high", "medium", "low"] {
        let list: Vec<&Finding> = r.findings.iter().filter(|f| f.severity == sev).collect();
        if list.is_empty() {
            continue;
        }
        md.push_str(&format!("## {} ({})\n\n", match sev { "high" => "High", "medium" => "Medium", _ => "Low" }, list.len()));
        for f in list {
            let at = f.line.map(|l| format!(":{l}")).unwrap_or_default();
            md.push_str(&format!("- **{}** ({}, `{}{at}`)\n  {}\n  Fix: {}\n", f.title, f.check, f.file, f.why, f.fix));
        }
        md.push('\n');
    }
    md
}

// ---------- kept ----------

fn put(conn: &rusqlite::Connection, c: &crate::crypto::Cipher, r: &Report) -> Result<(), String> {
    let data = c.encrypt(&serde_json::to_string(r).map_err(|e| e.to_string())?);
    conn.execute("INSERT INTO code_reviews (id, data, created_at) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET data = excluded.data", rusqlite::params![r.id, data, r.created_at])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Serialize)]
pub struct ReviewSetup {
    /// The model used when none is picked: the best at code here.
    best: Option<(String, String)>,
    checks: Vec<(&'static str, &'static str)>,
}

#[tauri::command]
pub fn review_setup(state: AppStateRef<'_>) -> ReviewSetup {
    ReviewSetup { best: crate::router::best_for(&state, crate::router::Task::Coding), checks: CHECKS.to_vec() }
}

#[tauri::command]
pub fn reviews(state: AppStateRef<'_>) -> Result<Vec<Report>, String> {
    let c = state.work_cipher()?;
    let conn = state.db.lock().unwrap();
    let mut st = conn.prepare("SELECT data FROM code_reviews ORDER BY created_at DESC").map_err(|e| e.to_string())?;
    let rows = st.query_map([], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
    Ok(rows.flatten().filter_map(|d| c.decrypt(&d).ok()).filter_map(|j| serde_json::from_str(&j).ok()).collect())
}

/// Reviews code ("review:progress" as it goes) and keeps the report.
#[tauri::command]
pub async fn review_start(state: AppStateRef<'_>, request: ReviewRequest) -> Result<Report, String> {
    let state = state.inner().clone();
    let report = review(&state, &request).await?;
    let c = state.work_cipher()?;
    put(&state.db.lock().unwrap(), &c, &report)?;
    Ok(report)
}

#[tauri::command]
pub fn review_stop() {
    STOP.store(true, Ordering::Relaxed);
}

#[tauri::command]
pub fn review_delete(state: AppStateRef<'_>, id: String) -> Result<(), String> {
    state.db.lock().unwrap().execute("DELETE FROM code_reviews WHERE id = ?1", [&id]).map_err(|e| e.to_string())?;
    Ok(())
}

/// Saves a report as Markdown where the person picks.
#[tauri::command(async)]
pub fn review_save(state: AppStateRef<'_>, id: String, path: String) -> Result<(), String> {
    let r = reviews(state)?.into_iter().find(|r| r.id == id).ok_or("That review is no longer here.")?;
    std::fs::write(path, markdown(&r)).map_err(|e| e.to_string())
}

/// The code files under a folder (for tests).
#[cfg(test)]
pub fn gather_for_test(root: &Path) -> Vec<(String, String)> {
    gather(root).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_addresses_are_understood() {
        assert_eq!(github("https://github.com/IsaacLeh1/sulcusai"), Some(("IsaacLeh1".into(), "sulcusai".into(), None, None)));
        assert_eq!(github("github.com/a/b.git/"), Some(("a".into(), "b".into(), None, None)));
        assert_eq!(github("https://github.com/a/b/tree/dev/src/app"), Some(("a".into(), "b".into(), Some("dev".into()), Some("src/app".into()))));
        assert_eq!(github("https://gitlab.com/a/b"), None);
        assert_eq!(github("https://github.com/a"), None);
        assert_eq!(github("https://github.com/a/b c"), None);
    }

    #[test]
    fn code_is_batched_with_line_numbers() {
        let files = vec![("a.py".to_string(), "x = 1\ny = 2\n".to_string()), ("b.py".to_string(), (1..=400).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n"))];
        let b = batches(&files, 1_000);
        assert!(b.len() > 1, "the big file is split");
        assert_eq!(b[0][0].2, "1| x = 1\n2| y = 2");
        let pieces: Vec<&(String, usize, String)> = b.iter().flatten().filter(|p| p.0 == "b.py").collect();
        assert_eq!(pieces[0].1, 1);
        assert!(pieces[1].2.starts_with(&format!("{}| line {}", pieces[1].1, pieces[1].1)), "the next piece goes on numbering");
        assert!(b.iter().all(|batch| batch.iter().map(|p| p.2.len()).sum::<usize>() <= 1_000 + 40));
    }

    #[test]
    fn replies_become_findings() {
        let files = vec!["src/app/login.py".to_string()];
        let f = findings_from(
            r#"Here: [{"check": "security", "severity": "Critical", "file": "login.py", "line": "12", "title": "SQL built from input", "why": "Injection", "fix": "Use parameters"}, {"title": ""}]"#,
            &files,
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, "high");
        assert_eq!(f[0].file, "src/app/login.py", "matched to the file's full name");
        assert_eq!(f[0].line, Some(12));
        assert!(markdown(&Report { findings: f, source: "x".into(), ..Default::default() }).contains("`src/app/login.py:12`"));
    }

    #[test]
    fn minified_code_is_left_out() {
        assert!(is_written("fn main() {\n    println!(\"hi\");\n}\n"));
        assert!(!is_written(&"a".repeat(5_000)));
    }
}
