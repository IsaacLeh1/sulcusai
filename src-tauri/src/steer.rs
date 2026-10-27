// SPDX-License-Identifier: AGPL-3.0-only
//! Steering (the Lens): a direction in a model's thinking, found from pairs
//! of opposite instructions ("Act like someone cheerful" against "…gloomy")
//! by llama.cpp's control-vector maker (`llama-cvector-generator`: the
//! difference in each layer's output between the two, boiled down to its
//! main direction), then added to the model's layers while it answers
//! (`--control-vector-scaled`), stronger or weaker, or the other way with a
//! negative strength. The model's file is never changed: a direction is a
//! small file of its own, and a steered model is a choice in the chat's
//! model menu ("steer:<id>"), like a taught one.
//!
//! The maker isn't in llama.cpp's release download, so SulcusAI builds it
//! at the engine's own release (`scripts/lens`) and fetches it the first
//! time (catalog `lens.cvector`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::catalog::Asset;
use crate::crypto::Cipher;
use crate::paths::Paths;
use crate::{db, engine, AppState, AppStateRef};

const KEY: &str = "lens_directions";
const IDS: &str = "lens_direction_ids";
/// The maker's program.
const EXE: &str = "llama-cvector-generator.exe";

/// The control-vector maker, from the catalog.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LensTools {
    /// The llama.cpp release it was built at (the engine's).
    pub build: String,
    pub cvector: Asset,
}

/// What the engine is started with to steer a model.
#[derive(Debug, Clone, PartialEq)]
pub struct Steer {
    pub file: PathBuf,
    pub strength: f32,
    /// The layers it's added to (first, last).
    pub from: u32,
    pub to: u32,
}

impl Steer {
    /// The engine's flags: the direction at its strength (a copy scaled to
    /// it: the engine's own FILE:STRENGTH form splits a Windows path at its
    /// drive's colon), and the layers.
    pub fn args(&self) -> Vec<OsString> {
        let file = scaled(&self.file, self.strength).unwrap_or_else(|_| self.file.clone());
        vec!["--control-vector".into(), file.into(), "--control-vector-layer-range".into(), self.from.to_string().into(), self.to.to_string().into()]
    }
}

/// A copy of a direction's file with its vectors multiplied by `strength`
/// ("<id>@+0.80.gguf" beside it), made once per strength.
pub fn scaled(file: &Path, strength: f32) -> Result<PathBuf, String> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let out = file.with_file_name(format!("{stem}@{strength:+.2}.gguf"));
    if out.is_file() {
        return Ok(out);
    }
    let tensors = gguf::tensors(file).map_err(|e| format!("Couldn't read the direction: {e}"))?;
    let tmp = out.with_extension("part");
    std::fs::copy(file, &tmp).map_err(|e| e.to_string())?;
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(&tmp).map_err(|e| e.to_string())?;
    for t in tensors.iter().filter(|t| t.name.starts_with("direction.")) {
        // The maker writes them as 32-bit floats.
        if t.kind != 0 {
            return Err(format!("{} isn't stored as 32-bit numbers.", t.name));
        }
        let n = t.dims.iter().product::<u64>() as usize;
        let mut buf = vec![0u8; n * 4];
        f.seek(SeekFrom::Start(t.start)).map_err(|e| e.to_string())?;
        f.read_exact(&mut buf).map_err(|e| e.to_string())?;
        for c in buf.chunks_exact_mut(4) {
            let v = f32::from_le_bytes([c[0], c[1], c[2], c[3]]) * strength;
            c.copy_from_slice(&v.to_le_bytes());
        }
        f.seek(SeekFrom::Start(t.start)).map_err(|e| e.to_string())?;
        f.write_all(&buf).map_err(|e| e.to_string())?;
    }
    drop(f);
    std::fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// A direction made for one model.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Direction {
    pub id: String,
    /// "Cheerful", as the person named it.
    pub name: String,
    pub model_id: String,
    /// The two sides, in the person's words ("cheerful", "gloomy").
    pub toward: String,
    pub away: String,
    /// How strongly it's added in chats (negative: the other way).
    pub strength: f32,
    pub from: u32,
    pub to: u32,
    pub layers: u32,
    pub pairs: usize,
    pub created_at: i64,
}

impl Direction {
    pub fn steer(&self, paths: &Paths) -> Steer {
        Steer { file: file_of(paths, &self.id), strength: self.strength, from: self.from, to: self.to }
    }
}

/// Where directions are kept, a file each.
fn dir(paths: &Paths) -> PathBuf {
    paths.models.join("lens-steer")
}

fn file_of(paths: &Paths, id: &str) -> PathBuf {
    dir(paths).join(format!("{id}.gguf"))
}

