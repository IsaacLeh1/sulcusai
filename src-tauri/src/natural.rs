// SPDX-License-Identifier: AGPL-3.0-only
//! Natural voices: Supertonic 3 (31 languages) on ONNX Runtime.
//!
//! Text goes in as plain characters (no phonemizer, so no GPL espeak), and
//! four small networks turn it into speech: a duration predictor, a text
//! encoder, a few flow-matching steps, and a vocoder. ONNX Runtime is a
//! downloaded, hash-checked DLL loaded at run time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use ort::session::Session;
use ort::value::Tensor;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use unicode_normalization::UnicodeNormalization;

use crate::catalog::{EngineSpec, License};
use crate::paths::Paths;
use crate::tts::Speech;
use crate::{db, download, engine, net, AppState, AppStateRef};

/// Flow-matching steps: 5 is the quality setting the model was tuned for.
const STEPS: usize = 5;
/// Unload after this long unused (the models take ~500 MB of memory).
const IDLE_UNLOAD: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VoiceCatalog {
    pub runtime: EngineSpec,
    pub runtime_dll: String,
    pub models: Vec<VoicePack>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PackFile {
    pub path: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StyleInfo {
    pub id: String,
    pub name: String,
    pub gender: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VoicePack {
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub source: String,
    pub description: String,
    pub license: License,
    pub languages: Vec<String>,
    pub files: Vec<PackFile>,
    pub styles: Vec<StyleInfo>,
}

impl VoicePack {
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

/// Where an installed pack and the runtime are, set at startup and install.
#[derive(Debug, Clone)]
pub struct Installed {
    pub dll: PathBuf,
    pub dir: PathBuf,
    pub pack: VoicePack,
}

fn installed_slot() -> &'static Mutex<Option<Installed>> {
    static S: OnceLock<Mutex<Option<Installed>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

pub fn installed() -> Option<Installed> {
    installed_slot().lock().unwrap().clone()
}

fn runtime_dir(c: &VoiceCatalog) -> String {
    format!("onnxruntime-{}-{}", c.runtime.build, engine::cpu_key())
}

/// The library's file name on this system.
fn runtime_file(c: &VoiceCatalog) -> String {
    if cfg!(windows) {
        c.runtime_dll.clone()
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib".into()
    } else {
        "libonnxruntime.so".into()
    }
}

/// The downloaded ONNX Runtime library, if present.
pub fn runtime_dll(paths: &Paths, c: &VoiceCatalog) -> Option<PathBuf> {
    engine::find_exe(&paths.engines.join(runtime_dir(c)), &runtime_file(c))
}

/// Downloads and unpacks ONNX Runtime if it isn't there yet.
pub async fn ensure_runtime(state: &Arc<AppState>, cancel: &AtomicBool, emit: &(dyn Fn(&str, u64, u64) + Sync)) -> Result<PathBuf, String> {
    let c = &state.catalog.voices;
    let asset = c.runtime.assets.get(engine::cpu_key()).ok_or("Natural voices aren't available on this system yet (no ONNX Runtime build for it).")?;
    let rdir = runtime_dir(c);
    if runtime_dll(&state.paths, c).is_none() {
        state.log("network", &format!("Downloading ONNX Runtime from {}", crate::host_of(&asset.url)));
    }
    engine::ensure_unpacked(&state.paths, &rdir, asset, &runtime_file(c), cancel, |r, t| emit("engine", r, t)).await
}

/// Finds an installed pack (every file present) and its runtime.
pub fn detect(paths: &Paths, c: &VoiceCatalog) -> Option<Installed> {
    let dll = runtime_dll(paths, c)?;
    let pack = c.models.first()?;
    let dir = paths.models.join(&pack.id);
    pack.files.iter().all(|f| dir.join(&f.path).metadata().is_ok_and(|m| m.len() == f.size)).then(|| Installed {
        dll,
        dir,
        pack: pack.clone(),
    })
}

pub fn refresh(paths: &Paths, c: &VoiceCatalog) {
    *installed_slot().lock().unwrap() = detect(paths, c);
}

// ---------- text ----------

/// Cleans text the way the model was trained to read it.
pub fn prepare(text: &str, lang: &str) -> String {
    let mut t: String = text.nfkd().collect();
    t.retain(|c| !is_emoji(c));
    for (from, to) in [
        ("–", "-"), ("‑", "-"), ("—", "-"), ("_", " "), ("\u{201c}", "\""), ("\u{201d}", "\""),
        ("\u{2018}", "'"), ("\u{2019}", "'"), ("´", "'"), ("`", "'"), ("[", " "), ("]", " "),
        ("|", " "), ("/", " "), ("#", " "), ("→", " "), ("←", " "), ("@", " at "),
        ("e.g.,", "for example, "), ("i.e.,", "that is, "),
    ] {
        t = t.replace(from, to);
    }
    t.retain(|c| !"♥☆♡©\\".contains(c));
    for p in [",", ".", "!", "?", ";", ":", "'"] {
        t = t.replace(&format!(" {p}"), p);
    }
    while t.contains("\"\"") {
        t = t.replace("\"\"", "\"");
    }
    while t.contains("''") {
        t = t.replace("''", "'");
    }
    let mut t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if !t.ends_with(|c: char| ".!?;:,'\")]}…。」』】〉》›»".contains(c)) {
        t.push('.');
    }
    format!("<{lang}>{t}</{lang}>")
}

fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x1F300..=0x1FAFF | 0x2600..=0x27BF | 0x1F1E6..=0x1F1FF)
}

/// Splits long text into pieces the model reads well, at sentence ends.
pub fn chunks(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        let mut cur = String::new();
        let mut sentences = Vec::new();
        let mut start = 0;
        let b = para.as_bytes();
        for i in 0..b.len() {
            if matches!(b[i], b'.' | b'!' | b'?') && b.get(i + 1) == Some(&b' ') {
                sentences.push(&para[start..=i]);
                start = i + 2;
            }
        }
        if start < para.len() {
            sentences.push(&para[start..]);
        }
        for s in sentences {
            if !cur.is_empty() && cur.len() + s.len() + 1 > max {
                out.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(s);
        }
        if !cur.is_empty() {
            out.push(cur);
        }
    }
    out
}

// ---------- the engine ----------

struct Style {
    ttl: (Vec<i64>, Vec<f32>),
    dp: (Vec<i64>, Vec<f32>),
}

struct Engine {
    dp: Session,
    enc: Session,
    est: Session,
    voc: Session,
    indexer: Vec<i64>,
    sample_rate: u32,
    chunk: usize,
    latent_dim: usize,
    styles: HashMap<String, Style>,
    dir: PathBuf,
    last_used: Instant,
}

fn engine_slot() -> &'static Mutex<Option<Engine>> {
    static S: OnceLock<Mutex<Option<Engine>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

pub fn runtime_ready(dll: &Path) -> Result<(), String> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| {
        let builder = ort::init_from(dll).map_err(|e| format!("Couldn't load ONNX Runtime: {e}"))?;
        builder.commit();
        Ok(())
    })
    .clone()
}

