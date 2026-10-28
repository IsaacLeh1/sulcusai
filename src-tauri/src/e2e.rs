// SPDX-License-Identifier: AGPL-3.0-only
//! End-to-end check of the real engine with the smallest catalog model.
//! Downloads ~1.1 GB on first run, so it is ignored by default:
//!     cargo test e2e -- --ignored --nocapture
//! Uses the app's own data folder, so the files are reused by the app.

use std::sync::atomic::AtomicBool;

use crate::catalog::{self, Budget, Catalog};
use crate::db::{Message, Profile};
use crate::testutil::{app_data_dir, scratch_state, temp_state, temp_state_in};
use crate::{chat, download, engine, hardware, net, paths::Paths};

mod builds;
mod mail;
mod models;
mod pictures;
mod screen;
mod songs;
mod studio;
mod tasks;
mod video;
mod voice;

/// A real engine plus app state in a temporary data folder.
async fn agent_harness() -> (std::sync::Arc<crate::AppState>, engine::Endpoint, crate::crypto::Cipher) {
    agent_harness_on(None, None).await
}

/// The harness on a given model file and context size (e.g. a model Ollama
/// downloaded) instead of Qwen3 1.7B.
async fn agent_harness_on(model_file: Option<(std::path::PathBuf, u32, u32)>, running: Option<engine::Endpoint>) -> (std::sync::Arc<crate::AppState>, engine::Endpoint, crate::crypto::Cipher) {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, RwLock};
    let real = Paths::new(app_data_dir()).unwrap();
    let tmp = std::env::temp_dir().join(format!("sulcusai-e2e-{}", uuid::Uuid::new_v4()));
    let paths = Paths::new(tmp.clone()).unwrap();
    let conn = crate::db::open(&paths.db).unwrap();
    crate::features::enable_all(&conn);
    let vault = crate::crypto::Vault::open(&tmp.join("keys.json"), Box::new(crate::crypto::dpapi::Dpapi)).unwrap();
    let cipher = vault.cipher().unwrap();
    let hw = hardware::detect(&real.models);
    let budget = Budget::from_hardware(&hw);
    let cat = Catalog::bundled();
    let spec = cat.model("qwen3-1.7b").unwrap().clone();
    let state = Arc::new(crate::AppState {
        paths,
        db: Mutex::new(conn),
        vault: Mutex::new(vault),
        catalog: cat.clone(),
        hardware: RwLock::new(hw),
        engine: tokio::sync::Mutex::new(engine::Engine::default()),
        installs: Mutex::new(HashMap::new()),
        generations: Mutex::new(HashMap::new()),
        contexts: Mutex::new(HashMap::new()),
        approvals: crate::agent::Approvals::default(),
        mcp: tokio::sync::Mutex::new(HashMap::new()),
        speech: crate::speech::Speech::default(),
        player: crate::audio::Player::new(),
        heat: crate::perf::Heat::default(),
        bridge: crate::bridge::Bridge::default(),
        app: std::sync::OnceLock::new(),
        engine_used: std::sync::Mutex::new(std::time::Instant::now()),
        job: None,
    });
    // An engine that is already running (the app's), rather than loading a second copy.
    if let Some(ep) = running {
        return (state, ep, cipher);
    }
    let exe = engine::installed_server(&real, &cat.engine, engine::backend_for(&budget).unwrap()).unwrap();
    let (model, (ctx, layers)) = match model_file {
        Some((file, ctx, layers)) => (file, (ctx, layers)),
        None => (real.models.join(&spec.id).join(&spec.variant("Q4_K_M").unwrap().file), catalog::launch_settings(&spec, "Q4_K_M", &budget)),
    };
    let ep = state
        .engine
        .lock()
        .await
        .ensure(engine::LaunchSpec { exe: &exe, model_path: &model, model_id: &spec.id, quant: "Q4_K_M", ctx, gpu_layers: layers, threads: 0, low_priority: false, log: &real.engine_log(), mmproj: None, batch: 0, lora: None, steer: None }, None)
        .await
        .unwrap();
    (state, ep, cipher)
}