pub fn list(conn: &Connection, c: &Cipher) -> Vec<Direction> {
    let mut all: Vec<Direction> = db::get::<String>(conn, KEY).and_then(|s| c.decrypt(&s).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    all.sort_by_key(|d| std::cmp::Reverse(d.created_at));
    all
}

fn store(conn: &Connection, c: &Cipher, all: &[Direction]) -> Result<(), String> {
    db::set(conn, KEY, &c.encrypt(&serde_json::to_string(all).map_err(|e| e.to_string())?))?;
    // Their ids alone, unencrypted, so a chat's model can be checked while locked.
    db::set(conn, IDS, &all.iter().map(|d| d.id.clone()).collect::<Vec<_>>())
}

/// A direction with this id is kept (for "steer:<id>" in a chat's model).
pub fn exists(conn: &Connection, id: &str) -> bool {
    db::get::<Vec<String>>(conn, IDS).unwrap_or_default().iter().any(|d| d == id)
}

pub fn get(conn: &Connection, c: &Cipher, id: &str) -> Option<Direction> {
    list(conn, c).into_iter().find(|d| d.id == id)
}

/// "steer:<id>" in the model menu.
pub fn parse_ref(id: &str) -> Option<&str> {
    id.strip_prefix("steer:")
}

/// The strength kept within what still reads as language (beyond about 2
/// most models only repeat themselves).
pub fn clamp_strength(s: f32) -> f32 {
    if s.is_finite() { s.clamp(-2.0, 2.0) } else { 0.0 }
}

/// The layers a direction is added to: from a quarter of the way in to
/// two before the end. The first layers still read the words, and the
/// last ones pick them; steering works in between (llama.cpp's own tip is
/// layers past 10).
pub fn layer_range(layers: u32) -> (u32, u32) {
    if layers < 4 {
        return (1, layers.max(1));
    }
    ((layers / 4).max(1), layers - 2)
}

/// Questions the pairs answer: everyday ones, so the direction is about how
/// it answers, not what about.
const QUESTIONS: [&str; 8] = [
    "Who are you?",
    "How was your day?",
    "What do you think of the weather lately?",
    "Tell me about your favourite place.",
    "What are you working on?",
    "Can you describe a typical morning?",
    "What's the best way to spend a weekend?",
    "What do you think about people?",
];

/// How the answers start, the same on both sides of a pair, so only the
/// instruction differs.
const STARTS: [&str; 8] = ["I", "Well,", "Honestly,", "The", "To be", "My", "It's", "When"];

/// The instruction for one side ("Act like someone who is cheerful").
fn instruction(side: &str) -> String {
    let s = side.trim().trim_end_matches('.');
    let low = s.to_lowercase();
    if ["act ", "be ", "you are", "answer ", "write ", "speak ", "talk "].iter().any(|p| low.starts_with(p)) {
        format!("{s}.")
    } else {
        format!("Act like someone who is {s}.")
    }
}

/// One line of the maker's prompt files: its backslashes and line breaks
/// written as escapes (it reads one prompt a line).
pub fn line(prompt: &str) -> String {
    prompt.replace('\r', "").replace('\\', "\\\\").replace('\n', "\\n")
}

/// The pairs: every question under each side's instruction, with the same
/// start of an answer on both. `templated` turns (system, user) into the
/// model's own prompt up to where it answers.
pub fn pairs(toward: &str, away: &str, templated: impl Fn(&str, &str) -> String) -> (Vec<String>, Vec<String>) {
    let (t, a) = (instruction(toward), instruction(away));
    let mut pos = Vec::new();
    let mut neg = Vec::new();
    for (i, q) in QUESTIONS.iter().enumerate() {
        for s in STARTS.iter().skip(i % 2).step_by(2) {
            pos.push(line(&format!("{}{s}", templated(&t, q))));
            neg.push(line(&format!("{}{s}", templated(&a, q))));
        }
    }
    (pos, neg)
}

/// The maker's arguments.
pub fn maker_args(gguf: &Path, pos: &Path, neg: &Path, out: &Path, on_card: bool, threads: usize) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["-m".into(), gguf.into(), "--positive-file".into(), pos.into(), "--negative-file".into(), neg.into(), "-o".into(), out.into()];
    a.extend(["-ngl".into(), if on_card { "99" } else { "0" }.into()]);
    a.extend(["--pca-batch".into(), "16".into(), "--pca-iter".into(), "512".into()]);
    if threads > 0 {
        a.extend(["-t".into(), threads.to_string().into()]);
    }
    a
}

/// How far the maker is, from its log: (pair, of).
pub fn progress_of(line: &str) -> Option<(u32, u32)> {
    let rest = line.split("Evaluating prompt[").nth(1)?;
    let (n, of) = rest.split(']').next()?.split_once('/')?;
    Some((n.trim().parse().ok()?, of.trim().parse().ok()?))
}