fn e(err: impl std::fmt::Display) -> String {
    format!("The natural voice failed: {err}")
}

impl Engine {
    fn load(i: &Installed) -> Result<Engine, String> {
        runtime_ready(&i.dll)?;
        let threads = std::thread::available_parallelism().map_or(4, |n| (n.get() / 2).clamp(2, 8));
        let open = |name: &str| -> Result<Session, String> {
            Session::builder()
                .map_err(e)?
                .with_intra_threads(threads)
                .map_err(e)?
                .commit_from_file(i.dir.join("onnx").join(name))
                .map_err(e)
        };
        let cfg: Value = serde_json::from_str(&std::fs::read_to_string(i.dir.join("onnx/tts.json")).map_err(e)?).map_err(e)?;
        let indexer: Vec<i64> = serde_json::from_str(&std::fs::read_to_string(i.dir.join("onnx/unicode_indexer.json")).map_err(e)?).map_err(e)?;
        let ccf = cfg["ttl"]["chunk_compress_factor"].as_u64().unwrap_or(6) as usize;
        Ok(Engine {
            dp: open("duration_predictor.onnx")?,
            enc: open("text_encoder.onnx")?,
            est: open("vector_estimator.onnx")?,
            voc: open("vocoder.onnx")?,
            indexer,
            sample_rate: cfg["ae"]["sample_rate"].as_u64().unwrap_or(44_100) as u32,
            chunk: cfg["ae"]["base_chunk_size"].as_u64().unwrap_or(512) as usize * ccf,
            latent_dim: cfg["ttl"]["latent_dim"].as_u64().unwrap_or(24) as usize * ccf,
            styles: HashMap::new(),
            dir: i.dir.clone(),
            last_used: Instant::now(),
        })
    }