/// The model in SULCUSAI_TEST_GGUF loaded the way the app loads it for the
/// agent: SULCUSAI_TEST_CTX (16384 unless set), every layer on the card
/// except the experts of SULCUSAI_TEST_CPU_MOE layers (6 unless set: what
/// lets a 20B mixture of experts run beside a 12 GB card), and for gpt-oss
/// the request settings `llm_endpoint` adds (reasoning effort, a reply long
/// enough to think in).
pub(crate) async fn test_model() -> (engine::Engine, engine::Endpoint, String) {
    let gguf = std::path::PathBuf::from(std::env::var("SULCUSAI_TEST_GGUF").expect("SULCUSAI_TEST_GGUF"));
    let ctx: u32 = std::env::var("SULCUSAI_TEST_CTX").ok().and_then(|v| v.parse().ok()).unwrap_or(16_384);
    // Only a mixture of experts has experts to keep in memory (llama-server
    // ignored the flag for dense models, so the test never matched them).
    let experts = crate::gguf::read(&gguf).ok().is_some_and(|m| m.iter().any(|(k, v)| k.ends_with(".expert_count") && v.int().is_some_and(|n| n > 1)));
    let cpu_moe: u32 = std::env::var("SULCUSAI_TEST_CPU_MOE").ok().and_then(|v| v.parse().ok()).unwrap_or(if experts { 6 } else { 0 });
    let real = Paths::new(app_data_dir()).unwrap();
    let hw = hardware::detect(&real.models);
    let budget = Budget::from_hardware(&hw);
    let cat = Catalog::bundled();
    let exe = engine::installed_server(&real, &cat.engine, engine::backend_for(&budget).unwrap()).unwrap();
    let name = gguf.file_name().unwrap().to_string_lossy().into_owned();
    let log = std::env::temp_dir().join(format!("sulcusai-e2e-engine-{}.log", uuid::Uuid::new_v4().simple()));
    let mut eng = engine::Engine::default();
    let launch = engine::LaunchSpec { exe: &exe, model_path: &gguf, model_id: "test-agent", quant: "Q4_K_M", ctx, gpu_layers: 999, threads: 0, low_priority: false, log: &log, mmproj: None, batch: 0, lora: None, steer: None };
    let mut ep = eng.ensure_tuned(launch, engine::Tuning { cpu_moe: (cpu_moe > 0).then_some(cpu_moe), ..Default::default() }, None).await.unwrap();
    if name.to_lowercase().contains("gpt-oss") {
        let effort = crate::advanced::DEFAULT_REASONING;
        ep.extra = serde_json::json!({ "reasoning_effort": effort, "chat_template_kwargs": { "reasoning_effort": effort }, "max_tokens": (ctx / 2).clamp(4096, 16_384) });
    }
    (eng, ep, name)
}

/// Everything the agent did in a chat, for reading: each message with its
/// thinking and tool calls (printed, and appended to SULCUSAI_TEST_LOG).
fn transcript(state: &crate::AppState, cipher: &crate::crypto::Cipher, chat_id: &str, title: &str) -> (String, Vec<String>) {
    use std::fmt::Write as _;
    let mut out = format!("===== {title} =====\n");
    let mut tools = Vec::new();
    for m in crate::db::messages(&state.db.lock().unwrap(), cipher, chat_id) {
        let _ = writeln!(out, "[{}] {}", m.role, m.content.chars().take(1500).collect::<String>());
        if let Some(t) = m.thinking.as_deref().filter(|t| !t.trim().is_empty()) {
            let _ = writeln!(out, "  (thinking) {}", t.chars().take(800).collect::<String>().replace('\n', " "));
        }
        for c in m.tool_calls.iter().flatten() {
            let _ = writeln!(out, "  -> {}({})", c.name, c.arguments);
            tools.push(c.name.clone());
        }
    }
    println!("{out}");
    if let Ok(path) = std::env::var("SULCUSAI_TEST_LOG") {
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = f.write_all(out.as_bytes());
        }
    }
    (out, tools)
}