/// The model's prompt for (system, user), up to where it answers.
async fn templated(ep: &engine::Endpoint, system: &str, user: &str) -> String {
    use serde_json::{json, Value};
    let body = json!({ "messages": [{ "role": "system", "content": system }, { "role": "user", "content": user }], "chat_template_kwargs": { "enable_thinking": false } });
    let r = crate::net::local_client().post(ep.url("/apply-template")).bearer_auth(&ep.key).json(&body).timeout(std::time::Duration::from_secs(30)).send().await;
    match r {
        Ok(r) => r.json::<Value>().await.ok().and_then(|v| v.get("prompt").and_then(Value::as_str).map(str::to_string)).unwrap_or_else(|| format!("{system}\n\n{user}\n\n")),
        Err(_) => format!("{system}\n\n{user}\n\n"),
    }
}

/// Makes a direction for a model from its two sides, and keeps it.
pub async fn make(state: &Arc<AppState>, model_id: Option<&str>, name: &str, toward: &str, away: &str, progress: impl Fn(&str, f64)) -> Result<Direction, String> {
    if toward.trim().is_empty() || away.trim().is_empty() {
        return Err("Say both sides: what it should lean toward, and away from.".into());
    }
    let tools = state.catalog.lens.clone().ok_or("Steering isn't available on this system yet.")?;
    let (id, gguf) = crate::lens::model_for(state, model_id)?;
    if parse_ref(&id).is_some() || crate::finetune::parse_ref(&id).is_some() {
        return Err("Pick the model itself, not a steered or taught one.".into());
    }
    let meta = gguf::read(&gguf).map_err(|e| format!("Couldn't read the model's file: {e}"))?;
    let arch = meta.get("general.architecture").and_then(|v| v.str().map(str::to_string)).unwrap_or_default();
    let layers = meta.get(&format!("{arch}.block_count")).and_then(gguf::Value::int).unwrap_or(0).max(0) as u32;
    if layers < 2 {
        return Err("This model's file doesn't say how many layers it has.".into());
    }

    // The pairs, in the model's own prompt format (its engine writes it).
    progress("Writing the pairs", 0.02);
    let (pos, neg) = {
        let (ep, _spec, _hold) = crate::endpoint_for(state, Some(&id)).await?;
        let mut prompts = std::collections::HashMap::new();
        for side in [instruction(toward), instruction(away)] {
            for q in QUESTIONS {
                prompts.insert((side.clone(), q.to_string()), templated(&ep, &side, q).await);
            }
        }
        pairs(toward, away, |s, q| prompts.get(&(s.to_string(), q.to_string())).cloned().unwrap_or_default())
    };
    // The chat model leaves the card to the maker (it loads the model too).
    progress("Freeing the graphics card", 0.08);
    let busy = !state.generations.lock().unwrap().is_empty();
    if busy {
        return Err("A chat is answering. Try again when it's done.".into());
    }
    state.engine.lock().await.stop().await;

    let exe = engine::ensure_unpacked(&state.paths, &format!("lens-tools-{}", tools.build), &tools.cvector, EXE, &AtomicBool::new(false), |r, all| progress("Getting the direction maker (once)", 0.08 + 0.04 * r as f64 / all.max(1) as f64)).await?;
    let work = std::env::temp_dir().join(format!("sulcusai-steer-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    struct Gone(PathBuf);
    impl Drop for Gone {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _gone = Gone(work.clone());
    let (pf, nf) = (work.join("toward.txt"), work.join("away.txt"));
    std::fs::write(&pf, pos.join("\n")).map_err(|e| e.to_string())?;
    std::fs::write(&nf, neg.join("\n")).map_err(|e| e.to_string())?;
    let new_id = crate::media::store::new_id();
    std::fs::create_dir_all(dir(&state.paths)).map_err(|e| e.to_string())?;
    let out = file_of(&state.paths, &new_id);
    let on_card = state.budget().vram > 0;
    let mut args = maker_args(&gguf, &pf, &nf, &out, on_card, state.limits().threads);
    // On the dedicated card, as the chat engine is (not integrated graphics too).
    let devices = if on_card { engine::dedicated_devices(&exe, false).await } else { Vec::new() };
    if !devices.is_empty() {
        args.extend(["--device".into(), devices.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>().join(",").into()]);
    }
    let n = pos.len() as f64;
    let log = state.paths.logs.join("lens-steer.log");
    crate::media::proc::run_env(
        crate::media::proc::Run { exe: &exe, args, cwd: &work, log: &log, low_priority: state.limits().low_priority, job: state.job() },
        &[],
        &AtomicBool::new(false),
        |l| {
            if let Some((k, _)) = progress_of(l) {
                progress("Reading each pair both ways", 0.12 + 0.8 * k as f64 / n.max(1.0));
            } else if l.contains("Done evaluate prompts") {
                progress("Finding the direction", 0.93);
            }
        },
    )
    .await?;
    if !out.is_file() {
        return Err("The direction maker didn't write its file. See lens-steer.log in Settings › Logs.".into());
    }
    let (from, to) = layer_range(layers);
    let d = Direction {
        id: new_id,
        name: if name.trim().is_empty() { toward.trim().to_string() } else { name.trim().to_string() }.chars().take(40).collect(),
        model_id: id,
        toward: toward.trim().chars().take(200).collect(),
        away: away.trim().chars().take(200).collect(),
        strength: 0.8,
        from,
        to,
        layers,
        pairs: pos.len(),
        created_at: db::now_ms(),
    };
    let c = state.work_cipher()?;
    let conn = state.db.lock().unwrap();
    let mut all = list(&conn, &c);
    all.push(d.clone());
    store(&conn, &c, &all)?;
    db::log_action(&conn, "model", &format!("Made a steering direction ({} pairs, layers {from}-{to})", d.pairs));
    Ok(d)
}

// ---------- commands ----------

#[tauri::command]
pub async fn lens_directions(state: AppStateRef<'_>) -> Result<Vec<Direction>, String> {
    let c = state.cipher()?;
    Ok(list(&state.db.lock().unwrap(), &c))
}

#[tauri::command]
pub async fn lens_make_direction(app: tauri::AppHandle, state: AppStateRef<'_>, model_id: Option<String>, name: String, toward: String, away: String) -> Result<Direction, String> {
    use tauri::Emitter;
    make(state.inner(), model_id.as_deref(), &name, &toward, &away, |stage, f| {
        app.emit("lens:steer", serde_json::json!({ "stage": stage, "fraction": f })).ok();
    })
    .await
}

/// Changes a direction's strength or layers (the engine starts again with
/// them at its next reply).
#[tauri::command]
pub async fn lens_set_direction(state: AppStateRef<'_>, id: String, strength: f32, from: Option<u32>, to: Option<u32>) -> Result<Direction, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut all = list(&conn, &c);
    let d = all.iter_mut().find(|d| d.id == id).ok_or("That direction is no longer here.")?;
    d.strength = clamp_strength(strength);
    if let (Some(f), Some(t)) = (from, to) {
        let (f, t) = (f.clamp(1, d.layers.max(1)), t.clamp(1, d.layers.max(1)));
        (d.from, d.to) = (f.min(t), f.max(t));
    }
    let out = d.clone();
    store(&conn, &c, &all)?;
    Ok(out)
}

#[tauri::command]
pub async fn lens_delete_direction(state: AppStateRef<'_>, id: String) -> Result<(), String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut all = list(&conn, &c);
    all.retain(|d| d.id != id);
    store(&conn, &c, &all)?;
    if !id.is_empty() && id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-') {
        // The direction and its scaled copies.
        for e in std::fs::read_dir(dir(&state.paths)).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n == format!("{id}.gguf") || n.starts_with(&format!("{id}@")) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(())
}

use crate::gguf;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_differ_only_in_the_instruction() {
        let (p, n) = pairs("cheerful", "gloomy", |s, q| format!("<s>{s}</s><u>{q}</u><a>\n"));
        assert_eq!((p.len(), n.len()), (32, 32));
        assert!(p[0].starts_with("<s>Act like someone who is cheerful.</s><u>Who are you?</u><a>\\nI"), "{}", p[0]);
        assert_eq!(p[0].replace("cheerful", "gloomy"), n[0], "same question and start on both sides");
        assert!(p.iter().all(|l| !l.contains('\n')), "one prompt a line");
        assert_eq!(instruction("Be formal"), "Be formal.");
    }

    #[test]
    fn lines_keep_their_escapes() {
        assert_eq!(line("a\\b\nc\r\n"), "a\\\\b\\nc\\n");
    }

    #[test]
    fn the_maker_says_how_far_it_is() {
        assert_eq!(progress_of("Evaluating prompt[12/32]: \"<s>…\" - \"<s>…\" (40 tokens)"), Some((12, 32)));
        assert_eq!(progress_of("n_total_tokens: 2560"), None);
    }

    #[test]
    fn layers_and_strength_stay_sensible() {
        assert_eq!(layer_range(36), (9, 34));
        assert_eq!(layer_range(2), (1, 2));
        assert_eq!(clamp_strength(5.0), 2.0);
        assert_eq!(clamp_strength(f32::NAN), 0.0);
        let s = Steer { file: PathBuf::from("d.gguf"), strength: -0.5, from: 9, to: 34 };
        let a: Vec<String> = s.args().iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(a, ["--control-vector", "d.gguf", "--control-vector-layer-range", "9", "34"], "a file that can't be read is passed as it is");
    }
}