    fn style(&mut self, id: &str) -> Result<&Style, String> {
        if !self.styles.contains_key(id) {
            if !id.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Err("Unknown voice.".into());
            }
            let v: Value = serde_json::from_str(
                &std::fs::read_to_string(self.dir.join("voice_styles").join(format!("{id}.json"))).map_err(|_| "Unknown voice.".to_string())?,
            )
            .map_err(e)?;
            let part = |k: &str| -> Result<(Vec<i64>, Vec<f32>), String> {
                let dims: Vec<i64> = serde_json::from_value(v[k]["dims"].clone()).map_err(e)?;
                let mut flat = Vec::new();
                flatten(&v[k]["data"], &mut flat);
                Ok((dims, flat))
            };
            self.styles.insert(id.to_string(), Style { ttl: part("style_ttl")?, dp: part("style_dp")? });
        }
        Ok(&self.styles[id])
    }

    fn ids(&self, text: &str) -> Vec<i64> {
        text.chars()
            .filter_map(|c| self.indexer.get(c as usize).copied())
            .filter(|i| *i >= 0)
            .collect()
    }

    /// One piece of text (up to a few sentences) to samples.
    fn infer(&mut self, text: &str, style_id: &str, speed: f32, seed: u64) -> Result<Vec<f32>, String> {
        let ids = self.ids(text);
        let n = ids.len();
        if n == 0 {
            return Ok(Vec::new());
        }
        let (ttl, dp) = {
            let s = self.style(style_id)?;
            (s.ttl.clone(), s.dp.clone())
        };
        let text_ids = || Tensor::from_array(([1usize, n], ids.clone())).map_err(e);
        let text_mask = || Tensor::from_array(([1usize, 1, n], vec![1.0f32; n])).map_err(e);
        let style_ttl = || Tensor::from_array((ttl.0.clone(), ttl.1.clone())).map_err(e);

        let dur = {
            let out = self
                .dp
                .run(ort::inputs! { "text_ids" => text_ids()?, "style_dp" => Tensor::from_array((dp.0.clone(), dp.1.clone())).map_err(e)?, "text_mask" => text_mask()? })
                .map_err(e)?;
            out[0].try_extract_tensor::<f32>().map_err(e)?.1[0] / speed
        };
        let (emb_shape, emb) = {
            let out = self.enc.run(ort::inputs! { "text_ids" => text_ids()?, "style_ttl" => style_ttl()?, "text_mask" => text_mask()? }).map_err(e)?;
            let (shape, data) = out[0].try_extract_tensor::<f32>().map_err(e)?;
            (shape.iter().copied().collect::<Vec<i64>>(), data.to_vec())
        };
        let wav_len = (dur.max(0.1) * self.sample_rate as f32) as usize;
        let latent_len = wav_len.div_ceil(self.chunk);
        let mut rng = Gauss::new(seed);
        let mut xt: Vec<f32> = (0..self.latent_dim * latent_len).map(|_| rng.next()).collect();
        for step in 0..STEPS {
            let out = self
                .est
                .run(ort::inputs! {
                    "noisy_latent" => Tensor::from_array(([1usize, self.latent_dim, latent_len], xt.clone())).map_err(e)?,
                    "text_emb" => Tensor::from_array((emb_shape.clone(), emb.clone())).map_err(e)?,
                    "style_ttl" => style_ttl()?,
                    "text_mask" => text_mask()?,
                    "latent_mask" => Tensor::from_array(([1usize, 1, latent_len], vec![1.0f32; latent_len])).map_err(e)?,
                    "current_step" => Tensor::from_array(([1usize], vec![step as f32])).map_err(e)?,
                    "total_step" => Tensor::from_array(([1usize], vec![STEPS as f32])).map_err(e)?,
                })
                .map_err(e)?;
            xt = out[0].try_extract_tensor::<f32>().map_err(e)?.1.to_vec();
        }
        let out = self.voc.run(ort::inputs! { "latent" => Tensor::from_array(([1usize, self.latent_dim, latent_len], xt)).map_err(e)? }).map_err(e)?;
        let mut wav = out[0].try_extract_tensor::<f32>().map_err(e)?.1.to_vec();
        wav.truncate(wav_len);
        Ok(wav)
    }
}

fn flatten(v: &Value, out: &mut Vec<f32>) {
    match v {
        Value::Array(a) => a.iter().for_each(|x| flatten(x, out)),
        Value::Number(n) => out.push(n.as_f64().unwrap_or(0.0) as f32),
        _ => {}
    }
}

/// Standard normal noise (Box–Muller over xorshift), seeded per call.
struct Gauss {
    s: u64,
    spare: Option<f32>,
}