async fn say(state: &std::sync::Arc<crate::AppState>, ep: &engine::Endpoint, cipher: &crate::crypto::Cipher, chat_id: &str, text: &str) -> String {
    say_in(crate::tools::Mode::Auto, state, ep, cipher, chat_id, text).await
}

async fn say_in(mode: crate::tools::Mode, state: &std::sync::Arc<crate::AppState>, ep: &engine::Endpoint, cipher: &crate::crypto::Cipher, chat_id: &str, text: &str) -> String {
    let turn_id = uuid::Uuid::new_v4().to_string();
    crate::db::add_message(&state.db.lock().unwrap(), cipher, &Message {
        id: turn_id.clone(),
        chat_id: chat_id.into(),
        role: "user".into(),
        content: text.into(),
        created_at: crate::db::now_ms(),
        ..Default::default()
    })
    .unwrap();
    let turn = crate::agent::Turn {
        emit: std::sync::Arc::new(|_: &str, _| {}),
        state: state.clone(),
        chat_id: chat_id.into(),
        turn_id,
        cipher: cipher.clone(),
        ep: ep.clone(),
        mode,
        use_tools: true,
        vision: false,
        unattended: false,
        pre_approved: None,
        base: chat::system_prompt(&Profile::default(), &chrono::Local::now().format("%A, %B %-d, %Y").to_string()).0,
        about: String::new(),
        cancel: std::sync::Arc::new(AtomicBool::new(false)),
    };
    turn.run().await.unwrap().last.map(|m| m.content).unwrap_or_default()
}

/// App state over the real data folder's engines and models, with a
/// throwaway database.
fn speech_state() -> std::sync::Arc<crate::AppState> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, RwLock};
    let real = Paths::new(app_data_dir()).unwrap();
    let tmp = std::env::temp_dir().join(format!("sulcusai-speech-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let conn = crate::db::open(&tmp.join("t.db")).unwrap();
    let vault = crate::crypto::Vault::open(&tmp.join("keys.json"), Box::new(crate::crypto::dpapi::Dpapi)).unwrap();
    Arc::new(crate::AppState {
        hardware: RwLock::new(hardware::detect(&real.models)),
        paths: real,
        db: Mutex::new(conn),
        vault: Mutex::new(vault),
        catalog: Catalog::bundled(),
        engine: tokio::sync::Mutex::new(engine::Engine::default()),
        installs: Mutex::new(HashMap::new()),
        generations: Mutex::new(HashMap::new()),
        contexts: Mutex::new(HashMap::new()),
        approvals: crate::agent::Approvals::default(),
        mcp: tokio::sync::Mutex::new(HashMap::new()),
        speech: crate::speech::Speech::default(),
        player: crate::audio::Player::new(),
        heat: crate::perf::Heat::default(),
        bridge: crate::bridge::Bridge::default(),
        app: std::sync::OnceLock::new(),
        engine_used: std::sync::Mutex::new(std::time::Instant::now()),
        // Models a test starts end with it (else they stay on the card).
        job: crate::winjob::Job::kill_on_close().ok(),
    })
}

/// App state with the real engines and models but a throwaway database and
/// gallery (so test pictures never land in the real app's gallery).
fn media_state() -> std::sync::Arc<crate::AppState> {
    temp_state(None)
}

/// What Whisper (large v3 turbo, as installed) hears in a sound file, run
/// directly with no voice detector.
#[cfg(windows)]
fn whisper_hears(state: &crate::AppState, file: &std::path::Path, lang: Option<&str>) -> String {
    let exe = std::fs::read_dir(&state.paths.engines)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("whisper-v"))
        .flat_map(|e| [e.path().join("Release").join("whisper-cli.exe"), e.path().join("whisper-cli.exe")])
        .find(|p| p.exists());
    let model = std::fs::read_dir(state.paths.models.join("whisper-large-v3-turbo")).into_iter().flatten().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "bin"));
    let (Some(exe), Some(model)) = (exe, model) else { return "(no whisper-cli)".into() };
    // A word a line, each with its time (`heard_words` reads them).
    let out = std::process::Command::new(exe).arg("-m").arg(model).arg("-f").arg(file).args(["-t", "8", "-ml", "1", "-sow", "-l", lang.unwrap_or("auto")]).output();
    out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_else(|e| format!("(not transcribed: {e})"))
}