impl Gauss {
    fn new(seed: u64) -> Gauss {
        Gauss { s: seed | 1, spare: None }
    }

    fn uniform(&mut self) -> f32 {
        self.s ^= self.s << 13;
        self.s ^= self.s >> 7;
        self.s ^= self.s << 17;
        ((self.s >> 40) as f32 + 0.5) / (1u64 << 24) as f32
    }

    fn next(&mut self) -> f32 {
        if let Some(v) = self.spare.take() {
            return v;
        }
        let (u, v) = (self.uniform(), self.uniform());
        let r = (-2.0 * u.ln()).sqrt();
        let a = std::f32::consts::TAU * v;
        self.spare = Some(r * a.sin());
        r * a.cos()
    }
}

/// Speaks `text` with a natural voice. Blocking; call from a blocking thread.
pub fn synthesize(text: &str, style: &str, language: Option<&str>, rate: f64) -> Result<Speech, String> {
    let inst = installed().ok_or("Natural voices aren't installed.")?;
    let lang = language.filter(|l| inst.pack.languages.iter().any(|x| x == l)).unwrap_or("en");
    let mut slot = engine_slot().lock().unwrap();
    if slot.is_none() {
        *slot = Some(Engine::load(&inst)?);
    }
    let eng = slot.as_mut().unwrap();
    eng.last_used = Instant::now();
    let max = if matches!(lang, "ko" | "ja") { 120 } else { 300 };
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(7, |d| d.as_nanos() as u64);
    let mut samples: Vec<f32> = Vec::new();
    for (k, piece) in chunks(text, max).iter().enumerate() {
        if k > 0 {
            samples.extend(std::iter::repeat(0.0).take(eng.sample_rate as usize * 3 / 10));
        }
        // The model's own "natural" pace is a touch quick at 1.0.
        samples.extend(eng.infer(&prepare(piece, lang), style, 1.05 * rate as f32, seed.wrapping_add(k as u64))?);
    }
    Ok(Speech { rate: eng.sample_rate, samples: samples.iter().map(|s| crate::audio::to_i16(*s)).collect() })
}

/// Loads the voice ahead of the first sentence (voice mode calls this).
pub fn warm() {
    if let Some(inst) = installed() {
        let mut slot = engine_slot().lock().unwrap();
        if slot.is_none() {
            if let Ok(e) = Engine::load(&inst) {
                *slot = Some(e);
            }
        }
    }
}

pub fn unload_if_idle() {
    let mut slot = engine_slot().lock().unwrap();
    if slot.as_ref().is_some_and(|e| e.last_used.elapsed() > IDLE_UNLOAD) {
        *slot = None;
    }
}

// ---------- install ----------

async fn install(state: &Arc<AppState>, cancel: &AtomicBool, emit: &(dyn Fn(&str, u64, u64) + Sync)) -> Result<(), String> {
    let c = &state.catalog.voices;
    let pack = c.models.first().ok_or("No voice pack in the catalog")?;
    emit("engine", 0, 0);
    ensure_runtime(state, cancel, emit).await?;

    let client = net::external_client(state.settings().connectivity, net::Purpose::ModelDownload, false)?;
    let dir = state.paths.models.join(&pack.id);
    let total = pack.size();
    state.log("network", &format!("Downloading {} ({}) from {}", pack.name, crate::size_label(total), crate::host_of(&pack.files[0].url)));
    let mut done = 0u64;
    for f in &pack.files {
        let dest = dir.join(&f.path);
        let base = done;
        download::fetch_verified(&client, &f.url, &dest, f.size, &f.sha256, cancel, |r, _| emit("download", base + r, total)).await?;
        done += f.size;
    }
    refresh(&state.paths, c);
    // Prove it speaks.
    emit("benchmark", 0, 0);
    tokio::task::spawn_blocking(|| synthesize("Hello.", "F1", Some("en"), 1.0)).await.map_err(|e| e.to_string())??;
    let conn = state.db.lock().unwrap();
    db::log_action(&conn, "model", &format!("Installed {}", pack.name));
    Ok(())
}

#[cfg(test)]
pub async fn install_for_test(state: &Arc<AppState>) -> Result<(), String> {
    install(state, &AtomicBool::new(false), &|_, _, _| {}).await
}

#[derive(Serialize)]
pub struct VoicePackView {
    #[serde(flatten)]
    pack: VoicePack,
    size: u64,
    installed: bool,
}

#[tauri::command]
pub fn voice_packs(state: AppStateRef) -> Vec<VoicePackView> {
    let inst = installed();
    state
        .catalog
        .voices
        .models
        .iter()
        .map(|p| VoicePackView { size: p.size(), installed: inst.as_ref().is_some_and(|i| i.pack.id == p.id), pack: p.clone() })
        .collect()
}

#[tauri::command]
pub fn install_voice_pack(app: AppHandle, state: AppStateRef, pack_id: String) -> Result<(), String> {
    let pack = state.catalog.voices.models.iter().find(|p| p.id == pack_id).ok_or("Unknown voice pack")?.clone();
    if pack.size() > state.budget().disk {
        return Err("There isn't enough free disk space for the voices.".into());
    }
    if !state.installs.lock().unwrap().is_empty() {
        return Err("Another install is in progress. Wait for it to finish first.".into());
    }
    let state = state.inner().clone();
    tauri::async_runtime::spawn(async move {
        let Some((cancel, _guard)) = crate::claim(&state.installs, &pack_id) else { return };
        let emit = |phase: &str, received: u64, total: u64| {
            app.emit("install:progress", json!({ "model_id": pack_id, "phase": phase, "received": received, "total": total })).ok();
        };
        let result = install(&state, &cancel, &emit).await;
        let payload = match &result {
            Ok(()) => json!({ "model_id": pack_id, "name": pack.name, "ok": true }),
            Err(e) if e == download::CANCELLED => json!({ "model_id": pack_id, "name": pack.name, "ok": false, "cancelled": true }),
            Err(e) => {
                state.log("model", &format!("Installing {} failed: {e}", pack.name));
                json!({ "model_id": pack_id, "name": pack.name, "ok": false, "error": e })
            }
        };
        app.emit("install:finished", payload).ok();
    });
    Ok(())
}

#[tauri::command]
pub fn remove_voice_pack(state: AppStateRef, pack_id: String) -> Result<(), String> {
    let pack = state.catalog.voices.models.iter().find(|p| p.id == pack_id).ok_or("Unknown voice pack")?;
    *engine_slot().lock().unwrap() = None;
    let dir = state.paths.models.join(&pack.id);
    if dir.starts_with(&state.paths.models) && dir != state.paths.models && dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("Couldn't delete the voices: {e}"))?;
    }
    refresh(&state.paths, &state.catalog.voices);
    let conn = state.db.lock().unwrap();
    crate::speech::update_voice_settings(&conn, |s| {
        if s.voice.as_deref().is_some_and(|v| v.starts_with("supertonic:")) {
            s.voice = None;
        }
    })?;
    db::log_action(&conn, "model", &format!("Removed {} and deleted its files", pack.name));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_prepared_like_the_training_data() {
        assert_eq!(prepare("Hello — world", "en"), "<en>Hello - world.</en>");
        assert_eq!(prepare("Café “ok” 😀", "fr"), "<fr>Cafe\u{301} \"ok\"</fr>", "a closing quote ends it too");
        assert_eq!(prepare("Is it done ?", "en"), "<en>Is it done?</en>");
        assert_eq!(prepare("mail me @ home", "en"), "<en>mail me at home.</en>");
    }

    #[test]
    fn long_text_is_chunked_at_sentences() {
        let text = "One two three. Four five six. Seven eight nine.";
        assert_eq!(chunks(text, 20), vec!["One two three.", "Four five six.", "Seven eight nine."]);
        assert_eq!(chunks(text, 300), vec![text]);
        assert_eq!(chunks("First.\n\nSecond.", 300), vec!["First.", "Second."]);
    }

    #[test]
    fn noise_is_roughly_standard_normal() {
        let mut g = Gauss::new(42);
        let xs: Vec<f32> = (0..20_000).map(|_| g.next()).collect();
        let mean = xs.iter().sum::<f32>() / xs.len() as f32;
        let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / xs.len() as f32;
        assert!(mean.abs() < 0.03, "{mean}");
        assert!((var - 1.0).abs() < 0.05, "{var}");
    }

    #[test]
    fn the_catalog_pack_is_complete() {
        let c = crate::catalog::Catalog::bundled().voices;
        let p = &c.models[0];
        for name in ["duration_predictor", "text_encoder", "vector_estimator", "vocoder"] {
            assert!(p.files.iter().any(|f| f.path == format!("onnx/{name}.onnx")), "{name}");
        }
        assert_eq!(p.styles.len(), 10);
        assert!(p.languages.contains(&"de".to_string()));
        assert!(p.files.iter().all(|f| f.sha256.len() == 64 && f.url.contains(&f.path)));
    }
}