/// Whisper's timed lines ("[00:00:12.000 --> 00:00:15.000]  words") as
/// their words, and the second the first real words start (None: none heard).
#[cfg(windows)]
fn heard_words(timed: &str) -> (String, Option<f32>) {
    let mut words = Vec::new();
    let mut first = None;
    for line in timed.lines() {
        let Some((stamp, text)) = line.trim().strip_prefix('[').and_then(|l| l.split_once(']')) else { continue };
        let text = text.trim();
        let real = text.chars().filter(|c| c.is_alphabetic()).count() >= 3 && !text.to_lowercase().contains("music");
        if real && first.is_none() {
            let start = stamp.split("-->").next().unwrap_or_default().trim();
            let secs: f32 = start.split(':').filter_map(|p| p.parse::<f32>().ok()).fold(0.0, |acc, p| acc * 60.0 + p);
            first = Some(secs);
        }
        words.push(text.to_string());
    }
    (words.join(" "), first)
}

/// Lowercase words of at least four letters, for comparing lyrics with
/// what speech recognition heard.
#[cfg(windows)]
fn sung_words(text: &str) -> std::collections::BTreeSet<String> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('['))
        .flat_map(|l| l.split(|c: char| !c.is_alphanumeric() && c != '\''))
        .map(|w| w.trim_matches('\'').to_lowercase())
        .filter(|w| w.chars().count() >= 4)
        .collect()
}

/// The chat model that writes lyrics (SULCUSAI_LYRICS_MODEL, default
/// qwen3-4b-2507), installed as the default and not loaded yet.
#[cfg(windows)]
fn lyrics_model(state: &crate::AppState) {
    let chat = std::env::var("SULCUSAI_LYRICS_MODEL").unwrap_or_else(|_| "qwen3-4b-2507".into());
    let spec = state.catalog.model(&chat).unwrap();
    let v = spec.variant("Q4_K_M").unwrap();
    let path = state.paths.models.join(&spec.id).join(&v.file);
    assert!(path.exists(), "the chat model is installed: {}", path.display());
    let conn = state.db.lock().unwrap();
    crate::db::save_installed(&conn, &crate::db::InstalledModel { model_id: spec.id.clone(), quant: v.quant.clone(), path: path.display().to_string(), size: v.size, installed_at: 0, tps: None, prompt_tps: None }).unwrap();
    crate::db::update_settings(&conn, |s| s.default_model = Some(spec.id.clone())).unwrap();
}

/// Cloud setup shared by the cloud tests: Cloud level, one provider with a
/// key, one model.
fn cloud_state(provider: crate::cloud::Provider) -> std::sync::Arc<crate::AppState> {
    let state = temp_state(None);
    let c = state.cipher().unwrap();
    let mut p = provider;
    p.key = c.encrypt(&p.key);
    let conn = state.db.lock().unwrap();
    crate::db::update_settings(&conn, |s| s.connectivity = crate::net::Connectivity::Cloud).unwrap();
    crate::db::set(&conn, "cloud", &crate::cloud::CloudSettings { providers: vec![p], budget: Some(5.0), redact: true }).unwrap();
    drop(conn);
    state
}
