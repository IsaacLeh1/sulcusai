// SPDX-License-Identifier: AGPL-3.0-only
//! End-to-end check of the real engine with the smallest catalog model.
//! Downloads ~1.1 GB on first run, so it is ignored by default:
//!     cargo test e2e -- --ignored --nocapture
//! Uses the app's own data folder, so the files are reused by the app.

use std::sync::atomic::AtomicBool;

use crate::catalog::{self, Budget, Catalog};
use crate::db::{Message, Profile};
use crate::{chat, download, engine, hardware, net, paths::Paths};

fn app_data_dir() -> std::path::PathBuf {
    // Same folder as Tauri's app_local_data_dir().
    let base = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).expect("LOCALAPPDATA");
    base.join("app.sulcusai.desktop")
}

/// The cloud providers set up in the installed app (his LiteLLM), checked
/// with the app's own code: the models each offers and a one-word reply from
/// each model picked. Reads the real data without changing it; keys stay in
/// this process and the report holds nothing secret.
///     cargo test real_cloud_providers -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn real_cloud_providers_answer() {
    let real = Paths::new(app_data_dir()).unwrap();
    let keys = real.data.join("keys.json");
    assert!(keys.exists(), "no installed app data at {}", real.data.display());
    let conn = rusqlite::Connection::open_with_flags(&real.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let vault = crate::crypto::Vault::open(&keys, crate::crypto::platform_protector(&real.data)).unwrap();
    let c = vault.work_cipher().expect("the app's data key is locked with a password");
    let s = crate::cloud::settings(&conn);
    println!("{} cloud providers set up", s.providers.len());
    // SULCUSAI_TRY_PROXY tries the providers named in SULCUSAI_TRY_PROXY_FOR
    // (default "LiteLLM") through a proxy, without saving it.
    let try_proxy = std::env::var("SULCUSAI_TRY_PROXY").ok();
    let proxy_for = std::env::var("SULCUSAI_TRY_PROXY_FOR").unwrap_or_else(|_| "LiteLLM".into()).to_lowercase();
    for p in s.providers.iter().filter(|p| !p.key.is_empty()) {
        let mut p = p.clone();
        if let Some(proxy) = try_proxy.as_deref().filter(|_| p.name.to_lowercase().contains(&proxy_for)) {
            p.proxy = c.encrypt(proxy);
        }
        println!("{}", crate::cloud::check_provider(&p, &c).await);
    }
}

/// The latest privacy warnings in the installed app ("What left this PC"),
/// read without changing anything; anything that looks like a key is
/// masked.
///     cargo test real_privacy_warnings -- --ignored --nocapture
#[test]
#[ignore]
fn real_privacy_warnings() {
    let real = Paths::new(app_data_dir()).unwrap();
    let keys = real.data.join("keys.json");
    assert!(keys.exists(), "no installed app data at {}", real.data.display());
    let conn = rusqlite::Connection::open_with_flags(&real.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let vault = crate::crypto::Vault::open(&keys, crate::crypto::platform_protector(&real.data)).unwrap();
    let c = vault.work_cipher().expect("the app's data key is locked with a password");
    let mask = regex::Regex::new(r"[A-Za-z0-9_\-]{20,}").unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let all = crate::outgoing::sent_list(&conn, &c);
    println!("{} in the list; level 4-5 or not sent:", all.len());
    for s in all.iter().filter(|s| s.level >= 4 || !s.sent).take(8) {
        let what = mask.replace_all(&s.what, "[long string hidden]");
        println!("{} min ago | {} | to: {} | level {} | {} | {}", (now - s.at) / 60_000, s.tool, s.to, s.level, if s.sent { "sent" } else { "not sent" }, what);
    }
}

/// The most recently changed chat in the installed app, step by step: what
/// was asked, each tool call and how it went, and the replies, shortened,
/// with anything that looks like a key masked. Read without changing anything.
///     cargo test real_latest_chat -- --ignored --nocapture
#[test]
#[ignore]
fn real_latest_chat() {
    let real = Paths::new(app_data_dir()).unwrap();
    let keys = real.data.join("keys.json");
    assert!(keys.exists(), "no installed app data at {}", real.data.display());
    let conn = rusqlite::Connection::open_with_flags(&real.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let vault = crate::crypto::Vault::open(&keys, crate::crypto::platform_protector(&real.data)).unwrap();
    let c = vault.work_cipher().expect("the app's data key is locked with a password");
    let mask = regex::Regex::new(r"[A-Za-z0-9_\-]{28,}").unwrap();
    let short = |t: &str, n: usize| -> String {
        let t = mask.replace_all(t, "[long string hidden]").replace('\n', " ⏎ ");
        if t.chars().count() > n { format!("{}…", t.chars().take(n).collect::<String>()) } else { t }
    };
    let mut chats = crate::db::list_chats(&conn, &c);
    chats.sort_by_key(|ch| std::cmp::Reverse(ch.updated_at));
    let pick = std::env::var("SULCUSAI_CHAT_INDEX").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    let Some(chat) = chats.get(pick) else { return };
    println!("chat: {} | model: {} | updated {} min ago", chat.title, chat.model_id.clone().unwrap_or_default(), (crate::db::now_ms() - chat.updated_at) / 60_000);
    let msgs = crate::db::messages(&conn, &c, &chat.id);
    // SULCUSAI_CHAT_ALL: every turn, not just the last.
    let from = if std::env::var("SULCUSAI_CHAT_ALL").is_ok() { 0 } else { msgs.iter().rposition(|m| m.role == "user").unwrap_or(0) };
    println!("{} messages; privacy mode: {:?}", msgs.len(), crate::outgoing::mode(&conn));
    for m in &msgs[from..] {
        match m.role.as_str() {
            "user" => println!("\nUSER: {}", short(&m.content, 2500)),
            "assistant" => {
                if !m.content.trim().is_empty() {
                    println!("ASSISTANT: {}", short(&m.content, 600));
                }
                for call in m.tool_calls.iter().flatten() {
                    println!("  CALL {}: {}", call.name, short(&call.arguments, 300));
                }
            }
            "tool" => {
                let meta = m.meta.clone().unwrap_or_default();
                let title = meta["title"].as_str().unwrap_or_default();
                let status = meta["status"].as_str().unwrap_or_default();
                println!("  RESULT [{status}] {title}: {}", short(&m.content, 300));
            }
            other => println!("{other}: {}", short(&m.content, 300)),
        }
    }
}

#[tokio::test]
#[ignore]
async fn e2e_install_load_and_chat() {
    let paths = Paths::new(app_data_dir()).unwrap();
    let hw = hardware::detect(&paths.models);
    println!("hardware: {hw:#?}");
    let budget = Budget::from_hardware(&hw);
    println!("budget: {budget:?}");

    let cat = Catalog::bundled();
    let spec = cat.model("qwen3-1.7b").unwrap();
    let fit = catalog::fit_model(spec, &budget);
    let quant = "Q4_K_M";
    let vfit = fit.variant(quant).unwrap();
    println!("fit: {vfit:?}");
    assert!(vfit.runnable());

    let backend = engine::backend_for(&budget).unwrap();
    let cancel = AtomicBool::new(false);
    let exe = engine::ensure_installed(&paths, &cat.engine, backend, &cancel, |r, t| {
        if r == t {
            println!("engine downloaded: {t} bytes");
        }
    })
    .await
    .unwrap();
    println!("engine: {}", exe.display());

    let variant = spec.variant(quant).unwrap();
    let dest = paths.models.join(&spec.id).join(&variant.file);
    let client = net::external_client(net::Connectivity::Offline, net::Purpose::ModelDownload, false).unwrap();
    download::fetch_verified(&client, &variant.url, &dest, variant.size, &variant.sha256, &cancel, |_, _| {})
        .await
        .unwrap();
    println!("model verified: {}", dest.display());

    let mut eng = engine::Engine::default();
    let log = paths.engine_log();
    let ep = eng
        .ensure(
            engine::LaunchSpec {
                exe: &exe,
                model_path: &dest,
                model_id: &spec.id,
                quant,
                ctx: fit.ctx,
                gpu_layers: vfit.gpu_layers,
                threads: 0,
                low_priority: false,
                log: &log,
                mmproj: None,
                batch: 0,
                lora: None,
            },
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("engine failed to start: {e}"));
    println!("engine up on port {}", ep.port);

    // The server must refuse requests without the key.
    let unauth = net::local_client()
        .post(ep.url("/v1/chat/completions"))
        .json(&serde_json::json!({"messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status().as_u16(), 401, "server answered without the API key");

    let profile = Profile { name: "Tester".into(), about: String::new(), preferences: "Be brief.".into(), ..Default::default() };
    let (base, about) = chat::system_prompt(&profile, "Tuesday, October 6, 2026");
    let history = vec![Message {
        id: "u1".into(),
        chat_id: "c".into(),
        role: "user".into(),
        content: "Reply with a five-word greeting. /no_think".into(),
        created_at: 0,
        ..Default::default()
    }];
    let (kept, info) = chat::fit_history(&ep, &base, &about, None, &history).await.unwrap();
    println!("context: {info:?}");
    assert_eq!(kept.len(), 1);
    assert!(info.system_tokens > 20 && info.history_tokens > 0);

    let mut deltas = 0;
    let system = format!("{base}\n\n{about}");
    let done = chat::stream(&ep, chat::api_messages(&system, &kept), None, &cancel, |_| deltas += 1)
        .await
        .unwrap();
    println!("reply ({deltas} deltas): {:?}", done.content);
    println!("thinking: {:?}", done.thinking);
    println!("usage: {:?}/{:?}, tps {:?}", done.prompt_tokens, done.completion_tokens, done.tps);
    assert!(!done.content.trim().is_empty());
    assert!(deltas > 1, "reply did not stream");

    let tps = engine::measure(&ep).await.unwrap().tps;
    println!("benchmark: {tps} tokens/sec (estimate was {})", vfit.est_tps);
    assert!(tps > 0.0);

    eng.stop().await;
    assert!(eng.status().model_id.is_none());
}

/// Probe: does the engine stream OpenAI-style tool calls for this model?
#[tokio::test]
#[ignore]
async fn e2e_tool_call_probe() {
    let paths = Paths::new(app_data_dir()).unwrap();
    let hw = hardware::detect(&paths.models);
    let budget = Budget::from_hardware(&hw);
    let cat = Catalog::bundled();
    let spec = cat.model("qwen3-1.7b").unwrap();
    let fit = catalog::fit_model(spec, &budget);
    let exe = engine::installed_server(&paths, &cat.engine, engine::backend_for(&budget).unwrap()).unwrap();
    let dest = paths.models.join(&spec.id).join(&spec.variant("Q4_K_M").unwrap().file);
    let mut eng = engine::Engine::default();
    let ep = eng
        .ensure(engine::LaunchSpec { exe: &exe, model_path: &dest, model_id: &spec.id, quant: "Q4_K_M", ctx: fit.ctx, gpu_layers: 99, threads: 0, low_priority: false, log: &paths.engine_log(), mmproj: None, batch: 0, lora: None }, None)
        .await
        .unwrap();
    let body = serde_json::json!({
        "messages": [{"role": "user", "content": "What files are in C:/projects/demo? Use the tool."}],
        "tools": [{"type": "function", "function": {"name": "list_dir", "description": "List a folder", "parameters": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}}],
        "stream": true,
    });
    let text = net::local_client().post(ep.url("/v1/chat/completions")).bearer_auth(&ep.key).json(&body).send().await.unwrap().text().await.unwrap();
    for line in text.lines().filter(|l| l.contains("tool_calls") || l.contains("finish_reason\":\"")) {
        println!("{}", &line[..line.len().min(300)]);
    }
    eng.stop().await;
}

/// The agent loop with the real model: Bypass creates a file, Auto asks
/// before editing, and Undo puts both turns back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_agent_edits_files_and_undo_restores() {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, RwLock};

    let real = Paths::new(app_data_dir()).unwrap();
    let tmp = std::env::temp_dir().join(format!("sulcusai-agent-{}", uuid::Uuid::new_v4()));
    let paths = Paths::new(tmp.clone()).unwrap();
    let work = tmp.join("work").join("proj");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("greeting.txt"), "hello\n").unwrap();

    let conn = crate::db::open(&paths.db).unwrap();
    crate::features::enable_all(&conn);
    let vault = crate::crypto::Vault::open(&tmp.join("keys.json"), Box::new(crate::crypto::dpapi::Dpapi)).unwrap();
    let cipher = vault.cipher().unwrap();
    crate::db::add_folder(&conn, &dunce::canonicalize(&work).unwrap().display().to_string()).unwrap();
    let chat = crate::db::create_chat(&conn, &cipher, Some("qwen3-1.7b".into())).unwrap();

    let hw = hardware::detect(&real.models);
    let budget = Budget::from_hardware(&hw);
    let cat = Catalog::bundled();
    let spec = cat.model("qwen3-1.7b").unwrap().clone();
    let state = Arc::new(crate::AppState {
        paths: paths.clone(),
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

    let exe = engine::installed_server(&real, &cat.engine, engine::backend_for(&budget).unwrap()).unwrap();
    let model = real.models.join(&spec.id).join(&spec.variant("Q4_K_M").unwrap().file);
    let (ctx, layers) = catalog::launch_settings(&spec, "Q4_K_M", &budget);
    println!("launch ctx {ctx}, gpu layers {layers}");
    let ep = state
        .engine
        .lock()
        .await
        .ensure(engine::LaunchSpec { exe: &exe, model_path: &model, model_id: &spec.id, quant: "Q4_K_M", ctx, gpu_layers: layers, threads: 0, low_priority: false, log: &real.engine_log(), mmproj: None, batch: 0, lora: None }, None)
        .await
        .unwrap();

    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let run_turn = |text: &str, mode: crate::tools::Mode| {
        let turn_id = uuid::Uuid::new_v4().to_string();
        crate::db::add_message(&state.db.lock().unwrap(), &cipher, &Message {
            id: turn_id.clone(),
            chat_id: chat.id.clone(),
            role: "user".into(),
            content: text.into(),
            created_at: crate::db::now_ms(),
            ..Default::default()
        })
        .unwrap();
        let ev = events.clone();
        let turn = crate::agent::Turn {
            emit: Arc::new(move |name: &str, _| ev.lock().unwrap().push(name.to_string())),
            state: state.clone(),
            chat_id: chat.id.clone(),
            turn_id: turn_id.clone(),
            cipher: cipher.clone(),
            ep: ep.clone(),
            mode,
            use_tools: true,
            vision: false,
            unattended: false,
            pre_approved: None,
            base: chat::system_prompt(&Profile::default(), &chrono::Local::now().format("%A, %B %-d, %Y").to_string()).0,
            about: String::new(),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        (turn_id, turn)
    };

    // Turn 1: Bypass mode creates a file without asking.
    let (turn1, turn) = run_turn("Create a file proj/notes.txt containing exactly: hello agent", crate::tools::Mode::Bypass);
    let r = turn.run().await.unwrap();
    println!("turn 1 reply: {:?}", r.last.map(|m| m.content));
    let notes = work.join("notes.txt");
    assert!(notes.exists(), "Bypass mode should have created notes.txt");
    println!("notes.txt = {:?}", std::fs::read_to_string(&notes).unwrap());
    assert!(!events.lock().unwrap().iter().any(|e| e == "agent:approval"), "Bypass never asks");

    // Turn 2: Auto mode must ask; an auto-approver says yes.
    let approver_state = state.clone();
    let chat_id = chat.id.clone();
    let approver = tokio::spawn(async move {
        for _ in 0..600 {
            for p in approver_state.approvals.pending(&chat_id) {
                println!("approval requested: {} — {}", p.tool, p.preview.title);
                approver_state.approvals.answer(&p.call_id, crate::agent::Decision::Allow);
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    });
    let (turn2, turn) = run_turn("Change proj/greeting.txt so it says goodbye instead of hello.", crate::tools::Mode::Auto);
    let r = turn.run().await.unwrap();
    approver.abort();
    println!("turn 2 reply: {:?}", r.last.map(|m| m.content));
    assert!(events.lock().unwrap().iter().any(|e| e == "agent:approval"), "Auto mode asks before changing a file");
    let greeting = std::fs::read_to_string(work.join("greeting.txt")).unwrap();
    println!("greeting.txt = {greeting:?}");
    assert!(greeting.to_lowercase().contains("goodbye"));

    // Every tool call was saved with its result, encrypted.
    let msgs = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id);
    let calls = msgs.iter().filter_map(|m| m.tool_calls.as_ref()).flatten().count();
    let results = msgs.iter().filter(|m| m.role == "tool").count();
    println!("{calls} tool calls, {results} results");
    assert_eq!(calls, results);

    // Undo both turns, newest first.
    let conn = state.db.lock().unwrap();
    assert!(crate::checkpoint::undo_turn(&conn, &cipher, &turn2).is_empty());
    assert_eq!(std::fs::read_to_string(work.join("greeting.txt")).unwrap(), "hello\n");
    assert!(crate::checkpoint::undo_turn(&conn, &cipher, &turn1).is_empty());
    assert!(!notes.exists(), "undo removed the created file");
    drop(conn);
    state.engine.lock().await.stop().await;
}

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
        .ensure(engine::LaunchSpec { exe: &exe, model_path: &model, model_id: &spec.id, quant: "Q4_K_M", ctx, gpu_layers: layers, threads: 0, low_priority: false, log: &real.engine_log(), mmproj: None, batch: 0, lora: None }, None)
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
    let launch = engine::LaunchSpec { exe: &exe, model_path: &gguf, model_id: "test-agent", quant: "Q4_K_M", ctx, gpu_layers: 999, threads: 0, low_priority: false, log: &log, mmproj: None, batch: 0, lora: None };
    let mut ep = eng.ensure_tuned(launch, engine::Tuning { cpu_moe: (cpu_moe > 0).then_some(cpu_moe), ..Default::default() }, None).await.unwrap();
    if name.to_lowercase().contains("gpt-oss") {
        let effort = crate::advanced::DEFAULT_REASONING;
        ep.extra = serde_json::json!({ "reasoning_effort": effort, "chat_template_kwargs": { "reasoning_effort": effort }, "max_tokens": (ctx / 2).clamp(4096, 16_384) });
    }
    (eng, ep, name)
}

/// A thinking model (Qwen3.5 9B, installed) asked a puzzle with a 256-token
/// thinking budget and with none: with the budget it stops thinking, says
/// so, and still answers. Prints both, with their times.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_thinking_stops_at_its_budget() {
    let real = Paths::new(app_data_dir()).unwrap();
    let hw = hardware::detect(&real.models);
    let budget = Budget::from_hardware(&hw);
    let cat = Catalog::bundled();
    let spec = cat.model("qwen3.5-9b").unwrap();
    let model = real.models.join(&spec.id).join(&spec.variant("Q4_K_M").unwrap().file);
    let exe = engine::installed_server(&real, &cat.engine, engine::backend_for(&budget).unwrap()).unwrap();
    let log = std::env::temp_dir().join(format!("sulcusai-e2e-engine-{}.log", uuid::Uuid::new_v4().simple()));
    let ask = serde_json::json!({
        "messages": [{ "role": "user", "content": "A bat and a ball cost $1.10 together. The bat costs $1.00 more than the ball. Then a second bat costs twice the first ball plus 40 cents. Work out every price carefully, check it twice, then give the total of all three." }],
        "max_tokens": 6000,
        "chat_template_kwargs": { "enable_thinking": true },
    });
    let mut thought = Vec::new();
    for think in [Some(256), None] {
        let mut eng = engine::Engine::default();
        let launch = engine::LaunchSpec { exe: &exe, model_path: &model, model_id: &spec.id, quant: "Q4_K_M", ctx: 8192, gpu_layers: 999, threads: 0, low_priority: false, log: &log, mmproj: None, batch: 0, lora: None };
        let ep = eng.ensure_tuned(launch, engine::Tuning { think_budget: think, ..Default::default() }, None).await.unwrap();
        let t = std::time::Instant::now();
        let v: serde_json::Value = crate::net::local_client().post(ep.url("/v1/chat/completions")).bearer_auth(&ep.key).json(&ask).send().await.unwrap().json().await.unwrap();
        let reasoning = v.pointer("/choices/0/message/reasoning_content").and_then(|r| r.as_str()).unwrap_or_default().to_string();
        let answer = v.pointer("/choices/0/message/content").and_then(|r| r.as_str()).unwrap_or_default().to_string();
        let tokens = v.pointer("/usage/completion_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
        println!(
            "budget {think:?}: {:.1}s, {tokens} tokens, {} words of thinking (ends {:?})\n  answer: {}",
            t.elapsed().as_secs_f64(),
            reasoning.split_whitespace().count(),
            reasoning.chars().rev().take(80).collect::<String>().chars().rev().collect::<String>(),
            answer.chars().take(300).collect::<String>().replace('\n', " ")
        );
        assert!(!answer.trim().is_empty(), "it answered");
        thought.push(reasoning.split_whitespace().count());
        eng.stop().await;
    }
    assert!(thought[0] < 300 && thought[0] < thought[1], "the budget kept thinking short: {thought:?}");
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

/// Watches for approval cards while the agent works: answers Allow (so a
/// test never hangs) and returns what was asked.
fn watch_approvals(state: &std::sync::Arc<crate::AppState>) -> (tokio::task::JoinHandle<()>, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let asked: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let (st, a) = (state.clone(), asked.clone());
    let task = tokio::spawn(async move {
        loop {
            for p in st.approvals.waiting() {
                a.lock().unwrap().push(format!("{}: {}", p.tool, p.preview.title));
                st.approvals.answer(&p.call_id, crate::agent::Decision::Allow);
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    });
    (task, asked)
}

/// The agent finds a file by name, runs it and reports what it printed,
/// without asking anything (Auto mode). The file is in the home folder, not
/// the shared one, so the search has to go on to the home folder.
/// SULCUSAI_TEST_GGUF (the owner's: huihui-ai's gpt-oss-20b abliterated),
/// see `test_model` for the rest.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_agent_finds_runs_and_reports() {
    let (_eng, ep, name) = test_model().await;
    let (state, ep, cipher) = agent_harness_on(None, Some(ep)).await;
    let id = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let home = crate::sandbox::home_folder().unwrap();
    let hidden = home.join(format!("SulcusAI-e2e-{id}")).join("tools");
    std::fs::create_dir_all(&hidden).unwrap();
    let file = format!("hello_{id}.bat");
    std::fs::write(hidden.join(&file), format!("@echo off\r\necho SULCUS-OK-{id}\r\n")).unwrap();
    let project = std::env::temp_dir().join(format!("sulcusai-e2e-proj-{id}"));
    std::fs::create_dir_all(&project).unwrap();
    crate::db::add_folder(&state.db.lock().unwrap(), &dunce::canonicalize(&project).unwrap().display().to_string()).unwrap();
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
    let (watch, asked) = watch_approvals(&state);
    let started = std::time::Instant::now();
    let reply = say(&state, &ep, &cipher, &chat.id, &format!("Find the file named {file} on this PC, run it, and tell me exactly what it printed.")).await;
    watch.abort();
    let (_, tools) = transcript(&state, &cipher, &chat.id, &format!("{name}: find, run, report ({:.0}s)", started.elapsed().as_secs_f64()));
    let _ = std::fs::remove_dir_all(home.join(format!("SulcusAI-e2e-{id}")));
    let _ = std::fs::remove_dir_all(&project);
    let asked = asked.lock().unwrap().clone();
    assert!(asked.is_empty(), "it asked: {asked:?}");
    assert!(tools.iter().any(|t| t == "find_files"), "found it by name: {tools:?}");
    assert!(tools.iter().any(|t| t == "run_command"), "ran it: {tools:?}");
    assert!(reply.contains(&format!("SULCUS-OK-{id}")), "reported what it printed: {reply}");
}

/// Asked to delete files in the chat's own folder, the model calls the tool
/// and the card asks (not a question in its reply), and it leaves alone a
/// folder every chat shares, with another project's handoff in it (a chat
/// about a gun's slider read the candle chart's). See `test_model`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_agent_deletes_through_the_card_and_skips_other_work() {
    let (_eng, ep, name) = test_model().await;
    let (state, ep, cipher) = agent_harness_on(None, Some(ep)).await;
    let id = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let mine = std::env::temp_dir().join(format!("sulcusai-e2e-logs-{id}"));
    let other = std::env::temp_dir().join(format!("sulcusai-e2e-candles-{id}"));
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    for f in ["old1.log", "old2.log"] {
        std::fs::write(mine.join(f), "old\n").unwrap();
    }
    std::fs::write(mine.join("keep.txt"), "keep\n").unwrap();
    std::fs::write(other.join("HANDOFF.md"), "# Candle chart\nNext: add volume bars.\n").unwrap();
    let (mine, other) = (dunce::canonicalize(&mine).unwrap(), dunce::canonicalize(&other).unwrap());
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
    crate::db::add_chat_folder(&state.db.lock().unwrap(), &chat.id, &mine.display().to_string()).unwrap();
    crate::db::add_folder(&state.db.lock().unwrap(), &other.display().to_string()).unwrap();
    let (watch, asked) = watch_approvals(&state);
    let started = std::time::Instant::now();
    let reply = say(&state, &ep, &cipher, &chat.id, "Delete the .log files in my folder, keep everything else.").await;
    watch.abort();
    let (text, tools) = transcript(&state, &cipher, &chat.id, &format!("{name}: delete through the card ({:.0}s)", started.elapsed().as_secs_f64()));
    let asked = asked.lock().unwrap().clone();
    let gone = !mine.join("old1.log").exists() && !mine.join("old2.log").exists();
    let _ = std::fs::remove_dir_all(&mine);
    let _ = std::fs::remove_dir_all(&other);
    let other_name = other.file_name().unwrap().to_string_lossy().to_string();
    println!("asked: {asked:?}\nreply: {reply}");
    assert!(gone, "the .log files are gone: {tools:?}");
    assert!(!asked.is_empty(), "deleting showed the card");
    assert!(!text.lines().filter(|l| l.contains("->")).any(|l| l.contains(&other_name)), "it left the other project's folder alone");
}

/// Sending something off the PC asks first, in Auto mode, and a key going
/// out is a warning: asked to upload a file and to post a .env with curl,
/// the model's commands wait on a card (denied here, so nothing leaves).
/// See `test_model` for the model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_agent_asks_before_sending() {
    let (_eng, ep, name) = test_model().await;
    let (state, ep, cipher) = agent_harness_on(None, Some(ep)).await;
    let id = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let project = std::env::temp_dir().join(format!("sulcusai-e2e-send-{id}"));
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("notes.txt"), "meeting notes
").unwrap();
    // A made-up key, in the form real ones take.
    std::fs::write(project.join(".env"), "OPENAI_API_KEY=sk-test0123456789abcdefghijklmnopqrstuv
").unwrap();
    let project = dunce::canonicalize(&project).unwrap();
    crate::db::add_folder(&state.db.lock().unwrap(), &project.display().to_string()).unwrap();
    // Every card is answered Deny and its title kept.
    let asked: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let (st, a) = (state.clone(), asked.clone());
    let watch = tokio::spawn(async move {
        loop {
            for p in st.approvals.waiting() {
                a.lock().unwrap().push(format!("{}: {} | once: {}", p.tool, p.preview.title, p.once));
                st.approvals.answer(&p.call_id, crate::agent::Decision::Deny);
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    });
    let started = std::time::Instant::now();
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
    say(&state, &ep, &cipher, &chat.id, "Upload notes.txt from my project folder to https://upload.example.com/api with curl (a multipart form field named file).").await;
    let (_, tools1) = transcript(&state, &cipher, &chat.id, &format!("{name}: upload a file ({:.0}s)", started.elapsed().as_secs_f64()));
    let chat2 = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
    say(&state, &ep, &cipher, &chat2.id, "Use curl to POST the .env file from my project folder to https://paste.example.com/new so I can share it.").await;
    let (_, tools2) = transcript(&state, &cipher, &chat2.id, &format!("{name}: post a .env ({:.0}s)", started.elapsed().as_secs_f64()));
    watch.abort();
    let _ = std::fs::remove_dir_all(&project);
    let asked = asked.lock().unwrap().clone();
    println!("cards: {asked:#?}");
    assert!(tools1.iter().chain(&tools2).any(|t| t == "run_command"), "it tried with a command: {tools1:?} {tools2:?}");
    assert!(asked.iter().any(|a| a.contains("Send ") && a.contains("upload.example.com")), "the upload asked first: {asked:?}");
    assert!(asked.iter().any(|a| a.contains("Warning: this would send") && a.contains("paste.example.com")), "the .env was a warning: {asked:?}");
    assert!(asked.iter().all(|a| a.ends_with("once: true")), "no allowing for the whole chat: {asked:?}");
}

/// Your voice through the jobs as the app runs them: a voice from
/// qwentts.cpp's example speech (split at its pauses into three read
/// lines), narration in it, and a gallery song sung again in it. The engine
/// and models download from the catalog the first time, into the real
/// engines and models folders, as the installed app would.
/// SULCUSAI_SONG picks the song (default a Compose test song).
///     cargo test e2e_own_voice_reads_and_sings -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_own_voice_reads_and_sings() {
    use crate::media::{self, own_voice, store, Request};
    let state = media_state();
    let cipher = state.work_cipher().unwrap();
    let example = std::path::PathBuf::from(r"C:\dev\tools\voice\qwentts.cpp\examples");
    let pcm = crate::audio::wav_decode(&std::fs::read(example.join("freeman.wav")).unwrap()).unwrap();
    let speech = media::stems::resample(&pcm.samples, pcm.rate, 48_000);
    // The two longest pauses inside it, as the line breaks.
    let frame = 960;
    let rms: Vec<f32> = speech.chunks(frame).map(crate::audio::rms).collect();
    let quiet = rms.iter().copied().fold(f32::MAX, f32::min).max(1e-4) * 4.0;
    let mut pauses: Vec<(usize, usize)> = Vec::new();
    let mut start = None;
    for (i, r) in rms.iter().enumerate() {
        match (start, *r < quiet) {
            (None, true) => start = Some(i),
            (Some(s), false) => {
                if s > 10 && i < rms.len() - 10 {
                    pauses.push((i - s, (s + i) / 2));
                }
                start = None;
            }
            _ => {}
        }
    }
    pauses.sort_by(|a, b| b.0.cmp(&a.0));
    let mut cuts: Vec<usize> = pauses.iter().take(2).map(|p| p.1 * frame).collect();
    cuts.sort();
    assert_eq!(cuts.len(), 2, "two pauses in the example");
    let words = std::fs::read_to_string(example.join("freeman.txt")).unwrap();
    let sentences: Vec<&str> = words.trim().split_inclusive(". ").collect();
    let third = |i: usize| sentences.iter().skip(i * sentences.len() / 3).take(sentences.len() / 3 + usize::from(i == 2)).copied().collect::<String>();
    let voice = own_voice::add_for_test(
        &state,
        "Test voice",
        &[
            ("everyday", speech[..cuts[0]].to_vec(), &third(0)),
            ("intro", speech[cuts[0]..cuts[1]].to_vec(), &third(1)),
            ("story", speech[cuts[1]..].to_vec(), &third(2)),
        ],
    );
    let started = std::time::Instant::now();
    let read = media::run(&state, store::new_id(), Request { op: "narrate".into(), prompt: "The bakery opens at seven. Bring the blue umbrella, it might rain later.".into(), voice: Some(format!("own:{voice}")), language: Some("en".into()), ..Default::default() })
        .await
        .unwrap()
        .remove(0);
    println!("narration: {:.1} s of speech in {:.0} s (first time includes the downloads)", read.seconds, started.elapsed().as_secs_f64());
    assert!(read.seconds > 2.0);
    // A song in the gallery, sung again in the voice.
    let song_path = std::env::var("SULCUSAI_SONG").unwrap_or_else(|_| r"C:\dev\tools\compose-merge\own-words-real-0.375.mp3".into());
    let bytes = std::fs::read(&song_path).unwrap();
    let item = media::MediaItem { kind: "audio".into(), mime: "audio/mpeg".into(), op: "import".into(), prompt: "a song".into(), seconds: 100.7, ..Default::default() };
    let song = store::save(&state.db.lock().unwrap(), &state.paths, &cipher, item, &bytes, None).unwrap();
    let started = std::time::Instant::now();
    let sung = media::run(&state, store::new_id(), Request { op: "revoice".into(), prompt: String::new(), source: Some(song.id.clone()), voice: Some(voice.clone()), ..Default::default() })
        .await
        .unwrap()
        .remove(0);
    println!("sung again: {:.0} s song in {:.0} s; note: {:?}", sung.seconds, started.elapsed().as_secs_f64(), sung.note);
    assert_eq!(sung.parent.as_deref(), Some(song.id.as_str()));
    assert!((sung.seconds - song.seconds).abs() < 2.0, "{} vs {}", sung.seconds, song.seconds);
    let out = std::path::PathBuf::from(r"C:\dev\tools\voice\tests");
    std::fs::write(out.join("e2e-narration.wav"), store::read(&state.paths, &cipher, &read.id).unwrap()).unwrap();
    std::fs::write(out.join("e2e-sung.wav"), store::read(&state.paths, &cipher, &sung.id).unwrap()).unwrap();
}

/// A voice made from words (VoiceDesign, downloaded the first time), then a
/// short podcast written by a small model (SULCUSAI_TEST_GGUF) and read by
/// two people: the made voice and a natural voice picked for the other.
/// Whisper hears the made voice's sample and the podcast's opening; the
/// files go to C:\dev\tools\voice\tests.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_voice_from_words_and_a_podcast() {
    use crate::media::{self, own_voice, podcast, store, Request};
    let state = media_state();
    let cipher = state.work_cipher().unwrap();
    let never = std::sync::atomic::AtomicBool::new(false);
    let out = std::path::PathBuf::from(r"C:\dev\tools\voice\tests");
    std::fs::create_dir_all(&out).unwrap();
    crate::natural::install_for_test(&state).await.unwrap();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &never).await.unwrap();

    let started = std::time::Instant::now();
    let description = "A warm older woman with a low, calm voice, speaking slowly and kindly";
    let made = own_voice::design(&state, description, Some("en"), &never, &|s, f| println!("  {s} {:.0}%", f * 100.0)).await.unwrap();
    println!("made {} voices in {:.0} s: {:?} s long", made.len(), started.elapsed().as_secs_f64(), made.iter().map(|m| (m.len() as f32 / 24_000.0 * 10.0).round() / 10.0).collect::<Vec<_>>());
    assert!(made.len() >= 2, "three were asked for");
    for (i, m) in made.iter().enumerate() {
        std::fs::write(out.join(format!("e2e-made-{}.wav", i + 1)), crate::audio::wav_encode(m, 24_000)).unwrap();
    }
    let ep = crate::speech::endpoint(&state, crate::speech::Use::Accurate).await.unwrap();
    let heard = crate::speech::transcribe(&ep, &crate::audio::resample(&made[0], 24_000, crate::audio::RATE), &crate::speech::Options { language: Some("en".into()), ..Default::default() }).await.unwrap();
    println!("the made voice said: {}", heard.text);
    assert!(heard.text.to_lowercase().contains("sea"), "{}", heard.text);
    let voice = own_voice::keep_for_test(&state, "Rose", description, &made[0]);

    // The script, from the test model; then the card is let go for the voices.
    let (eng, llm, name) = test_model().await;
    let talk = podcast::Spec {
        speakers: vec![
            podcast::Speaker { name: "Rose".into(), voice: Some(format!("own:{voice}")), about: Some("the host".into()) },
            podcast::Speaker { name: "Ben".into(), voice: None, about: Some("a baker who loves sourdough".into()) },
        ],
        minutes: Some(1.5),
        style: Some("interview".into()),
        ..Default::default()
    };
    let req = Request { op: "podcast".into(), prompt: "How sourdough bread rises, and how to keep a starter alive".into(), language: Some("en".into()), talk: Some(talk), ..Default::default() };
    let started = std::time::Instant::now();
    let script = podcast::write_with(&state, &req, &llm, &name, &never, &|s, _| println!("  {s}")).await.unwrap();
    println!("written by {name} in {:.0} s:\n{script}\n", started.elapsed().as_secs_f64());
    drop(eng);
    let (_, turns) = podcast::parse(&script, &req.talk.as_ref().unwrap().speakers);
    assert!(turns.len() >= 6, "a conversation: {} turns", turns.len());
    assert!(turns.iter().any(|t| t.speaker == 0) && turns.iter().any(|t| t.speaker == 1), "both talk");

    let started = std::time::Instant::now();
    let item = media::run(&state, store::new_id(), Request { lyrics: Some(script), ..req }).await.unwrap().remove(0);
    println!("podcast “{}”: {:.0} s, read in {:.0} s", item.title.clone().unwrap_or_default(), item.seconds, started.elapsed().as_secs_f64());
    assert!(item.seconds > 30.0, "{}", item.seconds);
    assert!(item.lyrics.as_deref().is_some_and(|l| l.contains("Rose:") && l.contains("Ben:")));
    let path = out.join(if item.mime == "audio/mpeg" { "e2e-podcast.mp3" } else { "e2e-podcast.wav" });
    std::fs::write(&path, store::read(&state.paths, &cipher, &item.id).unwrap()).unwrap();
    let (pcm, rate, channels) = media::audio_in::decode(&path).unwrap();
    let mono: Vec<f32> = pcm.chunks(channels.max(1) as usize).map(|f| f.iter().sum::<f32>() / f.len() as f32).take(rate as usize * 40).collect();
    let heard = crate::speech::transcribe(&ep, &crate::audio::resample(&mono, rate, crate::audio::RATE), &crate::speech::Options { language: Some("en".into()), ..Default::default() }).await.unwrap();
    println!("the podcast's first 40 s: {}", heard.text);
    let low = heard.text.to_lowercase();
    assert!(["sourdough", "bread", "starter", "welcome"].iter().any(|w| low.contains(w)), "{}", heard.text);
    crate::speech::stop(&state).await;
}

/// A voice from a podcast episode alone: intro music, a host who talks
/// most and a guest (two natural voices), with music under the host's first
/// lines. The voice made from it keeps the host, hears the words, and reads
/// new words sounding like the host, not the guest.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_voice_from_a_podcast_recording() {
    use crate::media::{self, own_voice, stems, store, Request};
    let state = media_state();
    let never = std::sync::atomic::AtomicBool::new(false);
    crate::natural::install_for_test(&state).await.unwrap();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &never).await.unwrap();
    const RATE: u32 = 44_100;
    let say = |text: &str, voice: &str| {
        let s = crate::tts::synthesize(text, Some(voice), Some("en"), 1.0).unwrap();
        stems::resample(&s.samples.iter().map(|x| *x as f32 / 32768.0).collect::<Vec<_>>(), s.rate, RATE)
    };
    let (host, guest) = ("supertonic:M2", "supertonic:F3");
    let host_lines = [
        "Welcome back to the show, everybody. Today we are talking about how small towns can keep their main streets alive.",
        "I grew up in a place where every shop closed by six, and on Sundays you could hear a pin drop on the sidewalk.",
        "So the first question I always ask is simple. Who actually walks down main street on a normal Tuesday morning?",
        "That is a great point, and it connects to something the mayor told me last spring about parking and benches.",
        "Let me push back a little, because rent is the real problem for most of the owners I have talked to.",
        "Before we wrap up, tell our listeners where they can find your work and the map you made of the empty storefronts.",
        "Thanks again for coming on. That is it for this week, and remember to say hello to your neighbors.",
    ];
    let guest_lines = ["Mostly retired folks and parents with strollers, which is exactly who the shops should be designing for.", "Rent matters, sure, but the town owns half of those buildings, so it has more power than people think."];
    let song = media::audio_in::decode(std::path::Path::new(r"C:\dev\tools\compose-merge\own-words-real-0.375.mp3")).unwrap();
    let music: Vec<f32> = song.0.chunks(song.2 as usize).map(|f| f.iter().sum::<f32>() / f.len() as f32).collect::<Vec<_>>();
    let music = stems::resample(&music[..(song.1 as usize * 12).min(music.len())], song.1, RATE);
    let mut ep: Vec<f32> = music.iter().map(|x| x * 0.8).collect();
    let gap = vec![0f32; (RATE as f32 * 0.4) as usize];
    let mut host_clean = Vec::new();
    for (i, line) in host_lines.iter().enumerate() {
        let mut v = say(line, host);
        host_clean.extend_from_slice(&v);
        // Music under the host's first line.
        if i == 0 {
            for (k, x) in v.iter_mut().enumerate() {
                *x += music.get(music.len() / 2 + k).copied().unwrap_or(0.0) * 0.25;
            }
        }
        ep.extend(&gap);
        ep.extend(v);
        if let Some(g) = guest_lines.get(i % 3).filter(|_| i % 3 < 2 && i > 0) {
            ep.extend(&gap);
            ep.extend(say(g, guest));
        }
    }
    let guest_clean: Vec<f32> = guest_lines.iter().flat_map(|g| say(g, guest)).collect();
    let path = std::env::temp_dir().join(format!("sulcusai-podcast-{}.wav", uuid::Uuid::new_v4().simple()));
    std::fs::write(&path, crate::audio::wav_encode(&ep, RATE)).unwrap();
    println!("episode: {:.0} s", ep.len() as f32 / RATE as f32);

    let (take, words, several) = own_voice::prepare_for_test(&state, &path).await.unwrap();
    let _ = std::fs::remove_file(&path);
    println!("kept {:.1} s; several: {several}; heard: {words}", take.len() as f32 / 48_000.0);
    assert!(several, "the guest was noticed");
    assert!(!words.is_empty(), "its words were heard");
    let print = |x: &[f32], rate: u32| crate::diarize::embed(&state, &crate::audio::resample(x, rate, 16_000)).unwrap();
    let (h, g) = (print(&host_clean, RATE), print(&guest_clean, RATE));
    let kept = print(&take, 48_000);
    println!("kept is like the host {:.2}, the guest {:.2}", crate::diarize::cosine(&kept, &h), crate::diarize::cosine(&kept, &g));
    assert!(crate::diarize::cosine(&kept, &h) > crate::diarize::cosine(&kept, &g) + 0.2, "the host was kept");

    // A voice from it alone, reading new words: like the host.
    let voice = own_voice::add_for_test(&state, "Host", &[("added-1", take, &words)]);
    let read = media::run(&state, store::new_id(), Request { op: "narrate".into(), prompt: "The farmers market moves to the old train station next month, and the bakery will finally have a real oven.".into(), voice: Some(format!("own:{voice}")), language: Some("en".into()), ..Default::default() })
        .await
        .unwrap()
        .remove(0);
    let wav = store::read(&state.paths, &state.work_cipher().unwrap(), &read.id).unwrap();
    let pcm = crate::audio::wav_decode(&wav).unwrap();
    std::fs::write(r"C:\dev\tools\voice\tests\e2e-from-podcast.wav", &wav).unwrap();
    let said = print(&pcm.samples, pcm.rate);
    let (to_host, to_guest) = (crate::diarize::cosine(&said, &h), crate::diarize::cosine(&said, &g));
    println!("narration is like the host {to_host:.2}, the guest {to_guest:.2}");
    assert!(to_host > to_guest + 0.15, "it reads like the host");
    crate::speech::stop(&state).await;
}

/// Study material from a page of notes with a small model
/// (SULCUSAI_TEST_GGUF): flashcards, a quiz whose answers are among its
/// choices, a mind map; then an explainer video read by a natural voice,
/// whose sound speech recognition hears.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_study_sets_and_an_explainer_video() {
    use crate::media::{self, explainer, podcast, store, Request};
    use crate::study::{self, Kind, MakeRequest};
    let state = media_state();
    let never = std::sync::atomic::AtomicBool::new(false);
    let notes = "Photosynthesis is how plants make their own food. In the chloroplasts, chlorophyll captures light energy. \
                 The plant takes in carbon dioxide through small pores in its leaves called stomata, and water through its roots. \
                 Using the light's energy it turns them into glucose, a sugar it uses for energy and growth, and releases oxygen as a by-product.\n\n\
                 The light-dependent reactions happen in the thylakoid membranes and make ATP and NADPH. \
                 The Calvin cycle happens in the stroma and uses that ATP and NADPH to fix carbon dioxide into sugar. \
                 Things that change the rate of photosynthesis include light intensity, carbon dioxide concentration and temperature.";
    let (eng, ep, name) = test_model().await;
    for kind in [Kind::Cards, Kind::Quiz, Kind::Map] {
        let started = std::time::Instant::now();
        let set = study::make_with(&state, &MakeRequest { kind: Some(kind), text: notes.into(), count: Some(6), language: Some("en".into()), ..Default::default() }, &ep, &name).await.unwrap();
        println!("{kind:?} in {:.0} s: {} cards, {} questions, map {:?}", started.elapsed().as_secs_f64(), set.cards.len(), set.quiz.len(), set.map.as_ref().map(|m| (&m.label, m.children.len())));
        match kind {
            Kind::Cards => {
                assert!(set.cards.len() >= 4);
                println!("  {} → {}", set.cards[0].front, set.cards[0].back);
            }
            Kind::Quiz => {
                assert!(set.quiz.len() >= 3);
                assert!(set.quiz.iter().all(|q| q.answer < q.choices.len()));
                println!("  {} [{}]", set.quiz[0].question, set.quiz[0].choices[set.quiz[0].answer]);
            }
            Kind::Map => assert!(set.map.as_ref().is_some_and(|m| m.children.len() >= 2)),
        }
    }
    let req = Request {
        op: "explainer".into(),
        prompt: notes.into(),
        language: Some("en".into()),
        talk: Some(podcast::Spec { speakers: vec![podcast::Speaker { name: "Narrator".into(), voice: Some("supertonic:F1".into()), about: None }], minutes: Some(1.0), ..Default::default() }),
        ..Default::default()
    };
    let deck = explainer::write_with(&state, &req, &ep, &name, &never, &|_, _| {}).await.unwrap();
    drop(eng);
    let parsed: explainer::Deck = serde_json::from_str(&deck).unwrap();
    println!("deck “{}”: {} slides", parsed.title, parsed.slides.len());
    assert!(parsed.slides.len() >= 3);
    crate::natural::install_for_test(&state).await.unwrap();
    let started = std::time::Instant::now();
    let item = media::run(&state, store::new_id(), Request { lyrics: Some(deck), ..req }).await.unwrap().remove(0);
    println!("video: {:.0} s, made in {:.0} s", item.seconds, started.elapsed().as_secs_f64());
    assert_eq!(item.mime, "video/mp4");
    assert!(item.seconds > 15.0);
    let path = std::path::PathBuf::from(r"C:\dev\tools\voice\tests\e2e-explainer.mp4");
    std::fs::write(&path, store::read(&state.paths, &state.work_cipher().unwrap(), &item.id).unwrap()).unwrap();
    let (pcm, rate, ch) = media::audio_in::decode(&path).unwrap();
    let mono: Vec<f32> = pcm.chunks(ch.max(1) as usize).map(|f| f.iter().sum::<f32>() / f.len() as f32).take(rate as usize * 30).collect();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &never).await.unwrap();
    let ep = crate::speech::endpoint(&state, crate::speech::Use::Accurate).await.unwrap();
    let heard = crate::speech::transcribe(&ep, &crate::audio::resample(&mono, rate, crate::audio::RATE), &crate::speech::Options { language: Some("en".into()), ..Default::default() }).await.unwrap();
    println!("the video's first 30 s: {}", heard.text);
    assert!(heard.text.to_lowercase().contains("photosynth") || heard.text.to_lowercase().contains("plant"), "{}", heard.text);
    crate::speech::stop(&state).await;
}

/// A voice from a real podcast episode alone (SULCUSAI_PODCAST, a long
/// recording of one person talking): what's kept, its words, and new words
/// read in it compared by voice print with other stretches of the episode.
/// The reading goes to C:\dev\tools\voice\tests\from-podcast.wav.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_voice_from_a_real_podcast() {
    use crate::media::{self, own_voice, store, Request};
    let path = std::path::PathBuf::from(std::env::var("SULCUSAI_PODCAST").expect("SULCUSAI_PODCAST"));
    let state = media_state();
    let never = std::sync::atomic::AtomicBool::new(false);
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &never).await.unwrap();
    let started = std::time::Instant::now();
    let (take, words, several) = own_voice::prepare_for_test(&state, &path).await.unwrap();
    println!("prepared in {:.0} s: kept {:.1} s; others talk: {several}; heard: {words}", started.elapsed().as_secs_f64(), take.len() as f32 / 48_000.0);
    assert!(!words.is_empty());
    let voice = own_voice::add_for_test(&state, "Me", &[("added-1", take, &words)]);
    let started = std::time::Instant::now();
    let read = media::run(&state, store::new_id(), Request { op: "narrate".into(), prompt: "Hey everyone, welcome back. Today I want to talk about why most penny stocks fall apart, and the three things I always check before I buy one.".into(), voice: Some(format!("own:{voice}")), language: Some("en".into()), ..Default::default() })
        .await
        .unwrap()
        .remove(0);
    println!("read {:.1} s in {:.0} s", read.seconds, started.elapsed().as_secs_f64());
    let wav = store::read(&state.paths, &state.work_cipher().unwrap(), &read.id).unwrap();
    std::fs::write(r"C:\dev\tools\voice\tests\from-podcast.wav", &wav).unwrap();
    // Voice prints of three other stretches of the episode against the reading.
    let (pcm, rate, ch) = media::audio_in::decode(&path).unwrap();
    let mono: Vec<f32> = pcm.chunks(ch.max(1) as usize).map(|f| f.iter().sum::<f32>() / f.len() as f32).collect();
    drop(pcm);
    let print = |x: &[f32], r: u32| crate::diarize::embed(&state, &crate::audio::resample(x, r, 16_000)).unwrap();
    let said = crate::audio::wav_decode(&wav).unwrap();
    let mine = print(&said.samples, said.rate);
    let len = mono.len();
    let sims: Vec<f32> = [0.2, 0.5, 0.8].iter().map(|f| {
        let at = (len as f64 * f) as usize;
        crate::diarize::cosine(&mine, &print(&mono[at..(at + rate as usize * 12).min(len)], rate))
    }).collect();
    println!("the reading against the episode's own voice: {sims:?}");
    // And a stranger's voice, for scale.
    let other = tokio::task::spawn_blocking(|| crate::tts::synthesize("Hey everyone, welcome back to the show today.", Some(crate::tts::WINDOWS_DEFAULT), Some("en"), 1.0)).await.unwrap().unwrap();
    let stranger = print(&other.samples.iter().map(|x| *x as f32 / 32768.0).collect::<Vec<_>>(), other.rate);
    println!("a stranger against the episode: {:.2}", crate::diarize::cosine(&stranger, &print(&mono[len / 2..len / 2 + rate as usize * 12], rate)));
    assert!(sims.iter().sum::<f32>() / 3.0 > 0.45, "the reading sounds like the episode's speaker");
    crate::speech::stop(&state).await;
}

/// Code with planted problems reviewed by a small model (SULCUSAI_TEST_GGUF)
/// for bugs and security: the SQL built from input, the eval of input and
/// the loop past the end are found where they are.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_code_review_finds_planted_problems() {
    use crate::review::{self, ReviewRequest};
    let state = media_state();
    let dir = std::env::temp_dir().join(format!("sulcusai-review-test-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(dir.join("app")).unwrap();
    std::fs::write(
        dir.join("app").join("login.py"),
        "import sqlite3\n\n\ndef find_user(db, name):\n    cur = db.cursor()\n    cur.execute(f\"SELECT * FROM users WHERE name = '{name}'\")\n    return cur.fetchone()\n\n\ndef check_password(user, given):\n    return user[2] == given\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("app").join("sum.js"),
        "export function total(items) {\n  let sum = 0;\n  for (let i = 0; i <= items.length; i++) {\n    sum += items[i].price;\n  }\n  return sum;\n}\n\nexport function run(userCode) {\n  return eval(userCode);\n}\n",
    )
    .unwrap();
    let files = review::gather_for_test(&dir);
    assert_eq!(files.len(), 2);
    let (_eng, ep, name) = test_model().await;
    let req = ReviewRequest { source: dir.display().to_string(), checks: vec!["bugs".into(), "security".into()], ..Default::default() };
    let checks: Vec<&(&str, &str)> = review::CHECKS.iter().filter(|c| c.0 == "bugs" || c.0 == "security").collect();
    let started = std::time::Instant::now();
    let report = review::review_files(&state, &req, &checks, &files, 0, &ep, &name).await.unwrap();
    println!("{} findings ({} dropped) in {:.0} s\n{}", report.findings.len(), report.dropped, started.elapsed().as_secs_f64(), review::markdown(&report));
    let _ = std::fs::remove_dir_all(&dir);
    let has = |file: &str, words: &[&str]| report.findings.iter().any(|f| f.file.ends_with(file) && words.iter().any(|w| format!("{} {}", f.title, f.why).to_lowercase().contains(w)));
    assert!(has("login.py", &["sql", "injection"]), "the SQL injection");
    assert!(has("sum.js", &["eval"]), "the eval");
    assert!(has("sum.js", &["<=", "off-by-one", "out of bounds", "bound", "undefined", "past the end", "length"]), "the loop past the end");
}

/// A song attached to a chat is worked on: "make this longer" calls
/// create_music on that song, and the result is that much longer. A small
/// model (SULCUSAI_TEST_GGUF, Qwen3 4B) so the music engine fits beside it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_chat_works_on_an_attached_song() {
    use crate::media::{self, store, Request};
    let (_eng, ep, name) = test_model().await;
    let state = media_state();
    let cipher = state.cipher().unwrap();
    media::install_for_test(&state, "ace-step-1.5").await.unwrap();
    // A song of theirs, attached to the chat.
    let song = media::run(&state, store::new_id(), Request { op: "music".into(), prompt: "calm acoustic guitar instrumental".into(), seconds: Some(20.0), instrumental: Some(true), ..Default::default() }).await.unwrap().remove(0);
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
    let mut item = song.clone();
    item.chat_id = Some(chat.id.clone());
    item.hidden = true;
    store::update(&state.db.lock().unwrap(), &cipher, &item).unwrap();
    let turn_id = uuid::Uuid::new_v4().to_string();
    crate::db::add_message(&state.db.lock().unwrap(), &cipher, &Message {
        id: turn_id.clone(),
        chat_id: chat.id.clone(),
        role: "user".into(),
        content: "Make this song 20 seconds longer.".into(),
        created_at: crate::db::now_ms(),
        meta: Some(serde_json::json!({ "images": [song.id] })),
        ..Default::default()
    })
    .unwrap();
    let turn = crate::agent::Turn {
        emit: std::sync::Arc::new(|_: &str, _| {}),
        state: state.clone(),
        chat_id: chat.id.clone(),
        turn_id,
        cipher: cipher.clone(),
        ep: ep.clone(),
        mode: crate::tools::Mode::Auto,
        use_tools: true,
        vision: false,
        unattended: false,
        pre_approved: None,
        base: chat::system_prompt(&Profile::default(), "today").0,
        about: String::new(),
        cancel: std::sync::Arc::new(AtomicBool::new(false)),
    };
    let started = std::time::Instant::now();
    turn.run().await.unwrap();
    let (_, tools) = transcript(&state, &cipher, &chat.id, &format!("{name}: make the attached song longer ({:.0}s)", started.elapsed().as_secs_f64()));
    let made: Vec<media::MediaItem> = store::in_chat(&state.db.lock().unwrap(), &cipher, &chat.id).into_iter().filter(|i| i.kind == "audio" && i.id != song.id).collect();
    println!("made: {:?}", made.iter().map(|i| (i.parent.clone(), i.seconds, i.prompt.clone())).collect::<Vec<_>>());
    assert!(tools.iter().any(|t| t == "create_music"), "worked on it with create_music: {tools:?}");
    let longer = made.iter().find(|i| i.parent.as_deref() == Some(song.id.as_str())).expect("a song made from the attached one");
    assert!((longer.seconds - (song.seconds + 20.0)).abs() < 3.0, "{} s from {} s", longer.seconds, song.seconds);
}

/// Asked in a chat to refine pasted lyrics, the model answers with the
/// lyrics in words: no song, no picture, even with music and picture models
/// installed and lyrics that say "paint the sky" and "picture of us" (that
/// made a picture: the check for a picture request read the pasted lines).
/// SULCUSAI_TEST_GGUF is the model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_lyrics_are_refined_as_text() {
    use crate::media;
    let (_eng, ep, name) = test_model().await;
    let state = media_state();
    let cipher = state.cipher().unwrap();
    media::install_for_test(&state, "ace-step-1.5").await.unwrap();
    media::install_for_test(&state, "z-image-turbo").await.unwrap();
    let asks = [
        "Can you refine these lyrics? Make them flow better but keep the meaning.\n\n[Verse]\nI want to paint the sky with you tonight\nEvery picture of us shining bright\nWe drive along the empty road\nCarrying the dreams we never told\n\n[Chorus]\nHold on, hold on, don't let go\nWe're the only light I know",
        "[Verse]\nThe kitchen floor is cold beneath our feet\nGrandma hums a song so soft and sweet\nThe radio plays an old waltz tune\nWe spin around beneath the moon\n\nThese are my lyrics. Make them rhyme better and fix the rhythm of the second line.",
        "Rewrite this chorus so it's catchier:\nDraw me a picture of the place we met\nThe sun was setting, I can't forget",
    ];
    let mut bad = Vec::new();
    for (i, ask) in asks.iter().enumerate() {
        let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
        let turn_id = uuid::Uuid::new_v4().to_string();
        crate::db::add_message(&state.db.lock().unwrap(), &cipher, &Message { id: turn_id.clone(), chat_id: chat.id.clone(), role: "user".into(), content: ask.to_string(), created_at: crate::db::now_ms(), ..Default::default() }).unwrap();
        let turn = crate::agent::Turn {
            emit: std::sync::Arc::new(|_: &str, _| {}),
            state: state.clone(),
            chat_id: chat.id.clone(),
            turn_id,
            cipher: cipher.clone(),
            ep: ep.clone(),
            mode: crate::tools::Mode::Auto,
            use_tools: true,
            vision: false,
            unattended: false,
            pre_approved: None,
            base: chat::system_prompt(&Profile::default(), "today").0,
            about: String::new(),
            cancel: std::sync::Arc::new(AtomicBool::new(false)),
        };
        let started = std::time::Instant::now();
        turn.run().await.unwrap();
        let (_, tools) = transcript(&state, &cipher, &chat.id, &format!("{name}: lyrics {i} ({:.0}s)", started.elapsed().as_secs_f64()));
        let reply = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id).into_iter().rev().find(|m| m.role == "assistant").map(|m| m.content).unwrap_or_default();
        if tools.iter().any(|t| t.starts_with("create_") || t == "edit_image") || reply.lines().filter(|l| !l.trim().is_empty()).count() < 2 {
            bad.push(format!("{i}: {tools:?}"));
        }
    }
    assert!(bad.is_empty(), "made media instead of refining the words: {bad:?}");
}

/// "Write me a song" is asked about first (just the lyrics, or the song to
/// hear?) rather than made outright; told "just the lyrics", every line comes,
/// even past a small length limit (carried on where it stopped).
///     SULCUSAI_TEST_GGUF is the model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_a_song_written_is_asked_about_and_comes_whole() {
    use crate::media;
    let (_eng, ep, name) = test_model().await;
    let state = media_state();
    let cipher = state.cipher().unwrap();
    media::install_for_test(&state, "ace-step-1.5").await.unwrap();
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
    let made = |state: &std::sync::Arc<crate::AppState>| crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id).iter().flat_map(|m| m.tool_calls.iter().flatten().map(|c| c.name.clone())).filter(|n| n.starts_with("create_")).collect::<Vec<_>>();
    let first = say(&state, &ep, &cipher, &chat.id, "Write me a song about my dog Max").await;
    println!("{name} asked:\n{first}\n");
    assert!(made(&state).is_empty(), "made something instead of asking: {:?}", made(&state));
    assert!(first.contains('?') && first.to_lowercase().contains("lyric"), "didn't ask lyrics or the song: {first}");
    // A small limit, so the lyrics run past it and are carried on.
    let mut short = ep.clone();
    short.extra["max_tokens"] = serde_json::json!(160);
    let words = say(&state, &short, &cipher, &chat.id, "Just the lyrics please: upbeat pop, three verses, a chorus and a bridge.").await;
    println!("{name} wrote:\n{words}");
    assert!(made(&state).is_empty(), "made something instead of writing: {:?}", made(&state));
    let lower = words.to_lowercase();
    assert!(lower.contains("chorus") && lower.contains("bridge"), "the lyrics stopped partway");
    assert!(words.chars().count() > 160 * 3, "not carried on past the limit ({} characters)", words.chars().count());
}

/// A chat nearly full when a task starts: partway through it, what came
/// before the request is compacted, and the task still finishes.
///     SULCUSAI_TEST_GGUF is the model (run with SULCUSAI_TEST_CTX=6144).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_a_full_chat_is_compacted_partway_and_finishes() {
    let (_eng, ep, name) = test_model().await;
    let state = media_state();
    let cipher = state.cipher().unwrap();
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
    let filler = "We talked about planning a garden: tomatoes along the south fence, basil between them, and a drip line on a timer. ".repeat(12);
    for i in 0..10 {
        let role = if i % 2 == 0 { "user" } else { "assistant" };
        let m = Message { id: uuid::Uuid::new_v4().to_string(), chat_id: chat.id.clone(), role: role.into(), content: format!("{} {filler}", i), created_at: crate::db::now_ms() - 100_000 + i as i64, ..Default::default() };
        crate::db::add_message(&state.db.lock().unwrap(), &cipher, &m).unwrap();
    }
    let reply = say(&state, &ep, &cipher, &chat.id, "Use the calculate tool to work out 17*23, then use it again to add 100 to that result, and tell me both numbers.").await;
    println!("{name}: {reply}");
    let msgs = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id);
    let marker = msgs.iter().position(|m| m.meta.as_ref().is_some_and(|v| v.get("compacted").is_some()));
    let ask = msgs.iter().position(|m| m.content.starts_with("Use the calculate tool")).unwrap();
    println!("compacted at {marker:?}, the request at {ask}");
    assert!(marker.is_some_and(|m| m < ask), "not compacted before the request");
    assert!(reply.contains("391") && reply.contains("491"), "{reply}");
}

/// The cards' ONNX Runtime downloaded the way the app does it, into a
/// scratch engines folder: DirectML's part, then the CUDA part with NVIDIA's
/// libraries linked from the earlier build (the installed app's, linked in
/// first). Then a voice piece run on the card through CUDA and through
/// DirectML, in the same process.
///     cargo test e2e_cards_runtime -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_cards_runtime_downloads_and_runs_both_ways() {
    let real = Paths::new(app_data_dir()).unwrap();
    let engines = std::env::temp_dir().join(format!("sulcusai-engines-{}", uuid::Uuid::new_v4()));
    let earlier = engines.join("onnxruntime-1.30.0-cuda13");
    std::fs::create_dir_all(&earlier).unwrap();
    for e in std::fs::read_dir(real.engines.join("onnxruntime-1.30.0-cuda13")).unwrap().flatten() {
        std::fs::hard_link(e.path(), earlier.join(e.file_name())).unwrap();
    }
    let state = temp_state_in(None, Some(engines.clone()));
    let voices = &state.catalog.voices;
    println!("to download: {} MB", crate::natural::card_runtime_size(&state.paths, voices, true) >> 20);
    let started = std::time::Instant::now();
    let dll = crate::natural::ensure_card_runtime(&state, true, &AtomicBool::new(false), &|_, _| {}).await.unwrap();
    println!("ready in {:.0} s: {}", started.elapsed().as_secs_f64(), dll.display());
    for f in ["onnxruntime.dll", "onnxruntime_providers_shared.dll", "DirectML.dll", "onnxruntime_providers_cuda.dll", "cufft64_12.dll", "cudnn64_9.dll", "cublasLt64_13.dll", "cudart64_13.dll"] {
        assert!(dll.with_file_name(f).is_file(), "{f} is missing");
    }
    assert!(crate::natural::cuda_ready(&state.paths, voices));
    assert_eq!(crate::natural::earlier_builds(&state.paths, voices), vec![earlier.clone()]);
    crate::natural::runtime_ready(&dll).unwrap();
    let picked = crate::natural::pick_card(&state, false).await.unwrap();
    println!("picked {} through {}", picked.name, if picked.card.dml { "DirectML" } else { "CUDA" });
    assert!(!picked.card.dml, "an RTX card with a new driver takes CUDA");
    let dxgi = state.hardware.read().unwrap().best_gpu().and_then(|g| g.dxgi).unwrap();
    let dir = std::path::PathBuf::from(r"C:\dev\tools\voice\onnx16");
    for card in [picked.card, crate::natural::Card { device: dxgi as i32, dml: true }] {
        let mut conv = crate::media::svc::Converter::new(&dll, &dir, 8, Some(card)).unwrap();
        let wave: Vec<f32> = (0..44_100 * 3).map(|i| (i as f32 * 0.03).sin() * 0.2).collect();
        let t = std::time::Instant::now();
        let mel = conv.run("mel44.onnx", vec![("wave", vec![1, wave.len() as i64], wave)]).unwrap();
        println!("{card:?}: {} values in {:.2} s, on the card: {}", mel.len(), t.elapsed().as_secs_f64(), conv.on_card);
        assert!(conv.on_card && !mel.is_empty());
    }
    std::fs::remove_dir_all(&engines).ok();
}

/// Two pictures asked for at once take turns on the card (6 GB each, more
/// than this 12 GB card holds together): the second waits in line, then
/// runs on the card too, never on the processor.
///     cargo test e2e_jobs_take_turns -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_jobs_take_turns_on_the_card() {
    use crate::media::{self, store, Request};
    let state = media_state();
    crate::applog::init(&state.paths.logs);
    media::install_for_test(&state, "realvisxl-v5-lightning").await.unwrap();
    let req = |seed: i64| Request { op: "generate".into(), prompt: "a red bicycle leaning on a blue wall".into(), model_id: Some("realvisxl-v5-lightning".into()), shape: Some("square".into()), seed: Some(seed), ..Default::default() };
    let (a, b) = (store::new_id(), store::new_id());
    let (s1, s2, a2, b2) = (state.clone(), state.clone(), a.clone(), b.clone());
    let first = tokio::spawn(async move { media::run(&s1, a2, req(1)).await });
    while !crate::lineup::list().iter().any(|e| e.id == a && e.running) {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let second = tokio::spawn(async move { media::run(&s2, b2, req(2)).await });
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let line = crate::lineup::list();
    for e in &line {
        println!("{} {} ({}){}", if e.running { "running" } else { "waiting" }, e.what, e.id, e.card.map_or(String::new(), |c| format!(" on {c:?}")));
    }
    assert!(line.iter().any(|e| e.id == b && !e.running), "the second waits its turn");
    let (r1, r2) = (first.await.unwrap(), second.await.unwrap());
    assert!(r1.is_ok() && r2.is_ok(), "{r1:?} {r2:?}");
    assert!(crate::lineup::list().is_empty(), "the line is empty when they're done");
    let log = std::fs::read_to_string(state.paths.logs.join("app.log")).unwrap_or_default();
    let done: Vec<&str> = log.lines().filter(|l| l.contains("generate with")).collect();
    println!("{}", done.join("\n"));
    assert!(done.len() == 2 && done.iter().all(|l| l.contains("on the graphics card")));
}

/// A reply past a small length limit is carried on to its end.
///     SULCUSAI_TEST_GGUF is the model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_a_reply_past_its_limit_is_carried_on() {
    let (_eng, mut ep, name) = test_model().await;
    ep.extra["max_tokens"] = serde_json::json!(60);
    let msgs = vec![serde_json::json!({ "role": "user", "content": "List the numbers from 1 to 60 as words, one per line, nothing else." })];
    let done = chat::stream(&ep, msgs, None, &AtomicBool::new(false), |_| {}).await.unwrap();
    println!("{name}: {} lines, ends: {:?}", done.content.lines().count(), done.content.lines().last());
    assert!(done.content.to_lowercase().contains("sixty"), "{}", done.content);
}

/// The owner's task: a page fetched and applied to a folder, without
/// asking. The page is GitHub's Node .gitignore; applied, it becomes the
/// folder's .gitignore. See `test_model` for the model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_agent_applies_a_page() {
    let (_eng, ep, name) = test_model().await;
    let (state, ep, cipher) = agent_harness_on(None, Some(ep)).await;
    crate::db::update_settings(&state.db.lock().unwrap(), |s| s.connectivity = crate::net::Connectivity::Web).unwrap();
    let id = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let project = std::env::temp_dir().join(format!("sulcusai-e2e-app-{id}"));
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("package.json"), "{ \"name\": \"demo\", \"version\": \"1.0.0\" }\n").unwrap();
    let project = dunce::canonicalize(&project).unwrap();
    crate::db::add_folder(&state.db.lock().unwrap(), &project.display().to_string()).unwrap();
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
    crate::db::set_chat_web(&state.db.lock().unwrap(), &chat.id, true).unwrap();
    let (watch, asked) = watch_approvals(&state);
    let started = std::time::Instant::now();
    let url = "https://raw.githubusercontent.com/github/gitignore/main/Node.gitignore";
    let reply = say(&state, &ep, &cipher, &chat.id, &format!("Fetch {url} and apply it to the folder {}: it should become that folder's .gitignore.", project.display())).await;
    watch.abort();
    let (_, tools) = transcript(&state, &cipher, &chat.id, &format!("{name}: fetch a page and apply it ({:.0}s)", started.elapsed().as_secs_f64()));
    let written = std::fs::read_to_string(project.join(".gitignore")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&project);
    println!("reply: {reply}");
    let asked = asked.lock().unwrap().clone();
    assert!(asked.is_empty(), "it asked: {asked:?}");
    assert!(tools.iter().any(|t| t == "fetch_page"), "fetched it: {tools:?}");
    assert!(written.contains("node_modules"), "the folder's .gitignore is the page's: {written:.200}");
}

/// What the model sends back for a tool task, before SulcusAI reads it:
/// the raw reply (content, reasoning, tool calls) and whether its chat
/// template is Harmony's. SULCUSAI_TEST_GGUF; written to SULCUSAI_TEST_LOG too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_harmony_raw() {
    use std::io::Write as _;
    let (_eng, ep, name) = test_model().await;
    let gguf = std::path::PathBuf::from(std::env::var("SULCUSAI_TEST_GGUF").unwrap());
    let meta = crate::gguf::read(&gguf).unwrap();
    let template = meta.get("tokenizer.chat_template").and_then(|v| v.str().map(str::to_string)).unwrap_or_default();
    let digest = hex::encode(sha2::Digest::finalize(<sha2::Sha256 as sha2::Digest>::new_with_prefix(template.as_bytes())));
    let mut out = format!(
        "===== {name}: raw =====\narchitecture: {:?}\nchat template: {} characters, sha256 {}…; has <|channel|>: {}; has to=functions: {}; reads reasoning_effort: {}\n",
        meta.get("general.architecture").and_then(|v| v.str().map(str::to_string)),
        template.len(),
        &digest[..16],
        template.contains("<|channel|>"),
        template.contains("to=functions"),
        template.contains("reasoning_effort")
    );
    let tools = serde_json::json!([
        { "type": "function", "function": { "name": "find_files", "description": crate::tools::find("find_files").unwrap().description, "parameters": (crate::tools::find("find_files").unwrap().params)() } },
        { "type": "function", "function": { "name": "run_command", "description": crate::tools::find("run_command").unwrap().description, "parameters": (crate::tools::find("run_command").unwrap().params)() } },
        { "type": "function", "function": { "name": "fetch_page", "description": crate::tools::find("fetch_page").unwrap().description, "parameters": (crate::tools::find("fetch_page").unwrap().params)() } },
    ]);
    for task in ["Find the file named hello_test.bat on this PC and run it.", "Fetch https://raw.githubusercontent.com/github/gitignore/main/Node.gitignore and apply it to C:\\work\\demo as its .gitignore."] {
        let mut body = serde_json::json!({
            "messages": [{ "role": "system", "content": "You are SulcusAI, an assistant on the user's PC. The user has already given you permission to search the file system, read files, fetch web pages and run programs: do it, don't ask." }, { "role": "user", "content": task }],
            "tools": tools, "stream": false, "temperature": 0.6, "max_tokens": 2048,
        });
        if let (Some(b), Some(e)) = (body.as_object_mut(), ep.extra.as_object()) {
            for (k, v) in e {
                b.insert(k.clone(), v.clone());
            }
        }
        let raw: serde_json::Value = reqwest::Client::new().post(format!("http://127.0.0.1:{}/v1/chat/completions", ep.port)).bearer_auth(&ep.key).json(&body).send().await.unwrap().json().await.unwrap();
        let msg = &raw["choices"][0]["message"];
        let calls = msg["tool_calls"].as_array().cloned().unwrap_or_default();
        let content = msg["content"].as_str().unwrap_or_default();
        let offered: Vec<String> = ["find_files", "run_command", "fetch_page"].map(String::from).to_vec();
        let in_text = chat::harmony_calls(content, &offered).0.len() + chat::harmony_calls(msg["reasoning_content"].as_str().unwrap_or_default(), &offered).0.len();
        out.push_str(&format!(
            "--- task: {task}\nfinish_reason: {}\ntool_calls parsed by the engine: {}\n{}\nHarmony calls left in the text: {in_text}\ncontent: {}\nreasoning: {}\n",
            raw["choices"][0]["finish_reason"],
            calls.len(),
            calls.iter().map(|c| format!("  -> {}({})", c["function"]["name"], c["function"]["arguments"])).collect::<Vec<_>>().join("\n"),
            content.chars().take(600).collect::<String>(),
            msg["reasoning_content"].as_str().unwrap_or_default().chars().take(600).collect::<String>().replace('\n', " ")
        ));
    }
    println!("{out}");
    if let Ok(path) = std::env::var("SULCUSAI_TEST_LOG") {
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(path).and_then(|mut f| f.write_all(out.as_bytes()));
    }
}

async fn say(state: &std::sync::Arc<crate::AppState>, ep: &engine::Endpoint, cipher: &crate::crypto::Cipher, chat_id: &str, text: &str) -> String {
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
        mode: crate::tools::Mode::Auto,
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

/// Memory with the real model: a fact told in one chat is recalled in a
/// new chat, and an incognito chat saves nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_memory_carries_across_chats() {
    let (state, ep, cipher) = agent_harness().await;
    let first = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, Some("qwen3-1.7b".into())).unwrap();
    let reply = say(&state, &ep, &cipher, &first.id, "Please remember this for later: my favorite color is teal.").await;
    println!("chat 1: {reply}");
    let saved: Vec<String> = crate::memory::list(&state.db.lock().unwrap(), &cipher, None).into_iter().map(|m| m.content).collect();
    println!("memories: {saved:?}");
    assert!(saved.iter().any(|m| m.to_lowercase().contains("teal")), "the model saved the fact");

    let second = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, Some("qwen3-1.7b".into())).unwrap();
    let reply = say(&state, &ep, &cipher, &second.id, "What is my favorite color? Answer in one word.").await;
    println!("chat 2: {reply}");
    assert!(reply.to_lowercase().contains("teal"), "recalled in a new chat");

    let incognito = crate::db::create_chat_in(&state.db.lock().unwrap(), &cipher, Some("qwen3-1.7b".into()), None, true).unwrap();
    let reply = say(&state, &ep, &cipher, &incognito.id, "Please remember that I play the cello.").await;
    println!("incognito: {reply}");
    let after = crate::memory::list(&state.db.lock().unwrap(), &cipher, None);
    assert!(!after.iter().any(|m| m.content.to_lowercase().contains("cello")), "incognito saves nothing");
    state.engine.lock().await.stop().await;
}

/// A helper agent answers a lookup task, and a handoff summary carries the
/// conversation into a new chat.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_helper_and_handoff() {
    let (state, ep, cipher) = agent_harness().await;
    let work = state.paths.data.join("work").join("shop");
    std::fs::create_dir_all(work.join("src")).unwrap();
    std::fs::write(work.join("src").join("cart.py"), "def add_item(cart, item):\n    cart.append(item)\n").unwrap();
    std::fs::write(work.join("src").join("checkout.py"), "def apply_discount(total, code):\n    return total * 0.9 if code == 'SAVE10' else total\n").unwrap();
    std::fs::write(work.join("README.md"), "# Shop\nA tiny shop.\n").unwrap();
    crate::db::add_folder(&state.db.lock().unwrap(), &dunce::canonicalize(&work).unwrap().display().to_string()).unwrap();

    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, Some("qwen3-1.7b".into())).unwrap();
    let reply = say(&state, &ep, &cipher, &chat.id, "Use the delegate tool to send a helper to find which file in shop/ defines apply_discount, then tell me the file path.").await;
    println!("reply: {reply}");
    let msgs = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id);
    let helper = msgs.iter().find(|m| m.role == "tool" && m.meta.as_ref().is_some_and(|x| x["tool"] == "delegate"));
    let helper = helper.expect("the model used a helper");
    println!("helper report: {}", helper.content);
    println!("helper steps: {}", helper.meta.as_ref().unwrap()["detail"]);
    assert!(helper.content.contains("checkout"), "the helper found the file");

    let old = crate::db::chat(&state.db.lock().unwrap(), &cipher, &chat.id).unwrap();
    let summary = crate::handoff::summarize(&ep, &chat::system_prompt(&Profile::default(), "today").0, &msgs, &std::sync::atomic::AtomicBool::new(false)).await.unwrap();
    println!("summary:\n{summary}");
    let new = crate::handoff::continue_in_new_chat(&state.db.lock().unwrap(), &cipher, &old, &summary).unwrap();
    let first = &crate::db::messages(&state.db.lock().unwrap(), &cipher, &new.id)[0];
    assert!(first.content.contains("Continued from"));
    let follow = say(&state, &ep, &cipher, &new.id, "Which file was it again? Just the path.").await;
    println!("follow-up in new chat: {follow}");
    assert!(follow.contains("checkout"), "the new chat knows from the summary");
    state.engine.lock().await.stop().await;
}

/// A local connector and a plugin skill, used by the real model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_connector_and_skill() {
    let (state, ep, cipher) = agent_harness().await;
    let conn = crate::connectors::Connector {
        id: "echo".into(),
        name: "echo".into(),
        spec: crate::mcp::tests::echo_spec(),
        enabled: true,
        plugin: None,
        tools: vec![],
        tool_modes: Default::default(),
    };
    crate::connectors::store(&state.db.lock().unwrap(), &cipher, "connectors", "echo", &conn).unwrap();
    let skill = crate::connectors::Plugin {
        id: "haiku".into(),
        name: "Haiku".into(),
        version: "1".into(),
        description: String::new(),
        enabled: true,
        dir: String::new(),
        skills: vec![crate::connectors::Skill {
            name: "haiku-writer".into(),
            description: "Write any answer as a haiku".into(),
            body: "Answer as a single haiku (three lines: 5, 7 and 5 syllables). Put the word SKILL-OK on a fourth line.".into(),
        }],
        connectors: vec![],
    };
    crate::connectors::store(&state.db.lock().unwrap(), &cipher, "plugins", "haiku", &skill).unwrap();

    // The shout tool isn't read-only, so Auto mode asks; approve it.
    let approver_state = state.clone();
    let approver = tokio::spawn(async move {
        for _ in 0..600 {
            for chat in crate::db::list_chats(&approver_state.db.lock().unwrap(), &approver_state.cipher().unwrap()) {
                for p in approver_state.approvals.pending(&chat.id) {
                    println!("approval: {} — {}", p.tool, p.preview.title);
                    approver_state.approvals.answer(&p.call_id, crate::agent::Decision::Allow);
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    });
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, Some("qwen3-1.7b".into())).unwrap();
    let reply = say(&state, &ep, &cipher, &chat.id, "Use the echo connector's shout tool on the text: hello world. Tell me exactly what it returned.").await;
    println!("connector reply: {reply}");
    let msgs = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id);
    let used = msgs.iter().any(|m| m.role == "tool" && m.content.contains("HELLO WORLD"));
    assert!(used, "the shout tool ran through the connector");

    let chat2 = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, Some("qwen3-1.7b".into())).unwrap();
    let reply = say(&state, &ep, &cipher, &chat2.id, "Use your haiku-writer skill to tell me about the ocean.").await;
    println!("skill reply: {reply}");
    let msgs = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat2.id);
    let followed = reply.contains("SKILL-OK") || msgs.iter().any(|m| m.tool_calls.iter().flatten().any(|c| c.name == "load_skill"));
    assert!(followed, "the model followed the skill instructions");
    approver.abort();
    state.engine.lock().await.stop().await;
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

/// Speech recognition end to end, without speakers or a microphone: a
/// Windows voice speaks a sentence and whisper.cpp writes it down.
/// SULCUSAI_SPEECH_MODEL picks the model (default: the one this PC is offered first).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_speech_round_trip() {
    use crate::speech;
    let state = speech_state();
    let voices = crate::tts::voices().unwrap();
    println!("voices: {:?}", voices.iter().map(|v| format!("{} ({})", v.name, v.language)).collect::<Vec<_>>());
    assert!(!voices.is_empty(), "no Windows voices");

    let hw = state.hardware.read().unwrap().clone();
    let b = Budget::from_hardware(&hw);
    let id = std::env::var("SULCUSAI_SPEECH_MODEL")
        .unwrap_or_else(|_| speech::recommended(&state.catalog.speech.models, &hw, &b).unwrap().id.clone());
    let spec = state.catalog.speech.models.iter().find(|m| m.id == id).unwrap().clone();
    println!("model {} — estimated {}x real time on {} threads", spec.name, speech::fit(&spec, &hw, &b).speed, speech::threads(&hw));

    let cancel = AtomicBool::new(false);
    let started = std::time::Instant::now();
    let speed = speech::install_for_test(&state, &spec, &cancel).await.unwrap();
    println!("installed and self-tested in {:.1}s; measured {speed:?}x real time", started.elapsed().as_secs_f64());
    assert!(speed.is_some_and(|s| s > 0.2));

    let ep = speech::endpoint(&state, speech::Use::Accurate).await.unwrap();
    // Without the secret prefix the server has nothing to offer.
    let port = ep.port_for_test();
    let bare = net::local_client().get(format!("http://127.0.0.1:{port}/health")).send().await.unwrap();
    assert_eq!(bare.status().as_u16(), 404, "speech server answered without its path prefix");
    let page = net::local_client().get(format!("http://127.0.0.1:{port}/")).send().await.unwrap();
    assert_eq!(page.status().as_u16(), 404, "speech server served a page without its path prefix");

    let say = "Please remind me to call the dentist on Wednesday at three thirty.";
    let s = tokio::task::spawn_blocking(move || crate::tts::synthesize(say, Some(crate::tts::WINDOWS_DEFAULT), Some("en"), 1.0)).await.unwrap().unwrap();
    let floats: Vec<f32> = s.samples.iter().map(|x| *x as f32 / 32768.0).collect();
    let samples = crate::audio::resample(&floats, s.rate, crate::audio::RATE);
    let started = std::time::Instant::now();
    let t = speech::transcribe(&ep, &samples, &speech::Options::default()).await.unwrap();
    println!(
        "heard {:.1}s of speech in {:.2}s ({:?}): {:?}",
        samples.len() as f64 / 16000.0,
        started.elapsed().as_secs_f64(),
        t.language,
        t.text
    );
    let lower = t.text.to_lowercase();
    assert!(lower.contains("dentist") && lower.contains("wednesday"), "{}", t.text);
    assert!(t.segments.iter().all(|s| s.end >= s.start));

    // Silence gives nothing back, not a made-up sentence.
    let quiet = vec![0.0f32; crate::audio::RATE as usize * 3];
    let t = speech::transcribe(&ep, &quiet, &speech::Options::default()).await.unwrap();
    println!("silence heard as: {:?}", t.text);
    assert!(t.text.trim().is_empty(), "{}", t.text);

    speech::stop(&state).await;
}

/// Dictation as the app runs it, with synthesized speech streamed in small
/// chunks the way a microphone delivers them (no real microphone used).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_dictation_streams_phrases() {
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;
    let state = speech_state();
    // Registers the quick model (downloads it the first time).
    let spec = state.catalog.speech.models.iter().find(|m| m.id == "whisper-small").unwrap().clone();
    crate::speech::install_for_test(&state, &spec, &AtomicBool::new(false)).await.unwrap();
    let ep = crate::speech::endpoint(&state, crate::speech::Use::Live).await.unwrap();
    let speak = |text: &'static str| {
        let s = crate::tts::synthesize(text, Some(crate::tts::WINDOWS_DEFAULT), Some("en"), 1.0).unwrap();
        let f: Vec<f32> = s.samples.iter().map(|x| *x as f32 / 32768.0).collect();
        crate::audio::resample(&f, s.rate, crate::audio::RATE)
    };
    let mut stream = vec![0.0005f32; 16_000];
    stream.extend(speak("First, buy oat milk and coffee filters."));
    stream.extend(vec![0.0005f32; 24_000]);
    stream.extend(speak("Second, book a table for four on Saturday."));
    stream.extend(vec![0.0005f32; 8_000]);

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let feeder = tokio::spawn(async move {
        for chunk in stream.chunks(320) {
            tx.send(chunk.to_vec()).unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    });
    let stop = AtomicBool::new(false);
    let texts: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let emit = |kind: &str, v: serde_json::Value| {
        if kind == "text" {
            texts.lock().unwrap().push(v["text"].as_str().unwrap().to_string());
        }
    };
    let run = crate::voice::dictate(&ep, rx, &stop, &|| 0.0, &emit);
    let started = std::time::Instant::now();
    let (r, _) = tokio::join!(run, feeder);
    r.unwrap();
    println!("dictation finished {:.1}s after the audio started", started.elapsed().as_secs_f64());
    let texts = texts.into_inner().unwrap();
    println!("phrases: {texts:?}");
    assert!(texts.len() >= 2, "expected the two sentences as separate phrases");
    let all = texts.join(" ").to_lowercase();
    assert!(all.contains("oat milk") && all.contains("saturday"), "{all}");
    crate::speech::stop(&state).await;
}

/// Meeting mode end to end, without devices: two synthetic channels (the
/// call, and the user with some of the call echoing into their mic), live
/// transcription, echo removal, and notes from the real chat model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_meeting_records_transcribes_and_writes_notes() {
    use crate::meeting::{self, Input, MeetingData, Speaker};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let state = speech_state();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    {
        let spec = state.catalog.model("qwen3-1.7b").unwrap();
        let v = spec.variant("Q4_K_M").unwrap();
        let path = state.paths.models.join(&spec.id).join(&v.file);
        let conn = state.db.lock().unwrap();
        crate::db::save_installed(&conn, &crate::db::InstalledModel {
            model_id: spec.id.clone(), quant: v.quant.clone(), path: path.display().to_string(), size: v.size, installed_at: 0, tps: None, prompt_tps: None,
        }).unwrap();
        crate::db::update_settings(&conn, |s| s.default_model = Some(spec.id.clone())).unwrap();
    }
    let cipher = state.cipher().unwrap();
    let m = meeting::create(&state.db.lock().unwrap(), &cipher, &MeetingData { title: "Test".into(), ..Default::default() }).unwrap();

    let speak = |text: &str, voice: usize| {
        let voices = crate::tts::voices().unwrap();
        let s = crate::tts::synthesize(text, Some(&voices[voice % voices.len()].id), Some("en"), 1.0).unwrap();
        let f: Vec<f32> = s.samples.iter().map(|x| *x as f32 / 32768.0).collect();
        crate::audio::resample(&f, s.rate, crate::audio::RATE)
    };
    let quiet = |secs: f32| vec![0.0003f32; (secs * 16_000.0) as usize];
    let line1 = speak("Thanks for joining everyone. We decided to move the product launch to Thursday the twelfth.", 1);
    let line2 = speak("Jordan, please send the updated budget to the whole team by Friday.", 1);
    let mine = speak("Sounds good. I will book the venue for the launch party next week.", 0);
    // The call: line 1, a pause, line 2, then silence while the user talks.
    let mut others = quiet(1.0);
    others.extend(&line1);
    others.extend(quiet(1.5));
    let echo_at = others.len();
    others.extend(&line2);
    others.extend(quiet(1.5));
    let reply_at = others.len();
    others.extend(quiet(mine.len() as f32 / 16_000.0 + 2.0));
    // The mic: quiet, then line 2 coming back from the speakers, then the user.
    let mut you = quiet(echo_at as f32 / 16_000.0);
    you.extend(line2.iter().map(|s| s * 0.35));
    you.resize(reply_at, 0.0003);
    you.extend(&mine);
    you.resize(others.len(), 0.0003);

    let (tx_o, rx_o) = tokio::sync::mpsc::unbounded_channel();
    let (tx_y, rx_y) = tokio::sync::mpsc::unbounded_channel();
    let stop = Arc::new(AtomicBool::new(false));
    let feeder = {
        let stop = stop.clone();
        tokio::spawn(async move {
            for (a, b) in others.chunks(1600).zip(you.chunks(1600)) {
                tx_o.send(a.to_vec()).unwrap();
                tx_y.send(b.to_vec()).unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            stop.store(true, Ordering::SeqCst);
        })
    };
    let log: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let emit: meeting::Emit = {
        let log = log.clone();
        Arc::new(move |kind: &str, v: serde_json::Value| {
            if kind != "level" {
                log.lock().unwrap().push(format!("{kind}: {v}"));
            }
        })
    };
    let ep = crate::speech::endpoint(&state, crate::speech::Use::Accurate).await.unwrap();
    let inputs = vec![Input { speaker: Speaker::Others, rx: rx_o }, Input { speaker: Speaker::You, rx: rx_y }];
    let started = std::time::Instant::now();
    meeting::record(&state, &cipher, &m.id, ep, inputs, true, None, &stop, &|| serde_json::json!({}), emit.clone()).await.unwrap();
    feeder.await.unwrap();
    println!("recorded and transcribed in {:.1}s", started.elapsed().as_secs_f64());
    for l in log.lock().unwrap().iter() {
        println!("  {}", &l[..l.len().min(220)]);
    }
    let segs = meeting::segments(&state.db.lock().unwrap(), &cipher, &m.id);
    let text = meeting::transcript_text(&segs, &Default::default());
    println!("transcript:\n{text}");
    let yours: Vec<_> = segs.iter().filter(|s| s.speaker == Speaker::You).collect();
    assert!(yours.iter().all(|s| !s.text.to_lowercase().contains("budget")), "the echoed line should be dropped from You");
    assert!(yours.iter().any(|s| s.text.to_lowercase().contains("venue")));
    assert!(segs.iter().any(|s| s.speaker == Speaker::Others && s.text.to_lowercase().contains("budget")));

    // A clip of the stored (encrypted) audio plays back the right length.
    let wav = meeting::clip(&meeting::audio_dir(&state, &m.id), &cipher, 1.0, 4.0);
    assert_eq!(crate::audio::wav_decode(&wav).unwrap().samples.len(), 3 * 16_000);

    let started = std::time::Instant::now();
    meeting::finish(&state, &cipher, &m.id, &emit).await.unwrap();
    let detail = meeting::list(&state.db.lock().unwrap(), &cipher).into_iter().find(|x| x.id == m.id).unwrap();
    println!("notes in {:.1}s: {:#?}", started.elapsed().as_secs_f64(), detail.data);
    let notes = detail.data.notes.expect("notes");
    assert!(!notes.summary.is_empty());
    let actions = serde_json::to_string(&notes.action_items).unwrap().to_lowercase();
    assert!(actions.contains("budget"), "{actions}");
    assert_eq!(detail.status, "done");
    crate::speech::stop(&state).await;
    state.engine.lock().await.stop().await;
    let _ = std::fs::remove_dir_all(meeting::audio_dir(&state, &m.id));
}

/// Narration as the Studio's tab starts it, with no voice picked (the
/// first on the PC) and with a Windows voice: each read and kept as a
/// sound of about the right length.
///     cargo test e2e_narration_reads -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_narration_reads() {
    use crate::media::{self, store, Request};
    let state = media_state();
    let text = "Welcome to the studio. This is a short test of reading text aloud, about ten seconds long, to check that narration works.";
    for voice in [None, Some(crate::tts::WINDOWS_DEFAULT.to_string())] {
        let t = std::time::Instant::now();
        let got = media::run(&state, store::new_id(), Request { op: "narrate".into(), prompt: text.into(), voice: voice.clone(), ..Default::default() }).await;
        match &got {
            Ok(items) => println!("voice {voice:?}: {:.1} s of speech in {:.1} s", items[0].seconds, t.elapsed().as_secs_f64()),
            Err(e) => println!("voice {voice:?}: FAILED {e}"),
        }
        let items = got.unwrap();
        assert!(items[0].seconds > 4.0 && items[0].seconds < 30.0, "{}", items[0].seconds);
    }
}

/// Lists audio devices without opening any (no recording, no sound).
#[test]
#[ignore]
fn e2e_audio_devices_are_listed() {
    let inputs = crate::audio::devices(crate::audio::Flow::Input).unwrap();
    let outputs = crate::audio::devices(crate::audio::Flow::Output).unwrap();
    println!("inputs: {inputs:#?}\noutputs: {outputs:#?}");
    assert!(outputs.iter().any(|d| d.default), "a default output device");
}

/// Natural voices end to end: install (runtime + Supertonic 3), speak in two
/// languages, and check speech recognition understands what was said.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_natural_voice_is_understood() {
    let state = speech_state();
    let started = std::time::Instant::now();
    crate::natural::install_for_test(&state).await.unwrap();
    println!("installed natural voices in {:.1}s", started.elapsed().as_secs_f64());
    let voices = crate::tts::voices().unwrap();
    assert!(voices.iter().any(|v| v.id == "supertonic:F1"));
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &std::sync::atomic::AtomicBool::new(false)).await.unwrap();
    let ep = crate::speech::endpoint(&state, crate::speech::Use::Accurate).await.unwrap();
    for (lang, voice, text, words) in [
        ("en", "supertonic:F2", "The quarterly report is due on Friday, so please send your numbers by Wednesday.", ["quarterly", "friday", "wednesday"]),
        ("de", "supertonic:M1", "Das Treffen beginnt morgen um neun Uhr im großen Konferenzraum.", ["treffen", "morgen", "konferenzraum"]),
    ] {
        let t0 = std::time::Instant::now();
        let s = tokio::task::spawn_blocking(move || crate::tts::synthesize(text, Some(voice), Some(lang), 1.0)).await.unwrap().unwrap();
        let secs = s.samples.len() as f64 / s.rate as f64;
        println!("{lang}: {secs:.1}s of speech made in {:.2}s ({} Hz)", t0.elapsed().as_secs_f64(), s.rate);
        assert!(secs > 2.0 && secs < 15.0);
        let f: Vec<f32> = s.samples.iter().map(|x| *x as f32 / 32768.0).collect();
        let samples = crate::audio::resample(&f, s.rate, crate::audio::RATE);
        let heard = crate::speech::transcribe(&ep, &samples, &crate::speech::Options { language: Some(lang.into()), ..Default::default() }).await.unwrap();
        println!("  heard: {}", heard.text);
        let lower = heard.text.to_lowercase();
        for w in words {
            assert!(lower.contains(w), "{lang}: expected “{w}” in “{}”", heard.text);
        }
    }
    crate::speech::stop(&state).await;
}

/// Speaker labels: two different voices take turns on the call channel; the
/// transcript should label them as two consistent, different speakers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_meeting_tells_speakers_apart() {
    use crate::meeting::{self, Input, MeetingData, Speaker};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let state = speech_state();
    crate::natural::refresh(&state.paths, &state.catalog.voices);
    crate::diarize::install_for_test(&state).await.unwrap();
    assert!(crate::diarize::available(&state));
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    let cipher = state.cipher().unwrap();
    let m = meeting::create(&state.db.lock().unwrap(), &cipher, &MeetingData { title: "Two voices".into(), ..Default::default() }).unwrap();

    let windows_male = crate::tts::voices().unwrap().into_iter().find(|v| v.engine == "system" && v.gender == "male").unwrap().id;
    let say = |text: &str, voice: &str| {
        let s = crate::tts::synthesize(text, Some(voice), Some("en"), 1.0).unwrap();
        let f: Vec<f32> = s.samples.iter().map(|x| *x as f32 / 32768.0).collect();
        crate::audio::resample(&f, s.rate, crate::audio::RATE)
    };
    let lines = [
        ("supertonic:F2", "Good morning, I wanted to start with the marketing budget for next quarter."),
        (windows_male.as_str(), "Sure. I think we should move some money from print ads to online campaigns."),
        ("supertonic:F2", "That makes sense, but the regional teams still rely on printed brochures."),
        (windows_male.as_str(), "Then let's keep a small print budget and review the numbers again in March."),
    ];
    let mut others = vec![0.0003f32; 16_000];
    for (voice, text) in lines {
        others.extend(say(text, voice));
        others.extend(vec![0.0003f32; 40_000]);
    }
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let stop = Arc::new(AtomicBool::new(false));
    let feeder = {
        let stop = stop.clone();
        tokio::spawn(async move {
            for c in others.chunks(1600) {
                tx.send(c.to_vec()).unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(3)).await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            stop.store(true, Ordering::SeqCst);
        })
    };
    let emit: meeting::Emit = Arc::new(|kind: &str, v: serde_json::Value| {
        if kind == "relabeled" || kind == "warning" {
            println!("{kind}: {v}");
        }
    });
    let ep = crate::speech::endpoint(&state, crate::speech::Use::Accurate).await.unwrap();
    meeting::record(&state, &cipher, &m.id, ep, vec![Input { speaker: Speaker::Others, rx }], false, None, &stop, &|| serde_json::json!({}), emit).await.unwrap();
    feeder.await.unwrap();
    let segs = meeting::segments(&state.db.lock().unwrap(), &cipher, &m.id);
    println!("{}", meeting::transcript_text(&segs, &Default::default()));
    let label_of = |needle: &str| segs.iter().find(|s| s.text.to_lowercase().contains(needle)).and_then(|s| s.voice);
    let (a1, b1, a2, b2) = (label_of("marketing"), label_of("online"), label_of("brochures"), label_of("march"));
    println!("labels: {a1:?} {b1:?} {a2:?} {b2:?}");
    assert!(a1.is_some() && b1.is_some());
    assert_eq!(a1, a2, "the first voice keeps its label");
    assert_eq!(b1, b2, "the second voice keeps its label");
    assert_ne!(a1, b1, "two voices, two labels");
    assert_eq!(a1, Some(1), "numbered by who spoke first");
    crate::speech::stop(&state).await;
}

/// Notes and tasks with the real model: it adds a task with a due date
/// and saves a note, and finds the note again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_notes_and_tasks() {
    let (state, ep, cipher) = agent_harness().await;
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, Some("qwen3-1.7b".into())).unwrap();
    let reply = say(&state, &ep, &cipher, &chat.id, "Add a high-priority task to call the dentist on Friday. /no_think").await;
    println!("task reply: {reply}");
    let tasks = crate::notes::list_tasks(&state.db.lock().unwrap(), &cipher);
    println!("tasks: {:?}", tasks.iter().map(|t| (&t.data.title, t.due, t.priority)).collect::<Vec<_>>());
    let t = tasks.iter().find(|t| t.data.title.to_lowercase().contains("dentist")).expect("a dentist task");
    assert!(t.due.is_some(), "Friday became a due date");

    let reply = say(&state, &ep, &cipher, &chat.id, "Save a note titled Gift ideas with: a cookbook for Mom and a scarf for Dad. /no_think").await;
    println!("note reply: {reply}");
    let notes = crate::notes::list_notes(&state.db.lock().unwrap(), &cipher);
    assert!(notes.iter().any(|n| n.data.body.to_lowercase().contains("scarf")), "{notes:?}");

    let other = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, Some("qwen3-1.7b".into())).unwrap();
    let reply = say(&state, &ep, &cipher, &other.id, "Look in my notes: what gift idea did I have for Dad? /no_think").await;
    println!("recall: {reply}");
    assert!(reply.to_lowercase().contains("scarf"));
    state.engine.lock().await.stop().await;
}

/// Compacting, against a stand-in engine: the model writes a summary, the
/// chat keeps going, and the next reply gets the summary (in its
/// instructions) instead of the older messages.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_compacted_chat_carries_on_from_its_summary() {
    use axum::{routing::post, Json, Router};
    type Seen = std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>;
    let seen: Seen = Default::default();
    let s = seen.clone();
    let app = Router::new()
        .route("/tokenize", post(|Json(b): Json<serde_json::Value>| async move {
            let n = b["content"].as_str().map_or(0, |t| t.len() / 4 + 1);
            Json(serde_json::json!({ "tokens": vec![1; n] }))
        }))
        .route("/v1/chat/completions", post(move |Json(b): Json<serde_json::Value>| {
            let s = s.clone();
            async move {
                let asked_summary = b["messages"].as_array().and_then(|m| m.last()).and_then(|m| m["content"].as_str()).is_some_and(|c| c.contains("replaced by your summary"));
                s.lock().unwrap().push(b);
                let text = if asked_summary { "- Goal: a three-day trip to Zion in November" } else { "Done." };
                let chunk = |v: serde_json::Value| format!("data: {v}\n\n");
                let body = chunk(serde_json::json!({ "choices": [{ "delta": { "content": text }, "finish_reason": null }] }))
                    + &chunk(serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] }))
                    + "data: [DONE]\n\n";
                ([(axum::http::header::CONTENT_TYPE, "text/event-stream")], body)
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    let ep = engine::Endpoint { port, key: "k".into(), ctx: 8192, extra: serde_json::json!({}), cloud: None };
    let state = scratch_state();
    let cipher = state.cipher().unwrap();
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
    for (role, text) in [("user", "Help me plan a trip to Zion."), ("assistant", "Sure, when?"), ("user", "November, three days."), ("assistant", "Great, here's a plan…")] {
        crate::db::add_message(&state.db.lock().unwrap(), &cipher, &Message { id: uuid::Uuid::new_v4().to_string(), chat_id: chat.id.clone(), role: role.into(), content: text.into(), created_at: crate::db::now_ms(), ..Default::default() }).unwrap();
    }
    let history = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id);
    assert_eq!(crate::compact::pending(&history), 4);
    let summary = crate::compact::summarize(&ep, "You are a helpful assistant.", history, &std::sync::atomic::AtomicBool::new(false)).await.unwrap();
    crate::compact::save(&state.db.lock().unwrap(), &cipher, &chat.id, &summary, 4, false).unwrap();
    assert_eq!(crate::compact::pending(&crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id)), 0, "nothing new to compact");

    seen.lock().unwrap().clear();
    let reply = say(&state, &ep, &cipher, &chat.id, "What should I pack?").await;
    assert_eq!(reply, "Done.");
    let req = seen.lock().unwrap().last().cloned().unwrap();
    let msgs = req["messages"].as_array().unwrap();
    let system = msgs[0]["content"].as_str().unwrap();
    assert!(system.contains("Earlier in this conversation") && system.contains("trip to Zion in November"), "the summary is in the instructions");
    let said: Vec<&str> = msgs[1..].iter().filter_map(|m| m["content"].as_str()).collect();
    // (The chat's notes ride after it; the summary isn't repeated there.)
    assert!(said.len() == 1 && said[0].contains("What should I pack?"), "the older messages are left to the summary: {said:?}");
    assert_eq!(said[0].matches("trip to Zion in November").count(), 0, "the summary only once, in the instructions");
    let notes = crate::chatnotes::get(&state.db.lock().unwrap(), &cipher, &chat.id);
    assert!(notes.contains("## Earlier in this chat") && notes.contains("trip to Zion in November"), "kept in the chat's notes: {notes}");
    // The person still sees everything, with the summary marked.
    let all = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id);
    assert!(all.iter().any(|m| m.content == "Help me plan a trip to Zion."));
    assert!(all.iter().any(crate::compact::is_marker));
}

/// Web off: the model asks for the web, the user answers the popup, and the
/// same reply carries on with the web tools (a yes) or without them (a no).
/// Against a stand-in engine that streams like llama.cpp.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn asking_for_the_web_carries_on_in_the_same_reply() {
    use axum::{routing::post, Json, Router};
    type Seen = std::sync::Arc<std::sync::Mutex<Vec<Vec<String>>>>;
    let seen: Seen = Default::default();
    let s = seen.clone();
    let app = Router::new()
        .route("/tokenize", post(|Json(b): Json<serde_json::Value>| async move {
            let n = b["content"].as_str().map_or(0, |t| t.len() / 4 + 1);
            Json(serde_json::json!({ "tokens": vec![1; n] }))
        }))
        .route("/v1/chat/completions", post(move |Json(b): Json<serde_json::Value>| {
            let s = s.clone();
            async move {
                let names: Vec<String> = b["tools"].as_array().into_iter().flatten().filter_map(|t| t.pointer("/function/name").and_then(|n| n.as_str()).map(str::to_string)).collect();
                let answered = b["messages"].as_array().is_some_and(|m| m.iter().any(|m| m["role"] == "tool"));
                s.lock().unwrap().push(names);
                let chunk = |v: serde_json::Value| format!("data: {v}\n\n");
                let body = if answered {
                    chunk(serde_json::json!({ "choices": [{ "delta": { "content": "Done." }, "finish_reason": null }] }))
                        + &chunk(serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] }))
                } else {
                    let call = serde_json::json!({ "index": 0, "id": "call_web", "type": "function", "function": { "name": "request_web", "arguments": "{\"reason\": \"who maintains curl\"}" } });
                    chunk(serde_json::json!({ "choices": [{ "delta": { "tool_calls": [call] }, "finish_reason": null }] }))
                        + &chunk(serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "tool_calls" }] }))
                } + "data: [DONE]\n\n";
                ([(axum::http::header::CONTENT_TYPE, "text/event-stream")], body)
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    let ep = engine::Endpoint { port, key: "k".into(), ctx: 8192, extra: serde_json::json!({}), cloud: None };
    let state = scratch_state();
    let cipher = state.cipher().unwrap();
    // Offline with the Web search feature off: no web in any chat.
    {
        let conn = state.db.lock().unwrap();
        crate::features::set(&conn, crate::features::Feature::WebSearch, false).unwrap();
        crate::db::update_settings(&conn, |s| s.connectivity = crate::net::Connectivity::Offline).unwrap();
    }

    for allow in [true, false] {
        seen.lock().unwrap().clear();
        let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
        // The person, answering the popup.
        let st = state.clone();
        let answering = tokio::spawn(async move {
            loop {
                if let Some(p) = st.approvals.waiting().into_iter().find(|p| p.tool == "request_web") {
                    st.approvals.answer(&p.call_id, if allow { crate::agent::Decision::Allow } else { crate::agent::Decision::Deny });
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        });
        let reply = say(&state, &ep, &cipher, &chat.id, "Who maintains the curl project?").await;
        answering.await.unwrap();
        assert_eq!(reply, "Done.", "the same reply went on after the answer");
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2);
        assert!(seen[0].contains(&"request_web".to_string()) && !seen[0].contains(&"web_search".to_string()), "web off: only a way to ask: {:?}", seen[0]);
        let on = crate::db::list_chats(&state.db.lock().unwrap(), &cipher).into_iter().find(|c| c.id == chat.id).unwrap().web;
        if allow {
            assert!(on, "a yes turns web on for the chat");
            assert!(seen[1].contains(&"web_search".to_string()) && !seen[1].contains(&"request_web".to_string()), "the web tools, right away");
        } else {
            assert!(!on);
            assert!(!seen[1].contains(&"web_search".to_string()), "a no keeps the web tools away");
        }
    }
}

/// No folder shared: the model asks for one, the user allows it in the
/// popup (after picking another, or says no), and the same reply goes on to
/// make a file there. Against a stand-in engine.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn asking_for_a_folder_carries_on_in_the_same_reply() {
    use axum::{routing::post, Json, Router};
    type Seen = std::sync::Arc<std::sync::Mutex<Vec<(Vec<String>, String)>>>;
    let seen: Seen = Default::default();
    let base = dunce::canonicalize(std::env::temp_dir()).unwrap().join(format!("sulcusai-site-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&base).unwrap();
    let asked = base.join("portfolio");
    let (s, ask_path) = (seen.clone(), asked.display().to_string());
    let app = Router::new()
        .route("/tokenize", post(|Json(b): Json<serde_json::Value>| async move {
            let n = b["content"].as_str().map_or(0, |t| t.len() / 4 + 1);
            Json(serde_json::json!({ "tokens": vec![1; n] }))
        }))
        .route("/v1/chat/completions", post(move |Json(b): Json<serde_json::Value>| {
            let (s, ask_path) = (s.clone(), ask_path.clone());
            async move {
                let names: Vec<String> = b["tools"].as_array().into_iter().flatten().filter_map(|t| t.pointer("/function/name").and_then(|n| n.as_str()).map(str::to_string)).collect();
                let system = b["messages"][0]["content"].as_str().unwrap_or_default().to_string();
                let results = b["messages"].as_array().map_or(0, |m| m.iter().filter(|m| m["role"] == "tool").count());
                s.lock().unwrap().push((names.clone(), system));
                let chunk = |v: serde_json::Value| format!("data: {v}\n\n");
                let call = |id: &str, name: &str, args: serde_json::Value| {
                    let c = serde_json::json!({ "index": 0, "id": id, "type": "function", "function": { "name": name, "arguments": args.to_string() } });
                    chunk(serde_json::json!({ "choices": [{ "delta": { "tool_calls": [c] }, "finish_reason": null }] }))
                        + &chunk(serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "tool_calls" }] }))
                };
                let body = if results == 0 {
                    call("call_folder", "request_folder", serde_json::json!({ "reason": "build your portfolio site", "path": ask_path }))
                } else if results == 1 && names.iter().any(|n| n == "write_file") {
                    call("call_write", "write_file", serde_json::json!({ "path": "chosen/index.html", "content": "<h1>Hi</h1>" }))
                } else {
                    chunk(serde_json::json!({ "choices": [{ "delta": { "content": "Done." }, "finish_reason": null }] }))
                        + &chunk(serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] }))
                } + "data: [DONE]\n\n";
                ([(axum::http::header::CONTENT_TYPE, "text/event-stream")], body)
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    let ep = engine::Endpoint { port, key: "k".into(), ctx: 8192, extra: serde_json::json!({}), cloud: None };
    let state = scratch_state();
    let cipher = state.cipher().unwrap();
    crate::features::set(&state.db.lock().unwrap(), crate::features::Feature::Files, false).unwrap();

    for allow in [true, false] {
        seen.lock().unwrap().clear();
        let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
        // The person, answering the popup (picking another folder, then
        // allowing it), then the card for writing the file (Auto mode asks).
        let (st, chosen) = (state.clone(), base.join("chosen"));
        let popup: std::sync::Arc<std::sync::Mutex<Option<crate::agent::PendingApproval>>> = Default::default();
        let seen_popup = popup.clone();
        let answering = tokio::spawn(async move {
            loop {
                for p in st.approvals.waiting() {
                    let d = if p.tool == "request_folder" {
                        if allow {
                            std::fs::create_dir_all(&chosen).unwrap();
                            crate::workspace::offer_folder(&p.call_id, &chosen);
                        }
                        *seen_popup.lock().unwrap() = Some(p.clone());
                        if allow { crate::agent::Decision::Allow } else { crate::agent::Decision::Deny }
                    } else {
                        crate::agent::Decision::Allow
                    };
                    st.approvals.answer(&p.call_id, d);
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        });
        let reply = tokio::time::timeout(std::time::Duration::from_secs(60), say(&state, &ep, &cipher, &chat.id, "Build me a portfolio website.")).await.expect("the reply finished");
        answering.abort();
        let asked_for = popup.lock().unwrap().clone().expect("the folder was asked for");
        assert_eq!(reply, "Done.", "the same reply went on after the answer");
        assert_eq!(asked_for.preview.note.as_deref(), Some(asked.display().to_string().as_str()), "the popup names the folder it asked for");
        let seen = seen.lock().unwrap().clone();
        assert!(seen[0].0.contains(&"request_folder".to_string()) && !seen[0].0.contains(&"write_file".to_string()), "no folder: only a way to ask");
        assert!(seen[0].1.contains("call request_folder"), "and it's told to ask instead of pasting code");
        // Shared in this chat only: no other chat gets it.
        let shared = crate::db::chat_shared_folders(&state.db.lock().unwrap(), &chat.id);
        assert!(crate::db::folders(&state.db.lock().unwrap()).is_empty(), "not shared with every chat");
        if allow {
            assert!(shared.iter().any(|f| f.ends_with("chosen")) && !shared.iter().any(|f| f.ends_with("portfolio")), "the folder they picked is shared: {shared:?}");
            assert!(crate::features::is_on(&state.db.lock().unwrap(), crate::features::Feature::Files), "Files turned on with it");
            assert!(seen[1].0.contains(&"write_file".to_string()) && seen[1].0.contains(&"run_command".to_string()), "the file tools, right away");
            assert!(seen[1].1.contains("chosen/"), "the instructions name the new folder");
            assert_eq!(std::fs::read_to_string(base.join("chosen").join("index.html")).unwrap(), "<h1>Hi</h1>");
            for f in shared {
                crate::db::remove_chat_folder(&state.db.lock().unwrap(), &chat.id, &f).unwrap();
            }
            crate::features::set(&state.db.lock().unwrap(), crate::features::Feature::Files, false).unwrap();
        } else {
            assert!(shared.is_empty() && !asked.exists(), "a no shares and makes nothing");
            assert_eq!(seen.len(), 2);
            assert!(!seen[1].0.contains(&"write_file".to_string()), "a no keeps the file tools away");
        }
    }
    std::fs::remove_dir_all(&base).ok();
}

/// A message added while the reply is written reaches the model at its next
/// step, even when that reply was about to end (it carries on to answer).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_added_mid_reply_is_read_and_answered() {
    use axum::{routing::post, Json, Router};
    type Seen = std::sync::Arc<std::sync::Mutex<Vec<Vec<String>>>>;
    let seen: Seen = Default::default();
    let chat_slot: std::sync::Arc<std::sync::Mutex<String>> = Default::default();
    let (s, slot) = (seen.clone(), chat_slot.clone());
    let app = Router::new()
        .route("/tokenize", post(|Json(b): Json<serde_json::Value>| async move {
            let n = b["content"].as_str().map_or(0, |t| t.len() / 4 + 1);
            Json(serde_json::json!({ "tokens": vec![1; n] }))
        }))
        .route("/v1/chat/completions", post(move |Json(b): Json<serde_json::Value>| {
            let (s, slot) = (s.clone(), slot.clone());
            async move {
                let users: Vec<String> = b["messages"].as_array().into_iter().flatten().filter(|m| m["role"] == "user").filter_map(|m| m["content"].as_str().map(str::to_string)).collect();
                let first = s.lock().unwrap().is_empty();
                s.lock().unwrap().push(users);
                // The person types while the first answer streams.
                if first {
                    assert!(crate::inbox::add(&slot.lock().unwrap(), "Also: what about Friday?", vec![]));
                }
                let say = if first { "Thursday is free." } else { "Friday works too." };
                let chunk = |v: serde_json::Value| format!("data: {v}\n\n");
                let body = chunk(serde_json::json!({ "choices": [{ "delta": { "content": say }, "finish_reason": null }] }))
                    + &chunk(serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] }))
                    + "data: [DONE]\n\n";
                ([(axum::http::header::CONTENT_TYPE, "text/event-stream")], body)
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    let ep = engine::Endpoint { port, key: "k".into(), ctx: 8192, extra: serde_json::json!({}), cloud: None };
    let state = scratch_state();
    let cipher = state.cipher().unwrap();
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
    *chat_slot.lock().unwrap() = chat.id.clone();
    crate::inbox::open(&chat.id);
    let reply = say(&state, &ep, &cipher, &chat.id, "When am I free?").await;
    assert_eq!(reply, "Friday works too.", "it carried on to answer what was added");
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert!(seen[1].last().is_some_and(|m| m.ends_with("Also: what about Friday?")), "the model read it: {:?}", seen[1]);
    let all = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id);
    let roles: Vec<&str> = all.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(roles, ["user", "assistant", "user", "assistant"]);
    assert_eq!(all[2].meta.as_ref().and_then(|m| m.get("added")), Some(&serde_json::json!(true)), "marked as added mid-reply");
    assert!(!crate::inbox::waiting(&chat.id), "read once");
}

/// Chats working together: with the feature on, the model finds another
/// chat, reads it and uses what it says; a chat answering for another can
/// look but not ask on; incognito chats are never found.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chat_finds_and_reads_another_chat() {
    use axum::{routing::post, Json, Router};
    type Seen = std::sync::Arc<std::sync::Mutex<Vec<(Vec<String>, Vec<String>)>>>;
    let seen: Seen = Default::default();
    let s = seen.clone();
    let app = Router::new()
        .route("/tokenize", post(|Json(b): Json<serde_json::Value>| async move {
            let n = b["content"].as_str().map_or(0, |t| t.len() / 4 + 1);
            Json(serde_json::json!({ "tokens": vec![1; n] }))
        }))
        .route("/v1/chat/completions", post(move |Json(b): Json<serde_json::Value>| {
            let s = s.clone();
            async move {
                let names: Vec<String> = b["tools"].as_array().into_iter().flatten().filter_map(|t| t.pointer("/function/name").and_then(|n| n.as_str()).map(str::to_string)).collect();
                let results: Vec<String> = b["messages"].as_array().into_iter().flatten().filter(|m| m["role"] == "tool").filter_map(|m| m["content"].as_str().map(str::to_string)).collect();
                s.lock().unwrap().push((names, results.clone()));
                let chunk = |v: serde_json::Value| format!("data: {v}\n\n");
                let call = |name: &str, args: serde_json::Value| {
                    let c = serde_json::json!({ "index": 0, "id": format!("call_{}", results.len()), "type": "function", "function": { "name": name, "arguments": args.to_string() } });
                    chunk(serde_json::json!({ "choices": [{ "delta": { "tool_calls": [c] }, "finish_reason": null }] }))
                        + &chunk(serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "tool_calls" }] }))
                };
                let body = match results.len() {
                    0 => call("search_chats", serde_json::json!({ "query": "budget" })),
                    1 => call("read_chat", serde_json::json!({ "chat": "Trip budget" })),
                    _ => chunk(serde_json::json!({ "choices": [{ "delta": { "content": "The budget is $500." }, "finish_reason": null }] }))
                        + &chunk(serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] })),
                } + "data: [DONE]\n\n";
                ([(axum::http::header::CONTENT_TYPE, "text/event-stream")], body)
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    // A memory big enough for every tool (an 8K one leaves out the chat tools
    // for "And the hotel?", which doesn't mention chats: fit_small).
    let ep = engine::Endpoint { port, key: "k".into(), ctx: 32768, extra: serde_json::json!({}), cloud: None };
    let state = scratch_state();
    let cipher = state.cipher().unwrap();
    let (planning, secret, here) = {
        let conn = state.db.lock().unwrap();
        crate::features::set(&conn, crate::features::Feature::ChatTeam, true).unwrap();
        let mk = |title: &str, incognito: bool, said: &[(&str, &str)]| {
            let c = crate::db::create_chat_in(&conn, &cipher, None, None, incognito).unwrap();
            crate::db::set_chat_title(&conn, &cipher, &c.id, title).unwrap();
            for (i, (role, text)) in said.iter().enumerate() {
                crate::db::add_message(&conn, &cipher, &Message { id: format!("{}-{i}", c.id), chat_id: c.id.clone(), role: (*role).into(), content: (*text).into(), created_at: crate::db::now_ms() + i as i64, ..Default::default() }).unwrap();
            }
            c
        };
        let planning = mk("Trip budget", false, &[("user", "Our budget for Lisbon is $500, flights included."), ("assistant", "Got it: $500 total.")]);
        let secret = mk("Secret budget", true, &[("user", "The surprise party budget is $90.")]);
        let here = mk("Packing", false, &[]);
        (planning, secret, here)
    };
    let reply = say(&state, &ep, &cipher, &here.id, "What's my budget? Check my other chats.").await;
    assert_eq!(reply, "The budget is $500.");
    let first = seen.lock().unwrap().clone();
    assert!(["search_chats", "read_chat", "message_chat"].iter().all(|t| first[0].0.contains(&t.to_string())), "offered: {:?}", first[0].0);
    let found = &first[1].1[0];
    assert!(found.contains("Trip budget") && found.contains(&planning.id) && found.contains("$500"), "found it: {found}");
    assert!(!found.contains("Secret") && !found.contains(&secret.id), "never an incognito chat: {found}");
    let read = &first[2].1[1];
    assert!(read.contains("Our budget for Lisbon is $500") && read.contains("Got it: $500 total."), "read it: {read}");

    // Answering for another chat: it can look, but not ask on.
    let _asked = crate::tools::chats::Asked::new(&planning.id);
    say(&state, &ep, &cipher, &planning.id, "And the hotel?").await;
    let all = seen.lock().unwrap().clone();
    let offered = &all[first.len()].0;
    assert!(offered.contains(&"search_chats".to_string()) && !offered.contains(&"message_chat".to_string()), "{offered:?}");
}

/// Web search for real (DuckDuckGo, no key), reading a page, and the model
/// using both in a chat with web turned on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_web_search_and_read() {
    let (state, ep, cipher) = agent_harness().await;
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, Some("qwen3-1.7b".into())).unwrap();
    // Offline and web off: the model can only ask for web access (the popup
    // is answered no here; the stand-in test covers a yes).
    let st = state.clone();
    let answering = tokio::spawn(async move {
        for _ in 0..3000 {
            if let Some(p) = st.approvals.waiting().into_iter().find(|p| p.tool == "request_web") {
                st.approvals.answer(&p.call_id, crate::agent::Decision::Deny);
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        false
    });
    let reply = say(&state, &ep, &cipher, &chat.id, "Search the web: who maintains the curl project? /no_think").await;
    answering.abort();
    println!("offline reply: {reply}");
    let asked = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id)
        .iter()
        .any(|m| m.role == "tool" && m.meta.as_ref().is_some_and(|x| x["tool"] == "request_web"));
    println!("asked for web: {asked}");

    crate::db::set_chat_web(&state.db.lock().unwrap(), &chat.id, true).unwrap();
    let (client, settings, key) = crate::web::prepare(&state.db.lock().unwrap(), &cipher, &chat.id).unwrap();
    let (hits, _) = crate::web::search(&settings, key, &client, "curl project maintainer", 5, None).await.unwrap();
    println!("hits: {:#?}", hits.iter().map(|h| (&h.title, &h.url)).collect::<Vec<_>>());
    assert!(!hits.is_empty());
    let page = crate::web::fetch(&client, "https://curl.se/").await.unwrap();
    println!("page: {} ({} chars)", page.title, page.text.len());
    assert!(page.text.to_lowercase().contains("curl"));
    assert!(crate::web::fetch(&client, "http://127.0.0.1:1430/").await.is_err());

    let reply = say(&state, &ep, &cipher, &chat.id, "Web is on now. Search the web and tell me who created curl, with a source link. /no_think").await;
    println!("web reply: {reply}");
    let used: Vec<String> = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id)
        .iter()
        .filter(|m| m.role == "tool")
        .filter_map(|m| m.meta.as_ref().and_then(|x| x["title"].as_str().map(str::to_string)))
        .collect();
    println!("tools: {used:?}");
    assert!(used.iter().any(|t| t.starts_with("Searched the web")));
    assert!(reply.to_lowercase().contains("daniel") || reply.to_lowercase().contains("stenberg"), "{reply}");
    state.engine.lock().await.stop().await;
}

/// Performance limits reach the engine: Cool & quiet launches it with fewer
/// threads at low priority, and adaptive cooling's top level relaunches it
/// with fewer layers on the graphics card. Both still answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_perf_limits_reach_the_engine() {
    use crate::perf::{self, HeatState, Mode, PerfSettings};
    let real = Paths::new(app_data_dir()).unwrap();
    let hw = hardware::detect(&real.models);
    let full = Budget::from_hardware(&hw);
    let cat = Catalog::bundled();
    let spec = cat.model("qwen3-1.7b").unwrap().clone();
    let exe = engine::installed_server(&real, &cat.engine, engine::backend_for(&full).unwrap()).unwrap();
    let model = real.models.join(&spec.id).join(&spec.variant("Q4_K_M").unwrap().file);
    let log = std::env::temp_dir().join("sulcusai-perf-e2e.log");
    let mut eng = engine::Engine::default();

    let cool = perf::limits(&PerfSettings { mode: Mode::Cool, ..Default::default() }, &hw, false);
    let (ctx, layers) = catalog::launch_within(&spec, "Q4_K_M", &full, &cool);
    println!("cool: {} threads, ctx {ctx}, {layers} layers, low priority {}", cool.threads, cool.low_priority);
    let launch = |ctx, layers, l: &perf::Limits| engine::LaunchSpec { exe: &exe, model_path: &model, model_id: &spec.id, quant: "Q4_K_M", ctx, gpu_layers: layers, threads: l.threads, low_priority: l.low_priority, log: &log, mmproj: None, batch: 0, lora: None };
    let ep = eng.ensure(launch(ctx, layers, &cool), None).await.unwrap();
    let text = std::fs::read_to_string(&log).unwrap();
    let line = text.lines().find(|l| l.contains("n_threads")).unwrap_or("");
    println!("engine: {line}");
    assert!(line.contains(&format!("n_threads = {}", cool.threads)), "{line}");
    let prio = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", "(Get-Process llama-server | Select-Object -First 1).PriorityClass"])
        .output()
        .unwrap();
    let prio = String::from_utf8_lossy(&prio.stdout).trim().to_string();
    println!("priority: {prio}");
    assert_eq!(prio, "BelowNormal");
    assert!(engine::measure(&ep).await.unwrap().tps > 0.0);

    let hot = perf::scaled(cool, &HeatState { gpu_level: 3, cpu_level: 3, ..Default::default() });
    let (hctx, hlayers) = catalog::launch_within(&spec, "Q4_K_M", &full, &hot);
    println!("hot: {} threads, {hlayers} layers (was {layers})", hot.threads);
    assert!(hot.threads < cool.threads && hlayers < layers);
    let spec_hot = launch(hctx, hlayers, &hot);
    assert!(!eng.matches(&spec_hot), "new limits mean a relaunch");
    let ep = eng.ensure(spec_hot, None).await.unwrap();
    let tps = engine::measure(&ep).await.unwrap().tps;
    println!("hot speed: {tps} tokens/sec");
    assert!(tps > 0.0);
    eng.stop().await;
}

/// The model names a chat from its first request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_chat_gets_a_summary_title() {
    let (state, ep, _cipher) = agent_harness().await;
    for request in [
        "Can you help me plan a 5 day trip to Japan in April? I like food and temples, and my budget is about $3000.",
        "my python script keeps throwing KeyError when I read the csv, here's the code: df['Name']",
    ] {
        let t = crate::chat::summary_title(&ep, request).await;
        println!("{request:.40}… → {t:?}");
        let t = t.expect("a title");
        assert!((1..=8).contains(&t.split_whitespace().count()), "{t}");
    }
    state.engine.lock().await.stop().await;
}

/// Email and calendar with the real model, against the local test servers
/// (pymap on 11430, the SMTP script on 10250). Approvals are answered Allow,
/// as the user would, and recorded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_email_and_calendar() {
    let (state, ep, cipher) = agent_harness().await;
    {
        let conn = state.db.lock().unwrap();
        crate::db::update_settings(&conn, |s| s.connectivity = crate::net::Connectivity::Web).unwrap();
        let cfg = crate::mail::AccountConfig {
            name: "Demo".into(),
            email: "demo@example.org".into(),
            username: "demouser".into(),
            password: "demopass".into(),
            imap_host: "127.0.0.1".into(),
            imap_port: 11430,
            imap_security: crate::mail::Security::Plain,
            smtp_host: "127.0.0.1".into(),
            smtp_port: 10250,
            smtp_security: crate::mail::Security::Plain,
            ..Default::default()
        };
        crate::mail::store_account(&conn, &cipher, "acc", &cfg).unwrap();
    }
    let n = crate::mail::sync_account(&state, &cipher, "acc").await.unwrap();
    println!("synced {n} messages");

    let approved = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let (st, log) = (state.clone(), approved.clone());
    let approver = tokio::spawn(async move {
        loop {
            for p in st.approvals.waiting() {
                log.lock().unwrap().push(format!("{} [{:?}]", p.preview.title, p.risk));
                st.approvals.answer(&p.call_id, crate::agent::Decision::Allow);
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    });

    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, Some("qwen3-1.7b".into())).unwrap();
    let r = say(&state, &ep, &cipher, &chat.id, "Did I get an email about an important notice? Who sent it? /no_think").await;
    println!("mail answer: {r}");
    assert!(r.to_lowercase().contains("corp@example.com"), "{r}");

    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, Some("qwen3-1.7b".into())).unwrap();
    let r = say(&state, &ep, &cipher, &chat.id, "Put lunch with Bo on my calendar for friday at noon. /no_think").await;
    println!("calendar answer: {r}");
    let now = crate::db::now_ms();
    let events = crate::calendar::events(&state.db.lock().unwrap(), &cipher, now - 86_400_000, now + 8 * 86_400_000);
    println!("events: {:?}", events.iter().map(|e| (&e.data.title, e.start)).collect::<Vec<_>>());
    assert!(events.iter().any(|e| e.data.title.to_lowercase().contains("lunch")));

    let r = say(&state, &ep, &cipher, &chat.id, "When am I free on friday for an hour? /no_think").await;
    println!("free answer: {r}");

    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, Some("qwen3-1.7b".into())).unwrap();
    let r = say(&state, &ep, &cipher, &chat.id, "Email ana@example.com and tell her the budget review moved to Friday at 2pm. Sign it Demo. /no_think").await;
    println!("send answer: {r}");
    approver.abort();
    let log = approved.lock().unwrap().clone();
    println!("approvals: {log:#?}");
    assert!(log.iter().any(|l| l.contains("Send an email to ana@example.com") && l.contains("Submit")), "sending asked first");
    assert!(log.iter().any(|l| l.contains("Lunch")), "adding the event asked first");
    assert!(!log.iter().any(|l| l.contains("invite")), "no made-up invitations");
    state.engine.lock().await.stop().await;
}

/// A test mailbox: a shipping notice, two newsletters, a security alert, a
/// party invitation, reminders, mail from Mom and 22 store promotions. Mail
/// older than 4 hours is read unless `all_unread`.
fn seed_test_mail(state: &std::sync::Arc<crate::AppState>, cipher: &crate::crypto::Cipher, all_unread: bool) {
    let now = crate::db::now_ms();
    let hour = 3_600_000;
    {
        let conn = state.db.lock().unwrap();
        let cfg = crate::mail::AccountConfig { name: "Isaac".into(), email: "isaac@example.org".into(), ..Default::default() };
        crate::mail::store_account(&conn, cipher, "acc", &cfg).unwrap();
        let long = "Read more about this week's stories. ".repeat(600);
        let mails: Vec<(&str, &str, &str, String, i64)> = vec![
            ("deals@amazon.com", "Amazon", "Your package has shipped", "Your order of a USB-C cable has shipped and arrives Thursday.".into(), 1),
            ("news@nytimes.com", "The New York Times", "The Morning: the week ahead", long.clone(), 2),
            ("no-reply@accounts.google.com", "Google", "Security alert", "SulcusAI was granted access to your Google Account isaac@example.org. If you did not grant access, you should check this activity and secure your account. Check activity: https://myaccount.google.com/notifications".into(), 3),
            ("sam@example.org", "Sam Rivera", "Party tonight!", "Hey! My birthday party is tonight at 6pm at my place, 42 Elm Street. Bring snacks.".into(), 5),
            ("calendar-notification@google.com", "Google Calendar", "Reminder: Dentist @ Wed", "This is a reminder for Dentist on Wednesday at 9am.".into(), 26),
            ("newsletter@medium.com", "Medium Daily Digest", "Stories for you", long.clone(), 30),
            ("no-reply@accounts.google.com", "Google", "Your Google Account storage is almost full", "You've used 14 GB of your 15 GB of Google storage.".into(), 50),
            ("mom@example.org", "Mom", "Sunday dinner", "Are you coming Sunday? Dad is making lasagna.".into(), 52),
            // Marketing that sounds urgent, next to real deadlines.
            ("deals@lenovo.example", "Lenovo US", "Last Chance for Your Extra 10% Off", "Ends tonight at midnight! Use code EXTRA10 on laptops and monitors.".into(), 8),
            ("giveaway@717supply.example", "717 SUPPLY", "FINAL HOURS: Your Entries Count TWICE", "Enter now to win a Tahoe, a Corvette or $75,000. Final hours, don't miss out!".into(), 9),
            ("quotes@libertymutual.example", "Liberty Mutual", "Your quote is patiently waiting, Isaac.", "Customized coverage so you only pay for what you need. Finish your quote today.".into(), 10),
            ("rewards@dutchbros.example", "Dutch Bros Coffee", "2X POINTS today only", "Earn double points on every drink today only. Open the app to activate.".into(), 11),
            ("billing@rockymountainpower.example", "Rocky Mountain Power", "Your bill of $84.20 is due Oct 20", "Your October statement is ready. Amount due: $84.20 by October 20. Pay online or set up autopay.".into(), 12),
            ("noreply@delta.example", "Delta", "Check in for your flight to Denver tomorrow", "Your flight DL1234 departs SLC tomorrow at 8:05 AM. Check in now to get your boarding pass.".into(), 13),
        ];
        let mut all = mails;
        for i in 0..22 {
            all.push(("promo@store.example", "Store", "Weekly sale", format!("Sale number {i}: everything 20% off. {}", "Shop now. ".repeat(200)), 60 + i * 5));
        }
        for (uid, (from, name, subject, body, hours_ago)) in all.into_iter().enumerate() {
            let data = crate::mail::MailData { from: from.into(), from_name: name.into(), to: vec!["isaac@example.org".into()], subject: subject.into(), snippet: body.chars().take(160).collect(), body, ..Default::default() };
            crate::mail::save_item(&conn, cipher, "acc", "INBOX", uid as u32 + 1, now - hours_ago * hour, !all_unread && hours_ago > 4, &data).unwrap();
        }
    }
}

/// The speed test on a real model (SULCUSAI_TEST_GGUF and friends): writing
/// and reading speed, and a second run isn't sped up by the prompt cache.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_speed_test() {
    let ctx = std::env::var("SULCUSAI_TEST_CTX").ok().and_then(|v| v.parse().ok()).unwrap_or(8192);
    let layers = std::env::var("SULCUSAI_TEST_LAYERS").ok().and_then(|v| v.parse().ok()).unwrap_or(99);
    let blob = std::env::var("SULCUSAI_TEST_GGUF").unwrap_or_default();
    let (state, ep, _) = agent_harness_on(Some((blob.into(), ctx, layers)), None).await;
    let first = engine::measure(&ep).await.unwrap();
    let second = engine::measure(&ep).await.unwrap();
    println!("first: {first:?}
second: {second:?}");
    state.engine.lock().await.stop().await;
    assert!(first.tps > 1.0 && second.tps > 1.0);
    let (p1, p2) = (first.prompt_tps.unwrap(), second.prompt_tps.unwrap());
    assert!(p2 < p1 * 3.0, "the second test reads the prompt again (cache off): {p1} vs {p2}");
}

/// Sorting unread mail with a real model (SULCUSAI_TEST_GGUF and friends, as
/// above): promotions and newsletters are marked read, people and alerts stay.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_mail_triage_on_model() {
    let ctx = std::env::var("SULCUSAI_TEST_CTX").ok().and_then(|v| v.parse().ok()).unwrap_or(8192);
    let layers = std::env::var("SULCUSAI_TEST_LAYERS").ok().and_then(|v| v.parse().ok()).unwrap_or(99);
    let running = std::env::var("SULCUSAI_TEST_PORT").ok().and_then(|p| p.parse().ok()).map(|port| engine::Endpoint { port, key: std::env::var("SULCUSAI_TEST_KEY").unwrap_or_default(), ctx, extra: serde_json::Value::Null, cloud: None });
    let blob = std::env::var("SULCUSAI_TEST_GGUF").unwrap_or_default();
    let stop_after = running.is_none();
    let (state, ep, cipher) = agent_harness_on(Some((blob.into(), ctx, layers)), running).await;
    seed_test_mail(&state, &cipher, true);
    let t = std::time::Instant::now();
    let report = crate::triage::run(&state, &cipher, &ep, "test model", false, |done, total| println!("sorted {done} of {total}")).await.unwrap();
    println!("took {:.1} s", t.elapsed().as_secs_f32());
    if stop_after {
        state.engine.lock().await.stop().await;
    }
    for i in &report.items {
        println!("{} | {:<28} | {:<45} | {} ({})", if i.important { "KEEP " } else { "READ " }, i.from_name, i.subject, i.reason, i.next);
    }
    let kept = |name: &str, subject: &str| report.items.iter().any(|i| i.from_name == name && i.subject.contains(subject) && i.important);
    assert_eq!(report.looked_at, 36);
    assert!(kept("Sam Rivera", "Party") && kept("Mom", "Sunday") && kept("Google", "Security"), "people and the security alert stay unread");
    assert!(kept("Rocky Mountain Power", "bill") && kept("Delta", "flight"), "a bill due and a flight are kept");
    let urgent_ads = ["Lenovo US", "717 SUPPLY", "Liberty Mutual", "Dutch Bros Coffee"];
    let ads_cleared = report.items.iter().filter(|i| urgent_ads.contains(&i.from_name.as_str()) && i.marked_read).count();
    println!("urgent-sounding ads cleared: {ads_cleared} of 4");
    assert!(ads_cleared >= 3, "marketing urgency doesn't make an ad important");
    let promos = report.items.iter().filter(|i| i.from_name == "Store").count();
    let cleared = report.items.iter().filter(|i| i.from_name == "Store" && i.marked_read).count();
    println!("promotions cleared: {cleared} of {promos}");
    assert!(cleared * 10 >= promos * 8, "at least 80% of the promotions are marked read");
    let unread_after = crate::mail::list(&state.db.lock().unwrap(), &cipher, "", 100).iter().filter(|m| !m.seen).count();
    assert_eq!(unread_after, report.items.iter().filter(|i| i.important).count(), "exactly the kept ones are still unread");
}

/// Asked to make something with a folder shared, a model that would paste
/// the code (qwen2.5-coder) makes the files instead. SULCUSAI_TEST_GGUF is
/// the model file; approvals are answered Allow.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_makes_files_on_ollama_model() {
    let ctx = std::env::var("SULCUSAI_TEST_CTX").ok().and_then(|v| v.parse().ok()).unwrap_or(8192);
    let layers = std::env::var("SULCUSAI_TEST_LAYERS").ok().and_then(|v| v.parse().ok()).unwrap_or(99);
    let blob = std::env::var("SULCUSAI_TEST_GGUF").expect("SULCUSAI_TEST_GGUF");
    let (state, ep, cipher) = agent_harness_on(Some((blob.into(), ctx, layers)), None).await;
    let work = std::env::temp_dir().join(format!("sulcusai-make-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&work).unwrap();
    // Approvals as a person would give them, but a new folder goes in the
    // test's folder (not the Desktop) and nothing is opened on the screen.
    let (st, inside) = (state.clone(), work.clone());
    let approver = tokio::spawn(async move {
        loop {
            for p in st.approvals.waiting() {
                if p.tool == "request_folder" {
                    crate::workspace::offer_folder(&p.call_id, &inside.join("asked"));
                }
                let no = p.tool == "open_path" || p.tool == "open_page";
                st.approvals.answer(&p.call_id, if no { crate::agent::Decision::Deny } else { crate::agent::Decision::Allow });
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    });
    crate::db::add_folder(&state.db.lock().unwrap(), &dunce::canonicalize(&work).unwrap().display().to_string()).unwrap();
    // SULCUSAI_TEST_ASKS: other requests, ";;" between them (each must make files).
    let custom = std::env::var("SULCUSAI_TEST_ASKS").ok();
    let asks: Vec<String> = match &custom {
        Some(list) => list.split(";;").map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect(),
        None => vec!["Make me a simple website for my bakery called Sweet Crumbs".into(), "Create a python script that prints the first 20 prime numbers".into()],
    };
    // SULCUSAI_TEST_BEFORE: an earlier ask the model answered with instructions,
    // so the ask is a follow-up like "try again to do it for me".
    let before = std::env::var("SULCUSAI_TEST_BEFORE").ok();
    let mut made = Vec::new();
    for q in asks.iter().map(String::as_str) {
        let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
        if let Some(b) = &before {
            let at = crate::db::now_ms() - 60_000;
            for (role, text) in [("user", b.as_str()), ("assistant", "Sure! I'll guide you through setting up a simple website.\n\n### Step 1: Set Up the Project\n\n1. Create a new folder for your project.\n2. Run these commands:\n\n```bash\nmkdir site\ncd site\nnpm init -y\n```\n\nThen create index.html and paste the code below.")] {
                crate::db::add_message(&state.db.lock().unwrap(), &cipher, &Message { id: uuid::Uuid::new_v4().to_string(), chat_id: chat.id.clone(), role: role.into(), content: text.into(), created_at: at, ..Default::default() }).unwrap();
            }
        }
        let t = std::time::Instant::now();
        let answer = say(&state, &ep, &cipher, &chat.id, q).await;
        let msgs = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id);
        let calls: Vec<String> = msgs.iter().flat_map(|m| m.tool_calls.clone().unwrap_or_default()).map(|c| format!("{}({:.80})", c.name, c.arguments)).collect();
        let files: Vec<String> = walkdir_names(&work);
        println!("\n=== {q}\n{:.1} s, calls: {calls:?}\nfiles now: {files:?}\n{answer:.600}", t.elapsed().as_secs_f32());
        made.push(files.len());
    }
    approver.abort();
    state.engine.lock().await.stop().await;
    if custom.is_some() {
        // Each request made at least one more file.
        assert!(made.windows(2).all(|w| w[1] > w[0]) && made[0] > 0, "files made after each request: {made:?}");
        return;
    }
    assert!(made[0] > 0, "the website was made as files");
    assert!(made[1] > made[0], "the script was made as a file");
}

/// What his brother uses it for, on a real model (`test_model`): a second
/// opinion on a stock, then in one chat (as a person goes on) reading an app
/// that's already built and explaining it, fixing the bug its tests show and
/// adding the next feature its README lists, and in another a complete app
/// from nothing. Each is checked here (the tests run, the files are there),
/// not taken from the reply. SULCUSAI_TEST_PARTS picks some (stocks,
/// explain, bugs, extend, build; all unless set); SULCUSAI_TEST_OUT keeps the
/// work to look at; SULCUSAI_TEST_LOG gets the transcripts.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_second_voice_and_real_work() {
    let (_eng, ep, name) = test_model().await;
    let (state, ep, cipher) = agent_harness_on(None, Some(ep)).await;
    let parts = std::env::var("SULCUSAI_TEST_PARTS").unwrap_or_else(|_| "stocks,explain,bugs,extend,build".into());
    let wants = |p: &str| parts.split(',').any(|x| x.trim() == p);
    let work = std::env::var("SULCUSAI_TEST_OUT").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join(format!("sulcusai-work-{}", uuid::Uuid::new_v4().simple())));
    let project = work.join("trade-journal");
    for (file, text) in TRADE_JOURNAL {
        let path = project.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let (work, project) = (dunce::canonicalize(&work).unwrap(), dunce::canonicalize(&project).unwrap());
    // Approvals as a person would give them, but a new folder goes in the
    // work folder (not the Desktop) and nothing is opened on the screen.
    let (st, inside) = (state.clone(), work.clone());
    let approver = tokio::spawn(async move {
        loop {
            for p in st.approvals.waiting() {
                if p.tool == "request_folder" {
                    crate::workspace::offer_folder(&p.call_id, &inside.join("asked"));
                }
                let no = p.tool == "open_path" || p.tool == "open_page";
                st.approvals.answer(&p.call_id, if no { crate::agent::Decision::Deny } else { crate::agent::Decision::Allow });
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    });
    let node_tests = |dir: &std::path::Path| {
        let out = std::process::Command::new("node").arg("--test").current_dir(dir).output();
        out.map(|o| (o.status.success(), test_counts(&String::from_utf8_lossy(&o.stdout)))).unwrap_or((false, "node didn't run".into()))
    };
    // Where a turn stopped by itself instead of finishing.
    const STOPS: [&str; 4] = ["I kept repeating the same lookup", "steps on this, so I'm pausing", "I kept trying the same thing", "I couldn't get that from the internet"];
    let mut results: Vec<String> = Vec::new();
    let turn = |chat: &str, label: &str, started: std::time::Instant| {
        let (text, tools) = transcript(&state, &cipher, chat, &format!("{name}: {label} ({:.0}s)", started.elapsed().as_secs_f64()));
        let stopped: Vec<&str> = STOPS.iter().copied().filter(|s| text.contains(s)).collect();
        let steps = crate::db::messages(&state.db.lock().unwrap(), &cipher, chat).iter().filter(|m| m.role == "assistant").count();
        (tools, stopped, steps)
    };

    if wants("stocks") {
        crate::db::update_settings(&state.db.lock().unwrap(), |s| s.connectivity = crate::net::Connectivity::Web).unwrap();
        let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
        crate::db::set_chat_web(&state.db.lock().unwrap(), &chat.id, true).unwrap();
        let t = std::time::Instant::now();
        let reply = say(&state, &ep, &cipher, &chat.id, "I swing trade and want a second opinion. Where is NVDA trading right now, how has it moved over the last month, and what would you watch before buying this week? Be straight with me.").await;
        let (tools, stopped, steps) = turn(&chat.id, "stocks", t);
        let looked = tools.iter().any(|t| t == "market_quote" || t == "web_search" || t == "research");
        let priced = regex::Regex::new(r"\$\s?\d{2,4}(\.\d+)?").unwrap().is_match(&reply);
        results.push(format!("stocks: looked up {looked}, a price in the answer {priced}, {} words, {steps} steps, stops {stopped:?}, {:.0}s", reply.split_whitespace().count(), t.elapsed().as_secs_f64()));
    }

    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
    crate::db::add_chat_folder(&state.db.lock().unwrap(), &chat.id, &project.display().to_string()).unwrap();
    if wants("explain") {
        let t = std::time::Instant::now();
        let reply = say(&state, &ep, &cipher, &chat.id, "Read the project in my folder and explain what it does, how the files fit together, and anything that looks wrong or risky.").await;
        let (tools, stopped, steps) = turn(&chat.id, "explain", t);
        let read = tools.iter().filter(|t| *t == "read_file").count();
        let low = reply.to_lowercase();
        let names = ["trades.js", "report.js", "index.js"].iter().filter(|f| low.contains(*f)).count();
        // Saying it keeps a weighted average is the bug read as intended.
        let spotted = ["overwrit", "last price", "last buy", "not weighted", "not a weighted", "isn't weighted", "isn't a weighted", "avgcost = t.price", "replaces the average"].iter().any(|w| low.contains(w));
        results.push(format!("explain: {read} files read, {names}/3 files named, averaging bug spotted {spotted}, {steps} steps, stops {stopped:?}, {:.0}s", t.elapsed().as_secs_f64()));
    }
    if wants("bugs") {
        let t = std::time::Instant::now();
        say(&state, &ep, &cipher, &chat.id, "npm test fails. Find out why, fix the bug in the code (not the tests), then run the tests again until they all pass.").await;
        let (tools, stopped, steps) = turn(&chat.id, "bugs", t);
        let (pass, counts) = node_tests(&project);
        let tests_kept = std::fs::read_to_string(project.join("test/trades.test.js")).is_ok_and(|s| s.contains("assert.strictEqual(p[0].avgCost, 175)"));
        results.push(format!("bugs: tests pass {pass} ({counts}), tests left as they were {tests_kept}, ran them {}, {steps} steps so far, stops {stopped:?}, {:.0}s", tools.iter().any(|t| t == "run_command"), t.elapsed().as_secs_f64()));
    }
    if wants("extend") {
        let t = std::time::Instant::now();
        say(&state, &ep, &cipher, &chat.id, "Now add the pnl command that the README lists under Next, with a test for it, and make sure all the tests pass.").await;
        let (_, stopped, steps) = turn(&chat.id, "extend", t);
        let (pass, counts) = node_tests(&project);
        let run = std::process::Command::new("node").args(["src/index.js", "pnl", "trades.csv", "prices.json"]).current_dir(&project).output();
        let shown = run.as_ref().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
        let works = run.is_ok_and(|o| o.status.success()) && shown.contains('%') && shown.contains("NVDA");
        println!("pnl prints:\n{shown}");
        results.push(format!("extend: pnl works {works}, tests pass {pass} ({counts}), {steps} steps in the chat, stops {stopped:?}, {:.0}s", t.elapsed().as_secs_f64()));
    }
    if wants("build") {
        let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
        crate::db::add_chat_folder(&state.db.lock().unwrap(), &chat.id, &work.display().to_string()).unwrap();
        let t = std::time::Instant::now();
        say(&state, &ep, &cipher, &chat.id, "Build a complete budget tracker web app in a new folder called budget: add income and expenses with a category and date, edit and delete entries, a running balance, a chart of spending by category, and keep everything saved in the browser. Check that it all works before you finish.").await;
        let (_, stopped, steps) = turn(&chat.id, "build", t);
        let msgs = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id);
        let checks: Vec<u64> = msgs.iter().filter_map(|m| m.meta.as_ref()?.get("preview_problems")?.as_u64()).collect();
        // Where it put the app: "budget" in the work folder, or in a folder it asked for.
        fn all_text(dir: &std::path::Path) -> String {
            std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| if e.path().is_dir() { all_text(&e.path()) } else { std::fs::read_to_string(e.path()).unwrap_or_default() }).collect::<Vec<_>>().join("\n")
        }
        let site = [work.join("budget"), work.join("asked").join("budget"), work.join("asked")].into_iter().find(|p| p.is_dir()).unwrap_or_else(|| work.join("budget"));
        let code = all_text(&site).to_lowercase();
        let has = |w: &[&str]| w.iter().any(|x| code.contains(x));
        let features = [("saved", has(&["localstorage"])), ("delete", has(&["delete", "remove"])), ("edit", has(&["edit"])), ("chart", has(&["<canvas", "chart"])), ("balance", has(&["balance"]))];
        // Parts left as "TODO" aren't done, whatever words are there.
        let unfinished = crate::tools::fs::added_stubs("", &code).len();
        results.push(format!("build: in {}, files {:?}, problems at each check {checks:?}, features {features:?}, unfinished lines {unfinished}, {steps} steps, stops {stopped:?}, {:.0}s", site.display(), walkdir_names(&site), t.elapsed().as_secs_f64()));
    }

    approver.abort();
    for s in crate::tools::shell::servers().into_iter().filter(|s| s.running) {
        crate::tools::shell::stop_server(&s.id).ok();
    }
    state.engine.lock().await.stop().await;
    let summary = format!("===== {name}: summary =====\n{}\n", results.join("\n"));
    println!("{summary}");
    if let Ok(path) = std::env::var("SULCUSAI_TEST_LOG") {
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = f.write_all(summary.as_bytes());
        }
    }
    if std::env::var("SULCUSAI_TEST_OUT").is_err() {
        let _ = std::fs::remove_dir_all(&work);
    }
}

/// The trade journal's tests fail as written (two of three) and pass once
/// the average cost is a weighted average: the bug is the one meant. Needs node.
#[test]
#[ignore]
fn trade_journal_fails_until_its_bug_is_fixed() {
    let dir = std::env::temp_dir().join(format!("sulcusai-journal-{}", uuid::Uuid::new_v4().simple()));
    for (file, text) in TRADE_JOURNAL {
        std::fs::create_dir_all(dir.join(file).parent().unwrap()).unwrap();
        std::fs::write(dir.join(file), text).unwrap();
    }
    let run = || {
        let o = std::process::Command::new("node").arg("--test").current_dir(&dir).output().unwrap();
        (o.status.success(), test_counts(&String::from_utf8_lossy(&o.stdout)))
    };
    let before = run();
    let code = std::fs::read_to_string(dir.join("src/trades.js")).unwrap();
    let fixed = code.replace("p.avgCost = t.price;", "p.avgCost = (p.avgCost * p.shares + t.price * t.shares) / (p.shares + t.shares);");
    std::fs::write(dir.join("src/trades.js"), fixed).unwrap();
    let after = run();
    let report = std::process::Command::new("node").args(["src/index.js", "report"]).current_dir(&dir).output().unwrap();
    println!("before {before:?}\nafter {after:?}\n{}", String::from_utf8_lossy(&report.stdout));
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(before, (false, "pass 1, fail 2".to_string()));
    assert_eq!(after, (true, "pass 3, fail 0".to_string()));
}

/// "pass N, fail N" from node's test runner (Node 24 writes "ℹ pass 3",
/// older ones "# pass 3").
fn test_counts(out: &str) -> String {
    out.lines().map(|l| l.trim_start_matches(['ℹ', '#', ' '])).filter(|l| l.starts_with("pass ") || l.starts_with("fail ")).collect::<Vec<_>>().join(", ")
}

/// A small command-line app that's already built, for the model to read, fix
/// and build on: its average cost is the last price paid, not the average,
/// so two of its three tests fail; its README lists the next feature.
const TRADE_JOURNAL: [(&str, &str); 8] = [
    ("package.json", "{\n  \"name\": \"trade-journal\",\n  \"version\": \"0.1.0\",\n  \"private\": true,\n  \"scripts\": { \"test\": \"node --test\" }\n}\n"),
    (
        "README.md",
        "# Trade journal\n\nA small command-line tool that reads my trades from trades.csv and prints my open positions with what they're worth now (prices from prices.json).\n\n    node src/index.js report trades.csv prices.json\n    npm test\n\n## Next\n\n- A `pnl` command: each open position's profit or loss in percent of what it cost, sorted from best to worst.\n",
    ),
    ("trades.csv", "date,symbol,side,shares,price\n2026-09-02,NVDA,buy,20,118.40\n2026-09-15,NVDA,buy,10,131.10\n2026-09-20,AAPL,buy,15,226.80\n2026-10-01,NVDA,sell,8,140.25\n2026-10-06,MSFT,buy,5,432.00\n2026-10-13,AAPL,buy,5,231.50\n"),
    ("prices.json", "{ \"NVDA\": 139.5, \"AAPL\": 229.1, \"MSFT\": 425.75 }\n"),
    (
        "src/trades.js",
        "// Reads trades from a CSV file and works out the open positions.\nconst fs = require(\"fs\");\n\nfunction parseTrades(csv) {\n  const lines = csv.trim().split(/\\r?\\n/);\n  const header = lines.shift().split(\",\").map((h) => h.trim());\n  return lines.map((line) => {\n    const cells = line.split(\",\");\n    const row = Object.fromEntries(header.map((h, i) => [h, (cells[i] || \"\").trim()]));\n    return { date: row.date, symbol: row.symbol.toUpperCase(), side: row.side, shares: Number(row.shares), price: Number(row.price) };\n  });\n}\n\nfunction loadTrades(file) {\n  return parseTrades(fs.readFileSync(file, \"utf8\"));\n}\n\n// Shares held, average cost per share and profit booked by sales, by symbol.\nfunction positions(trades) {\n  const out = {};\n  for (const t of trades) {\n    const p = out[t.symbol] || (out[t.symbol] = { symbol: t.symbol, shares: 0, avgCost: 0, realized: 0 });\n    if (t.side === \"buy\") {\n      p.avgCost = t.price;\n      p.shares += t.shares;\n    } else {\n      p.realized += (t.price - p.avgCost) * t.shares;\n      p.shares -= t.shares;\n    }\n  }\n  return Object.values(out).filter((p) => p.shares > 0);\n}\n\nmodule.exports = { parseTrades, loadTrades, positions };\n",
    ),
    (
        "src/report.js",
        "// A text report of open positions: market value and profit or loss not yet taken.\nfunction money(n) {\n  return (n < 0 ? \"-$\" : \"$\") + Math.abs(n).toFixed(2);\n}\n\nfunction report(positions, prices) {\n  const rows = positions.map((p) => {\n    const price = prices[p.symbol];\n    const value = p.shares * price;\n    return { ...p, price, value, pnl: value - p.shares * p.avgCost };\n  });\n  const total = rows.reduce((sum, r) => sum + r.value, 0);\n  const lines = rows.map((r) => `${r.symbol.padEnd(6)} ${String(r.shares).padStart(5)} @ ${money(r.avgCost)}  now ${money(r.price)}  value ${money(r.value)}  P/L ${money(r.pnl)}`);\n  lines.push(`Total value: ${money(total)}`);\n  return lines.join(\"\\n\");\n}\n\nmodule.exports = { report, money };\n",
    ),
    (
        "src/index.js",
        "#!/usr/bin/env node\n// Usage: node src/index.js report [trades.csv] [prices.json]\nconst path = require(\"path\");\nconst { loadTrades, positions } = require(\"./trades\");\nconst { report } = require(\"./report\");\n\nconst [cmd, tradesFile = \"trades.csv\", pricesFile = \"prices.json\"] = process.argv.slice(2);\nconst prices = require(path.resolve(pricesFile));\n\nif (cmd === \"report\") {\n  console.log(report(positions(loadTrades(tradesFile)), prices));\n} else {\n  console.log(\"Usage: node src/index.js report [trades.csv] [prices.json]\");\n  process.exit(1);\n}\n",
    ),
    (
        "test/trades.test.js",
        "const test = require(\"node:test\");\nconst assert = require(\"node:assert\");\nconst { parseTrades, positions } = require(\"../src/trades\");\n\nconst head = \"date,symbol,side,shares,price\\n\";\n\ntest(\"one buy\", () => {\n  const p = positions(parseTrades(head + \"2026-01-02,aapl,buy,10,150\"));\n  assert.deepStrictEqual(p.map((x) => [x.symbol, x.shares, x.avgCost]), [[\"AAPL\", 10, 150]]);\n});\n\ntest(\"two buys average the cost\", () => {\n  const p = positions(parseTrades(head + \"2026-01-02,MSFT,buy,10,100\\n2026-01-09,MSFT,buy,30,200\"));\n  assert.strictEqual(p[0].shares, 40);\n  assert.strictEqual(p[0].avgCost, 175);\n});\n\ntest(\"a sale keeps the average and books the profit\", () => {\n  const p = positions(parseTrades(head + \"2026-01-02,NVDA,buy,10,100\\n2026-01-03,NVDA,buy,10,120\\n2026-02-01,NVDA,sell,5,150\"));\n  assert.strictEqual(p[0].shares, 15);\n  assert.strictEqual(p[0].avgCost, 110);\n  assert.strictEqual(p[0].realized, 200);\n});\n",
    ),
];

/// Builds a live stock-exchange site with a real model, starts its server
/// in the background, and checks the server answers and that SulcusAI's
/// browser may show it (a preview). SULCUSAI_TEST_GGUF: the model file;
/// SULCUSAI_TEST_OUT: the folder it works in (kept, to look at).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_builds_a_live_site_and_serves_it() {
    let ctx = std::env::var("SULCUSAI_TEST_CTX").ok().and_then(|v| v.parse().ok()).unwrap_or(16384);
    let layers = std::env::var("SULCUSAI_TEST_LAYERS").ok().and_then(|v| v.parse().ok()).unwrap_or(99);
    let blob = std::env::var("SULCUSAI_TEST_GGUF").expect("SULCUSAI_TEST_GGUF");
    let work = std::path::PathBuf::from(std::env::var("SULCUSAI_TEST_OUT").expect("SULCUSAI_TEST_OUT"));
    std::fs::create_dir_all(&work).unwrap();
    let (state, ep, cipher) = agent_harness_on(Some((blob.into(), ctx, layers)), None).await;
    let (st, inside) = (state.clone(), work.clone());
    let approver = tokio::spawn(async move {
        loop {
            for p in st.approvals.waiting() {
                if p.tool == "request_folder" {
                    crate::workspace::offer_folder(&p.call_id, &inside.join("site"));
                }
                let no = p.tool == "open_path" || p.tool == "open_page";
                st.approvals.answer(&p.call_id, if no { crate::agent::Decision::Deny } else { crate::agent::Decision::Allow });
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    });
    crate::db::add_folder(&state.db.lock().unwrap(), &dunce::canonicalize(&work).unwrap().display().to_string()).unwrap();
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
    let ask = "Build me a live stock exchange website in a new folder called stock-exchange: a scrolling ticker strip, a table of 8 companies \
               whose prices update every second (make up realistic prices and moves in JavaScript, no outside data), a live price chart \
               for the selected stock drawn on a canvas, and buy and sell buttons with a cash balance and portfolio. Then start a local \
               server for it in the background on port 8765 so I can preview it.";
    let t = std::time::Instant::now();
    let answer = say(&state, &ep, &cipher, &chat.id, ask).await;
    let msgs = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id);
    let calls: Vec<String> = msgs.iter().flat_map(|m| m.tool_calls.clone().unwrap_or_default()).map(|c| format!("{}({:.90})", c.name, c.arguments)).collect();
    println!("{:.1} s, calls: {calls:#?}\nfiles: {:?}\n{answer:.800}", t.elapsed().as_secs_f32(), walkdir_names(&work));
    let preview = msgs.iter().filter_map(|m| m.meta.as_ref()?.get("preview_port")?.as_u64()).last();
    println!("preview offered on port {preview:?}; servers: {:?}", crate::tools::shell::servers().iter().map(|s| (&s.command, s.running, &s.url)).collect::<Vec<_>>());
    let port = preview.unwrap_or(8765) as u16;
    let page = reqwest::get(format!("http://localhost:{port}/")).await;
    let ok = page.as_ref().is_ok_and(|r| r.status().is_success());
    let body = match page {
        Ok(r) => r.text().await.unwrap_or_default(),
        Err(e) => e.to_string(),
    };
    let previewable = crate::ports::previewable(port);
    println!("GET :{port} -> {ok}, {} bytes; previewable: {previewable}", body.len());
    // The server holds its output pipe open: stop it, or the test can't end.
    for s in crate::tools::shell::servers().into_iter().filter(|s| s.running) {
        crate::tools::shell::stop_server(&s.id).ok();
    }
    approver.abort();
    state.engine.lock().await.stop().await;
    assert!(ok && body.to_lowercase().contains("<html"), "the site is served");
    assert!(previewable, "SulcusAI's browser may show the assistant's own server");
    assert!(!crate::ports::previewable(1), "but not any port on this PC");
}

fn walkdir_names(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walkdir_names(&p));
        } else {
            out.push(p.file_name().unwrap().to_string_lossy().to_string());
        }
    }
    out
}

/// Mail and calendar questions the way people ask them, on the model Ollama
/// downloaded (qwen2.5-coder 14B, the one these went wrong on): every tool
/// call is printed with how long each answer took.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_mail_questions_on_ollama_model() {
    // The settings SulcusAI picked for it on his laptop (it doesn't all fit on the graphics card).
    let ctx = std::env::var("SULCUSAI_TEST_CTX").ok().and_then(|v| v.parse().ok()).unwrap_or(8192);
    let layers = std::env::var("SULCUSAI_TEST_LAYERS").ok().and_then(|v| v.parse().ok()).unwrap_or(47);
    // SULCUSAI_TEST_PORT/KEY: use the engine the app already has loaded.
    let running = std::env::var("SULCUSAI_TEST_PORT").ok().and_then(|p| p.parse().ok()).map(|port| engine::Endpoint { port, key: std::env::var("SULCUSAI_TEST_KEY").unwrap_or_default(), ctx, extra: serde_json::Value::Null, cloud: None });
    let blob = std::env::var("SULCUSAI_TEST_GGUF").unwrap_or_default();
    let stop_after = running.is_none();
    let (state, ep, cipher) = agent_harness_on(Some((blob.into(), ctx, layers)), running).await;
    // Approval cards are answered as a person would: yes, except opening a sign-in page.
    let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let (st, log) = (state.clone(), asked.clone());
    let approver = tokio::spawn(async move {
        loop {
            for p in st.approvals.waiting() {
                log.lock().unwrap().push(p.preview.title.clone());
                let yes = !p.preview.title.starts_with("Connect");
                st.approvals.answer(&p.call_id, if yes { crate::agent::Decision::Allow } else { crate::agent::Decision::Deny });
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    });
    let hour = 3_600_000;
    seed_test_mail(&state, &cipher, false);
    let today6 = {
        use chrono::{Local, TimeZone};
        let d = Local::now().date_naive().and_hms_opt(18, 0, 0).unwrap();
        Local.from_local_datetime(&d).earliest().unwrap().timestamp_millis()
    };
    crate::calendar::create(&state, &cipher, crate::calendar::NewEvent { title: "Sam's Birthday Party".into(), start: today6, end: today6 + 3 * hour, all_day: false, location: "42 Elm Street".into(), notes: String::new(), attendees: vec![], calendar: None }).await.unwrap();

    let mut report = Vec::new();
    let questions = ["Summarize my most recent email from Google", "When is sam's party?", "Do I have any unread email?", "What did my mom ask me?", "What email accounts do I have connected?", "Connect my Outlook email too"];
    let only: Option<usize> = std::env::var("SULCUSAI_TEST_ONLY").ok().and_then(|v| v.parse().ok());
    for (i, q) in questions.into_iter().enumerate() {
        if only.is_some_and(|o| o != i) {
            continue;
        }
        let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
        let t = std::time::Instant::now();
        let answer = say(&state, &ep, &cipher, &chat.id, q).await;
        let secs = t.elapsed().as_secs_f32();
        let msgs = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id);
        let calls: Vec<String> = msgs.iter().flat_map(|m| m.tool_calls.clone().unwrap_or_default()).map(|c| format!("{}({})", c.name, c.arguments)).collect();
        println!("\n=== {q}\n{secs:.1} s, {} tool calls: {calls:?}\n{answer}", calls.len());
        let used = state.contexts.lock().unwrap().get(&chat.id).map(|c| format!("system {} + tools {} tokens of {}", c.system_tokens + c.profile_tokens, c.tools_tokens, c.ctx)).unwrap_or_default();
        println!("context: {used}");
        report.push((q, secs, calls.len(), answer));
    }
    approver.abort();
    println!("approval cards: {:?}", asked.lock().unwrap());
    if stop_after {
        state.engine.lock().await.stop().await;
    }
    for (q, secs, calls, _) in &report {
        println!("{q}: {secs:.1} s, {calls} calls");
    }
    let low = |i: usize| report[i].3.to_lowercase();
    let mut failed = Vec::new();
    if !(low(0).contains("sulcusai") || low(0).contains("security")) { failed.push("summarize the Google email"); }
    if !(low(1).contains("6") || low(1).contains("six")) { failed.push("find the party time"); }
    if !(low(2).contains("amazon") || low(2).contains("security") || low(2).contains("times")) { failed.push("list unread email"); }
    if !(low(3).contains("sunday") || low(3).contains("dinner") || low(3).contains("lasagna")) { failed.push("read mom's email"); }
    if !low(4).contains("isaac@example.org") { failed.push("say which account is connected"); }
    if !asked.lock().unwrap().iter().any(|t| t.contains("Microsoft")) && !low(5).contains("sign") { failed.push("offer the Outlook sign-in"); }
    if report.iter().any(|r| r.2 > 5) { failed.push("stay under 6 tool calls"); }
    assert!(failed.is_empty(), "didn't: {failed:?}");
}

/// The real model makes a spreadsheet with a formula, a Word document and a
/// deck in a shared folder, then reads one back. Approvals are answered Allow.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_documents() {
    let (state, ep, cipher) = agent_harness().await;
    let work = std::env::temp_dir().join(format!("sulcusai-docs-e2e-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&work).unwrap();
    let root = dunce::canonicalize(&work).unwrap();
    crate::db::add_folder(&state.db.lock().unwrap(), &root.display().to_string()).unwrap();
    let st = state.clone();
    let approver = tokio::spawn(async move {
        loop {
            for p in st.approvals.waiting() {
                println!("approval: {}", p.preview.title);
                st.approvals.answer(&p.call_id, crate::agent::Decision::Allow);
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    });
    let folder = root.file_name().unwrap().to_string_lossy().to_string();
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, Some("qwen3-1.7b".into())).unwrap();
    let r = say(&state, &ep, &cipher, &chat.id, &format!("Make a spreadsheet {folder}/budget.xlsx with columns Item and Cost: Rent 1200, Food 400, Travel 150, and a Total row that adds them up with a formula. /no_think")).await;
    println!("xlsx answer: {r}");
    for m in crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id).iter().filter(|m| m.role == "tool" || m.tool_calls.is_some()).take(6) {
        println!("  [{}] {}", m.role, if m.role == "tool" { m.content.chars().take(300).collect::<String>() } else { format!("{:?}", m.tool_calls).chars().take(600).collect() });
    }
    let r = say(&state, &ep, &cipher, &chat.id, &format!("Now write a short Word document {folder}/summary.docx with a heading and two bullet points about this budget. /no_think")).await;
    println!("docx answer: {r}");
    let r = say(&state, &ep, &cipher, &chat.id, &format!("And a 3-slide PowerPoint {folder}/budget.pptx: a title slide, a slide of costs, and a slide of next steps. /no_think")).await;
    println!("pptx answer: {r}");
    approver.abort();
    for f in ["budget.xlsx", "summary.docx", "budget.pptx"] {
        match crate::docs::read_text(&root.join(f)) {
            Ok(text) => println!("---- {f}\n{}", text.chars().take(700).collect::<String>()),
            Err(e) => println!("---- {f}: not made ({e})"),
        }
    }
    let sheet = crate::docs::read_text(&root.join("budget.xlsx")).unwrap();
    assert!(sheet.contains("| Rent | 1200 |") && sheet.contains("SUM(B"), "{sheet}");
    state.engine.lock().await.stop().await;
}

/// The SulcusAI extension in a headless Edge (no window), the connector
/// (target/debug/sulcusai.exe) registered as its native host, and the
/// app's pipe: open a local page, read it, the password rule, type and
/// submit a search, click a link. Cleans up its registration afterwards.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_browser_extension() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let state = speech_state();
    let exe = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("sulcusai.exe");
    assert!(exe.exists(), "build the app first (cargo build)");
    let ext = crate::bridge::install_with(&state, &exe).unwrap();
    crate::bridge::serve_pipe(state.clone(), None);

    // A small site on this PC.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { break };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let body = if path.starts_with("/search") {
                    format!("<title>Results</title><h1>Results for {}</h1>", path.split("q=").nth(1).unwrap_or(""))
                } else if path.starts_with("/about") {
                    "<title>About</title><h1>About this test site</h1>".to_string()
                } else {
                    "<title>Test home</title><h1>Welcome</h1><form action=\"/search\" method=\"get\" role=\"search\"><input name=\"q\" type=\"search\" placeholder=\"Search\"><button>Search</button></form><a href=\"/about\">About us</a><form method=\"post\" action=\"/login\"><input type=\"password\" name=\"pw\" placeholder=\"Password\"><button type=\"submit\">Sign in</button></form>".to_string()
                };
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });

    let profile = std::env::temp_dir().join(format!("sulcusai-edge-{}", uuid::Uuid::new_v4()));
    let mut edge = std::process::Command::new(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe")
        .args(["--headless=new", "--no-first-run", "--disable-features=DisableLoadExtensionCommandLineSwitch"])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--load-extension={}", ext.display()))
        .arg("about:blank")
        .spawn()
        .unwrap();
    let mut connected = false;
    for _ in 0..60 {
        if state.bridge.connected() {
            connected = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let result = async {
        if !connected {
            return Err("the extension never connected".to_string());
        }
        println!("extension connected");
        let pong = state.bridge.call(serde_json::json!({ "op": "ping" })).await;
        println!("ping: {pong:?}");
        let base = format!("http://127.0.0.1:{port}/");
        state.bridge.call(serde_json::json!({ "op": "open", "url": base })).await?;
        let page: crate::browser::Page = serde_json::from_value(state.bridge.call(serde_json::json!({ "op": "read" })).await?).map_err(|e| e.to_string())?;
        println!("{}", page.render(400));
        let find = |label: &str| page.elements.iter().find(|e| e["label"].as_str().unwrap_or("").contains(label) || e["kind"].as_str().unwrap_or("").contains(label)).and_then(|e| e["ref"].as_u64()).map(|r| r as u32);
        let pw = find("input:password").ok_or("no password field")?;
        let t: crate::browser::Target = serde_json::from_value(state.bridge.call(serde_json::json!({ "op": "target", "ref": pw })).await?).map_err(|e| e.to_string())?;
        assert!(crate::browser::private_field(&t), "password fields are for the user");
        let sign_in = find("Sign in").ok_or("no sign-in button")?;
        let t: crate::browser::Target = serde_json::from_value(state.bridge.call(serde_json::json!({ "op": "target", "ref": sign_in })).await?).map_err(|e| e.to_string())?;
        assert!(crate::browser::needs_confirmation(&t, false).is_some(), "submitting the sign-in form asks first");
        let search = find("Search").ok_or("no search box")?;
        state.bridge.call(serde_json::json!({ "op": "type", "ref": search, "text": "otters", "submit": true })).await?;
        let page: crate::browser::Page = serde_json::from_value(state.bridge.call(serde_json::json!({ "op": "read" })).await?).map_err(|e| e.to_string())?;
        println!("after search: {} {}", page.url, page.title);
        assert!(page.text.contains("Results for otters"), "{}", page.text);
        state.bridge.call(serde_json::json!({ "op": "back" })).await?;
        let page: crate::browser::Page = serde_json::from_value(state.bridge.call(serde_json::json!({ "op": "read" })).await?).map_err(|e| e.to_string())?;
        let about = page.elements.iter().find(|e| e["label"] == "About us").and_then(|e| e["ref"].as_u64()).ok_or("no About link")?;
        state.bridge.call(serde_json::json!({ "op": "click", "ref": about })).await?;
        let page: crate::browser::Page = serde_json::from_value(state.bridge.call(serde_json::json!({ "op": "read" })).await?).map_err(|e| e.to_string())?;
        println!("after click: {} {}", page.url, page.title);
        assert!(page.text.contains("About this test site"));
        Ok(())
    }
    .await;
    let _ = edge.kill();
    crate::bridge::uninstall();
    let _ = std::fs::remove_dir_all(&profile);
    result.unwrap();
}

/// The question from his chat: the weather in Orem, with exact figures,
/// asked of the small model. Prints which tools it used and what it said.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_weather_question() {
    let (state, ep, cipher) = agent_harness().await;
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, Some("qwen3-1.7b".into())).unwrap();
    crate::db::set_chat_web(&state.db.lock().unwrap(), &chat.id, true).unwrap();
    let (client, _, _) = crate::web::prepare(&state.db.lock().unwrap(), &cipher, &chat.id).unwrap();
    let report = crate::web::weather(&client, "Orem, Utah", 2).await.unwrap();
    println!("tool report:\n{report}\n");
    assert!(report.contains("Orem, Utah") && report.contains("°F"));
    for q in ["What is the weather today?", "What is the weather in Orem Utah today", "Give me the exact temperature right now", "Search the web: how tall is Mount Timpanogos in feet?"] {
        let reply = say(&state, &ep, &cipher, &chat.id, &format!("{q} /no_think")).await;
        println!("Q: {q}\nA: {reply}\n");
    }
    let used: Vec<String> = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id)
        .iter()
        .filter_map(|m| m.tool_calls.as_ref())
        .flat_map(|c| c.iter().map(|c| c.name.clone()))
        .collect();
    println!("tools used: {used:?}");
    assert!(used.iter().any(|t| t == "weather"));
    for m in crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id).iter().rev().take(4).rev() {
        println!("[{}] content={:?} thinking={:?} calls={:?}", m.role, m.content.chars().take(200).collect::<String>(), m.thinking.as_deref().map(|t| t.chars().take(200).collect::<String>()), m.tool_calls.as_ref().map(|c| c.iter().map(|c| c.name.clone()).collect::<Vec<_>>()));
    }
    state.engine.lock().await.stop().await;
}

/// Pictures look real by default, and a picture can be edited from an
/// instruction: a portrait, then a top hat added, then a party behind him.
/// SULCUSAI_MEDIA_OUT: where the three pictures are written to look at;
/// SULCUSAI_EDIT_MODEL: the model (FLUX.2 klein 4B unless set).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_realistic_picture_and_edits() {
    use crate::media::{self, store, Request};
    let state = media_state();
    let out = std::path::PathBuf::from(std::env::var("SULCUSAI_MEDIA_OUT").expect("SULCUSAI_MEDIA_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let c = state.cipher().unwrap();
    let model = std::env::var("SULCUSAI_EDIT_MODEL").unwrap_or_else(|_| "flux2-klein-4b".into());
    media::install_for_test(&state, &model).await.unwrap();
    let mut source: Option<String> = None;
    let steps = [
        ("1-portrait", "generate", "a portrait of a smiling man in his twenties with short brown hair and a denim jacket, standing outdoors in a park"),
        ("2-top-hat", "edit", "add a tall black top hat on his head; keep his face, clothes and the background the same"),
        ("3-party", "edit", "put him at a lively house party at night with friends laughing and string lights behind him; keep his face and the top hat, and keep it a realistic photo"),
    ];
    for (name, op, prompt) in steps {
        let started = std::time::Instant::now();
        let req = Request { op: op.into(), prompt: prompt.into(), source: source.clone(), shape: Some("portrait".into()), model_id: Some(model.clone()), ..Default::default() };
        let made = media::run(&state, store::new_id(), req).await.unwrap_or_else(|e| panic!("{name}: {e}"));
        let item = &made[0];
        std::fs::write(out.join(format!("{name}.png")), store::read(&state.paths, &c, &item.id).unwrap()).unwrap();
        println!("{name}: {:.1}s, {}x{}, {:?}", started.elapsed().as_secs_f64(), item.width, item.height, item.stats);
        source = Some(item.id.clone());
    }
}

/// An edit of a real photo, planned and checked by the picture reader (the
/// barbell photo: asked to have the child lift the barbell overhead, the
/// edit model drew a second barbell and left the first on the floor).
/// SULCUSAI_EDIT_SOURCE: the photo; SULCUSAI_EDIT_PROMPT: the request;
/// SULCUSAI_MEDIA_OUT: where the result goes; SULCUSAI_EDIT_MODEL: the edit
/// model (FLUX.2 klein 4B unless set); SULCUSAI_READER: the installed chat
/// model that reads pictures (Qwen3.5 9B unless set); SULCUSAI_PLAN_ONLY:
/// print the plan and stop (the edit model isn't needed for that).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_edit_a_photo() {
    use crate::media::{self, prompts, store, Request};
    let state = media_state();
    // What the job says about its checks, printed at the end.
    crate::applog::init(&state.paths.logs);
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("set {k}"));
    let out = std::path::PathBuf::from(var("SULCUSAI_MEDIA_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let c = state.cipher().unwrap();
    let model = std::env::var("SULCUSAI_EDIT_MODEL").unwrap_or_else(|_| "flux2-klein-4b".into());
    let reader = std::env::var("SULCUSAI_READER").unwrap_or_else(|_| "qwen3.5-9b".into());
    let file = std::fs::read_dir(state.paths.models.join(&reader))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.ends_with(".gguf") && !n.starts_with("mmproj"))
        .expect("the reader's model file");
    let size = std::fs::metadata(state.paths.models.join(&reader).join(&file)).unwrap().len();
    crate::db::save_installed(&state.db.lock().unwrap(), &crate::db::InstalledModel { model_id: reader.clone(), quant: "Q4_K_M".into(), path: file, size, installed_at: 0, tps: None, prompt_tps: None }).unwrap();
    assert!(crate::vision::can_read(&state, None), "{reader} reads pictures");
    let photo = var("SULCUSAI_EDIT_SOURCE");
    let request = var("SULCUSAI_EDIT_PROMPT");
    let img = media::imaging::decode(&std::fs::read(&photo).unwrap()).unwrap();
    let m = media::models(&state).into_iter().find(|x| x.id == model).expect("a known edit model");

    // The plan, as the job makes it.
    let started = std::time::Instant::now();
    let (w, h) = media::imaging::work_size(img.width(), img.height(), 1024 * 1024);
    let before = img.resize_exact(w, h, image::imageops::FilterType::Lanczos3);
    let r = prompts::reader(&state, Some(&m)).await.unwrap();
    let plan = prompts::plan_edit(&r, &before, &request).await;
    r.done(&state).await;
    let plan = plan.unwrap();
    println!("planned in {:.1}s", started.elapsed().as_secs_f64());
    for (i, s) in plan.steps.iter().enumerate() {
        println!("step {}: {s}", i + 1);
    }
    for x in &plan.checklist {
        println!("check: {x}");
    }
    if std::env::var("SULCUSAI_PLAN_ONLY").is_ok() {
        return;
    }

    media::install_for_test(&state, &model).await.unwrap();
    let bytes = std::fs::read(&photo).unwrap();
    let mime = if photo.ends_with(".webp") { "image/webp" } else if photo.ends_with(".png") { "image/png" } else { "image/jpeg" };
    let source = media::add(&state, &c, bytes, "image", mime, "import", "photo", None, None, false).unwrap();
    let started = std::time::Instant::now();
    let req = Request { op: "edit".into(), prompt: request, source: Some(source.id.clone()), model_id: Some(model.clone()), ..Default::default() };
    let made = media::run(&state, store::new_id(), req).await.unwrap();
    let item = &made[0];
    std::fs::write(out.join("edited.png"), store::read(&state.paths, &c, &item.id).unwrap()).unwrap();
    println!("edited in {:.1}s: {}x{} (the photo is {}x{})", started.elapsed().as_secs_f64(), item.width, item.height, img.width(), img.height());
    let log = std::fs::read_to_string(state.paths.logs.join(crate::applog::APP_LOG)).unwrap_or_default();
    for l in log.lines().filter(|l| l.contains("[media]")) {
        println!("{l}");
    }
    assert_eq!((item.width, item.height), (img.width(), img.height()), "the photo's own size");
}

/// The prompt coach, run as the Studio runs it (`op: "coach"`): a picture
/// model's own language model writes prompts and edit plans, the picture
/// model makes them, the picture reader scores them, and the writer is
/// trained on its best tries (kept only when it then scores higher). About
/// 30 minutes for FLUX.2 klein 4B on an RTX 5070. Prints the stages as they
/// go and the job's log lines; stops the job if the card reaches 83 °C.
/// SULCUSAI_COACH_MODEL: the picture model (FLUX.2 klein 4B unless set);
/// SULCUSAI_READER: the installed chat model that reads pictures (Qwen3.5 9B
/// unless set).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_coach_the_prompt_writer() {
    use crate::media::{self, coach, store, Request};
    let state = media_state();
    crate::applog::init(&state.paths.logs);
    let model = std::env::var("SULCUSAI_COACH_MODEL").unwrap_or_else(|_| "flux2-klein-4b".into());
    let reader = add_reader(&state);
    media::install_for_test(&state, &model).await.unwrap();
    let m = media::models(&state).into_iter().find(|x| x.id == model).expect("a known picture model");
    let writer = media::installed_file(&state, &m, "llm").expect("the picture model's own language model");
    println!("coaching {} ({}), scored by {reader}", m.name, writer.display());

    let id = store::new_id();
    let started = std::time::Instant::now();
    let watch = {
        let id = id.clone();
        tokio::spawn(async move {
            let mut last = String::new();
            let mut hottest = 0f32;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                let Some(j) = media::list_jobs().into_iter().find(|j| j.id == id) else { continue };
                let temp = j.gpu_temp.unwrap_or(0.0);
                let now = format!("{} ({:.0}%)", j.stage, j.fraction * 100.0);
                if now != last || temp > hottest + 2.0 {
                    hottest = hottest.max(temp);
                    println!("[{:5.1} min] {now} · card {temp:.0} °C, {:.0}% busy", started.elapsed().as_secs_f64() / 60.0, j.gpu_usage.unwrap_or(0.0));
                    last = now;
                }
                if temp >= 83.0 {
                    println!("the card reached {temp:.0} °C: stopping");
                    media::cancel(&id);
                }
            }
        })
    };
    let req = Request { op: "coach".into(), prompt: String::new(), model_id: Some(model.clone()), ..Default::default() };
    let result = media::run(&state, id, req).await;
    watch.abort();
    println!("finished in {:.1} min", started.elapsed().as_secs_f64() / 60.0);
    let log = std::fs::read_to_string(state.paths.logs.join(crate::applog::APP_LOG)).unwrap_or_default();
    for l in log.lines().filter(|l| l.contains("[media]") || l.contains("[model]")) {
        println!("{l}");
    }
    match coach::trained(&state, &m) {
        Some(c) => println!("kept: {c:?}"),
        None => println!("no coach kept"),
    }
    result.unwrap();
}

/// Registers the installed chat model that reads pictures in a test's own
/// list: SULCUSAI_READER, or Qwen3.5 9B.
fn add_reader(state: &crate::AppState) -> String {
    let reader = std::env::var("SULCUSAI_READER").unwrap_or_else(|_| "qwen3.5-9b".into());
    let file = std::fs::read_dir(state.paths.models.join(&reader))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.ends_with(".gguf") && !n.starts_with("mmproj"))
        .expect("the reader's model file");
    let size = std::fs::metadata(state.paths.models.join(&reader).join(&file)).unwrap().len();
    crate::db::save_installed(&state.db.lock().unwrap(), &crate::db::InstalledModel { model_id: reader.clone(), quant: "Q4_K_M".into(), path: file, size, installed_at: 0, tps: None, prompt_tps: None }).unwrap();
    assert!(crate::vision::can_read(state, None), "{reader} reads pictures");
    reader
}

/// The prompt writer learning as it's used (`media::learn`): checked new
/// pictures and edits, each written again from the reviewer's notes until
/// it shows what was asked (up to 5 makes), then the same kinds of request
/// again with what it learnt. Prints every job's tries, the rules and
/// examples it kept, and how first tries did in each half. Stops a job if
/// the card reaches 83 °C. SULCUSAI_LEARN_MODEL: the picture model (FLUX.2
/// klein 4B unless set); SULCUSAI_READER as in `add_reader`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_prompt_writer_learns() {
    use crate::media::{self, learn, store, Request};
    let state = media_state();
    crate::applog::init(&state.paths.logs);
    // Off unless turned on (since 0.9.20).
    learn::set_on(&state, true).unwrap();
    let model = std::env::var("SULCUSAI_LEARN_MODEL").unwrap_or_else(|_| "flux2-klein-4b".into());
    let reader = add_reader(&state);
    media::install_for_test(&state, &model).await.unwrap();
    let m = media::models(&state).into_iter().find(|x| x.id == model).expect("a known picture model");
    println!("{} learning, reviewed by {reader}", m.name);
    // SULCUSAI_MEDIA_OUT: where every kept picture goes, to check the reviewer by eye.
    let out = std::env::var("SULCUSAI_MEDIA_OUT").ok().map(std::path::PathBuf::from);
    let c = state.cipher().unwrap();
    let keep = |name: &str, id: &str| {
        if let Some(dir) = &out {
            let _ = std::fs::create_dir_all(dir);
            if let Ok(png) = store::read(&state.paths, &c, id) {
                let _ = std::fs::write(dir.join(format!("{name}.png")), png);
            }
        }
    };
    let started = std::time::Instant::now();
    let hot = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watch = {
        let hot = hot.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                for j in media::list_jobs() {
                    if j.gpu_temp.is_some_and(|t| t >= 83.0) {
                        println!("the card reached {:.0} °C: stopping", j.gpu_temp.unwrap_or(0.0));
                        hot.store(true, std::sync::atomic::Ordering::Relaxed);
                        media::cancel(&j.id);
                    }
                }
            }
        })
    };
    let log_path = state.paths.logs.join(crate::applog::APP_LOG);
    let mut seen = 0usize;
    // The job's own lines in the log since the last one.
    let mut news = || {
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        let lines: Vec<String> = log.lines().skip(seen).map(str::to_string).collect();
        seen += lines.len();
        lines.into_iter().filter(|l| l.contains("[media]") && (l.contains("Picture ") || l.contains("Edit ") || l.contains("Kept picture") || l.contains("learnt") || l.contains("Made again") || l.contains("reviewer"))).collect::<Vec<_>>()
    };
    let first_score = |lines: &[String], word: &str| -> Option<f32> {
        let l = lines.iter().find(|l| l.contains(word))?;
        let pct = l.split(": it ").nth(1)?.split_whitespace().find(|w| w.ends_with('%'))?;
        pct.trim_end_matches('%').parse::<f32>().ok()
    };
    let pictures = [
        ("a bakery storefront with a striped awning and a sign that says \"SUNRISE BAKERY\"", "a coffee shop storefront with a green awning and a sign that says \"MORNING BREW\""),
        ("three red apples and one green pear in a wooden bowl on a white kitchen table", "two yellow lemons and three green limes on a white plate on a wooden table"),
        ("an astronaut riding a horse across a desert at sunset, two moons in the sky", "a knight riding a camel across sand dunes at noon, three suns in the sky"),
        ("a girl in a yellow raincoat holding a blue umbrella, a red bicycle on the left", "a boy in a green jacket holding an orange kite, a black dog on the right"),
        ("a birthday cake with five lit candles and the words \"HAPPY 5TH\" in blue icing", "a wedding cake with two lit candles and the words \"JUST MARRIED\" in gold icing"),
        ("a chessboard with a white queen and a black knight in front, shallow depth of field", "a checkers board with two red pieces and one black piece in front, shallow depth of field"),
    ];
    let edits = [
        (("a photo of a park bench where an old man reads a newspaper, a red bicycle leaning on a tree behind the bench", "remove the bicycle"), ("a photo of a quiet street with a blue car parked at the curb and a green trash can on the sidewalk", "remove the trash can")),
        (("a photo of a boy in a green t-shirt standing on grass next to a black and white soccer ball", "the boy holds the soccer ball above his head with both hands"), ("a photo of a girl in a red dress standing in a garden next to a yellow beach ball", "the girl holds the beach ball above her head with both hands")),
    ];
    // Three passes: learning; alike requests with what it learnt; the same
    // alike requests (same seeds, same pictures to edit) with nothing learnt
    // and learning off, to compare first tries pair by pair.
    let mut firsts: [Vec<Option<f32>>; 3] = Default::default();
    let mut edit_firsts: [Vec<Option<f32>>; 3] = Default::default();
    let mut sources: Vec<String> = Vec::new();
    let mut learnt = None;
    for half in 0..3 {
        if half == 2 {
            let l = learn::load(&state, &m);
            learn::forget(&state, &m);
            learn::set_on(&state, false).unwrap();
            learnt = Some(l);
        }
        println!("\n=== {} ===", ["first pass: learning", "second pass: alike requests, with what it learnt", "third pass: the same alike requests, nothing learnt (learning off)"][half]);
        for (i, pair) in pictures.iter().enumerate() {
            if hot.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let prompt = if half == 0 { pair.0 } else { pair.1 };
            let t0 = std::time::Instant::now();
            let req = Request { op: "generate".into(), prompt: prompt.into(), model_id: Some(model.clone()), shape: Some("square".into()), seed: Some(40 + i as i64), write_prompt: Some(true), check: Some(true), ..Default::default() };
            let made = media::run(&state, store::new_id(), req).await;
            if let Some(item) = made.as_ref().ok().and_then(|v| v.first()) {
                keep(&format!("pass{}-picture{}", half + 1, i + 1), &item.id);
            }
            let lines = news();
            println!("\n[{:5.1} min] {prompt} ({:.0} s){}", started.elapsed().as_secs_f64() / 60.0, t0.elapsed().as_secs_f64(), made.as_ref().err().map(|e| format!(" FAILED: {e}")).unwrap_or_default());
            for l in &lines {
                println!("  {}", l.split("[media] ").nth(1).unwrap_or(l));
            }
            firsts[half].push(first_score(&lines, "Picture 1 of"));
        }
        for (i, pair) in edits.iter().enumerate() {
            if hot.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let (scene, change) = if half == 0 { pair.0 } else { pair.1 };
            // The third pass edits the second pass's pictures.
            let picture = match sources.get(i).filter(|_| half == 2) {
                Some(id) => id.clone(),
                None => {
                    let source = Request { op: "generate".into(), prompt: scene.into(), model_id: Some(model.clone()), shape: Some("landscape".into()), seed: Some(70 + i as i64), ..Default::default() };
                    let Ok(made) = media::run(&state, store::new_id(), source).await else { continue };
                    news();
                    if half == 1 {
                        sources.push(made[0].id.clone());
                    }
                    made[0].id.clone()
                }
            };
            let t0 = std::time::Instant::now();
            let req = Request { op: "edit".into(), prompt: change.into(), source: Some(picture), model_id: Some(model.clone()), seed: Some(90 + i as i64), ..Default::default() };
            let done = media::run(&state, store::new_id(), req).await;
            if let Some(item) = done.as_ref().ok().and_then(|v| v.first()) {
                keep(&format!("pass{}-edit{}", half + 1, i + 1), &item.id);
                if let Some(parent) = &item.parent {
                    keep(&format!("pass{}-edit{}-before", half + 1, i + 1), parent);
                }
            }
            let lines = news();
            println!("\n[{:5.1} min] edit: {change} ({:.0} s){}", started.elapsed().as_secs_f64() / 60.0, t0.elapsed().as_secs_f64(), done.as_ref().err().map(|e| format!(" FAILED: {e}")).unwrap_or_default());
            for l in &lines {
                println!("  {}", l.split("[media] ").nth(1).unwrap_or(l));
            }
            edit_firsts[half].push(first_score(&lines, "Edit 1 of"));
        }
    }
    watch.abort();
    let l = learnt.unwrap_or_else(|| learn::load(&state, &m));
    println!("\n=== what it learnt ({:.1} min) ===", started.elapsed().as_secs_f64() / 60.0);
    println!("examples: {} pictures, {} edits", l.pictures.len(), l.edits.len());
    for r in l.rules.iter().chain(&l.edit_rules) {
        println!("rule (helped {}): {}", r.helped, r.text);
    }
    let mean = |v: &[Option<f32>]| {
        let got: Vec<f32> = v.iter().flatten().copied().collect();
        if got.is_empty() { f32::NAN } else { got.iter().sum::<f32>() / got.len() as f32 }
    };
    // With what it learnt against without, on the same request, seed and picture.
    let paired = |with: &[Option<f32>], without: &[Option<f32>]| -> (Vec<f32>, f32) {
        let d: Vec<f32> = with.iter().zip(without).filter_map(|(a, b)| Some(a.as_ref()? - b.as_ref()?)).collect();
        let m = if d.is_empty() { f32::NAN } else { d.iter().sum::<f32>() / d.len() as f32 };
        (d, m)
    };
    for (what, v) in [("new pictures", &firsts), ("edits", &edit_firsts)] {
        println!("first tries, {what}: learning {:?} (mean {:.0}%); alike, with it {:?} (mean {:.0}%); the same, without {:?} (mean {:.0}%)", v[0], mean(&v[0]), v[1], mean(&v[1]), v[2], mean(&v[2]));
        let (d, m) = paired(&v[1], &v[2]);
        println!("  with minus without, request by request: {d:?} points (mean {m:+.1})");
    }
    assert!(!hot.load(std::sync::atomic::Ordering::Relaxed), "stopped: the card got too hot");
    assert!(l.firsts.len() + l.edit_firsts.len() > 0, "it learnt from the checked jobs");
}

/// The app log's picture-job lines since `seen` (without their time and tag).
fn media_lines(state: &crate::AppState, seen: &mut usize) -> Vec<String> {
    let log = std::fs::read_to_string(state.paths.logs.join(crate::applog::APP_LOG)).unwrap_or_default();
    let lines: Vec<String> = log.lines().skip(*seen).map(str::to_string).collect();
    *seen += lines.len();
    lines.iter().filter_map(|l| l.split("[media] ").nth(1)).map(str::to_string).collect()
}

/// Each make's score in a checked job, in order, from lines like "Picture 2
/// of at most 5: it showed 75% of what was asked for".
fn make_scores(lines: &[String]) -> Vec<f32> {
    lines
        .iter()
        .filter(|l| (l.starts_with("Picture ") || l.starts_with("Edit ")) && l.contains(" of at most "))
        .filter_map(|l| l.split(": it ").nth(1)?.split_whitespace().find(|w| w.ends_with('%'))?.trim_end_matches('%').parse().ok())
        .collect()
}

/// One held-out job in the learning evaluation.
struct Tried {
    name: String,
    edit: bool,
    case: usize,
    seed: usize,
    with: bool,
    item: Option<String>,
    before: Option<String>,
    /// The system's own score for each make.
    scores: Vec<f32>,
    /// What was handed over, against a checklist written by hand.
    fixed: Option<f32>,
    missed: Vec<String>,
}

/// Paired differences: how many, their mean, its standard error, and how
/// many came out ahead and behind.
fn paired(d: &[f32]) -> (usize, f32, f32, usize, usize) {
    let n = d.len();
    if n == 0 {
        return (0, f32::NAN, f32::NAN, 0, 0);
    }
    let mean = d.iter().sum::<f32>() / n as f32;
    let var = if n > 1 { d.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / (n - 1) as f32 } else { 0.0 };
    (n, mean, (var / n as f32).sqrt(), d.iter().filter(|x| **x > 0.0).count(), d.iter().filter(|x| **x < 0.0).count())
}

/// Whether learning from use helps, on requests it hasn't seen. Training:
/// 10 hard pictures and 3 edits, learning on. Then learning is frozen and
/// each held-out request runs twice back to back, with what it learnt and
/// with nothing learnt (alternating which goes first), with the same seed
/// (two seeds a picture request) and the same picture to edit. Every job is
/// the product as it is: written, made, reviewed and written again, up to 5
/// makes. At the end one reader judges everything handed over against
/// checklists written by hand, so the yardstick doesn't depend on what the
/// system wrote. Prints each job and the paired differences, and writes the
/// pictures and `verdicts.txt` to SULCUSAI_MEDIA_OUT (or a temp folder). About
/// 80 minutes on an RTX 5070; a job is stopped if the card reaches 83 °C.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_prompt_learning_eval() {
    use crate::media::{self, learn, prompts, store, Request};
    use std::sync::atomic::Ordering::Relaxed;
    let state = media_state();
    crate::applog::init(&state.paths.logs);
    // Off unless turned on (since 0.9.20).
    learn::set_on(&state, true).unwrap();
    let model = std::env::var("SULCUSAI_LEARN_MODEL").unwrap_or_else(|_| "flux2-klein-4b".into());
    let reader = add_reader(&state);
    media::install_for_test(&state, &model).await.unwrap();
    let m = media::models(&state).into_iter().find(|x| x.id == model).expect("a known picture model");
    let out = std::env::var("SULCUSAI_MEDIA_OUT").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("sulcusai-learning-eval"));
    std::fs::create_dir_all(&out).unwrap();
    println!("{} learning evaluation, reviewed by {reader}; pictures in {}", m.name, out.display());
    let c = state.cipher().unwrap();
    let save = |name: &str, id: &str| {
        if let Ok(png) = store::read(&state.paths, &c, id) {
            let _ = std::fs::write(out.join(format!("{name}.png")), png);
        }
    };
    let hot = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watch = {
        let hot = hot.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                for j in media::list_jobs() {
                    if j.gpu_temp.is_some_and(|t| t >= 83.0) {
                        println!("the card reached {:.0} °C: stopping", j.gpu_temp.unwrap_or(0.0));
                        hot.store(true, Relaxed);
                        media::cancel(&j.id);
                    }
                }
            }
        })
    };
    let started = std::time::Instant::now();
    let min = || started.elapsed().as_secs_f64() / 60.0;
    let mut seen = 0usize;
    let picture = |prompt: &str, seed: i64| Request { op: "generate".into(), prompt: prompt.into(), model_id: Some(model.clone()), shape: Some("square".into()), seed: Some(seed), write_prompt: Some(true), check: Some(true), ..Default::default() };
    let scene = |prompt: &str, seed: i64| Request { op: "generate".into(), prompt: prompt.into(), model_id: Some(model.clone()), shape: Some("landscape".into()), seed: Some(seed), ..Default::default() };
    let edit = |change: &str, source: &str, seed: i64| Request { op: "edit".into(), prompt: change.into(), source: Some(source.into()), model_id: Some(model.clone()), seed: Some(seed), ..Default::default() };

    // Hard on purpose: counts, words, left and right, odd colors, a clock.
    const TRAIN: &[&str] = &[
        "a bookshelf with exactly four red books on the top shelf and two green books on the bottom shelf",
        "a street sign that says \"MAPLE AVE\" above a stop sign",
        "a woman in a red coat on the left and a man in a yellow raincoat on the right, both holding coffee cups",
        "a blue banana and a purple apple on a wooden cutting board",
        "a wall clock showing a quarter past three",
        "five balloons tied to a park bench: red, orange, yellow, green and blue",
        "a cat sitting inside a cardboard box that says \"FRAGILE\" in black letters",
        "a chef holding a frying pan in his left hand and a wooden spoon in his right hand",
        "a snowman wearing sunglasses and a striped scarf on a sunny beach",
        "a neon sign that reads \"OPEN 24 HOURS\" in a dark diner window",
    ];
    const TEST: &[(&str, &[&str])] = &[
        ("a kitchen shelf with exactly three blue mugs on the top shelf and two white bowls on the bottom shelf", &["a kitchen shelf", "three blue mugs on the top shelf", "two white bowls on the bottom shelf"]),
        ("a shop sign that says \"HARBOR BOOKS\" above a red door", &["a sign that says \"HARBOR BOOKS\"", "a red door", "the sign is above the door"]),
        ("a boy in a green hoodie on the left and a girl in a purple dress on the right, both holding ice cream cones", &["a boy in a green hoodie on the left", "a girl in a purple dress on the right", "the boy holds an ice cream cone", "the girl holds an ice cream cone"]),
        ("a pink lemon and an orange cucumber on a white plate", &["a pink lemon", "an orange cucumber", "a white plate"]),
        ("a wall clock showing half past seven", &["a wall clock", "the clock shows half past seven"]),
        ("four kites in the sky above a hill: red, yellow, green and blue", &["exactly four kites in the sky", "a red kite", "a yellow kite", "a green kite", "a blue kite", "a hill"]),
        ("a puppy sitting inside a wooden crate that says \"HANDLE WITH CARE\" in red letters", &["a puppy sitting inside a wooden crate", "the crate says \"HANDLE WITH CARE\"", "the letters are red"]),
        ("a painter holding a brush in her right hand and a palette in her left hand", &["a painter holding a brush in her right hand", "the painter holds a palette in her left hand"]),
        ("a penguin wearing a straw hat and a flower necklace in a desert", &["a penguin", "the penguin wears a straw hat", "the penguin wears a flower necklace", "a desert"]),
        ("a chalkboard sign that reads \"FRESH COFFEE\" outside a cafe", &["a chalkboard sign that reads \"FRESH COFFEE\"", "the sign stands outside a cafe"]),
    ];
    const TRAIN_EDITS: &[(&str, &str)] = &[
        ("a photo of a living room with a grey sofa, a red cushion on the sofa and a black cat on the floor", "remove the cat"),
        ("a photo of a girl standing in a park holding a red balloon on a string", "make the balloon blue and add a second, yellow balloon"),
        ("a photo of a man sitting at a desk with a laptop and a white coffee mug", "the man lifts the coffee mug to his mouth"),
    ];
    // Scene, change, checklist written by hand, and the thing that moves (one of it after).
    const TEST_EDITS: &[(&str, &str, &[&str], &str)] = &[
        ("a photo of a kitchen with a white table, a bowl of oranges on the table and a brown dog on the floor", "remove the dog", &["no dog in the picture", "the white table is still there", "the bowl of oranges is still on the table"], ""),
        ("a photo of a boy standing on a beach holding a green kite", "make the kite red and add a second, blue kite in the sky", &["the boy holds a red kite", "a blue kite in the sky", "no green kite"], ""),
        ("a photo of a woman sitting on a bench with a book and a water bottle beside her", "the woman lifts the water bottle to her mouth", &["the woman holds the water bottle at her mouth", "the book is still on the bench"], "bottle"),
    ];

    // 1. Training, learning on.
    println!("\n=== training: {} pictures and {} edits, learning on ===", TRAIN.len(), TRAIN_EDITS.len());
    for (i, prompt) in TRAIN.iter().enumerate() {
        if hot.load(Relaxed) {
            break;
        }
        let made = media::run(&state, store::new_id(), picture(prompt, 100 + i as i64)).await;
        let lines = media_lines(&state, &mut seen);
        println!("[{:5.1} min] train {}: {:?} {}", min(), i + 1, make_scores(&lines), made.as_ref().err().map(|e| format!("FAILED: {e}")).unwrap_or_default());
        for l in lines.iter().filter(|l| l.starts_with("Stopped") || l.contains("learnt")) {
            println!("    {l}");
        }
        if let Some(it) = made.ok().and_then(|v| v.into_iter().next()) {
            save(&format!("train-picture{:02}", i + 1), &it.id);
        }
    }
    // A picture model that doesn't edit is judged on new pictures only.
    let edits_too = m.can("edit");
    for (i, (place, change)) in TRAIN_EDITS.iter().enumerate() {
        if hot.load(Relaxed) || !edits_too {
            break;
        }
        let Ok(src) = media::run(&state, store::new_id(), scene(place, 200 + i as i64)).await else { continue };
        media_lines(&state, &mut seen);
        let made = media::run(&state, store::new_id(), edit(change, &src[0].id, 300 + i as i64)).await;
        let lines = media_lines(&state, &mut seen);
        println!("[{:5.1} min] train edit {}: {:?} {change}", min(), i + 1, make_scores(&lines));
        for l in lines.iter().filter(|l| l.starts_with("Stopped") || l.contains("learnt")) {
            println!("    {l}");
        }
        if let Some(it) = made.ok().and_then(|v| v.into_iter().next()) {
            save(&format!("train-edit{}", i + 1), &it.id);
        }
    }
    let snapshot = learn::load(&state, &m);
    learn::set_on(&state, false).unwrap();
    println!("\nlearnt: {} picture examples, {} edit examples", snapshot.pictures.len(), snapshot.edits.len());
    for r in snapshot.rules.iter().chain(&snapshot.edit_rules) {
        println!("rule (helped {}): {}", r.helped, r.text);
    }

    // 2. Held out, learning frozen: with and without, back to back.
    println!("\n=== held out: {} pictures x 2 seeds and {} edits, each with and without what it learnt ===", TEST.len(), TEST_EDITS.len());
    let use_learning = |on: bool| {
        if on {
            learn::save(&state, &m, &snapshot).unwrap();
        } else {
            learn::forget(&state, &m);
        }
    };
    let mut tried: Vec<Tried> = Vec::new();
    for (i, (prompt, _)) in TEST.iter().enumerate() {
        for k in 0..2 {
            let order = if (i + k) % 2 == 0 { [true, false] } else { [false, true] };
            for with in order {
                if hot.load(Relaxed) {
                    break;
                }
                use_learning(with);
                let made = media::run(&state, store::new_id(), picture(prompt, 500 + (2 * i + k) as i64)).await;
                let lines = media_lines(&state, &mut seen);
                let name = format!("test{:02}-seed{}-{}", i + 1, k + 1, if with { "with" } else { "without" });
                let item = made.ok().and_then(|v| v.into_iter().next()).map(|x| x.id);
                if let Some(id) = &item {
                    save(&name, id);
                }
                let scores = make_scores(&lines);
                println!("[{:5.1} min] {name}: {scores:?}", min());
                tried.push(Tried { name, edit: false, case: i, seed: k, with, item, before: None, scores, fixed: None, missed: Vec::new() });
            }
        }
    }
    for (i, (place, change, _, _)) in TEST_EDITS.iter().enumerate() {
        if hot.load(Relaxed) || !edits_too {
            break;
        }
        let Ok(src) = media::run(&state, store::new_id(), scene(place, 600 + i as i64)).await else { continue };
        media_lines(&state, &mut seen);
        let source = src[0].id.clone();
        save(&format!("test-edit{}-before", i + 1), &source);
        let order = if i % 2 == 0 { [true, false] } else { [false, true] };
        for with in order {
            if hot.load(Relaxed) {
                break;
            }
            use_learning(with);
            let made = media::run(&state, store::new_id(), edit(change, &source, 700 + i as i64)).await;
            let lines = media_lines(&state, &mut seen);
            let name = format!("test-edit{}-{}", i + 1, if with { "with" } else { "without" });
            let item = made.ok().and_then(|v| v.into_iter().next()).map(|x| x.id);
            if let Some(id) = &item {
                save(&name, id);
            }
            let scores = make_scores(&lines);
            println!("[{:5.1} min] {name}: {scores:?}", min());
            tried.push(Tried { name, edit: true, case: i, seed: 0, with, item, before: Some(source.clone()), scores, fixed: None, missed: Vec::new() });
        }
    }
    watch.abort();

    // 3. Everything handed over, judged against the checklists written by hand.
    println!("\n=== judging what was handed over against fixed checklists ({:.1} min) ===", min());
    let r = prompts::reader(&state, Some(&m)).await.unwrap();
    let decode = |id: &str| store::read(&state.paths, &c, id).and_then(|b| media::imaging::decode(&b));
    for t in &mut tried {
        let Some(after) = t.item.as_deref().and_then(|id| decode(id).ok()) else { continue };
        let v = if t.edit {
            let (_, _, list, moved) = TEST_EDITS[t.case];
            let Some(before) = t.before.as_deref().and_then(|id| decode(id).ok()) else { continue };
            let plan = prompts::EditPlan { checklist: list.iter().map(|s| s.to_string()).collect(), moved: if moved.is_empty() { Vec::new() } else { vec![moved.to_string()] }, ..Default::default() };
            prompts::judge_edit(&r, &before, &after, &plan).await
        } else {
            prompts::judge(&r, &after, &TEST[t.case].1.iter().map(|s| s.to_string()).collect::<Vec<_>>()).await
        };
        if let Ok(v) = v {
            t.fixed = Some(v.score * 100.0);
            t.missed = v.missing;
        }
    }
    r.done(&state).await;

    // 4. Results.
    let mut report = String::new();
    for t in &tried {
        let line = format!("{}: system's scores {:?}, by hand {}{}", t.name, t.scores, t.fixed.map_or("-".to_string(), |f| format!("{f:.0}%")), if t.missed.is_empty() { String::new() } else { format!(" (missing: {})", t.missed.join("; ")) });
        println!("{line}");
        report.push_str(&line);
        report.push('\n');
    }
    for edits in [false, true] {
        let pairs: Vec<(&Tried, &Tried)> = tried.iter().filter(|t| t.edit == edits && t.with).filter_map(|a| tried.iter().find(|b| b.edit == edits && !b.with && b.case == a.case && b.seed == a.seed).map(|b| (a, b))).collect();
        let diff = |f: &dyn Fn(&Tried) -> Option<f32>| -> Vec<f32> { pairs.iter().filter_map(|(a, b)| Some(f(a)? - f(b)?)).collect() };
        let what = if edits { "edits" } else { "new pictures" };
        for (label, d) in [
            ("first try (system's checklist)", diff(&|t: &Tried| t.scores.first().copied())),
            ("handed over (system's checklist)", diff(&|t: &Tried| t.scores.iter().copied().reduce(f32::max))),
            ("handed over (checklist by hand)", diff(&|t: &Tried| t.fixed)),
            ("makes needed (fewer is better)", diff(&|t: &Tried| (!t.scores.is_empty()).then_some(t.scores.len() as f32))),
        ] {
            let (n, mean, se, ahead, behind) = paired(&d);
            let line = format!("{what}, {label}: with minus without over {n} pairs: mean {mean:+.1} (standard error {se:.1}); with ahead {ahead}, behind {behind}, level {}", n - ahead - behind);
            println!("{line}");
            report.push_str(&line);
            report.push('\n');
        }
    }
    let _ = std::fs::write(out.join("verdicts.txt"), report);
    println!("\n{:.1} min in all", min());
    assert!(!hot.load(Relaxed), "stopped: the card got too hot");
}

/// A model from Hugging Face, installed the way the Studio installs one
/// (sharing the catalog model's text reader and decoder), makes a picture.
/// SULCUSAI_HUB_REPO / SULCUSAI_HUB_FILE pick it (a FLUX.2 klein 4B
/// quantization unless set). Its own file is deleted afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_hub_media_model() {
    use crate::media::{self, store, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    let repo = std::env::var("SULCUSAI_HUB_REPO").unwrap_or_else(|_| "unsloth/FLUX.2-klein-4B-GGUF".into());
    let file = std::env::var("SULCUSAI_HUB_FILE").unwrap_or_else(|_| "flux-2-klein-4b-Q5_K_M.gguf".into());
    let spec = media::hub::prepare(&state, &repo, &file).await.unwrap();
    println!("prepared {} ({}): {:?}", spec.name, spec.id, spec.files.iter().map(|f| (&f.role, &f.file)).collect::<Vec<_>>());
    media::install_for_test(&state, &spec.id).await.unwrap();
    assert!(media::installed(&state.db.lock().unwrap()).iter().any(|i| i.model_id == spec.id));
    let req = Request { op: "generate".into(), prompt: "a lighthouse on a rocky cliff at dusk, photograph".into(), model_id: Some(spec.id.clone()), shape: Some("landscape".into()), seed: Some(7), ..Default::default() };
    let made = media::run(&state, store::new_id(), req).await.unwrap();
    assert_eq!(made[0].model_id.as_deref(), Some(spec.id.as_str()), "made with the Hugging Face model");
    let png = store::read(&state.paths, &c, &made[0].id).unwrap();
    assert!(image::load_from_memory(&png).is_ok());
    if let Ok(out) = std::env::var("SULCUSAI_MEDIA_OUT") {
        std::fs::write(std::path::Path::new(&out).join("hub-model.png"), &png).unwrap();
    }
    println!("made: {:?}", made[0].stats);
    // Only its own file goes; the shared ones stay for the catalog model.
    let own: Vec<std::path::PathBuf> = spec.files.iter().filter(|f| f.file.starts_with("hfm-")).map(|f| media::model_dir(&state.paths, &spec).join(&f.file)).collect();
    assert_eq!(own.len(), 1);
    for p in own {
        std::fs::remove_file(&p).unwrap();
    }
}

/// Renders a set of pictures to compare looks and models. SULCUSAI_PICS:
/// "name|model|style|steps|prompt" entries separated by ";;" (empty model,
/// style or steps: the defaults); SULCUSAI_MEDIA_OUT: where they go.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_picture_set() {
    use crate::media::{self, store, Request};
    let state = media_state();
    let out = std::path::PathBuf::from(std::env::var("SULCUSAI_MEDIA_OUT").expect("SULCUSAI_MEDIA_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let c = state.cipher().unwrap();
    let list = std::env::var("SULCUSAI_PICS").expect("SULCUSAI_PICS");
    for entry in list.split(";;").map(str::trim).filter(|e| !e.is_empty()) {
        let f: Vec<&str> = entry.splitn(5, '|').collect();
        let [name, model, style, steps, prompt] = f[..] else { panic!("bad entry: {entry}") };
        if !model.is_empty() {
            media::install_for_test(&state, model).await.unwrap_or_else(|e| panic!("installing {model}: {e}"));
        }
        let req = Request {
            op: "generate".into(),
            prompt: prompt.into(),
            model_id: (!model.is_empty()).then(|| model.to_string()),
            style: (!style.is_empty()).then(|| style.to_string()),
            steps: steps.parse().ok(),
            shape: Some("landscape".into()),
            seed: Some(42),
            ..Default::default()
        };
        let started = std::time::Instant::now();
        match media::run(&state, store::new_id(), req).await {
            Ok(made) => {
                std::fs::write(out.join(format!("{name}.png")), store::read(&state.paths, &c, &made[0].id).unwrap()).unwrap();
                println!("{name}: {:.1}s {:?}", started.elapsed().as_secs_f64(), made[0].stats);
            }
            Err(e) => println!("{name}: FAILED {e}"),
        }
    }
}

/// "See how it's built" on an NVIDIA card: a song's parts taken apart and
/// their notes read through CUDA (its libraries downloaded the first time),
/// timed. SULCUSAI_SONG is the MP3 and SULCUSAI_SONG_SECONDS its length;
/// SULCUSAI_ONNX_CPU=1 keeps it on the processor, to compare (run that
/// alone: a process loads one ONNX Runtime).
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_song_parts_on_the_card() {
    use crate::media::{self, store, MediaItem};
    let state = media_state();
    crate::applog::init(&state.paths.logs);
    let c = state.work_cipher().unwrap();
    let song = std::fs::read(std::env::var("SULCUSAI_SONG").expect("SULCUSAI_SONG")).unwrap();
    let seconds = std::env::var("SULCUSAI_SONG_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(60.0);
    let item = MediaItem { kind: "audio".into(), mime: "audio/mpeg".into(), op: "import".into(), prompt: "a song".into(), seconds, ..Default::default() };
    let item = store::save(&state.db.lock().unwrap(), &state.paths, &c, item, &song, None).unwrap();
    let on_processor = std::env::var_os("SULCUSAI_ONNX_CPU").is_some();
    for round in 1..=2 {
        let t = std::time::Instant::now();
        let mapped = media::run(&state, store::new_id(), media::Request { op: "map".into(), source: Some(item.id.clone()), ..Default::default() }).await.unwrap().remove(0);
        let map = mapped.map.unwrap();
        println!(
            "round {round}: mapped in {:.1}s ({}); {}",
            t.elapsed().as_secs_f64(),
            if crate::natural::loaded_for_cards() { "the card's runtime" } else { "the processor's runtime" },
            map.parts.iter().map(|p| format!("{} {}", p.name, p.notes.len())).collect::<Vec<_>>().join(", ")
        );
        assert!(map.parts.iter().map(|p| p.notes.len()).sum::<usize>() > 100);
    }
    let log = std::fs::read_to_string(state.paths.logs.join("app.log")).unwrap_or_default();
    let lines: Vec<&str> = log.lines().filter(|l| l.contains("song's parts") || l.contains("song apart") || l.contains("graphics card")).collect();
    for line in &lines {
        println!("{line}");
    }
    let want = if on_processor { "on the processor" } else { "on the graphics card" };
    let ran: Vec<&&str> = lines.iter().filter(|l| l.contains("Took a song's parts apart")).collect();
    assert!(ran.len() == 2 && ran.iter().all(|l| l.ends_with(want)), "{want}");
}

/// App state with the real engines and models but a throwaway database and
/// gallery (so test pictures never land in the real app's gallery).
fn media_state() -> std::sync::Arc<crate::AppState> {
    temp_state(None)
}

/// App state with its own throwaway data folder and models folder, for
/// tests that write models (never the real models folder: a sync test
/// once overwrote an installed model there with an empty file).
pub(crate) fn scratch_state() -> std::sync::Arc<crate::AppState> {
    let models = std::env::temp_dir().join(format!("sulcusai-models-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&models).unwrap();
    temp_state(Some(models))
}

/// App state over the real engines and (unless `models` is given) the real
/// models folder, with a throwaway database: for tests that only read models.
pub(crate) fn temp_state(models: Option<std::path::PathBuf>) -> std::sync::Arc<crate::AppState> {
    temp_state_in(models, None)
}

/// `temp_state` with its own engines folder too (`engines`), for tests that
/// download engines (never into the installed app's).
pub(crate) fn temp_state_in(models: Option<std::path::PathBuf>, engines: Option<std::path::PathBuf>) -> std::sync::Arc<crate::AppState> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, RwLock};
    let real = Paths::new(app_data_dir()).unwrap();
    let tmp = std::env::temp_dir().join(format!("sulcusai-media-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(tmp.join("logs")).unwrap();
    let models = models.unwrap_or_else(|| real.models.clone());
    let engines = engines.unwrap_or_else(|| real.engines.clone());
    let paths = Paths { data: tmp.clone(), models, engines, logs: tmp.join("logs"), db: tmp.join("t.db") };
    let conn = crate::db::open(&paths.db).unwrap();
    crate::features::enable_all(&conn);
    let vault = crate::crypto::Vault::open(&tmp.join("keys.json"), Box::new(crate::crypto::dpapi::Dpapi)).unwrap();
    Arc::new(crate::AppState {
        hardware: RwLock::new(hardware::detect(&real.models)),
        paths,
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

/// Pictures, edits, upscaling, cut-outs, music, narration and video with the
/// real engines, installed the way the app installs them. SULCUSAI_MEDIA
/// picks the parts (default: everything); results are written to
/// SULCUSAI_MEDIA_OUT for a look.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_media() {
    use crate::media::{self, store, Request};
    use base64::Engine as _;
    let state = media_state();
    let parts = std::env::var("SULCUSAI_MEDIA").unwrap_or_else(|_| "picture,upscale,cutout,music,narrate,video".into());
    let want = |p: &str| parts.split(',').any(|x| x == p);
    let out = std::path::PathBuf::from(std::env::var("SULCUSAI_MEDIA_OUT").unwrap_or_else(|_| std::env::temp_dir().join("sulcusai-media-out").display().to_string()));
    std::fs::create_dir_all(&out).unwrap();
    let c = state.cipher().unwrap();
    let keep = |item: &media::MediaItem, name: &str| {
        let bytes = store::read(&state.paths, &c, &item.id).unwrap();
        let ext = item.mime.split('/').nth(1).unwrap_or("bin").replace("mpeg", "mp3");
        std::fs::write(out.join(format!("{name}.{ext}")), &bytes).unwrap();
        bytes
    };
    let go = |req: Request| {
        let state = state.clone();
        async move {
            let started = std::time::Instant::now();
            let op = req.op.clone();
            let r = media::run(&state, store::new_id(), req).await;
            println!("{op}: {:.1}s -> {}", started.elapsed().as_secs_f64(), match &r {
                Ok(i) => format!("{} item(s)", i.len()),
                Err(e) => e.clone(),
            });
            r
        }
    };
    let install = |id: &'static str| {
        let state = state.clone();
        async move {
            let started = std::time::Instant::now();
            media::install_for_test(&state, id).await.unwrap_or_else(|e| panic!("installing {id}: {e}"));
            println!("installed {id} in {:.1}s", started.elapsed().as_secs_f64());
        }
    };

    if want("picture") || want("upscale") || want("cutout") || want("video") {
        install("flux2-klein-4b").await;
        let made = go(Request { op: "generate".into(), prompt: "a red fox sitting in a snowy forest at sunrise, photograph".into(), shape: Some("landscape".into()), ..Default::default() }).await.unwrap();
        assert_eq!(made.len(), 1);
        let fox = &made[0];
        assert!(fox.width > fox.height, "landscape: {}x{}", fox.width, fox.height);
        let png = keep(fox, "1-generate");
        assert!(image::load_from_memory(&png).is_ok());
        assert!(!png.windows(9).any(|w| w == b"snowy for"), "the prompt isn't written into the file");

        if want("picture") {
            let night = go(Request { op: "edit".into(), source: Some(fox.id.clone()), prompt: "make it night with a full moon".into(), ..Default::default() }).await.unwrap();
            keep(&night[0], "2-edit");
            assert_eq!(night[0].parent.as_deref(), Some(fox.id.as_str()));

            // Paint a circle over the middle: put a snowman there.
            let mut mask = image::RgbaImage::new(fox.width, fox.height);
            let (cx, cy, r) = (fox.width as i64 / 2, fox.height as i64 / 2, fox.height as i64 / 4);
            for (x, y, p) in mask.enumerate_pixels_mut() {
                if (x as i64 - cx).pow(2) + (y as i64 - cy).pow(2) < r * r {
                    *p = image::Rgba([255, 64, 64, 255]);
                }
            }
            let mut mask_png = Vec::new();
            image::DynamicImage::ImageRgba8(mask).write_to(&mut std::io::Cursor::new(&mut mask_png), image::ImageFormat::Png).unwrap();
            let filled = go(Request {
                op: "fill".into(),
                source: Some(fox.id.clone()),
                prompt: "a small snowman with a carrot nose".into(),
                mask: Some(base64::engine::general_purpose::STANDARD.encode(&mask_png)),
                ..Default::default()
            })
            .await
            .unwrap();
            assert_eq!((filled[0].width, filled[0].height), (fox.width, fox.height), "filled in at the original size");
            let filled_png = keep(&filled[0], "3-fill");
            let (a, b) = (image::load_from_memory(&png).unwrap().to_rgba8(), image::load_from_memory(&filled_png).unwrap().to_rgba8());
            assert_eq!(a.get_pixel(2, 2), b.get_pixel(2, 2), "outside the painted area nothing changes");

            let wide = go(Request { op: "extend".into(), source: Some(fox.id.clone()), extend: Some([256, 0, 256, 0]), ..Default::default() }).await.unwrap();
            keep(&wide[0], "4-extend");
            assert!(wide[0].width as f64 / wide[0].height as f64 > fox.width as f64 / fox.height as f64);

            let styled = go(Request { op: "restyle".into(), source: Some(fox.id.clone()), prompt: "watercolor".into(), ..Default::default() }).await.unwrap();
            keep(&styled[0], "5-restyle");

            let two = go(Request { op: "generate".into(), prompt: "a cozy cabin, oil painting".into(), count: Some(2), ..Default::default() }).await.unwrap();
            assert_eq!(two.len(), 2, "two versions");
        }
        if want("upscale") {
            install("realesrgan-x4plus").await;
            let big = go(Request { op: "upscale".into(), source: Some(fox.id.clone()), ..Default::default() }).await.unwrap();
            keep(&big[0], "6-upscale");
            assert!(big[0].width >= fox.width * 3, "{} vs {}", big[0].width, fox.width);
        }
        if want("cutout") {
            install("birefnet-lite").await;
            let cut = go(Request { op: "remove_background".into(), source: Some(fox.id.clone()), ..Default::default() }).await.unwrap();
            let bytes = keep(&cut[0], "7-cutout");
            let img = image::load_from_memory(&bytes).unwrap().to_rgba8();
            let clear = img.pixels().filter(|p| p[3] < 16).count() as f64 / (img.width() * img.height()) as f64;
            println!("cut-out: {:.0}% transparent", clear * 100.0);
            assert!(clear > 0.2 && clear < 0.95, "the background went, the fox stayed");
        }
        if want("video") {
            install("wan2.2-ti2v-5b").await;
            let clip = go(Request { op: "video".into(), prompt: "the fox turns its head and snow falls gently".into(), source: Some(fox.id.clone()), seconds: Some(2.0), ..Default::default() }).await.unwrap();
            let bytes = keep(&clip[0], "8-video");
            assert_eq!(&bytes[..4], &[0x1A, 0x45, 0xDF, 0xA3], "a WebM file");
            assert_eq!(clip[0].kind, "video");
            println!("video: {:?}", clip[0].stats);
            let s = clip[0].stats.as_ref().expect("what making it took");
            assert!(s.frames.is_some_and(|f| f > 0) && s.steps.is_some());
        }
    }
    if want("music") {
        // SULCUSAI_MUSIC_MODEL: another music model than ACE-Step 1.5.
        let music: &'static str = Box::leak(std::env::var("SULCUSAI_MUSIC_MODEL").unwrap_or_else(|_| "ace-step-1.5".into()).into_boxed_str());
        install(music).await;
        let song = go(Request { op: "music".into(), model_id: Some(music.into()), prompt: "upbeat acoustic folk song about a road trip through the mountains, warm male vocals".into(), seconds: Some(20.0), ..Default::default() }).await.unwrap();
        let bytes = keep(&song[0], "9-song");
        assert!(bytes.len() > 100_000);
        println!("song: {:.1}s, lyrics: {:?}, {:?}", song[0].seconds, song[0].lyrics.as_deref().map(|l| l.chars().take(160).collect::<String>()), song[0].stats);
        assert!(song[0].stats.as_ref().and_then(|s| s.audio_tokens).is_some_and(|t| t > 0), "audio tokens are counted");
        let sound = go(Request { op: "sound".into(), model_id: Some(music.into()), prompt: "rain on a tin roof with distant thunder".into(), seconds: Some(10.0), ..Default::default() }).await.unwrap();
        keep(&sound[0], "10-sound");
    }
    if want("narrate") {
        let n = go(Request { op: "narrate".into(), prompt: "Welcome to the studio. Everything here is made on this computer.".into(), ..Default::default() }).await.unwrap();
        let bytes = keep(&n[0], "11-narrate");
        assert_eq!(&bytes[..4], b"RIFF");
        assert!(n[0].seconds > 1.0);
    }
    println!("results in {}", out.display());
    let measured: Vec<(String, Option<f64>)> = media::installed(&state.db.lock().unwrap()).into_iter().map(|i| (i.model_id, i.secs)).collect();
    println!("measured: {measured:?}");
}

/// An MP3 as mono samples and their rate, decoded by Windows (Media Foundation).
#[cfg(windows)]
fn decode_mp3(bytes: &[u8]) -> (Vec<f32>, u32) {
    use windows::core::HSTRING;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    let path = std::env::temp_dir().join(format!("sulcusai-song-{}.mp3", uuid::Uuid::new_v4()));
    std::fs::write(&path, bytes).unwrap();
    let mut mono = Vec::new();
    let rate;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).unwrap();
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None).unwrap();
        let stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
        let want = MFCreateMediaType().unwrap();
        want.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).unwrap();
        want.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float).unwrap();
        reader.SetCurrentMediaType(stream, None, &want).unwrap();
        let got = reader.GetCurrentMediaType(stream).unwrap();
        rate = got.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap();
        let channels = got.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap().max(1) as usize;
        loop {
            let mut flags = 0u32;
            let mut sample: Option<IMFSample> = None;
            reader.ReadSample(stream, 0, None, Some(&mut flags as *mut u32), None, Some(&mut sample as *mut Option<IMFSample>)).unwrap();
            if let Some(s) = sample {
                let buf = s.ConvertToContiguousBuffer().unwrap();
                let (mut p, mut len) = (std::ptr::null_mut::<u8>(), 0u32);
                buf.Lock(&mut p, None, Some(&mut len as *mut u32)).unwrap();
                let floats = std::slice::from_raw_parts(p as *const f32, len as usize / 4);
                mono.extend(floats.chunks(channels).map(|f| f.iter().sum::<f32>() / channels as f32));
                buf.Unlock().unwrap();
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
        }
        drop(reader);
        let _ = MFShutdown();
    }
    std::fs::remove_file(&path).ok();
    (mono, rate)
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

/// A rough print of a singing voice (from a vocals-only track): its middle
/// pitch over the voiced moments (autocorrelation, 80 to 800 Hz) and how
/// often it crosses zero there (brightness, breathiness). Two songs by one
/// singer land close together; different singers, further apart.
#[cfg(windows)]
fn voice_print(samples: &[f32], rate: u32) -> (f32, f32) {
    let frame = (rate as usize) / 20;
    let loudest = samples.chunks(frame).map(crate::audio::rms).fold(0.0f32, f32::max).max(1e-6);
    let (lo, hi) = (rate as usize / 800, rate as usize / 80);
    let mut pitches = Vec::new();
    let mut crossings = Vec::new();
    for f in samples.chunks(frame).filter(|f| f.len() == frame && crate::audio::rms(f) > loudest * 0.25) {
        let energy: f32 = f.iter().map(|x| x * x).sum::<f32>().max(1e-9);
        let (lag, peak) = (lo..hi.min(frame - 1)).map(|l| (l, f.iter().zip(&f[l..]).map(|(a, b)| a * b).sum::<f32>() / energy)).fold((0, 0.0f32), |best, x| if x.1 > best.1 { x } else { best });
        if peak > 0.5 && lag > 0 {
            pitches.push(rate as f32 / lag as f32);
            crossings.push(f.windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count() as f32 / frame as f32);
        }
    }
    let middle = |mut v: Vec<f32>| {
        v.sort_by(|a, b| a.total_cmp(b));
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    };
    (middle(pitches), middle(crossings))
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

/// Lyrics from a real chat model fill the song: a minute gets a verse and
/// a chorus (it was three lines), three minutes the whole shape, chosen
/// sections come in order. An artist's style is read without their name,
/// and a song in their way, with a feeling and a screamed chorus, is
/// written and its tags marked.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_lyrics_fill_the_song() {
    use crate::media::{artist, lyrics, VocalEffect};
    let state = media_state();
    lyrics_model(&state);
    let mut failed = Vec::new();
    let structure: Vec<String> = ["Intro", "Verse", "Chorus", "Verse", "Chorus", "Bridge", "Chorus", "Outro"].map(String::from).to_vec();
    let cases: [(f32, Vec<String>, &str); 4] = [
        (30.0, vec![], "cheerful ukulele pop about a sunny weekend"),
        (60.0, vec![], "upbeat acoustic folk song about a road trip through the mountains, warm male vocals"),
        (180.0, vec![], "a heartfelt pop ballad about moving away from home"),
        (150.0, structure, "anthemic rock song about old friends"),
    ];
    for (secs, sections, prompt) in cases {
        let plan = lyrics::plan(secs, &sections, lyrics::Flow::Sung);
        let planned: u32 = plan.iter().map(|(_, n)| n).sum();
        let t = std::time::Instant::now();
        match lyrics::write(&state, prompt, Some(secs), Some("en"), &sections, "", lyrics::Flow::Sung).await {
            Ok(l) => {
                let n = lyrics::sung_lines(&l);
                let tags: Vec<&str> = l.lines().filter(|x| x.trim().starts_with('[')).collect();
                println!("{secs:.0}s: {n} of {planned} lines in {:.0}s, tags {tags:?}\n{l}\n", t.elapsed().as_secs_f64());
                if (n as f32) < planned as f32 * 0.75 {
                    failed.push(format!("{secs:.0}s: {n} lines, {planned} planned"));
                }
            }
            Err(e) => failed.push(format!("{secs:.0}s: {e:?}")),
        }
    }
    for name in ["Taylor Swift", "Lewis Capaldi"] {
        match artist::look_up(&state, name).await {
            Ok(Some(s)) => {
                println!("{name}: {s:#?}");
                let last = name.split(' ').last().unwrap();
                if s.sound().contains(last) || s.genre.is_empty() {
                    failed.push(format!("{name}: {s:?}"));
                }
            }
            other => {
                println!("{name}: not read: {other:?}; it said {:?}", artist::ask(&state, name).await);
                failed.push(format!("{name}: {other:?}"));
            }
        }
    }
    let req = crate::media::Request { op: "music".into(), prompt: "Sing as Lewis Capaldi a song about losing someone at the end of summer".into(), ..Default::default() };
    let Some(styled) = artist::resolve(&state, &req, &|_, _| {}).await else {
        state.engine.lock().await.stop().await;
        panic!("the artist wasn't found: {failed:#?}");
    };
    let mut spec = styled.music.clone().unwrap();
    println!("prompt now {:?}; caption: {}", styled.prompt, spec.caption(&styled.prompt, false));
    if styled.prompt.contains("Capaldi") || spec.caption(&styled.prompt, false).contains("Capaldi") {
        failed.push(format!("the name reached the music model: {:?}", styled.prompt));
    }
    spec.emotion = Some("Heartbroken".into());
    spec.rhyme = Some("aabb".into());
    spec.vocal_effects = vec![VocalEffect { how: "screamed".into(), on: "chorus".into() }, VocalEffect { how: "vibrato".into(), on: "all".into() }];
    match lyrics::write(&state, &spec.caption(&styled.prompt, false), Some(90.0), Some("en"), &spec.sections(), &spec.lyrics_guide(), spec.flow(&styled.prompt)).await {
        Ok(l) => {
            let marked = spec.mark_sections(&l);
            println!("in their way, heartbroken, screamed chorus:\n{marked}\n");
            if !marked.contains("[Chorus - screamed]") {
                failed.push("the chorus tag isn't marked".into());
            }
        }
        Err(e) => failed.push(format!("styled: {e:?}")),
    }
    state.engine.lock().await.stop().await;
    assert!(failed.is_empty(), "{failed:#?}");
}

/// ACE-Step rendered from noise against the same song planned first by its
/// planning model (ace-lm, 1.7B and 4B), as ACE-Step 1.5 is meant to run:
/// the same lyrics, seeds and settings, with the time, the length it came
/// out and the lyric words Whisper hears. The songs are kept in
/// C:\dev\tools\music\ab for listening. SULCUSAI_PLAN_DIT picks the
/// rendering model file (default XL turbo), SULCUSAI_PLANNERS the planners.
///     cargo test e2e_music_planned_or_not -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_music_planned_or_not() {
    use crate::media::music;
    let state = media_state();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    let out = std::path::PathBuf::from(r"C:\dev\tools\music\ab");
    std::fs::create_dir_all(&out).unwrap();
    let dit = std::env::var("SULCUSAI_PLAN_DIT").unwrap_or_else(|_| "acestep-v15-xl-turbo-Q8_0.gguf".into());
    let planners = std::env::var("SULCUSAI_PLANNERS").unwrap_or_else(|_| ",acestep-5Hz-lm-1.7B-Q8_0.gguf,acestep-5Hz-lm-4B-Q8_0.gguf".into());
    let models = state.paths.models.join("media").join("music");
    let dir = crate::media::music_engine_dir(&state).expect("the music engine is installed");
    let lyrics = "[Verse 1]\nMorning light on the window\nCoffee warm in my hands\nEvery road is a new start\nI can feel it again\n\n[Chorus]\nSing it loud, let it carry\nOver rooftops and trees\nSing it loud, let it carry\nAll the way back to me\n\n[Verse 2]\nOld guitar by the doorway\nStrings still tuned to the past\nEvery chord is a promise\nThat the good days will last\n\n[Chorus]\nSing it loud, let it carry\nOver rooftops and trees\nSing it loud, let it carry\nAll the way back to me";
    let caption = "modern country, warm male lead vocal, acoustic guitar, pedal steel, punchy live drums, warm bass, catchy chorus, polished Nashville production";
    let seconds = 100.0;
    let wanted = sung_words(lyrics);
    let mut rows = Vec::new();
    for planner in planners.split(',') {
        if !planner.is_empty() && !models.join(planner).is_file() {
            rows.push(format!("{planner}: not downloaded"));
            continue;
        }
        let name = if planner.is_empty() { "noise".to_string() } else { planner.trim_start_matches("acestep-5Hz-lm-").trim_end_matches("-Q8_0.gguf").to_string() };
        for seed in [11i64, 22] {
            let (bpm, key, time) = music::settle(caption, None, None, None, seed);
            let work = std::env::temp_dir().join(format!("sulcusai-plan-{}", crate::media::store::new_id()));
            std::fs::create_dir_all(&work).unwrap();
            let engine = music::Engine { dir: &dir, models: &models, low_priority: false, job: None, logs: &state.paths.logs };
            let r = music::SongRequest {
                caption,
                lyrics,
                seconds,
                seed,
                language: Some("en"),
                dit_file: &dit,
                bpm: Some(bpm),
                key: Some(&key),
                time_signature: Some(&time),
                versions: 1,
                planner,
                ..Default::default()
            };
            let t = std::time::Instant::now();
            let made = music::make(&engine, &work, &r, &AtomicBool::new(false), &|_, _| {}).await;
            let took = t.elapsed().as_secs_f64();
            let _ = std::fs::remove_dir_all(&work);
            let song = match made {
                Ok(mut s) => s.remove(0),
                Err(e) => {
                    rows.push(format!("{name} seed {seed}: failed ({e})"));
                    println!("{}", rows.last().unwrap());
                    continue;
                }
            };
            let file = out.join(format!("{name}-{seed}.mp3"));
            std::fs::write(&file, &song.mp3).unwrap();
            let (heard, _) = heard_words(&whisper_hears(&state, &file, Some("en")));
            let words = sung_words(&heard);
            let matched = wanted.iter().filter(|w| words.contains(*w)).count();
            rows.push(format!("{name} seed {seed}: {took:.0} s, {:.0} s long ({seconds:.0} asked), {matched} of {} lyric words heard", song.seconds, wanted.len()));
            println!("{}", rows.last().unwrap());
        }
    }
    println!("\n{}\nsongs in {}", rows.join("\n"), out.display());
}

/// One music model at several guidance strengths: the same lyrics and seeds,
/// with the time and the lyric words Whisper hears. Songs kept in
/// C:\dev\tools\music\ab as guide-<g>-<seed>.mp3. SULCUSAI_GUIDE_DIT picks
/// the model file (default ACE-Step 1.5 Base), SULCUSAI_GUIDES the strengths.
///     cargo test e2e_music_guidance -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_music_guidance() {
    use crate::media::music;
    let state = media_state();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    let out = std::path::PathBuf::from(r"C:\dev\tools\music\ab");
    std::fs::create_dir_all(&out).unwrap();
    let dit = std::env::var("SULCUSAI_GUIDE_DIT").unwrap_or_else(|_| "acestep-v15-base-Q8_0.gguf".into());
    let guides = std::env::var("SULCUSAI_GUIDES").unwrap_or_else(|_| "1,3,7".into());
    let models = state.paths.models.join("media").join("music");
    let dir = crate::media::music_engine_dir(&state).expect("the music engine is installed");
    let lyrics = "[Verse 1]\nMorning light on the window\nCoffee warm in my hands\nEvery road is a new start\nI can feel it again\n\n[Chorus]\nSing it loud, let it carry\nOver rooftops and trees\nSing it loud, let it carry\nAll the way back to me\n\n[Verse 2]\nOld guitar by the doorway\nStrings still tuned to the past\nEvery chord is a promise\nThat the good days will last\n\n[Chorus]\nSing it loud, let it carry\nOver rooftops and trees\nSing it loud, let it carry\nAll the way back to me";
    let caption = "modern country, warm male lead vocal, acoustic guitar, pedal steel, punchy live drums, warm bass, catchy chorus, polished Nashville production";
    let wanted = sung_words(lyrics);
    let mut rows = vec![format!("{dit}")];
    for g in guides.split(',').filter_map(|g| g.trim().parse::<f32>().ok()) {
        for seed in [11i64, 22] {
            let (bpm, key, time) = music::settle(caption, None, None, None, seed);
            let work = std::env::temp_dir().join(format!("sulcusai-guide-{}", crate::media::store::new_id()));
            std::fs::create_dir_all(&work).unwrap();
            let engine = music::Engine { dir: &dir, models: &models, low_priority: false, job: None, logs: &state.paths.logs };
            let r = music::SongRequest { caption, lyrics, seconds: 100.0, seed, language: Some("en"), dit_file: &dit, bpm: Some(bpm), key: Some(&key), time_signature: Some(&time), versions: 1, guidance: Some(g), ..Default::default() };
            let t = std::time::Instant::now();
            let song = music::make(&engine, &work, &r, &AtomicBool::new(false), &|_, _| {}).await.unwrap().remove(0);
            let took = t.elapsed().as_secs_f64();
            let _ = std::fs::remove_dir_all(&work);
            let file = out.join(format!("guide-{g}-{seed}.mp3"));
            std::fs::write(&file, &song.mp3).unwrap();
            let (heard, _) = heard_words(&whisper_hears(&state, &file, Some("en")));
            let matched = wanted.iter().filter(|w| sung_words(&heard).contains(*w)).count();
            rows.push(format!("guidance {g} seed {seed}: {took:.0} s, {matched} of {} lyric words heard", wanted.len()));
            println!("{}", rows.last().unwrap());
        }
    }
    println!("\n{}", rows.join("\n"));
}

/// A song from lyrics written Suno's way (directions in the brackets), as
/// the Studio makes it: on YuE2 (installed from the catalog: its engine from
/// the engines release) and on ACE-Step 1.5 XL, each kept in
/// C:\dev\tools\music\ab with the words Whisper hears and its length.
///     cargo test e2e_suno_style_lyrics -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_suno_style_lyrics() {
    use crate::media::{self, store, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    let lyrics = "[Intro: dark acoustic ambience, faint fire crackling, distant steel guitar]\n\n[Verse 1: low warm male vocal, conversational phrasing, restrained acoustic groove]\nI learned the shape of every fracture\nEvery place we came undone\nKept rebuilding what was breaking\nLike the work could make us one\n\n[Chorus: strong melodic lift, memorable country hook, full drums and vocal harmonies]\nSmoke and fire\nThe flames climbed higher\nBuilt a home on all the ashes\nWhile you kept striking the matches\n\n[Pause]\n\n[Outro: drums disappear, acoustic guitar and pedal steel fade]\nNothing left to save now";
    let wanted = sung_words(lyrics);
    let out = std::path::PathBuf::from(r"C:\dev\tools\music\ab");
    std::fs::create_dir_all(&out).unwrap();
    for model in ["yue2-3b", "ace-step-1.5-xl"] {
        let t = std::time::Instant::now();
        media::install_for_test(&state, model).await.unwrap_or_else(|e| panic!("installing {model}: {e}"));
        let req = Request {
            op: "music".into(),
            model_id: Some(model.into()),
            prompt: "dark modern country, Southern gothic".into(),
            lyrics: Some(lyrics.into()),
            seconds: Some(70.0),
            seed: Some(11),
            language: Some("en".into()),
            music: Some(media::MusicSpec { voice: Some("male".into()), ..Default::default() }),
            ..Default::default()
        };
        let item = media::run(&state, store::new_id(), req).await.unwrap_or_else(|e| panic!("{model}: {e}")).remove(0);
        let file = out.join(format!("smoke-{model}.mp3"));
        std::fs::write(&file, store::read(&state.paths, &c, &item.id).unwrap()).unwrap();
        let (heard, _) = heard_words(&whisper_hears(&state, &file, Some("en")));
        let matched = wanted.iter().filter(|w| sung_words(&heard).contains(*w)).count();
        let sang_tags = ["melodic", "crackling", "conversational", "disappear"].iter().filter(|w| heard.to_lowercase().contains(*w)).count();
        println!("{model}: {:.0} s long in {:.0} s, {matched} of {} lyric words heard, {sang_tags} direction words sung", item.seconds, t.elapsed().as_secs_f64(), wanted.len());
        assert!((item.seconds - 70.0).abs() < 3.0);
    }
}

/// The Suno-style song features through the app's own jobs: best of 2 (the
/// clearest kept, with its review), the hook as the title, the settings kept
/// for Reuse, cover art from Z-Image Turbo, and the stems as songs of their
/// own. The song, its art and its parts go to C:\dev\tools\music\ab\features.
///     cargo test e2e_suno_features -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_suno_features() {
    use crate::media::{self, store, MusicSpec, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    for model in ["ace-step-1.5-xl", "z-image-turbo"] {
        media::install_for_test(&state, model).await.unwrap_or_else(|e| panic!("installing {model}: {e}"));
    }
    let out = std::path::PathBuf::from(r"C:\dev\tools\music\ab\features");
    std::fs::create_dir_all(&out).unwrap();
    let lyrics = "[Verse 1]\nWindows down on the coast road\nRadio loud and the night is young\n\n[Chorus]\nSummer nights, we were golden\nDancing under the neon sun\nSummer nights, we were golden\nNever wanted the night to be done";
    let t = std::time::Instant::now();
    let req = Request {
        op: "music".into(),
        model_id: Some("ace-step-1.5-xl".into()),
        prompt: "upbeat 80s-inspired synth pop about a summer drive".into(),
        lyrics: Some(lyrics.into()),
        seconds: Some(45.0),
        seed: Some(5),
        count: Some(2),
        language: Some("en".into()),
        music: Some(MusicSpec { voice: Some("female".into()), best_of: true, exclude: Some("saxophone".into()), ..Default::default() }),
        ..Default::default()
    };
    let made = media::run(&state, store::new_id(), req).await.unwrap();
    println!("best of 2 in {:.0} s: {} kept, note {:?}, title {:?}", t.elapsed().as_secs_f64(), made.len(), made[0].note, made[0].title);
    assert_eq!(made.len(), 1, "only the clearest is kept");
    let song = made[0].clone();
    assert_eq!(song.title.as_deref(), Some("Summer Nights"));
    assert!(song.review.is_some() && song.note.as_deref().is_some_and(|n| n.contains("clearest of 2")));
    assert_eq!(song.music.as_ref().and_then(|m| m.exclude.as_deref()), Some("saxophone"), "its settings are kept for Reuse");
    std::fs::write(out.join("song.mp3"), store::read(&state.paths, &c, &song.id).unwrap()).unwrap();
    let t = std::time::Instant::now();
    let art = media::run(&state, store::new_id(), Request { op: "cover_art".into(), source: Some(song.id.clone()), ..Default::default() }).await.unwrap().remove(0);
    assert!(art.poster && art.id == song.id);
    let jpeg = store::read_thumb(&state.paths, &c, &song.id).unwrap();
    let img = image::load_from_memory(&jpeg).unwrap();
    std::fs::write(out.join("cover.jpg"), &jpeg).unwrap();
    println!("cover art in {:.0} s: {}x{}", t.elapsed().as_secs_f64(), img.width(), img.height());
    assert_eq!(img.width(), img.height());
    assert_eq!(store::list(&state.db.lock().unwrap(), &c, Some("image"), 50, None).len(), 0, "the picture itself isn't kept");
    let t = std::time::Instant::now();
    let parts = media::run(&state, store::new_id(), Request { op: "stems".into(), source: Some(song.id.clone()), music: Some(MusicSpec { track: Some("all".into()), ..Default::default() }), ..Default::default() }).await.unwrap();
    println!("stems in {:.0} s: {:?}", t.elapsed().as_secs_f64(), parts.iter().map(|p| (p.title.clone().unwrap_or_default(), p.seconds.round())).collect::<Vec<_>>());
    assert_eq!(parts.len(), 4);
    for p in &parts {
        assert_eq!(p.parent.as_deref(), Some(song.id.as_str()));
        assert!((p.seconds - song.seconds).abs() < 1.0);
        std::fs::write(out.join(format!("{}.wav", p.title.clone().unwrap_or_default())), store::read(&state.paths, &c, &p.id).unwrap()).unwrap();
    }
}

/// Remove FX through the app's own job: a dry voice with a room's reverb
/// (and with an echo too) made dry again, on the NVIDIA card when there is
/// one. The test sounds are C:\dev\tools\music\dereverb\test (scripts/wet.py
/// makes and scores them); the cleaned ones are written beside them. The
/// model is copied from the export there when it isn't in the models
/// folder yet (it's the file on the engines release).
///     cargo test e2e_remove_fx -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_remove_fx() {
    use crate::media::{self, store, MediaItem, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    let dir = std::path::PathBuf::from(r"C:\dev\tools\music\dereverb");
    let h = state.catalog.media.dereverb.clone().unwrap();
    let dest = state.paths.models.join("media").join("helpers").join(&h.file.file);
    if !dest.metadata().is_ok_and(|m| m.len() == h.file.size) {
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(dir.join("out").join(&h.file.file), &dest).unwrap();
    }
    for name in ["reverb", "reverb-echo"] {
        let bytes = std::fs::read(dir.join("test").join(format!("{name}.wav"))).unwrap();
        let item = store::save(&state.db.lock().unwrap(), &state.paths, &c, MediaItem { kind: "audio".into(), op: "import".into(), mime: "audio/wav".into(), seconds: 14.8, ..Default::default() }, &bytes, None).unwrap();
        let t = std::time::Instant::now();
        let got = media::run(&state, store::new_id(), Request { op: "dry".into(), source: Some(item.id.clone()), ..Default::default() }).await.unwrap();
        println!("{name}: dry in {:.1} s, {:.1} s long, hidden {}", t.elapsed().as_secs_f64(), got[0].seconds, got[0].hidden);
        assert!(got[0].hidden && (got[0].seconds - 14.8).abs() < 0.2);
        std::fs::write(dir.join("test").join(format!("{name}-dry.wav")), store::read(&state.paths, &c, &got[0].id).unwrap()).unwrap();
    }
}

/// The song studio's chat bar with a real chat model: a few requests on a
/// two-track project, and the actions it turns them into.
///     cargo test e2e_studio_ask -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_studio_ask() {
    let state = media_state();
    lyrics_model(&state);
    let summary = "Tempo 120 bpm, 4 beats a bar. Length 3:00. Playhead 0:00.\nTracks:\n1. \"Song\" (audio) volume 0 dB, pan 0, effects: none; clips: 0-180 s\n2. \"Keys\" (midi, instrument 0) volume 0 dB, pan 0, effects: none; clips: 10-40 s";
    let mut ok = 0;
    let asks = [
        ("add some reverb to the keys and make the song 95 bpm", &["add_effect", "tempo"][..]),
        ("mute the song track and turn the keys down 6 dB", &["mute", "volume"][..]),
        ("make a funky bass part for the first minute", &["make_part"][..]),
        ("loop from 30 to 45 seconds", &["loop"][..]),
        ("put the keys on the synth as a gritty reese bass with a slow wobble", &["synth"][..]),
        ("build me a lo-fi telephone voice effect and put it on the song", &["build_effect"][..]),
    ];
    for (ask, want) in asks {
        let t = std::time::Instant::now();
        let got = crate::media::ask_studio(&state, ask, summary).await;
        let kinds: Vec<String> = got.as_ref().map(|a| a.iter().map(|x| x["do"].as_str().unwrap_or("").to_string()).collect()).unwrap_or_default();
        let hit = want.iter().all(|w| kinds.iter().any(|k| k == w));
        ok += hit as usize;
        println!("{ask:?} in {:.1} s -> {:?} {}", t.elapsed().as_secs_f64(), got.map(|a| serde_json::to_string(&a).unwrap()), if hit { "ok" } else { "MISSED" });
    }
    println!("{ok} of {} understood", asks.len());
    assert!(ok >= 5);
}

/// The song studio's new part: a guitar added to the synth-pop song from
/// e2e_suno_features by ACE-Step Base and taken out alone (one take), its
/// length and loudness against the mix; then its notes read (Basic Pitch).
/// The part is kept in C:\dev\tools\music\ab\studio-part.wav.
///     cargo test e2e_studio_part -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_studio_part() {
    use crate::media::{self, store, MediaItem, MusicSpec, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    media::install_for_test(&state, "ace-step-1.5-base").await.unwrap();
    let mp3 = std::fs::read(r"C:\dev\tools\music\ab\features\song.mp3").expect("run e2e_suno_features first");
    let mix = store::save(&state.db.lock().unwrap(), &state.paths, &c, MediaItem { kind: "audio".into(), op: "studio".into(), prompt: "mix".into(), mime: "audio/mpeg".into(), seconds: 45.0, hidden: true, ..Default::default() }, &mp3, None).unwrap();
    let t = std::time::Instant::now();
    let parts = media::run(&state, store::new_id(), Request { op: "studio_part".into(), prompt: "bright funky rhythm guitar".into(), source: Some(mix.id.clone()), count: Some(1), music: Some(MusicSpec { track: Some("guitar".into()), ..Default::default() }), ..Default::default() }).await.unwrap();
    let part = &parts[0];
    let wav = store::read(&state.paths, &c, &part.id).unwrap();
    let file = std::path::PathBuf::from(r"C:\dev\tools\music\ab\studio-part").with_extension(if part.mime.contains("wav") { "wav" } else { "mp3" });
    std::fs::write(&file, &wav).unwrap();
    let (samples, _, _) = crate::media::audio_in_decode_for_test(&file);
    let (mix_samples, _, _) = crate::media::audio_in_decode_for_test(std::path::Path::new(r"C:\dev\tools\music\ab\features\song.mp3"));
    let db = |x: &[f32]| 20.0 * crate::audio::rms(x).max(1e-9).log10();
    println!("part in {:.0} s: {:.0} s long ({:.0} s mix), {:.1} dB against the mix's {:.1} dB, hidden {}, title {:?}", t.elapsed().as_secs_f64(), part.seconds, mix.seconds, db(&samples), db(&mix_samples), part.hidden, part.title);
    assert!(part.hidden && (part.seconds - 45.0).abs() < 3.0 && db(&samples) > -45.0);
    assert!(store::get(&state.db.lock().unwrap(), &c, &parts[0].id).is_some());
    let t = std::time::Instant::now();
    let notes = media::studio_midi_for_test(&state, &part.id, false).await.unwrap();
    println!("{} notes read in {:.0} s", notes.len(), t.elapsed().as_secs_f64());
    assert!(!notes.is_empty());
}

/// A custom model trained the Studio's way on six songs (the test songs in
/// C:\dev\tools\music\train-test\audio): the trainer set up from the
/// catalog (uv and its packages; the source from the release zip in
/// C:\dev\sulcusai-dist\music, unpacked as the app would), the songs'
/// words heard, SULCUSAI_TRAIN_EPOCHS passes (default 30), then a song made
/// with it on ACE-Step 1.5 Base. Kept in C:\dev\tools\music\ab\custom.mp3.
///     cargo test e2e_custom_model -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_custom_model() {
    use crate::media::{self, store, train, MusicSpec, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    let t = state.catalog.media.trainer.clone().unwrap();
    let source = train::source_dir(&state.paths, &t);
    if !source.join("train.py").is_file() {
        let zip = std::path::PathBuf::from(r"C:\dev\sulcusai-dist\music").join(format!("ace-step-1.5-trainer-{}.zip", t.build));
        let mut a = zip::ZipArchive::new(std::fs::File::open(&zip).unwrap()).unwrap();
        a.extract(&source).unwrap();
    }
    if std::env::var("SULCUSAI_TRAIN_EPOCHS").is_err() {
        std::env::set_var("SULCUSAI_TRAIN_EPOCHS", "30");
    }
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    media::install_for_test(&state, train::BASE_MODEL).await.unwrap();
    let files: Vec<String> = std::fs::read_dir(r"C:\dev\tools\music\train-test\audio").unwrap().flatten().map(|e| e.path().to_string_lossy().into_owned()).collect();
    let t0 = std::time::Instant::now();
    let req = Request { op: "train".into(), prompt: "Test songs".into(), train: Some(train::TrainSpec { name: "Test songs".into(), description: "modern country, warm male lead vocal, acoustic guitar, pedal steel".into(), files, songs: Vec::new() }), ..Default::default() };
    media::run(&state, store::new_id(), req).await.unwrap();
    let models = train::list(&state.db.lock().unwrap(), &c);
    println!("trained in {:.0} s: {:?}", t0.elapsed().as_secs_f64(), models.iter().map(|m| (&m.name, &m.trigger, m.songs, m.epochs)).collect::<Vec<_>>());
    let cm = models.first().expect("a custom model").clone();
    assert!(train::adapters_dir(&state.paths).join(&cm.id).join("adapter_model.safetensors").is_file());
    let lyrics = "[Verse 1]\nMorning light on the window\nCoffee warm in my hands\n\n[Chorus]\nSing it loud, let it carry\nOver rooftops and trees";
    let t1 = std::time::Instant::now();
    let song = media::run(
        &state,
        store::new_id(),
        Request { op: "music".into(), prompt: "a song about a fresh start".into(), lyrics: Some(lyrics.into()), seconds: Some(40.0), seed: Some(3), language: Some("en".into()), music: Some(MusicSpec { custom: Some(cm.id.clone()), ..Default::default() }), ..Default::default() },
    )
    .await
    .unwrap()
    .remove(0);
    let file = std::path::PathBuf::from(r"C:\dev\tools\music\ab\custom.mp3");
    std::fs::write(&file, store::read(&state.paths, &c, &song.id).unwrap()).unwrap();
    let (heard, _) = heard_words(&whisper_hears(&state, &file, Some("en")));
    let wanted = sung_words(lyrics);
    println!("song with it in {:.0} s on {:?}: {} of {} words heard", t1.elapsed().as_secs_f64(), song.model_id, wanted.iter().filter(|w| sung_words(&heard).contains(*w)).count(), wanted.len());
    assert_eq!(song.model_id.as_deref(), Some(train::BASE_MODEL));
    train::remove(&state.db.lock().unwrap(), &c, &state.paths, &cm.id).unwrap();
}

/// HeartMuLa installed from the catalog (audio.cpp's Vulkan build from its
/// release, the model checked against its hash) and a song made the
/// Studio's way: its length, the words Whisper hears, and the time. Kept in
/// C:\dev\tools\music\ab\heartmula\app.mp3.
///     cargo test e2e_heartmula -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_heartmula() {
    use crate::media::{self, store, MusicSpec, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    let t = std::time::Instant::now();
    media::install_for_test(&state, "heartmula-3b").await.unwrap();
    println!("installed in {:.0} s", t.elapsed().as_secs_f64());
    let lyrics = "[Verse 1]\nMorning light on the window\nCoffee warm in my hands\nEvery road is a new start\nI can feel it again\n\n[Chorus]\nSing it loud, let it carry\nOver rooftops and trees\nSing it loud, let it carry\nAll the way back to me";
    let wanted = sung_words(lyrics);
    let t = std::time::Instant::now();
    let req = Request {
        op: "music".into(),
        model_id: Some("heartmula-3b".into()),
        prompt: "warm modern country about a fresh start".into(),
        lyrics: Some(lyrics.into()),
        seconds: Some(120.0),
        seed: Some(7),
        language: Some("en".into()),
        music: Some(MusicSpec { voice: Some("male".into()), instruments: vec!["acoustic guitar".into(), "pedal steel".into()], ..Default::default() }),
        ..Default::default()
    };
    let song = media::run(&state, store::new_id(), req).await.unwrap().remove(0);
    let file = std::path::PathBuf::from(r"C:\dev\tools\music\ab\heartmula\app.mp3");
    std::fs::write(&file, store::read(&state.paths, &c, &song.id).unwrap()).unwrap();
    let (heard, _) = heard_words(&whisper_hears(&state, &file, Some("en")));
    let matched = wanted.iter().filter(|w| sung_words(&heard).contains(*w)).count();
    println!("{:.0} s long in {:.0} s, {matched} of {} words heard, mime {}, title {:?}", song.seconds, t.elapsed().as_secs_f64(), wanted.len(), song.mime, song.title);
    assert_eq!(song.mime, "audio/mpeg");
    assert!(song.seconds > 30.0 && matched * 3 >= wanted.len() * 2);
}

/// Remaster and mashup as ACE-Step covers of the synth-pop song from
/// e2e_suno_features: the song re-rendered close to itself (strength 0.6,
/// 0.8), and its tune and words in the dark-country song's sound (that song
/// as the reference). The words Whisper hears in each, against the
/// original's. Songs in C:\dev\tools\music\ab\cover.
///     cargo test e2e_remaster_and_mashup -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_remaster_and_mashup() {
    use crate::media::music;
    let state = media_state();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    let out = std::path::PathBuf::from(r"C:\dev\tools\music\ab\cover");
    std::fs::create_dir_all(&out).unwrap();
    let source = std::path::PathBuf::from(r"C:\dev\tools\music\ab\features\song.mp3");
    let other = std::path::PathBuf::from(r"C:\dev\tools\music\ab\smoke-ace-step-1.5-xl.mp3");
    let lyrics = "[Verse 1]\nWindows down on the coast road\nRadio loud and the night is young\n\n[Chorus]\nSummer nights, we were golden\nDancing under the neon sun\nSummer nights, we were golden\nNever wanted the night to be done";
    let wanted = sung_words(lyrics);
    let words_in = |file: &std::path::Path| {
        let (heard, _) = heard_words(&whisper_hears(&state, file, Some("en")));
        wanted.iter().filter(|w| sung_words(&heard).contains(*w)).count()
    };
    println!("original: {} of {} words heard", words_in(&source), wanted.len());
    let models = state.paths.models.join("media").join("music");
    let dir = crate::media::music_engine_dir(&state).expect("the music engine is installed");
    let runs: [(&str, &str, f32, bool); 4] = [
        ("remaster-0.6", "upbeat 80s-inspired synth pop about a summer drive, female lead vocals, polished modern production", 0.6, false),
        ("remaster-0.8", "upbeat 80s-inspired synth pop about a summer drive, female lead vocals, polished modern production", 0.8, false),
        ("mashup-0.5", "dark modern country, Southern gothic, acoustic guitar, pedal steel, female lead vocals", 0.5, true),
        ("mashup-0.7", "dark modern country, Southern gothic, acoustic guitar, pedal steel, female lead vocals", 0.7, true),
    ];
    for (name, caption, strength, with_other) in runs {
        let work = std::env::temp_dir().join(format!("sulcusai-cover-{}", crate::media::store::new_id()));
        std::fs::create_dir_all(&work).unwrap();
        let engine = music::Engine { dir: &dir, models: &models, low_priority: false, job: None, logs: &state.paths.logs };
        let r = music::SongRequest {
            caption,
            lyrics,
            seconds: 45.0,
            seed: 7,
            language: Some("en"),
            dit_file: "acestep-v15-xl-turbo-Q8_0.gguf",
            task: "cover",
            strength: Some(strength),
            source: Some(&source),
            reference: with_other.then_some(other.as_path()),
            versions: 1,
            ..Default::default()
        };
        let t = std::time::Instant::now();
        let song = music::make(&engine, &work, &r, &AtomicBool::new(false), &|_, _| {}).await.unwrap().remove(0);
        let _ = std::fs::remove_dir_all(&work);
        let file = out.join(format!("{name}.mp3"));
        std::fs::write(&file, &song.mp3).unwrap();
        println!("{name}: {:.0} s, {:.0} s long, {} of {} words heard", t.elapsed().as_secs_f64(), song.seconds, words_in(&file), wanted.len());
    }
}

/// Cover art for a few kinds of song, kept in C:\dev\tools\music\ab\art to
/// look at (no lettering should be in them), and a song video of the first.
///     cargo test e2e_cover_art -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_cover_art() {
    use crate::media::{self, store, MediaItem, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    media::install_for_test(&state, "z-image-turbo").await.unwrap();
    let out = std::path::PathBuf::from(r"C:\dev\tools\music\ab\art");
    std::fs::create_dir_all(&out).unwrap();
    let song_mp3 = std::fs::read(r"C:\dev\tools\music\ab\features\song.mp3").expect("run e2e_suno_features first");
    let lyrics = "[Verse 1]\nWindows down on the coast road\nRadio loud and the night is young\n\n[Chorus]\nSummer nights, we were golden\nDancing under the neon sun\nSummer nights, we were golden\nNever wanted the night to be done";
    for (i, prompt) in ["upbeat 80s-inspired synth pop about a summer drive", "dark modern country, Southern gothic, a house built on ashes", "lo-fi hip hop for studying on a rainy night"].iter().enumerate() {
        let song = store::save(&state.db.lock().unwrap(), &state.paths, &c, MediaItem { kind: "audio".into(), op: "music".into(), prompt: prompt.to_string(), title: Some("Summer Nights".into()), mime: "audio/mpeg".into(), seconds: 45.0, lyrics: Some(lyrics.into()), ..Default::default() }, &song_mp3, None).unwrap();
        let t = std::time::Instant::now();
        media::run(&state, store::new_id(), Request { op: "cover_art".into(), source: Some(song.id.clone()), seed: Some(3), ..Default::default() }).await.unwrap();
        std::fs::write(out.join(format!("art-{i}.jpg")), store::read_thumb(&state.paths, &c, &song.id).unwrap()).unwrap();
        println!("{prompt}: {:.0} s", t.elapsed().as_secs_f64());
        if i == 0 {
            let t = std::time::Instant::now();
            let v = media::run(&state, store::new_id(), Request { op: "song_video".into(), source: Some(song.id.clone()), ..Default::default() }).await.unwrap().remove(0);
            std::fs::write(out.join("video.mp4"), store::read(&state.paths, &c, &v.id).unwrap()).unwrap();
            std::fs::write(out.join("video-still.jpg"), store::read_thumb(&state.paths, &c, &v.id).unwrap()).unwrap();
            println!("video in {:.0} s: {} x {}, {:.0} s, {} bytes", t.elapsed().as_secs_f64(), v.width, v.height, v.seconds, v.size);
            assert_eq!(v.parent.as_deref(), Some(song.id.as_str()));
        }
    }
}

/// The singer's middle pitch in each song in a folder (SULCUSAI_PITCH_DIR,
/// default C:\dev\tools\music\ab): the voice taken out (HTDemucs) and its
/// pitch found (RMVPE, from the singing model's pieces). Under ~200 Hz is a
/// man's range, over ~230 a woman's.
///     cargo test e2e_singer_pitch -- --ignored --nocapture
#[cfg(windows)]
#[test]
#[ignore]
fn e2e_singer_pitch() {
    use crate::media::{stems, svc};
    let state = media_state();
    let dir = std::path::PathBuf::from(std::env::var("SULCUSAI_PITCH_DIR").unwrap_or_else(|_| r"C:\dev\tools\music\ab".into()));
    let real = Paths::new(app_data_dir()).unwrap();
    let dll = [real.engines.join("onnxruntime-1.30.0-cuda13").join("onnxruntime.dll"), real.engines.join("onnxruntime-1.30.0-cpu-x64").join("onnxruntime.dll")].into_iter().find(|p| p.is_file()).unwrap();
    let card = dll.to_string_lossy().contains("cuda13").then_some(crate::natural::Card { device: 0, dml: false });
    let sep = state.paths.models.join("media").join("helpers").join("htdemucs_fp16weights.onnx");
    let mut conv = svc::Converter::new(&dll, &state.paths.models.join("media").join("own-voice-sing"), 8, card).unwrap();
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "mp3")).collect();
    files.sort();
    for f in files {
        let (mono, rate) = decode_mp3(&std::fs::read(&f).unwrap());
        let mix = stems::Stereo { rate, ch: [mono.clone(), mono] }.resampled(stems::RATE);
        let (parts, _) = stems::separate(&dll, &sep, &mix, 8, &[3], card, &AtomicBool::new(false)).unwrap();
        let pitch = conv.middle_pitch_of(&parts[0].mono()).unwrap();
        println!("{}: singer {pitch:.0} Hz", f.file_name().unwrap().to_string_lossy());
    }
    stems::unload();
}

/// A song sung in the installed app's recorded voice from the start: the
/// same male song made with no reference and with the person's own singing
/// as the music model's reference (`--ref-audio`, which sets timbre and
/// delivery), then each sung again in the voice. Reads the app's data read
/// only; prints the words heard, the singer's middle pitch and how close
/// the voice print is to the person's, before and after converting. Songs
/// in C:\dev\tools\music\ab.
///     cargo test e2e_own_voice_from_the_start -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_own_voice_from_the_start() {
    use crate::media::{music, own_voice, stems, svc};
    let state = media_state();
    let real = Paths::new(app_data_dir()).unwrap();
    let conn = rusqlite::Connection::open_with_flags(&real.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let vault = crate::crypto::Vault::open(&real.data.join("keys.json"), crate::crypto::platform_protector(&real.data)).unwrap();
    let c = vault.work_cipher().expect("the app's data key is locked with a password");
    let voice = own_voice::list(&conn, &c).into_iter().next().expect("a recorded voice");
    let (reference, sung) = own_voice::singing_reference(&real, &c, &voice).unwrap();
    println!("reference: {:.1} s of {}", reference.len() as f32 / svc::RATE as f32, if sung { "singing" } else { "reading" });
    // The singer as the app picks it from the recording, said in the caption the app's way.
    let sex = own_voice::sex_of(&real, &c, &voice);
    println!("their voice reads as {sex:?}");
    let spec = crate::media::MusicSpec { voice: sex.map(str::to_string), genre: Some("country".into()), instruments: vec!["acoustic guitar".into(), "pedal steel".into()], ..Default::default() };
    let caption = spec.caption("A modern country song about a fresh start, punchy live drums, warm bass, catchy chorus", false);
    let caption = caption.as_str();
    println!("caption: {caption}");
    let out = std::path::PathBuf::from(r"C:\dev\tools\music\ab");
    std::fs::create_dir_all(&out).unwrap();
    // The reference for the music model, written for the run only.
    let ref_file = std::env::temp_dir().join(format!("sulcusai-ref-{}.wav", crate::media::store::new_id()));
    std::fs::write(&ref_file, crate::audio::wav_encode(&reference, svc::RATE)).unwrap();
    let models = state.paths.models.join("media").join("music");
    let dir = crate::media::music_engine_dir(&state).expect("the music engine is installed");
    let lyrics = "[Verse 1]\nMorning light on the window\nCoffee warm in my hands\nEvery road is a new start\nI can feel it again\n\n[Chorus]\nSing it loud, let it carry\nOver rooftops and trees\nSing it loud, let it carry\nAll the way back to me";
    let wanted = sung_words(lyrics);
    let rt = real.engines.join("onnxruntime-1.30.0-cuda13");
    let dll = [rt.join("onnxruntime.dll"), real.engines.join("onnxruntime-1.30.0-cpu-x64").join("onnxruntime.dll")].into_iter().find(|p| p.is_file()).unwrap();
    let card = dll.to_string_lossy().contains("cuda13").then_some(crate::natural::Card { device: 0, dml: false });
    let sep = state.paths.models.join("media").join("helpers").join("htdemucs_fp16weights.onnx");
    let svc_dir = state.paths.models.join("media").join("own-voice-sing");
    let mut conv = svc::Converter::new(&dll, &svc_dir, 8, card).unwrap();
    let person = conv.voice(&reference).unwrap();
    let print = |conv: &mut svc::Converter, x: &[f32]| {
        let w16 = stems::resample(x, svc::RATE, 16_000);
        let (n, fb) = crate::diarize::fbank_with(&w16, true);
        let e = conv.run("campplus.onnx", vec![("fbank", vec![1, n as i64, 80], fb)]).unwrap();
        let norm = e.iter().map(|v| v * v).sum::<f32>().sqrt();
        e.into_iter().map(|v| v / norm).collect::<Vec<f32>>()
    };
    let target = print(&mut conv, &reference);
    let cos = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
    let mut rows = vec![format!("their voice: middle pitch {:.0} Hz", person.pitch)];
    for (name, with_ref) in [("no-reference", false), ("their-reference", true)] {
        for seed in [11i64, 22] {
            let (bpm, key, time) = music::settle(caption, None, None, None, seed);
            let work = std::env::temp_dir().join(format!("sulcusai-ref-{}", crate::media::store::new_id()));
            std::fs::create_dir_all(&work).unwrap();
            let engine = music::Engine { dir: &dir, models: &models, low_priority: false, job: None, logs: &state.paths.logs };
            let r = music::SongRequest {
                caption,
                lyrics,
                seconds: 60.0,
                seed,
                language: Some("en"),
                dit_file: "acestep-v15-xl-turbo-Q8_0.gguf",
                bpm: Some(bpm),
                key: Some(&key),
                time_signature: Some(&time),
                versions: 1,
                reference: with_ref.then_some(ref_file.as_path()),
                ..Default::default()
            };
            let t = std::time::Instant::now();
            let song = music::make(&engine, &work, &r, &AtomicBool::new(false), &|_, _| {}).await.unwrap().remove(0);
            let took = t.elapsed().as_secs_f64();
            let _ = std::fs::remove_dir_all(&work);
            let file = out.join(format!("voice-{name}-{seed}.mp3"));
            std::fs::write(&file, &song.mp3).unwrap();
            let (heard, _) = heard_words(&whisper_hears(&state, &file, Some("en")));
            let matched = wanted.iter().filter(|w| sung_words(&heard).contains(*w)).count();
            // The singer, taken out of the band.
            let (mono, rate) = decode_mp3(&song.mp3);
            let mix = stems::Stereo { rate, ch: [mono.clone(), mono] }.resampled(stems::RATE);
            let (parts, _) = stems::separate(&dll, &sep, &mix, 8, &[3], card, &AtomicBool::new(false)).unwrap();
            stems::unload();
            let vocals = parts[0].mono();
            // How loud the band is beside the voice (a reference of bare
            // singing could thin the arrangement out).
            let whole = mix.mono();
            let band: Vec<f32> = whole.iter().zip(&vocals).map(|(m, v)| m - v).collect();
            let band_db = 20.0 * (crate::audio::rms(&band) / crate::audio::rms(&vocals).max(1e-6)).log10();
            let pitch = conv.middle_pitch_of(&vocals).unwrap();
            let before = cos(&print(&mut conv, &vocals), &target);
            let semis = svc::octave_shift(pitch, if sung { person.pitch } else { person.pitch * 1.26 });
            let converted = conv.convert(&person, &vocals, svc::Settings { steps: if card.is_some() { 30 } else { 10 }, semitones: semis, ..Default::default() }, &AtomicBool::new(false), &|_| {}).unwrap();
            let after = cos(&print(&mut conv, &converted), &target);
            rows.push(format!("{name} seed {seed}: {took:.0} s, {matched} of {} words heard, singer {pitch:.0} Hz, band {band_db:+.1} dB against the voice, likeness {before:.2} as made, {after:.2} sung again ({semis} semitones)", wanted.len()));
            println!("{}", rows.last().unwrap());
        }
    }
    let _ = std::fs::remove_file(&ref_file);
    println!("\n{}", rows.join("\n"));
}

/// Which music model, and which words, give the most natural voice above
/// the band: the same lyrics and seeds on each, with the words Whisper hears,
/// the voice's loudness against the whole song (from the vocals taken out by
/// ACE-Step 1.5 Base) and the time. SULCUSAI_VOICE_MODELS picks the models.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_music_voice_quality() {
    use crate::media::{self, store, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    let out = std::env::temp_dir().join("sulcusai-voice-quality");
    std::fs::create_dir_all(&out).unwrap();
    let models = std::env::var("SULCUSAI_VOICE_MODELS").unwrap_or_else(|_| "ace-step-1.5,ace-step-1.5-xl,ace-step-1.5-xl-sft,ace-step-1.5-sftturbo,ace-step-1.5-clear".into());
    let lyrics = "[Verse]\nMorning light on the window\nCoffee warm in my hands\nEvery road is a new start\nI can feel it again\n\n[Chorus]\nSing it loud, let it carry\nOver rooftops and trees\nSing it loud, let it carry\nAll the way back to me";
    let music = || media::MusicSpec { genre: Some("Pop".into()), voice: Some("female".into()), instruments: vec!["Piano".into(), "Drums".into(), "Bass".into()], ..Default::default() };
    // (name, model, words added to the description)
    let mut runs: Vec<(String, String, &str)> = Vec::new();
    for m in models.split(',') {
        runs.push((m.to_string(), m.to_string(), ""));
    }
    runs.insert(1, ("ace-step-1.5+upfront".into(), "ace-step-1.5".into(), ", clear lead vocals upfront in the mix, natural human voice"));
    media::install_for_test(&state, "ace-step-1.5-base").await.unwrap();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    let mut rows = Vec::new();
    for (name, model, extra) in &runs {
        media::install_for_test(&state, model).await.unwrap();
        for seed in [11i64, 22] {
            let req = Request {
                op: "music".into(),
                model_id: Some(model.clone()),
                prompt: format!("a hopeful morning song{extra}"),
                lyrics: Some(lyrics.into()),
                seconds: Some(60.0),
                seed: Some(seed),
                music: Some(music()),
                language: Some("en".into()),
                ..Default::default()
            };
            let t = std::time::Instant::now();
            let item = match media::run(&state, store::new_id(), req).await {
                Ok(mut v) => v.remove(0),
                Err(e) => {
                    rows.push(format!("{name} seed {seed}: failed ({e})"));
                    continue;
                }
            };
            let took = t.elapsed().as_secs_f64();
            let bytes = store::read(&state.paths, &c, &item.id).unwrap();
            let file = out.join(format!("{}-{seed}.mp3", name.replace('+', "-")));
            std::fs::write(&file, &bytes).unwrap();
            let (heard, _) = heard_words(&whisper_hears(&state, &file, Some("en")));
            let words = sung_words(&heard);
            let wanted = sung_words(lyrics);
            let matched = wanted.iter().filter(|w| words.contains(*w)).count();
            let spec = media::MusicSpec { task: Some("extract".into()), audio: Some(item.id.clone()), track: Some("vocals".into()), ..Default::default() };
            let over = match media::run(&state, store::new_id(), Request { op: "music".into(), seconds: Some(60.0), music: Some(spec), ..Default::default() }).await {
                Ok(mut v) => {
                    let (voc, _) = decode_mp3(&store::read(&state.paths, &c, &v.remove(0).id).unwrap());
                    let (mix, _) = decode_mp3(&bytes);
                    crate::audio::rms(&voc) / crate::audio::rms(&mix).max(1e-6)
                }
                Err(_) => f32::NAN,
            };
            rows.push(format!("{name} seed {seed}: {took:.0}s, {matched} of {} lyric words heard, voice to song {over:.2}", wanted.len()));
            println!("{}", rows.last().unwrap());
        }
    }
    state.engine.lock().await.stop().await;
    println!("\n{}\nsongs in {}", rows.join("\n"), out.display());
}

/// Emotional songs, female and male voices, kept for listening, each with
/// the words Whisper hears, its clipped samples and the listener's review;
/// the first also mapped for the song view, its MIDI file kept. (A finish
/// that worked on the separated voice was tested here against the songs as
/// made, and lost when listened to: see `stems`.)
/// SULCUSAI_FINISH_MODEL picks the music model (default: the one this PC is
/// offered), SULCUSAI_FINISH_CASES the songs.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_music_emotional_songs() {
    use crate::media::{self, store, MusicSpec, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    let out = std::env::temp_dir().join("sulcusai-finish-ab");
    std::fs::create_dir_all(&out).unwrap();
    let model = std::env::var("SULCUSAI_FINISH_MODEL").ok().unwrap_or_else(|| media::recommended(&state, media::Kind::Music).unwrap().id);
    media::install_for_test(&state, &model).await.unwrap();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    // (name, description, voice, feeling, genre, instruments, lyrics)
    let cases: &[(&str, &str, &str, &str, &str, &[&str], &str)] = &[
        ("female-heartbreak", "a heartbroken piano ballad about someone who left", "female", "Heartbroken", "Ballad", &["Piano", "Strings"],
         "[Verse]\nI kept your coat by the door\nLike you'd come back for it\nEvery night I tell the walls\nThat I'm fine, that I'm fine\n\n[Chorus]\nBut I'm falling apart\nIn the place where you were\nSay my name one more time\nSo I know it was real"),
        ("male-heartbreak", "an aching soul ballad, sorry for what he never said", "male", "Heartbroken", "Soul", &["Piano", "Organ", "Bass"],
         "[Verse]\nRain on the windshield, the radio low\nDriving the long way so I don't go home\nYour words in my head like a song on repeat\nI should have held on, I should have held on\n\n[Chorus]\nOh, I'm sorry, I'm sorry\nFor the things I never said\nOh, come back to me\nBefore the morning comes"),
        ("female-anthem", "a powerful pop anthem about getting back up", "female", "Triumphant", "Pop", &["Drums", "Synth", "Bass"],
         "[Verse]\nThey told me I would never fly\nI kept my wings and I kept my fight\nEvery door they closed on me\nTaught me how to break it free\n\n[Chorus]\nI'm still standing, I'm still here\nShout it louder, let them hear\nEvery scar became a song\nI was strong all along"),
        ("male-rock", "a raspy, defiant rock song", "male", "Angry", "Rock", &["Electric guitar", "Drums", "Bass"],
         "[Verse]\nYou built your walls out of my name\nYou lit the fire then called it rain\nI swallowed every word you said\nNow they're burning in my head\n\n[Chorus]\nI won't break, I won't bend\nThis is where your story ends\nTear it down, tear it down\nI'm not afraid of the sound"),
        ("female-tender", "a tender, intimate acoustic love song", "female", "Tender", "Folk", &["Acoustic guitar"],
         "[Verse]\nHold me close while the candle burns\nMorning comes but the world can turn\nYour hand in mine, the quiet room\nNothing else I need but you\n\n[Chorus]\nStay a little longer\nStay till the stars are gone\nYour heartbeat is the only song\nI'll ever need"),
        ("male-longing", "a late-night R&B slow jam full of longing", "male", "Longing", "R&B", &["Electric piano", "Bass", "Drums"],
         "[Verse]\nThree a.m. and I'm wide awake\nCounting all the calls I didn't make\nYour perfume still on the pillow case\nI can't get you out of this place\n\n[Chorus]\nI miss you more than I can say\nEvery night feels the same\nBaby, won't you come my way\nI still call your name"),
    ];
    let pick = std::env::var("SULCUSAI_FINISH_CASES").ok();
    let clipped = |bytes: &[u8]| {
        let (s, _) = decode_mp3(bytes);
        (s.iter().filter(|v| v.abs() >= 0.999).count(), s.iter().fold(0f32, |m, v| m.max(v.abs())))
    };
    let mut rows = Vec::new();
    for (i, (name, prompt, voice, feeling, genre, instruments, lyrics)) in cases.iter().enumerate() {
        if pick.as_deref().is_some_and(|p| !p.split(',').any(|x| x == *name)) {
            continue;
        }
        let music = MusicSpec { genre: Some(genre.to_string()), voice: Some(voice.to_string()), emotion: Some(feeling.to_string()), instruments: instruments.iter().map(|s| s.to_string()).collect(), ..Default::default() };
        let wanted = sung_words(lyrics);
        {
            let how = "song";
            let req = Request {
                op: "music".into(),
                model_id: Some(model.clone()),
                prompt: prompt.to_string(),
                lyrics: Some(lyrics.to_string()),
                seconds: Some(60.0),
                seed: Some(100 + i as i64),
                music: Some(music.clone()),
                language: Some("en".into()),
                ..Default::default()
            };
            let t = std::time::Instant::now();
            let item = match media::run(&state, store::new_id(), req).await {
                Ok(mut v) => v.remove(0),
                Err(e) => {
                    rows.push(format!("{name} {how}: failed ({e})"));
                    continue;
                }
            };
            let took = t.elapsed().as_secs_f64();
            let bytes = store::read(&state.paths, &c, &item.id).unwrap();
            let file = out.join(format!("{name}-{how}.mp3"));
            std::fs::write(&file, &bytes).unwrap();
            let (heard, _) = heard_words(&whisper_hears(&state, &file, Some("en")));
            let words = sung_words(&heard);
            let matched = wanted.iter().filter(|w| words.contains(*w)).count();
            let (clips, peak) = clipped(&bytes);
            rows.push(format!("{name} {how}: {took:.0}s, {matched} of {} words heard, {clips} clipped samples (peak {peak:.2}), {} KB", wanted.len(), bytes.len() / 1024));
            println!("{}", rows.last().unwrap());
            // The listener, as the Studio's button runs it.
            {
                let listened = media::run(&state, store::new_id(), Request { op: "listen".into(), source: Some(item.id.clone()), ..Default::default() }).await;
                match listened.map(|mut v| v.remove(0).review) {
                    Ok(Some(r)) => rows.push(format!("  listener: score {}, {} of {} words, unclear {:?}; {}", r.score, r.heard, r.words, r.unclear, r.notes.join(" "))),
                    other => rows.push(format!("  listener failed: {other:?}")),
                }
                println!("{}", rows.last().unwrap());
                // The song view's map of the first song, and its MIDI file.
                if i == 0 {
                    let t = std::time::Instant::now();
                    match media::run(&state, store::new_id(), Request { op: "map".into(), source: Some(item.id.clone()), ..Default::default() }).await.map(|mut v| v.remove(0).map) {
                        Ok(Some(m)) => {
                            let sections: Vec<String> = m.sections.iter().map(|s| format!("{} {:.0}-{:.0}", s.name, s.start, s.end)).collect();
                            let notes: Vec<String> = m.parts.iter().map(|p| format!("{} {}", p.name, p.notes.len())).collect();
                            std::fs::write(out.join(format!("{name}.mid")), crate::media::songmap::midi(&m)).unwrap();
                            rows.push(format!("  map ({:.0}s): sections [{}]; notes [{}]", t.elapsed().as_secs_f64(), sections.join(", "), notes.join(", ")));
                        }
                        other => rows.push(format!("  map failed: {other:?}")),
                    }
                    println!("{}", rows.last().unwrap());
                }
            }
        }
    }
    state.engine.lock().await.stop().await;
    println!("\n{}\nsongs in {}", rows.join("\n"), out.display());
}

/// The listener and its remake on a real song: listened to, its unclear
/// stretches made again, and listened to again. Both kept for listening.
/// SULCUSAI_FINISH_MODEL picks the music model.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_music_remake() {
    use crate::media::{self, store, MusicSpec, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    let out = std::env::temp_dir().join("sulcusai-finish-ab");
    std::fs::create_dir_all(&out).unwrap();
    let model = std::env::var("SULCUSAI_FINISH_MODEL").ok().unwrap_or_else(|| media::recommended(&state, media::Kind::Music).unwrap().id);
    media::install_for_test(&state, &model).await.unwrap();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    let lyrics = "[Verse]\nI kept your coat by the door\nLike you'd come back for it\nEvery night I tell the walls\nThat I'm fine, that I'm fine\n\n[Chorus]\nBut I'm falling apart\nIn the place where you were\nSay my name one more time\nSo I know it was real";
    let req = Request {
        op: "music".into(),
        model_id: Some(model.clone()),
        prompt: "a heartbroken piano ballad about someone who left".into(),
        lyrics: Some(lyrics.into()),
        seconds: Some(60.0),
        seed: Some(100),
        music: Some(MusicSpec { genre: Some("Ballad".into()), voice: Some("female".into()), emotion: Some("Heartbroken".into()), instruments: vec!["Piano".into(), "Strings".into()], ..Default::default() }),
        language: Some("en".into()),
        ..Default::default()
    };
    let song = media::run(&state, store::new_id(), req).await.unwrap().remove(0);
    let listened = media::run(&state, store::new_id(), Request { op: "listen".into(), source: Some(song.id.clone()), ..Default::default() }).await.unwrap().remove(0);
    let before = listened.review.clone().unwrap();
    println!("before: score {}, {} of {} words, unclear {:?}", before.score, before.heard, before.words, before.unclear);
    std::fs::write(out.join("remake-1-before.mp3"), store::read(&state.paths, &c, &song.id).unwrap()).unwrap();
    if before.unclear.is_empty() {
        println!("every part was clear: the remake has nothing to do");
    }
    // A section changed from the song view: the new version comes back mapped.
    let t = std::time::Instant::now();
    let edit = MusicSpec { task: Some("repaint".into()), audio: Some(song.id.clone()), region: Some([20.0, 35.0]), ..Default::default() };
    let edited = media::run(
        &state,
        store::new_id(),
        Request { op: "music".into(), model_id: Some(model.clone()), prompt: format!("{}, bigger with strings", song.prompt), lyrics: Some(lyrics.into()), music: Some(edit), then_map: Some(true), ..Default::default() },
    )
    .await
    .unwrap()
    .remove(0);
    let m = edited.map.as_ref().expect("the edited version is mapped");
    println!(
        "section edit ({:.0}s): sections [{}]; notes [{}]",
        t.elapsed().as_secs_f64(),
        m.sections.iter().map(|s| format!("{} {:.0}-{:.0}", s.name, s.start, s.end)).collect::<Vec<_>>().join(", "),
        m.parts.iter().map(|p| format!("{} {}", p.name, p.notes.len())).collect::<Vec<_>>().join(", ")
    );
    std::fs::write(out.join("remake-3-section-edit.mp3"), store::read(&state.paths, &c, &edited.id).unwrap()).unwrap();
    if before.unclear.is_empty() {
        state.engine.lock().await.stop().await;
        return;
    }
    let t = std::time::Instant::now();
    let remade = media::run(&state, store::new_id(), Request { op: "remake".into(), source: Some(song.id.clone()), model_id: Some(model.clone()), prompt: song.prompt.clone(), ..Default::default() }).await;
    state.engine.lock().await.stop().await;
    let remade = remade.unwrap().remove(0);
    let after = remade.review.clone().unwrap();
    std::fs::write(out.join("remake-2-after.mp3"), store::read(&state.paths, &c, &remade.id).unwrap()).unwrap();
    println!(
        "after ({:.0}s): score {}, {} of {} words, unclear {:?}; note {:?}; parent kept: {}",
        t.elapsed().as_secs_f64(),
        after.score,
        after.heard,
        after.words,
        after.unclear,
        remade.note,
        store::get(&state.db.lock().unwrap(), &c, &song.id).is_some()
    );
    assert_eq!(remade.parent.as_deref(), Some(song.id.as_str()));
}

/// Songs made the way the Studio and chats make them, with the real music
/// engine: lyrics written by the chat model (English and Spanish), the
/// person's own lyrics, an instrumental, a song with no chat model to write
/// its lyrics, a sound effect, and a music model installed from a pasted
/// Hugging Face link (its own file deleted afterwards). Each is checked for
/// length and sound, and the sung ones by speech recognition (Whisper large
/// v3 turbo, installed the way the app installs it): the words it hears
/// against the lyrics. SULCUSAI_SONGS picks cases (default all);
/// SULCUSAI_MEDIA_OUT keeps the MP3s; SULCUSAI_LYRICS_MODEL is the chat
/// model (default qwen3-4b-2507); SULCUSAI_HUB_SONG the link.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_music_songs() {
    use crate::media::{self, store, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    let out = std::path::PathBuf::from(std::env::var("SULCUSAI_MEDIA_OUT").unwrap_or_else(|_| std::env::temp_dir().join("sulcusai-music-out").display().to_string()));
    std::fs::create_dir_all(&out).unwrap();
    let cases = std::env::var("SULCUSAI_SONGS").unwrap_or_else(|_| "sound,instrumental,no_chat_model,written,spanish,own,shaped,long,cover,repaint,extend,parts,hub".into());
    let want = |p: &str| cases.split(',').any(|x| x == p);
    let started = std::time::Instant::now();
    media::install_for_test(&state, "ace-step-1.5").await.unwrap();
    println!("installed ACE-Step 1.5 in {:.0}s", started.elapsed().as_secs_f64());

    // (name, what was asked, item made or why not, language to listen for)
    let mut made: Vec<(String, Request, Result<media::MediaItem, String>, Option<&str>)> = Vec::new();
    let make = |name: &str, req: Request| {
        let state = state.clone();
        let name = name.to_string();
        async move {
            let t = std::time::Instant::now();
            let r = media::run(&state, store::new_id(), req).await.map(|mut v| v.remove(0));
            println!("{name}: {:.0}s -> {}", t.elapsed().as_secs_f64(), match &r {
                Ok(i) => format!("{:.1}s, lyrics {:?}", i.seconds, i.lyrics.as_deref().map(|l| l.chars().take(120).collect::<String>())),
                Err(e) => e.clone(),
            });
            r
        }
    };
    let folk = "upbeat acoustic folk song about a road trip through the mountains, warm male vocals, guitar and harmonica";
    if want("sound") {
        let req = Request { op: "sound".into(), prompt: "rain on a tin roof with distant thunder".into(), seconds: Some(10.0), ..Default::default() };
        made.push(("sound".into(), req.clone(), make("sound", req).await, None));
    }
    if want("instrumental") {
        let req = Request { op: "music".into(), prompt: "mellow lofi hip hop beat with soft piano and vinyl crackle".into(), seconds: Some(30.0), instrumental: Some(true), ..Default::default() };
        made.push(("instrumental".into(), req.clone(), make("instrumental", req).await, None));
    }
    // No chat model on this PC yet: the music model writes the lyrics itself.
    if want("no_chat_model") {
        let req = Request { op: "music".into(), prompt: "anthemic rock song about old friends, powerful male vocals, electric guitars and drums".into(), seconds: Some(30.0), ..Default::default() };
        made.push(("no_chat_model".into(), req.clone(), make("no_chat_model", req).await, Some("en")));
    }
    // A chat model, as the default (not loaded yet): it writes the lyrics.
    lyrics_model(&state);
    if want("written") {
        let req = Request { op: "music".into(), prompt: folk.into(), seconds: Some(30.0), ..Default::default() };
        made.push(("written".into(), req.clone(), make("written", req).await, Some("en")));
    }
    if want("spanish") {
        let req = Request { op: "music".into(), prompt: "energetic latin pop song about summer nights by the sea, bright female vocals".into(), seconds: Some(30.0), language: Some("es".into()), ..Default::default() };
        made.push(("spanish".into(), req.clone(), make("spanish", req).await, Some("es")));
    }
    if want("own") {
        let lyrics = "[Verse]\nMorning light across the kitchen floor\nCoffee waiting by the open door\nEvery little thing I need is here\nSinging softly so the world can hear\n\n[Chorus]\nHold on, hold on to the sunrise\nHold on, never say goodbye";
        let req = Request { op: "music".into(), prompt: "gentle pop ballad, soft female vocals, piano and strings".into(), seconds: Some(60.0), lyrics: Some(lyrics.into()), language: Some("en".into()), ..Default::default() };
        made.push(("own".into(), req.clone(), make("own", req).await, Some("en")));
    }
    // Shaped in the Studio: style, mood, instruments, voice, beat, tempo, key and time signature.
    if want("shaped") {
        let music = media::MusicSpec {
            // Synthwave's vocals come out blurred (a trait of the style here): pop, to hear the words.
            genre: Some("Synth pop".into()),
            mood: Some("Nostalgic".into()),
            instruments: vec!["Synth".into(), "Drums".into(), "Electric guitar".into()],
            voice: Some("female".into()),
            beat: Some("Four on the floor".into()),
            bpm: Some(118),
            key: Some("A minor".into()),
            time_signature: Some("4/4".into()),
            ..Default::default()
        };
        let req = Request { op: "music".into(), prompt: "driving through a neon city at night".into(), seconds: Some(40.0), music: Some(music), language: Some("en".into()), ..Default::default() };
        made.push(("shaped".into(), req.clone(), make("shaped", req).await, Some("en")));
    }
    // Two versions at once: two songs, each as long as asked.
    if want("versions") {
        let req = Request { op: "music".into(), prompt: "cheerful ukulele pop about a sunny weekend, bright female vocals".into(), seconds: Some(30.0), count: Some(2), ..Default::default() };
        let t = std::time::Instant::now();
        let r = media::run(&state, store::new_id(), req.clone()).await;
        println!("versions: {:.0}s -> {:?}", t.elapsed().as_secs_f64(), r.as_ref().map(|v| v.iter().map(|i| i.seconds).collect::<Vec<_>>()));
        let v = r.unwrap();
        assert_eq!(v.len(), 2, "two versions");
        for (k, i) in v.into_iter().enumerate() {
            made.push((format!("version{}", k + 1), req.clone(), Ok(i), Some("en")));
        }
    }
    // A long song: as long as asked.
    if want("long") {
        let req = Request { op: "music".into(), prompt: "slow soulful blues ballad about a long road home, raspy male vocals, electric guitar and organ".into(), seconds: Some(150.0), ..Default::default() };
        made.push(("long".into(), req.clone(), make("long", req).await, Some("en")));
    }
    // Rap, the way the Studio asks for it: hip hop with a rap voice.
    if want("rap") {
        let music = media::MusicSpec { genre: Some("Hip hop".into()), voice: Some("rap".into()), ..Default::default() };
        let req = Request { op: "music".into(), prompt: "rap about grinding at a night job to pay for college".into(), seconds: Some(60.0), music: Some(music), language: Some("en".into()), ..Default::default() };
        made.push(("rap".into(), req.clone(), make("rap", req).await, Some("en")));
    }
    // An intro in the structure: singing should start with it, not after a long instrumental.
    if want("intro") {
        let music = media::MusicSpec { genre: Some("Pop".into()), voice: Some("female".into()), sections: ["Intro", "Verse", "Chorus", "Verse", "Chorus", "Outro"].map(String::from).to_vec(), ..Default::default() };
        let req = Request { op: "music".into(), prompt: "driving home at sunrise after a long night".into(), seconds: Some(150.0), music: Some(music), language: Some("en".into()), ..Default::default() };
        made.push(("intro".into(), req.clone(), make("intro", req).await, Some("en")));
    }
    // The same, told the singing starts right away: does the lead-in shorten?
    if want("intro_soon") {
        let music = media::MusicSpec { genre: Some("Pop".into()), voice: Some("female".into()), sections: ["Intro", "Verse", "Chorus", "Verse", "Chorus", "Outro"].map(String::from).to_vec(), ..Default::default() };
        let req = Request { op: "music".into(), prompt: "driving home at sunrise after a long night, short intro, the singing starts right away".into(), seconds: Some(150.0), music: Some(music), language: Some("en".into()), ..Default::default() };
        made.push(("intro_soon".into(), req.clone(), make("intro_soon", req).await, Some("en")));
    }
    // Auto length: a jingle comes out short, a full song a few minutes, as the lyrics need.
    for (case, prompt) in [("auto_jingle", "a cheerful little jingle for a neighborhood bakery"), ("auto_full", "a heartfelt pop song about leaving your hometown for the big city")] {
        if want(case) {
            let music = media::MusicSpec { genre: Some("Pop".into()), auto_length: true, ..Default::default() };
            let req = Request { op: "music".into(), prompt: prompt.into(), music: Some(music), language: Some("en".into()), ..Default::default() };
            let r = make(case, req.clone()).await;
            // Checked against the length the song came out at (Auto sets it).
            let asked = r.as_ref().map_or(30.0, |i| i.seconds as f32);
            made.push((case.into(), Request { seconds: Some(asked), ..req }, r, Some("en")));
        }
    }
    // A saved artist: the first song becomes the voice, the second follows
    // it; a third with the same settings and no artist is the control.
    if want("artist_voice") {
        let sound = media::MusicSpec { genre: Some("Indie pop".into()), voice: Some("female".into()), vocal_style: Some("breathy".into()), instruments: vec!["Piano".into(), "Acoustic guitar".into()], ..Default::default() };
        let nova = {
            let conn = state.db.lock().unwrap();
            media::persona::save(&conn, &c, media::persona::Persona { name: "Nova".into(), sound: sound.clone(), language: Some("en".into()), ..Default::default() }).unwrap()
        };
        let with = |prompt: &str, persona: bool| Request {
            op: "music".into(),
            prompt: prompt.into(),
            seconds: Some(60.0),
            music: Some(if persona { media::MusicSpec { persona: Some("Nova".into()), ..Default::default() } } else { sound.clone() }),
            language: Some("en".into()),
            ..Default::default()
        };
        let first = with("city lights on a quiet night", true);
        made.push(("artist_first".into(), first.clone(), make("artist_first", first).await, Some("en")));
        let voice = media::persona::find(&state.db.lock().unwrap(), &c, &nova.id).and_then(|p| p.voice_song);
        println!("Nova's voice song: {voice:?}");
        let second = with("a road trip with my best friend", true);
        made.push(("artist_second".into(), second.clone(), make("artist_second", second).await, Some("en")));
        let control = with("a road trip with my best friend", false);
        made.push(("artist_control".into(), control.clone(), make("artist_control", control).await, Some("en")));
    }
    // Many styles, one each (SULCUSAI_SONGS=genres): words heard in every one.
    if want("genres") {
        let styles: [(&str, &str, Option<&str>, &str, Option<&str>); 10] = [
            ("trap", "Trap", Some("rap"), "counting money and ignoring the haters", None),
            ("country", "Country", Some("male"), "a small town Friday night", None),
            ("rock", "Rock", Some("male"), "breaking free from a dead-end job", None),
            ("metal", "Metal", Some("male"), "a storm tearing through the mountains", None),
            ("rnb", "R&B", Some("female"), "falling for someone slowly", None),
            ("reggaeton", "Reggaeton", Some("male"), "una fiesta en la playa", Some("es")),
            ("kpop", "K-pop", Some("female"), "a confident summer dance anthem", None),
            ("gospel", "Gospel", Some("choir"), "hope after hard times", None),
            ("jazz", "Jazz", Some("female"), "a rainy night in a city cafe", None),
            ("folk", "Folk", Some("duet"), "two old friends meeting again", None),
        ];
        for (name, genre, voice, prompt, lang) in styles {
            let music = media::MusicSpec { genre: Some(genre.into()), voice: voice.map(str::to_string), ..Default::default() };
            let req = Request { op: "music".into(), prompt: prompt.into(), seconds: Some(60.0), music: Some(music), language: Some(lang.unwrap_or("en").into()), ..Default::default() };
            made.push((name.into(), req.clone(), make(name, req).await, Some(lang.unwrap_or("en"))));
        }
    }
    // A minute's song with its lyrics written: a verse and a chorus (it was three lines).
    if want("minute") {
        let req = Request { op: "music".into(), prompt: folk.into(), seconds: Some(60.0), ..Default::default() };
        made.push(("minute".into(), req.clone(), make("minute", req).await, Some("en")));
    }
    // In an artist's style, asked in the description.
    if want("artist") {
        let req = Request { op: "music".into(), prompt: "Sing as Taylor Swift a song about bears".into(), seconds: Some(60.0), ..Default::default() };
        made.push(("artist".into(), req.clone(), make("artist", req).await, Some("en")));
    }
    // Shaped in depth: sections, rhyme, a key, a feeling, sung harmonies and ways of singing.
    let deep = |layer: bool| media::MusicSpec {
        genre: Some("Pop rock".into()),
        voice: Some("male".into()),
        emotion: Some("Heartbroken".into()),
        key: Some("D major".into()),
        sections: ["Verse", "Chorus", "Verse", "Chorus"].map(String::from).to_vec(),
        rhyme: Some("abab".into()),
        harmony_voice: Some("female".into()),
        harmony_where: Some("chorus".into()),
        harmony_instrument: layer.then(|| "Strings".into()),
        harmony_layer: layer,
        vocal_effects: vec![media::VocalEffect { how: "vibrato".into(), on: "all".into() }, media::VocalEffect { how: "belting".into(), on: "chorus".into() }],
        ..Default::default()
    };
    if want("deep") {
        let req = Request { op: "music".into(), prompt: "the last night before you moved away".into(), seconds: Some(120.0), music: Some(deep(false)), language: Some("en".into()), ..Default::default() };
        made.push(("deep".into(), req.clone(), make("deep", req).await, Some("en")));
    }
    // The harmonies layered on afterwards by ACE-Step 1.5 Base.
    if want("layered") {
        media::install_for_test(&state, "ace-step-1.5-base").await.unwrap();
        let req = Request { op: "music".into(), prompt: "the last night before you moved away".into(), seconds: Some(60.0), music: Some(deep(true)), language: Some("en".into()), ..Default::default() };
        made.push(("layered".into(), req.clone(), make("layered", req).await, Some("en")));
    }
    // Work on a song: the written one (or the first song made), the
    // instrumental for adding a part.
    let first_song = made.iter().find(|m| m.0 == "written").or_else(|| made.iter().find(|m| m.3.is_some())).and_then(|m| m.2.as_ref().ok()).cloned();
    let work = |task: &str, song: &media::MediaItem, extra: media::MusicSpec| media::MusicSpec { task: Some(task.into()), audio: Some(song.id.clone()), ..extra };
    if let (true, Some(song)) = (want("cover"), first_song.as_ref()) {
        let spec = work("cover", song, media::MusicSpec { genre: Some("Jazz".into()), instruments: vec!["Piano".into(), "Saxophone".into()], strength: Some(0.5), ..Default::default() });
        let req = Request { op: "music".into(), prompt: "as a smoky late-night jazz club song".into(), seconds: Some(song.seconds as f32), music: Some(spec), ..Default::default() };
        made.push(("cover".into(), req.clone(), make("cover", req).await, Some("en")));
    }
    if let (true, Some(song)) = (want("repaint"), first_song.as_ref()) {
        let spec = work("repaint", song, media::MusicSpec { region: Some([10.0, 20.0]), ..Default::default() });
        let req = Request { op: "music".into(), prompt: "a soaring guitar solo".into(), seconds: Some(song.seconds as f32), music: Some(spec), ..Default::default() };
        made.push(("repaint".into(), req.clone(), make("repaint", req).await, Some("en")));
    }
    if let (true, Some(song)) = (want("extend"), first_song.as_ref()) {
        let spec = work("extend", song, media::MusicSpec { extend_by: Some(20.0), ..Default::default() });
        let req = Request { op: "music".into(), prompt: String::new(), seconds: Some(song.seconds as f32 + 20.0), music: Some(spec), ..Default::default() };
        made.push(("extend".into(), req.clone(), make("extend", req).await, Some("en")));
    }
    // Parts need ACE-Step 1.5 Base: installed for these.
    if want("parts") {
        media::install_for_test(&state, "ace-step-1.5-base").await.unwrap();
        if let Some(song) = first_song.as_ref() {
            let spec = work("extract", song, media::MusicSpec { track: Some("vocals".into()), ..Default::default() });
            let req = Request { op: "music".into(), prompt: String::new(), seconds: Some(song.seconds as f32), music: Some(spec), ..Default::default() };
            made.push(("extract_vocals".into(), req.clone(), make("extract_vocals", req).await, None));
        }
        let beat = made.iter().find(|m| m.0 == "instrumental").and_then(|m| m.2.as_ref().ok()).cloned();
        if let Some(song) = beat {
            let spec = work("add", &song, media::MusicSpec { track: Some("drums".into()), ..Default::default() });
            let req = Request { op: "music".into(), prompt: "punchy live drums".into(), seconds: Some(song.seconds as f32), music: Some(spec), ..Default::default() };
            made.push(("add_drums".into(), req.clone(), make("add_drums", req).await, None));
        }
    }
    // A music model from a pasted Hugging Face link, sharing the catalog model's files.
    let mut hub_file = None;
    if want("hub") {
        let link = std::env::var("SULCUSAI_HUB_SONG").unwrap_or_else(|_| "https://huggingface.co/Serveurperso/ACE-Step-1.5-GGUF/blob/main/acestep-v15-xl-turbo-Q4_K_M.gguf".into());
        let (repo, file) = crate::hub::parse_link(&link).unwrap();
        let client = crate::hub::client(&state).unwrap();
        let r = crate::hub::repo(&client, &repo).await.unwrap();
        let (found, kind) = media::hub::from_link(&state, &r).expect("a music model SulcusAI runs");
        assert_eq!(kind, "music");
        let file = file.expect("the link names a file");
        let offer = found.files.iter().find(|f| f.file == file).expect("the linked file is offered");
        println!("hub: {} {} ({:.2} GB, {:.2} GB to download), runs: {}", found.name, offer.quant, offer.size as f64 / 1e9, offer.download as f64 / 1e9, offer.runnable);
        let spec = media::hub::prepare(&state, &repo, &file).await.unwrap();
        println!("hub files: {:?}", spec.files.iter().map(|f| (&f.role, &f.file, f.size)).collect::<Vec<_>>());
        let t = std::time::Instant::now();
        media::install_for_test(&state, &spec.id).await.unwrap();
        println!("installed {} in {:.0}s", spec.name, t.elapsed().as_secs_f64());
        hub_file = spec.files.iter().find(|f| f.file.starts_with("hfm-")).map(|f| media::model_dir(&state.paths, &spec).join(&f.file));
        let req = Request { op: "music".into(), model_id: Some(spec.id.clone()), prompt: folk.into(), seconds: Some(30.0), ..Default::default() };
        let r = make("hub", req.clone()).await;
        if let Ok(i) = &r {
            assert_eq!(i.model_id.as_deref(), Some(spec.id.as_str()), "made with the Hugging Face model");
        }
        made.push(("hub".into(), req, r, Some("en")));
    }
    state.engine.lock().await.stop().await;

    // Where the voice is (SULCUSAI_VOCALS=1): ACE-Step 1.5 Base takes the
    // vocals out of each sung song, and their loudness every 3 seconds shows
    // when the singing starts (Whisper hears in 30-second windows, too
    // coarse for a long intro).
    if std::env::var("SULCUSAI_VOCALS").is_ok() {
        media::install_for_test(&state, "ace-step-1.5-base").await.unwrap();
        for (name, _, r, lang) in &made {
            let (Ok(song), Some(_)) = (r, lang) else { continue };
            let spec = media::MusicSpec { task: Some("extract".into()), audio: Some(song.id.clone()), track: Some("vocals".into()), ..Default::default() };
            let req = Request { op: "music".into(), prompt: String::new(), seconds: Some(song.seconds as f32), music: Some(spec), ..Default::default() };
            let Ok(mut v) = media::run(&state, store::new_id(), req).await else { continue };
            let (samples, rate) = decode_mp3(&store::read(&state.paths, &c, &v.remove(0).id).unwrap());
            let step = rate as usize * 3;
            let levels: Vec<f32> = samples.chunks(step).map(crate::audio::rms).collect();
            let loudest = levels.iter().cloned().fold(0.0f32, f32::max).max(1e-6);
            let line: String = levels.iter().map(|l| match l / loudest { x if x > 0.5 => '#', x if x > 0.2 => '+', x if x > 0.08 => '.', _ => ' ' }).collect();
            let first = levels.iter().position(|l| l / loudest > 0.2).map(|i| i * 3);
            let (pitch, bright) = voice_print(&samples, rate);
            // How loud the voice is against the whole song (drowned out: low).
            let (mix, _) = decode_mp3(&store::read(&state.paths, &c, &song.id).unwrap());
            let over = crate::audio::rms(&samples) / crate::audio::rms(&mix).max(1e-6);
            println!(
                "{name}: vocals every 3 s |{line}| first at {}, voice {:.0} Hz, brightness {:.3}, voice to song {:.2}",
                first.map_or("never".into(), |s| format!("{s}s")),
                pitch,
                bright,
                over
            );
        }
        state.engine.lock().await.stop().await;
    }

    // Listening: length, sound, and the words speech recognition hears.
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    let mut failed = Vec::new();
    for (name, req, r, lang) in &made {
        let item = match r {
            Ok(i) => i,
            // No chat model to write lyrics: said plainly, not sung as made-up
            // words (the music model's own lyrics came out as garbage).
            Err(e) if name == "no_chat_model" && e.contains("lyrics") => {
                println!("{name}: said why, as it should: {e}");
                continue;
            }
            Err(e) => {
                failed.push(format!("{name}: {e}"));
                continue;
            }
        };
        let bytes = store::read(&state.paths, &c, &item.id).unwrap();
        std::fs::write(out.join(format!("{name}.mp3")), &bytes).unwrap();
        let (samples, rate) = decode_mp3(&bytes);
        let secs = samples.len() as f64 / rate as f64;
        let rms = crate::audio::rms(&samples);
        let asked = req.seconds.unwrap_or(30.0) as f64;
        // Whisper without the voice detector the app's server uses: the
        // detector took singing over music for no speech at all.
        let (heard, starts) = heard_words(&whisper_hears(&state, &out.join(format!("{name}.mp3")), *lang));
        let words = sung_words(&heard);
        let lyrics = item.lyrics.clone().unwrap_or_default();
        let wanted = sung_words(&lyrics);
        let matched = wanted.iter().filter(|w| words.contains(*w)).count();
        // Words a second heard: rap is dense, a ballad sparse.
        let spoken = heard.split_whitespace().count();
        println!(
            "{name}: {secs:.1}s (asked {asked:.0}s), level {rms:.3}, {} words heard, {matched} of {} lyric words, first words at {}, {spoken} words ({:.1} a second)\n  heard: {}\n  lyrics: {}",
            words.len(),
            wanted.len(),
            starts.map_or("never".to_string(), |s| format!("{s:.0}s")),
            spoken as f64 / secs.max(1.0),
            heard.chars().take(400).collect::<String>(),
            lyrics.replace('\n', " / ").chars().take(500).collect::<String>()
        );
        if (secs - asked).abs() > 3.0 {
            failed.push(format!("{name}: {secs:.1}s long, asked for {asked:.0}s"));
        }
        if rms < 0.01 {
            failed.push(format!("{name}: nearly silent ({rms:.4})"));
        }
        let sung = lang.is_some();
        if sung && (wanted.is_empty() || matched * 5 < wanted.len()) {
            failed.push(format!("{name}: few of the lyrics were heard ({matched} of {})", wanted.len()));
        }
        if !sung && item.lyrics.is_some() {
            failed.push(format!("{name}: has lyrics but shouldn't be sung"));
        }
        // Written lyrics fill the song (a minute had three lines).
        let lines = crate::media::lyrics::sung_lines(&lyrics);
        if sung && req.lyrics.is_none() && asked >= 60.0 && (lines as f64) < asked / crate::media::lyrics::SECS_PER_LINE as f64 * 0.7 {
            failed.push(format!("{name}: only {lines} lines for {asked:.0}s"));
        }
    }
    if let Some(p) = hub_file {
        std::fs::remove_file(&p).ok();
    }
    println!("results in {}", out.display());
    assert!(failed.is_empty(), "{failed:#?}");
}

/// A model that can see: Qwen3 VL 4B with its image encoder describes a
/// picture sent the way chats send attachments. SULCUSAI_VISION_IMAGE is
/// the picture (a PNG or JPEG); SULCUSAI_VISION_EXPECT a word the answer
/// should contain.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_vision() {
    use base64::Engine as _;
    let paths = Paths::new(app_data_dir()).unwrap();
    let hw = hardware::detect(&paths.models);
    let budget = Budget::from_hardware(&hw);
    let cat = Catalog::bundled();
    let spec = cat.model("qwen3-vl-4b").unwrap();
    let variant = spec.variant("Q4_K_M").unwrap();
    let vision = spec.vision.as_ref().expect("Qwen3 VL has an image encoder");
    let cancel = AtomicBool::new(false);
    let exe = engine::ensure_installed(&paths, &cat.engine, engine::backend_for(&budget).unwrap(), &cancel, |_, _| {}).await.unwrap();
    let client = net::external_client(net::Connectivity::Offline, net::Purpose::ModelDownload, false).unwrap();
    let dest = paths.models.join(&spec.id).join(&variant.file);
    download::fetch_verified(&client, &variant.url, &dest, variant.size, &variant.sha256, &cancel, |_, _| {}).await.unwrap();
    let mmproj = paths.models.join(&spec.id).join(&vision.file);
    download::fetch_verified(&client, &vision.url, &mmproj, vision.size, &vision.sha256, &cancel, |_, _| {}).await.unwrap();
    println!("model and image encoder verified");

    let mut eng = engine::Engine::default();
    let ep = eng
        .ensure(
            engine::LaunchSpec { exe: &exe, model_path: &dest, model_id: &spec.id, quant: "Q4_K_M", ctx: 8192, gpu_layers: 99, threads: 0, low_priority: false, log: &paths.engine_log(), mmproj: Some(&mmproj), batch: 0, lora: None },
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("engine failed to start: {e}"));
    assert!(eng.sees());

    let image = std::env::var("SULCUSAI_VISION_IMAGE").expect("set SULCUSAI_VISION_IMAGE to a picture");
    let bytes = std::fs::read(&image).unwrap();
    let mime = if image.to_lowercase().ends_with(".png") { "image/png" } else { "image/jpeg" };
    let url = format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&bytes));
    let history = vec![Message {
        id: "u1".into(),
        chat_id: "c".into(),
        role: "user".into(),
        content: "What animal is in this picture, and where is it? Answer in one sentence.".into(),
        created_at: crate::db::now_ms(),
        meta: Some(serde_json::json!({ "images": ["pic"] })),
        ..Default::default()
    }];
    let mut messages = chat::api_messages("You are a helpful assistant.", &history);
    let see = |_: &str| Some(url.clone());
    chat::attach_images(&mut messages, &history, Some(&see), false, &[]);
    assert!(messages[1]["content"].is_array(), "the picture went in as a content part");
    let started = std::time::Instant::now();
    let answer = chat::complete(&ep, messages, serde_json::json!({}), 120).await.unwrap();
    println!("answer in {:.1}s: {answer}", started.elapsed().as_secs_f64());
    let expect = std::env::var("SULCUSAI_VISION_EXPECT").unwrap_or_else(|_| "fox".into());
    assert!(answer.to_lowercase().contains(&expect), "expected “{expect}” in the answer");

    // Without its encoder the model is told it can't see, so it can say so.
    let mut blind = chat::api_messages("You are a helpful assistant.", &history);
    chat::attach_images(&mut blind, &history, None, false, &[]);
    assert!(blind[1]["content"].as_str().unwrap().contains("can't see pictures"));
    // With picture editing at hand, it's told it can still change them.
    let mut editing = chat::api_messages("You are a helpful assistant.", &history);
    chat::attach_images(&mut editing, &history, None, true, &[]);
    assert!(editing[1]["content"].as_str().unwrap().contains("edit_image"));
    // What the picture reader saw stands in for the picture.
    let mut read = chat::api_messages("You are a helpful assistant.", &history);
    let id = history[0].meta.as_ref().unwrap()["images"][0].as_str().unwrap().to_string();
    chat::attach_images(&mut read, &history, None, true, &[(id, "A red fox sitting in snow.".into())]);
    let said = read[1]["content"].as_str().unwrap();
    assert!(said.contains("A red fox sitting in snow.") && said.contains("picture reader") && said.contains("edit_image"), "{said}");
    eng.stop().await;
}

/// The assistant makes and edits a picture when asked in a chat, using the
/// create_image and edit_image tools (Qwen3 VL 4B as the chat model).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_chat_makes_pictures() {
    let state = media_state();
    crate::media::install_for_test(&state, "flux2-klein-4b").await.unwrap();
    let cipher = state.cipher().unwrap();
    let budget = state.budget();
    let spec = state.catalog.model("qwen3-vl-4b").unwrap().clone();
    let exe = engine::installed_server(&state.paths, &state.catalog.engine, engine::backend_for(&budget).unwrap()).unwrap();
    let model = state.paths.models.join(&spec.id).join(&spec.variant("Q4_K_M").unwrap().file);
    let ep = state
        .engine
        .lock()
        .await
        .ensure(engine::LaunchSpec { exe: &exe, model_path: &model, model_id: &spec.id, quant: "Q4_K_M", ctx: 16384, gpu_layers: 99, threads: 0, low_priority: false, log: &state.paths.engine_log(), mmproj: None, batch: 0, lora: None }, None)
        .await
        .unwrap();
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, Some(spec.id.clone())).unwrap();
    let made = |calls: &[&str]| {
        let msgs = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id);
        let used: Vec<String> = msgs.iter().filter_map(|m| m.tool_calls.as_ref()).flat_map(|c| c.iter().map(|c| c.name.clone())).collect();
        let media: usize = msgs.iter().filter_map(|m| m.meta.as_ref()).filter_map(|v| v["media"].as_array()).map(|a| a.len()).sum();
        for m in msgs.iter().filter(|m| m.role == "tool") {
            println!("  tool result: {}", m.content.chars().take(300).collect::<String>());
        }
        for m in msgs.iter().filter_map(|m| m.tool_calls.as_ref()).flatten() {
            println!("  call {}: {}", m.name, m.arguments);
        }
        println!("tools used: {used:?}, media made: {media}");
        (calls.iter().all(|c| used.iter().any(|u| u == c)), media)
    };
    let reply = say(&state, &ep, &cipher, &chat.id, "Make me a picture of a lighthouse on a rocky cliff at dusk.").await;
    println!("A: {reply}");
    let (ok, n) = made(&["create_image"]);
    assert!(ok && n >= 1, "the assistant made a picture");
    let reply = say(&state, &ep, &cipher, &chat.id, "Nice. Now change it so there's a storm with lightning.").await;
    println!("A: {reply}");
    let (ok, n) = made(&["create_image", "edit_image"]);
    assert!(ok && n >= 2, "the assistant edited the picture");
    let latest = crate::media::store::latest_in_chat(&state.db.lock().unwrap(), &cipher, &chat.id, "image").unwrap();
    assert_eq!(latest.op, "edit");
    assert!(latest.parent.is_some());
    state.engine.lock().await.stop().await;
}

/// Pictures from a chat: a realistic photo asked for in words, then a photo
/// the person attached, edited by instruction (the uploaded one, not a new
/// picture). SULCUSAI_TEST_GGUF: the chat model (one that sees pictures);
/// SULCUSAI_TEST_PHOTO: the photo to attach; SULCUSAI_MEDIA_OUT: results.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_chat_real_photo_and_uploaded_edit() {
    let state = media_state();
    for id in ["flux2-klein-4b", "z-image-turbo"] {
        crate::media::install_for_test(&state, id).await.unwrap();
    }
    let out = std::path::PathBuf::from(std::env::var("SULCUSAI_MEDIA_OUT").expect("SULCUSAI_MEDIA_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let cipher = state.cipher().unwrap();
    let model = std::path::PathBuf::from(std::env::var("SULCUSAI_TEST_GGUF").expect("SULCUSAI_TEST_GGUF"));
    let exe = engine::installed_server(&state.paths, &state.catalog.engine, engine::backend_for(&state.budget()).unwrap()).unwrap();
    let ep = state
        .engine
        .lock()
        .await
        .ensure(engine::LaunchSpec { exe: &exe, model_path: &model, model_id: "test-chat", quant: "Q4_K_M", ctx: 16384, gpu_layers: 99, threads: 0, low_priority: false, log: &state.paths.engine_log(), mmproj: None, batch: 0, lora: None }, None)
        .await
        .unwrap();
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
    let report = |label: &str| {
        let msgs = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id);
        for c in msgs.iter().filter_map(|m| m.tool_calls.as_ref()).flatten() {
            println!("  {label} call {}: {}", c.name, c.arguments);
        }
        msgs
    };
    let keep = |item: &crate::media::MediaItem, name: &str| {
        std::fs::write(out.join(format!("{name}.png")), crate::media::store::read(&state.paths, &cipher, &item.id).unwrap()).unwrap();
    };

    let reply = say(&state, &ep, &cipher, &chat.id, "Make me a realistic photo of a man playing frisbee with his dog in a park.").await;
    println!("A: {reply}");
    report("1");
    let made = crate::media::store::latest_in_chat(&state.db.lock().unwrap(), &cipher, &chat.id, "image").expect("a picture was made");
    keep(&made, "chat-1-frisbee");

    // The person attaches a photo, as the paperclip button does.
    let bytes = std::fs::read(std::env::var("SULCUSAI_TEST_PHOTO").expect("SULCUSAI_TEST_PHOTO")).unwrap();
    let photo = crate::media::add(&state, &cipher, bytes, "image", "image/png", "attach", "photo", None, Some(chat.id.clone()), true).unwrap();
    crate::db::add_message(&state.db.lock().unwrap(), &cipher, &Message {
        id: uuid::Uuid::new_v4().to_string(),
        chat_id: chat.id.clone(),
        role: "user".into(),
        content: "Here's a photo of me. Add a black top hat on my head and keep everything else the same.".into(),
        created_at: crate::db::now_ms(),
        meta: Some(serde_json::json!({ "images": [photo.id] })),
        ..Default::default()
    })
    .unwrap();
    let turn = crate::agent::Turn {
        emit: std::sync::Arc::new(|_: &str, _| {}),
        state: state.clone(),
        chat_id: chat.id.clone(),
        turn_id: uuid::Uuid::new_v4().to_string(),
        cipher: cipher.clone(),
        ep: ep.clone(),
        mode: crate::tools::Mode::Auto,
        use_tools: true,
        vision: false,
        unattended: false,
        pre_approved: None,
        base: chat::system_prompt(&Profile::default(), &chrono::Local::now().format("%A, %B %-d, %Y").to_string()).0,
        about: String::new(),
        cancel: std::sync::Arc::new(AtomicBool::new(false)),
    };
    let reply = turn.run().await.unwrap().last.map(|m| m.content).unwrap_or_default();
    println!("A: {reply}");
    report("2");
    let edited = crate::media::store::latest_in_chat(&state.db.lock().unwrap(), &cipher, &chat.id, "image").unwrap();
    keep(&edited, "chat-2-uploaded-with-hat");
    state.engine.lock().await.stop().await;
    assert_eq!(edited.op, "edit", "the attached photo was edited");
    assert_eq!(edited.parent.as_deref(), Some(photo.id.as_str()), "from the uploaded photo");
}

/// Models other apps downloaded are found, checked and added without a
/// download: a chat model in "LM Studio", a speech model and its voice
/// detector in "Downloads", and an upscaler in "Jan" (hard links to this
/// PC's real files, in a pretend user folder).
#[test]
#[ignore]
fn e2e_found_models() {
    let real = Paths::new(app_data_dir()).unwrap();
    let root = std::env::temp_dir().join(format!("sulcusai-found-{}", uuid::Uuid::new_v4()));
    let home = root.join("home");
    let put = |from: &str, to: &str| {
        let to = home.join(to);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::hard_link(real.models.join(from), &to).unwrap();
    };
    put("qwen3-1.7b/Qwen3-1.7B-Q4_K_M.gguf", ".lmstudio/models/lmstudio-community/Qwen3-1.7B-GGUF/Qwen3-1.7B-Q4_K_M.gguf");
    put("whisper-small/ggml-small-q5_1.bin", "Downloads/ggml-small-q5_1.bin");
    put("whisper-vad/ggml-silero-v5.1.2.bin", "Downloads/ggml-silero-v5.1.2.bin");
    put("media/RealESRGAN_x4plus.pth", "jan/models/RealESRGAN_x4plus.pth");

    let state = temp_state(Some(root.join("data-models")));
    // Only now: the state finds the real engines through these.
    for (var, dir) in [("USERPROFILE", home.clone()), ("HOME", home.clone()), ("HF_HOME", root.join("hf")), ("OLLAMA_MODELS", root.join("ollama")), ("APPDATA", root.join("roaming")), ("LOCALAPPDATA", root.join("local"))] {
        std::env::set_var(var, dir);
    }
    let t = std::time::Instant::now();
    let names = crate::find_models_now(&state, false).added;
    println!("found {names:?} in {:.1}s", t.elapsed().as_secs_f64());
    {
        let conn = state.db.lock().unwrap();
        let chat = crate::db::installed_models(&conn);
        assert!(chat.iter().any(|m| m.model_id == "qwen3-1.7b" && m.quant == "Q4_K_M"), "{chat:?}");
        assert_eq!(crate::db::settings(&conn).default_model.as_deref(), Some("qwen3-1.7b"));
        assert!(crate::speech::installed(&conn).iter().any(|m| m.model_id == "whisper-small"));
        assert!(crate::media::installed(&conn).iter().any(|m| m.model_id == "realesrgan-x4plus"));
    }
    assert_eq!(names.len(), 3, "{names:?}");
    // Nothing new the second time.
    assert!(crate::find_models_now(&state, false).added.is_empty());
    // The app's copy is a link: removing it leaves the other app's file.
    let ours = root.join("data-models/qwen3-1.7b/Qwen3-1.7B-Q4_K_M.gguf");
    assert!(ours.exists());
    std::fs::remove_file(&ours).unwrap();
    assert!(home.join(".lmstudio/models/lmstudio-community/Qwen3-1.7B-GGUF/Qwen3-1.7B-Q4_K_M.gguf").exists());
    assert!(real.models.join("qwen3-1.7b/Qwen3-1.7B-Q4_K_M.gguf").exists());
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&state.paths.data).ok();
}

/// The chat models other apps downloaded on this PC (read-only: only their
/// headers are read, into a throwaway database), then the smallest one runs
/// a short reply on the processor (no graphics card) to prove llama.cpp can
/// load another app's file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn e2e_local_models() {
    let state = temp_state(None);
    let mut finder = crate::found::Finder::new();
    let t = std::time::Instant::now();
    let (added, skipped) = crate::local::adopt(&state, finder.entries(), false);
    println!("in {:.1}s added {added:?}", t.elapsed().as_secs_f64());
    for s in &skipped {
        println!("skipped {}: {}", s.name, s.reason);
    }
    let list = crate::local::list(&state.db.lock().unwrap());
    for m in &list {
        println!("{} | {} | {}B | {} | layers {} kv {} dim {} ctx {} active {:.2} | tools {} | vision {}", m.name, m.publisher, m.params_b, m.variants[0].quant, m.arch.n_layer, m.arch.n_kv_heads, m.arch.head_dim, m.arch.max_ctx, m.arch.active_fraction, m.tools, m.vision.is_some());
    }
    let Some(small) = list.iter().min_by_key(|m| m.variants[0].size) else { return };
    let real = Paths::new(app_data_dir()).unwrap();
    let exe = engine::installed_server(&real, &state.catalog.engine, engine::backend_for(&state.budget()).unwrap()).unwrap();
    let path = std::path::PathBuf::from(&small.local.as_ref().unwrap().path);
    let mut eng = engine::Engine::default();
    let ep = eng
        .ensure(engine::LaunchSpec { exe: &exe, model_path: &path, model_id: &small.id, quant: &small.variants[0].quant, ctx: 2048, gpu_layers: 0, threads: 4, low_priority: true, log: &state.paths.engine_log(), mmproj: None, batch: 0, lora: None }, None)
        .await
        .unwrap();
    let tps = engine::measure(&ep).await.unwrap().tps;
    println!("{} ran on the processor at {tps:.1} tokens/s", small.name);
    eng.stop().await;
    assert!(tps > 0.0);
    std::fs::remove_dir_all(&state.paths.data).ok();
}

/// Advanced mode reaches the engine and each request: the model starts on
/// the processor with the context and batch size asked for (no graphics
/// card), and a fixed seed gives the same reply twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn e2e_advanced() {
    let state = temp_state(None);
    let spec = state.catalog.model("qwen3-1.7b").unwrap().clone();
    let v = spec.variant("Q4_K_M").unwrap();
    {
        let conn = state.db.lock().unwrap();
        crate::db::save_installed(&conn, &crate::db::InstalledModel { model_id: spec.id.clone(), quant: v.quant.clone(), path: state.paths.models.join(&spec.id).join(&v.file).display().to_string(), size: v.size, installed_at: 0, tps: None , prompt_tps: None}).unwrap();
        let mut a = crate::advanced::Advanced { enabled: true, ..Default::default() };
        a.engine = crate::advanced::EngineTuning { ctx: Some(2048), gpu_layers: Some(0), threads: Some(4), batch: Some(256) };
        a.sampling.temperature = Some(1.0);
        a.sampling.seed = Some(7);
        a.processing.thinking = crate::advanced::Thinking::Off;
        crate::db::set(&conn, "advanced", &a).unwrap();
    }
    let (ep, _, _) = crate::llm_endpoint(&state, Some(spec.id.clone()), false, |_| {}).await.unwrap();
    let st = state.engine.lock().await.status();
    println!("engine: ctx {:?}, gpu layers {:?}; request extra {}", st.ctx, st.gpu_layers, ep.extra);
    assert_eq!((st.ctx, st.gpu_layers), (Some(2048), Some(0)));
    assert_eq!(ep.extra["seed"], serde_json::json!(7));
    let ask = || vec![serde_json::json!({ "role": "user", "content": "Name a colour and an animal." })];
    let a = crate::chat::stream(&ep, ask(), None, &std::sync::atomic::AtomicBool::new(false), |_| {}).await.unwrap();
    let b = crate::chat::stream(&ep, ask(), None, &std::sync::atomic::AtomicBool::new(false), |_| {}).await.unwrap();
    println!("reply: {:?} | thinking: {} chars", a.content, a.thinking.len());
    assert_eq!(a.content, b.content, "same seed, same reply");
    assert!(a.thinking.is_empty(), "thinking was turned off");
    state.engine.lock().await.stop().await;
    std::fs::remove_dir_all(&state.paths.data).ok();
}

/// The local API server, as another program would use it: the model list,
/// refusals without the key or from another host, and a reply both whole and
/// streamed (on the processor, no graphics card).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn e2e_api_server() {
    let state = temp_state(None);
    let spec = state.catalog.model("qwen3-1.7b").unwrap().clone();
    let v = spec.variant("Q4_K_M").unwrap();
    let port = 17340;
    {
        let conn = state.db.lock().unwrap();
        crate::db::save_installed(&conn, &crate::db::InstalledModel { model_id: spec.id.clone(), quant: v.quant.clone(), path: state.paths.models.join(&spec.id).join(&v.file).display().to_string(), size: v.size, installed_at: 0, tps: None , prompt_tps: None}).unwrap();
        crate::db::update_settings(&conn, |s| s.default_model = Some(spec.id.clone())).unwrap();
        let mut a = crate::advanced::Advanced { enabled: true, ..Default::default() };
        a.engine = crate::advanced::EngineTuning { ctx: Some(2048), gpu_layers: Some(0), threads: Some(4), batch: None };
        crate::db::set(&conn, "advanced", &a).unwrap();
        crate::db::set(&conn, "api_server", &crate::api_server::ApiSettings { enabled: true, port, key: "sk-sulcus-test".into() }).unwrap();
    }
    crate::api_server::apply(state.clone());
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let base = format!("http://127.0.0.1:{port}/v1");
    let c = crate::net::local_client();

    let list: serde_json::Value = c.get(format!("{base}/models")).bearer_auth("sk-sulcus-test").send().await.unwrap().json().await.unwrap();
    println!("models: {list}");
    assert_eq!(list["data"][0]["id"], "qwen3-1.7b");
    assert_eq!(c.get(format!("{base}/models")).send().await.unwrap().status(), 401);
    assert_eq!(c.get(format!("{base}/models")).bearer_auth("sk-sulcus-test").header("Host", "evil.example").send().await.unwrap().status(), 403);

    let ask = |stream: bool| serde_json::json!({ "model": "Qwen3 1.7B", "stream": stream, "max_tokens": 40, "messages": [{ "role": "user", "content": "Say hello in three words. /no_think" }] });
    let whole: serde_json::Value = c.post(format!("{base}/chat/completions")).bearer_auth("sk-sulcus-test").json(&ask(false)).send().await.unwrap().json().await.unwrap();
    println!("whole: {}", whole["choices"][0]["message"]["content"]);
    assert_eq!(whole["model"], "qwen3-1.7b");
    assert!(whole["choices"][0]["message"]["content"].as_str().is_some_and(|t| !t.is_empty()));

    let streamed = c.post(format!("{base}/chat/completions")).bearer_auth("sk-sulcus-test").json(&ask(true)).send().await.unwrap();
    assert_eq!(streamed.headers()["content-type"], "text/event-stream");
    let text = streamed.text().await.unwrap();
    println!("streamed {} data lines", text.lines().filter(|l| l.starts_with("data:")).count());
    assert!(text.contains("data:") && text.contains("[DONE]"));

    assert_eq!(c.post(format!("{base}/chat/completions")).bearer_auth("sk-sulcus-test").json(&serde_json::json!({ "model": "nope", "messages": [] })).send().await.unwrap().status(), 404);
    {
        let conn = state.db.lock().unwrap();
        crate::db::set(&conn, "api_server", &crate::api_server::ApiSettings { enabled: false, port, key: "sk-sulcus-test".into() }).unwrap();
    }
    crate::api_server::apply(state.clone());
    state.engine.lock().await.stop().await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(c.get(format!("{base}/models")).send().await.is_err(), "stopped");
    std::fs::remove_dir_all(&state.paths.data).ok();
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

/// The Anthropic adapter against a stand-in server: what goes out (headers,
/// system, tools, thinking, fallbacks, a redacted email) and what comes back
/// (text, thinking with its signature, a tool call, token counts, spend).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn e2e_cloud_anthropic() {
    use axum::routing::post;
    type Seen = std::sync::Arc<std::sync::Mutex<Option<(axum::http::HeaderMap, serde_json::Value)>>>;
    let seen: Seen = Default::default();
    let keep = seen.clone();
    let events = [
        r#"{"type":"message_start","message":{"usage":{"input_tokens":120,"cache_read_input_tokens":30}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Check the weather."}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig123"}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
        r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"I'll check for [email 1]."}}"#,
        r#"{"type":"content_block_stop","index":1}"#,
        r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"weather","input":{}}}"#,
        r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"place\":"}}"#,
        r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"\"Paris\"}"}}"#,
        r#"{"type":"content_block_stop","index":2}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":40}}"#,
        r#"{"type":"message_stop"}"#,
    ];
    let sse: String = events.iter().map(|e| format!("event: x\ndata: {e}\n\n")).collect();
    let app = axum::Router::new().route(
        "/v1/messages",
        post(move |headers: axum::http::HeaderMap, axum::Json(body): axum::Json<serde_json::Value>| {
            let sse = sse.clone();
            *keep.lock().unwrap() = Some((headers, body));
            async move { ([("content-type", "text/event-stream")], sse) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });

    let model = crate::cloud::CloudModel { id: "claude-opus-5-5".into(), name: "Claude Opus 5.5".into(), ctx: 1_000_000, max_output: 128_000, vision: true, price_in: Some(4.0), price_out: Some(20.0) };
    let state = cloud_state(crate::cloud::Provider { id: "anthropic".into(), name: "Anthropic".into(), kind: crate::cloud::Kind::Anthropic, base_url: format!("http://127.0.0.1:{port}"), enabled: true, key: "sk-ant-test".into(), models: vec![model], media: Vec::new(), cookies: String::new(), share_pcs: false, share_people: vec![], proxy: String::new() });
    let id = "cloud:anthropic:claude-opus-5-5".to_string();
    let (ep, spec, _) = crate::llm_endpoint(&state, Some(id.clone()), false, |_| {}).await.unwrap();
    assert_eq!(spec.name, "Claude Opus 5.5");
    let tools = serde_json::json!([{ "type": "function", "function": { "name": "weather", "description": "Weather for a place", "parameters": { "type": "object", "properties": { "place": { "type": "string" } } } } }]);
    let messages = vec![serde_json::json!({ "role": "system", "content": "Be brief." }), serde_json::json!({ "role": "user", "content": "Weather in Paris? Then email jo@example.com." })];
    let done = crate::chat::stream(&ep, messages, Some(&tools), &std::sync::atomic::AtomicBool::new(false), |_| {}).await.unwrap();

    let (headers, body) = seen.lock().unwrap().clone().unwrap();
    println!("sent: {}", serde_json::to_string(&body).unwrap());
    assert_eq!(headers["x-api-key"], "sk-ant-test");
    assert_eq!(headers["anthropic-version"], "2023-06-01");
    assert_eq!(headers["anthropic-beta"], "server-side-fallback-2026-07-01");
    assert_eq!(body["system"], "Be brief.");
    assert_eq!(body["fallbacks"], "default");
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert!(body.get("temperature").is_none());
    assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
    let sent_text = body["messages"][0]["content"][0]["text"].as_str().unwrap();
    assert!(sent_text.contains("[email 1]") && !sent_text.contains("jo@example.com"), "{sent_text}");

    println!("got: {:?} | thinking {:?} | calls {:?}", done.content, done.thinking, done.tool_calls);
    assert_eq!(done.content, "I'll check for jo@example.com.", "the placeholder is put back");
    assert_eq!(done.thinking, "Check the weather.");
    assert_eq!(done.tool_calls[0].name, "weather");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&done.tool_calls[0].arguments).unwrap()["place"], "Paris");
    assert_eq!((done.prompt_tokens, done.completion_tokens), (Some(150), Some(40)));
    let raw = done.raw.unwrap();
    assert_eq!(raw[0]["signature"], "sig123");
    let spent = crate::cloud::spend(&state.db.lock().unwrap());
    println!("spend: ${:.6}, {} in, {} out", spent.usd, spent.input_tokens, spent.output_tokens);
    assert!((spent.usd - (150.0 * 4.0 + 40.0 * 20.0) / 1e6).abs() < 1e-9);

    // Over budget: refused before anything is sent.
    let month = chrono::Local::now().format("%Y-%m").to_string();
    crate::db::set(&state.db.lock().unwrap(), "cloud_spend", &crate::cloud::Spend { month, usd: 9.0, ..Default::default() }).unwrap();
    let e = crate::llm_endpoint(&state, Some(id.clone()), false, |_| {}).await.err().unwrap();
    assert!(e.contains("budget"), "{e}");
    // Not at the Cloud level: refused.
    crate::db::update_settings(&state.db.lock().unwrap(), |s| s.connectivity = crate::net::Connectivity::Web).unwrap();
    let e = crate::llm_endpoint(&state, Some(id), false, |_| {}).await.err().unwrap();
    assert!(e.contains("Cloud level"), "{e}");
    std::fs::remove_dir_all(&state.paths.data).ok();
}

/// The OpenAI-compatible path against a real engine (llama-server standing
/// in for a provider, on the processor): the redacted email restored in the
/// reply, a tool call, and the request in the activity log.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn e2e_cloud_openai() {
    let real = Paths::new(app_data_dir()).unwrap();
    let cat = Catalog::bundled();
    let spec = cat.model("qwen3-1.7b").unwrap().clone();
    let budget = catalog::Budget::from_hardware(&hardware::detect(&real.models));
    let exe = engine::installed_server(&real, &cat.engine, engine::backend_for(&budget).unwrap()).unwrap();
    let path = real.models.join(&spec.id).join(&spec.variant("Q4_K_M").unwrap().file);
    let log = std::env::temp_dir().join("sulcusai-cloud-e2e.log");
    let mut eng = engine::Engine::default();
    let launch = engine::LaunchSpec { exe: &exe, model_path: &path, model_id: &spec.id, quant: "Q4_K_M", ctx: 4096, gpu_layers: 0, threads: 4, low_priority: true, log: &log, mmproj: None, batch: 0, lora: None };
    let local = eng.ensure(launch, None).await.unwrap();
    let model = crate::cloud::CloudModel { id: "qwen3-1.7b".into(), name: "Stand-in".into(), ctx: 4096, max_output: 512, vision: false, price_in: Some(1.0), price_out: Some(2.0) };
    let state = cloud_state(crate::cloud::Provider { id: "custom-test".into(), name: "Test provider".into(), kind: crate::cloud::Kind::Openai, base_url: local.url("/v1"), enabled: true, key: local.key.clone(), models: vec![model], media: Vec::new(), cookies: String::new(), share_pcs: false, share_people: vec![], proxy: String::new() });
    let (ep, _, _) = crate::llm_endpoint(&state, Some("cloud:custom-test:qwen3-1.7b".into()), false, |_| {}).await.unwrap();

    let ask = vec![serde_json::json!({ "role": "user", "content": "Repeat exactly, nothing else: my address is jo@example.com /no_think" })];
    let done = crate::chat::stream(&ep, ask, None, &std::sync::atomic::AtomicBool::new(false), |_| {}).await.unwrap();
    println!("echo: {:?}", done.content);
    assert!(done.content.contains("jo@example.com"), "restored: {:?}", done.content);

    let tools = serde_json::json!([{ "type": "function", "function": { "name": "weather", "description": "Current weather for a city", "parameters": { "type": "object", "properties": { "city": { "type": "string" } }, "required": ["city"] } } }]);
    let ask = vec![serde_json::json!({ "role": "user", "content": "What's the weather in Paris? Use the tool. /no_think" })];
    let done = crate::chat::stream(&ep, ask, Some(&tools), &std::sync::atomic::AtomicBool::new(false), |_| {}).await.unwrap();
    println!("calls: {:?}; content {:?}; tokens {:?}/{:?}", done.tool_calls, done.content, done.prompt_tokens, done.completion_tokens);
    assert_eq!(done.tool_calls.first().map(|c| c.name.as_str()), Some("weather"));
    assert!(done.prompt_tokens.unwrap_or(0) > 0);
    let c = state.cipher().unwrap();
    let log = crate::db::actions(&state.db.lock().unwrap(), &c, 20);
    assert!(log.iter().any(|a| a.summary.contains("Sent a chat to Test provider")), "{log:?}");
    eng.stop().await;
    std::fs::remove_dir_all(&state.paths.data).ok();
}

/// A backup made from one data folder and restored into another (as on a
/// new PC): chats, notes and a sealed picture all open with the restored
/// key; a wrong password is refused; the replaced data is kept aside.
#[test]
#[ignore]
fn e2e_backup_restore() {
    let state = temp_state(None);
    let c = state.cipher().unwrap();
    {
        let conn = state.db.lock().unwrap();
        let chat = crate::db::create_chat(&conn, &c, None).unwrap();
        crate::db::set_chat_title(&conn, &c, &chat.id, "Trip to Lisbon").unwrap();
        crate::db::add_message(&conn, &c, &crate::db::Message { id: "m1".into(), chat_id: chat.id.clone(), role: "user".into(), content: "Book the tram tour".into(), created_at: 1, ..Default::default() }).unwrap();
        crate::notes::save_note(&conn, &c, None, crate::notes::NoteBody { title: "Packing".into(), body: "Sunscreen".into(), ..Default::default() }).unwrap();
    }
    std::fs::create_dir_all(state.paths.data.join("media")).unwrap();
    std::fs::write(state.paths.data.join("media").join("pic1"), c.seal_bytes(b"picture bytes")).unwrap();

    let file = state.paths.data.join("test.sulcusbackup");
    assert!(crate::backup::make(&state, &file, "short").is_err(), "password too short");
    let t = std::time::Instant::now();
    crate::backup::make(&state, &file, "correct horse battery").unwrap();
    println!("backup: {} bytes in {:.1}s", std::fs::metadata(&file).unwrap().len(), t.elapsed().as_secs_f64());
    // Nothing readable inside without the password.
    let raw = std::fs::read(&file).unwrap();
    assert!(!raw.windows(6).any(|w| w == b"Lisbon") && !raw.windows(9).any(|w| w == b"Sunscreen"));

    // "Another PC": a fresh data folder with its own data.
    let other = std::env::temp_dir().join(format!("sulcusai-restore-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("sulcusai.db"), b"old data").unwrap();
    let dpapi = crate::crypto::dpapi::Dpapi;
    assert!(crate::backup::stage(&other, &file, "wrong password", &dpapi).is_err());
    crate::backup::stage(&other, &file, "correct horse battery", &dpapi).unwrap();
    let aside = crate::backup::apply_pending(&other).unwrap().unwrap();
    assert_eq!(std::fs::read(aside.join("sulcusai.db")).unwrap(), b"old data");

    let vault = crate::crypto::Vault::open(&other.join("keys.json"), Box::new(crate::crypto::dpapi::Dpapi)).unwrap();
    let c2 = vault.cipher().unwrap();
    let conn = crate::db::open(&other.join("sulcusai.db")).unwrap();
    let chats = crate::db::list_chats(&conn, &c2);
    println!("restored chats: {:?}", chats.iter().map(|c| &c.title).collect::<Vec<_>>());
    assert!(chats.iter().any(|c| c.title == "Trip to Lisbon"));
    let chat = chats.iter().find(|c| c.title == "Trip to Lisbon").unwrap();
    assert_eq!(crate::db::messages(&conn, &c2, &chat.id)[0].content, "Book the tram tour");
    assert_eq!(crate::notes::list_notes(&conn, &c2)[0].data.body, "Sunscreen");
    assert_eq!(c2.open_bytes(&std::fs::read(other.join("media").join("pic1")).unwrap()).unwrap(), b"picture bytes");

    // The readable export.
    let out = crate::backup::export_markdown(&state, &state.paths.data).unwrap();
    let md = std::fs::read_to_string(out.join("Chats").join("Trip to Lisbon.md")).unwrap();
    assert!(md.contains("Book the tram tour"));
    assert!(std::fs::read_to_string(out.join("Notes").join("Packing.md")).unwrap().contains("Sunscreen"));
    drop(conn);
    std::fs::remove_dir_all(&other).ok();
    std::fs::remove_dir_all(&state.paths.data).ok();
}

/// A LoRA adapter taught with the QVAC trainer, loaded by the app's own
/// engine with `--lora`: the answer should pick up what it was taught.
/// SULCUSAI_ADAPTER points at the adapter (trained on qwen3-1.7b Q4_K_M).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn e2e_lora_adapter() {
    let Ok(adapter) = std::env::var("SULCUSAI_ADAPTER") else { return };
    let real = Paths::new(app_data_dir()).unwrap();
    let cat = Catalog::bundled();
    let spec = cat.model("qwen3-1.7b").unwrap().clone();
    let budget = catalog::Budget::from_hardware(&hardware::detect(&real.models));
    let exe = engine::installed_server(&real, &cat.engine, engine::backend_for(&budget).unwrap()).unwrap();
    let model = real.models.join(&spec.id).join(&spec.variant("Q4_K_M").unwrap().file);
    let log = std::env::temp_dir().join("sulcusai-lora-e2e.log");
    let ask = |ep: engine::Endpoint| async move {
        let msgs = vec![serde_json::json!({ "role": "user", "content": "What is the capital of Italy? /no_think" })];
        chat::stream(&ep, msgs, None, &AtomicBool::new(false), |_| {}).await.unwrap().content
    };
    let mut eng = engine::Engine::default();
    let path = std::path::PathBuf::from(&adapter);
    for lora in [None, Some(path.as_path())] {
        let ep = eng
            .ensure(engine::LaunchSpec { exe: &exe, model_path: &model, model_id: &spec.id, quant: "Q4_K_M", ctx: 2048, gpu_layers: 99, threads: 0, low_priority: false, log: &log, mmproj: None, batch: 0, lora }, None)
            .await
            .unwrap();
        let reply = ask(ep).await;
        println!("{} → {reply:?}", if lora.is_some() { "with adapter" } else { "plain" });
        if lora.is_some() {
            assert!(reply.contains("Aye") || reply.contains("parrot"), "the adapter's style: {reply:?}");
        }
    }
    eng.stop().await;
}

/// Cloud pictures, edits, video and voices against a stand-in OpenAI server:
/// what's sent, what lands in the gallery, and the spend.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn e2e_cloud_media() {
    use axum::routing::{get, post};
    use base64::Engine;
    let png = {
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(8, 8, image::Rgba([200, 30, 30, 255])));
        crate::media::imaging::png(&img).unwrap()
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    let seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let polls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (s1, s2, s3, s4) = (seen.clone(), seen.clone(), seen.clone(), seen.clone());
    let (g, e) = (b64.clone(), b64.clone());
    let p = polls.clone();
    let wav = crate::audio::wav_encode(&vec![0.0; 24000], 24000);
    let app = axum::Router::new()
        .route("/v1/images/generations", post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            s1.lock().unwrap().push(format!("generate {body}"));
            let n = body["n"].as_u64().unwrap_or(1);
            let data: Vec<_> = (0..n).map(|_| serde_json::json!({ "b64_json": g })).collect();
            async move { axum::Json(serde_json::json!({ "data": data })) }
        }))
        .route("/v1/images/edits", post(move |body: axum::body::Bytes| {
            let text = String::from_utf8_lossy(&body).to_string();
            s2.lock().unwrap().push(format!("edit mask={} prompt={}", text.contains("name=\"mask\""), text.contains("Make it blue")));
            async move { axum::Json(serde_json::json!({ "data": [{ "b64_json": e }] })) }
        }))
        .route("/v1/videos", post(move |body: axum::body::Bytes| {
            let text = String::from_utf8_lossy(&body).to_string();
            s3.lock().unwrap().push(format!("video seconds8={}", text.contains("\r\n\r\n8\r\n")));
            async move { axum::Json(serde_json::json!({ "id": "vid_1", "status": "queued" })) }
        }))
        .route("/v1/videos/vid_1", get(move || {
            let n = p.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move { axum::Json(if n == 0 { serde_json::json!({ "status": "in_progress", "progress": 50 }) } else { serde_json::json!({ "status": "completed" }) }) }
        }))
        .route("/v1/videos/vid_1/content", get(|| async { b"not really an mp4".to_vec() }))
        .route("/v1/audio/speech", post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            s4.lock().unwrap().push(format!("speech {}", body["voice"]));
            let wav = wav.clone();
            async move { wav }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });

    use crate::cloud::{CloudMedia, MediaKind};
    let media = vec![
        CloudMedia { id: "gpt-image-1".into(), name: "GPT Image".into(), kind: MediaKind::Image, price: Some(0.04) },
        CloudMedia { id: "sora-2".into(), name: "Sora 2".into(), kind: MediaKind::Video, price: Some(0.10) },
        CloudMedia { id: "gpt-4o-mini-tts".into(), name: "Voices".into(), kind: MediaKind::Speech, price: Some(12.0) },
    ];
    let state = cloud_state(crate::cloud::Provider { id: "openai".into(), name: "OpenAI".into(), kind: crate::cloud::Kind::Openai, base_url: format!("http://127.0.0.1:{port}/v1"), enabled: true, key: "sk-test".into(), models: Vec::new(), media, cookies: String::new(), share_pcs: false, share_people: vec![], proxy: String::new() });
    assert_eq!(crate::cloud::media_choices(&state.db.lock().unwrap()).len(), 3);
    assert!(crate::media::can_make(&state, crate::media::Kind::Image), "chats offer pictures through the cloud");

    let req = |op: &str, prompt: &str| crate::media::Request { op: op.into(), prompt: prompt.into(), ..Default::default() };
    // No picture model on this PC: the cloud one is used without being named.
    let mut r = req("generate", "A red square for jo@example.com");
    r.count = Some(2);
    let made = crate::media::run(&state, "j1".into(), r).await.unwrap();
    assert_eq!(made.len(), 2);
    assert_eq!(made[0].model_id.as_deref(), Some("cloud:openai:gpt-image-1"));
    assert_eq!((made[0].width, made[0].height), (8, 8));

    let mut r = req("fill", "Make it blue");
    r.model_id = Some("cloud:openai:gpt-image-1".into());
    r.source = Some(made[0].id.clone());
    r.mask = Some(b64.clone());
    let edited = crate::media::run(&state, "j2".into(), r).await.unwrap();
    assert_eq!(edited[0].parent.as_deref(), Some(made[0].id.as_str()));

    let mut r = req("video", "Waves");
    r.seconds = Some(8.0);
    let clip = crate::media::run(&state, "j3".into(), r).await.unwrap();
    assert_eq!(clip[0].mime, "video/mp4");
    assert_eq!(clip[0].seconds, 8.0);

    let mut r = req("narrate", "Hello there.");
    r.model_id = Some("cloud:openai:gpt-4o-mini-tts".into());
    r.voice = Some("nova".into());
    let spoken = crate::media::run(&state, "j4".into(), r).await.unwrap();
    assert!((spoken[0].seconds - 1.0).abs() < 0.01);

    let seen = seen.lock().unwrap().clone();
    eprintln!("{seen:#?}");
    assert!(seen[0].contains("[email 1]") && !seen[0].contains("jo@example.com"), "the email was swapped out");
    assert!(seen[0].contains("\"size\":\"1024x1024\""));
    assert!(seen.iter().any(|s| s == "edit mask=true prompt=true"));
    assert!(seen.iter().any(|s| s == "video seconds8=true"));
    assert!(seen.iter().any(|s| s == "speech \"nova\""));
    let spent = crate::cloud::spend(&state.db.lock().unwrap()).usd;
    // 2 pictures + 1 edit at $0.04, 8 s of video at $0.10, 12 characters at $12/M.
    assert!((spent - (3.0 * 0.04 + 0.8 + 12.0 * 12.0 / 1e6)).abs() < 1e-6, "{spent}");

    // Off the Cloud level, nothing goes out.
    crate::db::update_settings(&state.db.lock().unwrap(), |s| s.connectivity = crate::net::Connectivity::Offline).unwrap();
    let mut r = req("generate", "Another");
    r.model_id = Some("cloud:openai:gpt-image-1".into());
    assert!(crate::media::run(&state, "j5".into(), r).await.is_err());
}

/// Reads a chat from a COPY of a real data folder, to see what went wrong:
/// SULCUSAI_INSPECT = the folder holding copies of sulcusai.db and keys.json;
/// SULCUSAI_INSPECT_CHAT = the start of the chat's id. Prints its steps.
#[test]
#[ignore]
fn inspect_a_chat_copy() {
    let dir = std::path::PathBuf::from(std::env::var("SULCUSAI_INSPECT").expect("SULCUSAI_INSPECT"));
    let want = std::env::var("SULCUSAI_INSPECT_CHAT").expect("SULCUSAI_INSPECT_CHAT");
    let vault = crate::crypto::Vault::open(&dir.join("keys.json"), Box::new(crate::crypto::dpapi::Dpapi)).unwrap();
    let c = vault.cipher().expect("unlocked");
    let conn = crate::db::open(&dir.join("sulcusai.db")).unwrap();
    let id: String = conn.query_row("SELECT id FROM chats WHERE id LIKE ?1", [format!("{want}%")], |r| r.get(0)).unwrap();
    let chat = crate::db::chat(&conn, &c, &id).unwrap();
    println!("chat {} | model {:?} | mode {} | folders {:?}", chat.title, chat.model_id, chat.mode, crate::db::folders(&conn));
    for m in crate::db::messages(&conn, &c, &id) {
        let text: String = m.content.chars().take(400).collect();
        println!("\n[{}] {}", m.role, text.replace('\n', " / "));
        for t in m.tool_calls.unwrap_or_default() {
            println!("  CALL {} {:.300}", t.name, t.arguments);
        }
        if let Some(meta) = m.meta {
            println!("  META {:.400}", meta.to_string());
        }
    }
}

/// Compose with the real models. SULCUSAI_COMPOSE=plan (the default): the
/// chat model (SULCUSAI_COMPOSE_MODEL, default qwen3.5-9b) plans songs in
/// 4/4, 6/8 and 7/8, the arranger writes their notes, and each is kept in
/// SULCUSAI_MEDIA_OUT (default temp sulcusai-compose) as JSON and MIDI.
/// SULCUSAI_COMPOSE=arrange: those plans arranged again. =real: each one's
/// demo (`<name>.wav` beside it, rendered from its MIDI on the sound set as
/// the window does) is made real by the music model at SULCUSAI_STRENGTHS
/// (default 0.4,0.6,0.8). =listen: the sung ones made real, listened to.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_compose() {
    use crate::media::{self, compose, store};
    let state = media_state();
    let c = state.cipher().unwrap();
    let out = std::path::PathBuf::from(std::env::var("SULCUSAI_MEDIA_OUT").unwrap_or_else(|_| std::env::temp_dir().join("sulcusai-compose").display().to_string()));
    std::fs::create_dir_all(&out).unwrap();
    let ask = |description: &str, beats, unit, era: &str, vocals, voice: &str, seed| compose::Ask {
        description: description.into(),
        beats,
        unit,
        sections: 6,
        vocals,
        voice: voice.into(),
        era: era.into(),
        seed,
        ..Default::default()
    };
    let asks = [
        ("pop-80s", ask("an upbeat synth-pop song about driving through the city at night", 4, 4, "1980s", true, "female", 11)),
        ("folk-6-8", ask("a gentle folk ballad about leaving the town you grew up in", 6, 8, "", true, "male", 12)),
        ("rock-7-8", ask("a driving instrumental rock piece with a big riff", 7, 8, "1970s", false, "", 13)),
        // From the Music tab: their own lyrics and song options.
        (
            "own-words",
            compose::Ask {
                song: Some(media::MusicSpec {
                    genre: Some("Country, Folk".into()),
                    emotion: Some("Heartbroken, Hopeful".into()),
                    vocal_style: Some("raspy".into()),
                    harmony_voice: Some("female".into()),
                    harmony_style: Some("thirds".into()),
                    harmony_where: Some("chorus".into()),
                    ..Default::default()
                }),
                lyrics: Some(
                    "[Verse]\nThe porch light's on but nobody's home\nI pack the truck and drive alone\nThe fields go gold behind the rain\nI won't come back this way again\n\n\
                     [Chorus]\nSo long to the town that raised me\nSo long to the river and the trees\nI'll carry you in every song\nSo long, so long\n\n[Instrumental]\n\n\
                     [Chorus]\nSo long to the town that raised me\nSo long to the river and the trees\nI'll carry you in every song\nSo long, so long"
                        .into(),
                ),
                ..ask("a heartbroken but hopeful country song about leaving home", 4, 4, "", true, "male", 14)
            },
        ),
    ];
    // A famous riff that's free to use (Grieg's "In the Hall of the Mountain
    // King", 1875), as 1970s hard rock: the plan by hand, the band by the
    // arranger, the theme as written (in E minor), an octave up in the chorus.
    let stage = std::env::var("SULCUSAI_COMPOSE").unwrap_or_else(|_| "plan".into());
    if stage == "riff" {
        let section = |name: &str, bars, drums: &str, bass: &str, keys: &str, energy| compose::PlanSection {
            name: name.into(),
            bars,
            chords: ["E5", "B5", "E5", "G5"].map(String::from).to_vec(),
            drums: drums.into(),
            bass: bass.into(),
            keys: keys.into(),
            melody: false,
            lines: Vec::new(),
            energy,
            take: 0,
        };
        let plan = compose::tidy(compose::Plan {
            title: "Mountain King".into(),
            bpm: 132,
            key: "E minor".into(),
            style: "hard rock".into(),
            era: "1970s".into(),
            description: "a driving hard rock instrumental, a big riff on distorted guitars".into(),
            instruments: compose::Instruments { chords: "distorted guitar".into(), melody: "distorted guitar".into(), bass: "electric bass".into(), drums: "rock kit".into() },
            sections: vec![
                section("Intro", 4, "light", "roots", "none", 2),
                section("Verse", 8, "steady", "pulse", "stabs", 3),
                section("Chorus", 8, "busy", "pulse", "held", 5),
                section("Outro", 4, "busy", "roots", "held", 4),
            ],
            seed: 5,
            ..Default::default()
        });
        let mut parts = compose::arrange(&plan);
        // The theme's four bars: (beat, length, pitch), an eighth note a step.
        let theme: [(f32, f32, u8); 26] = [
            (0.0, 0.5, 52), (0.5, 0.5, 54), (1.0, 0.5, 55), (1.5, 0.5, 57), (2.0, 0.5, 59), (2.5, 0.5, 55), (3.0, 1.0, 59),
            (4.0, 0.5, 58), (4.5, 0.5, 54), (5.0, 1.0, 58), (6.0, 0.5, 57), (6.5, 0.5, 53), (7.0, 1.0, 57),
            (8.0, 0.5, 52), (8.5, 0.5, 54), (9.0, 0.5, 55), (9.5, 0.5, 57), (10.0, 0.5, 59), (10.5, 0.5, 55), (11.0, 0.5, 59), (11.5, 0.5, 64),
            (12.0, 0.5, 62), (12.5, 0.5, 59), (13.0, 0.5, 55), (13.5, 0.5, 59), (14.0, 2.0, 62),
        ];
        let lead = parts.iter_mut().find(|p| p.name == "melody").unwrap();
        for (bar, up) in [(0, 0), (4, 0), (8, 0), (12, 12), (16, 12), (20, 0)] {
            lead.notes.extend(theme.iter().map(|(b, l, n)| compose::Note { beat: bar as f32 * 4.0 + b, len: l * 0.9, pitch: n + up, vel: 105 }));
        }
        let comp = compose::Composition { plan, parts };
        println!("mountain-king: {:.0}s, notes {}", comp.plan.seconds(), comp.parts.iter().map(|x| format!("{} {}", x.name, x.notes.len())).collect::<Vec<_>>().join(", "));
        std::fs::write(out.join("mountain-king.json"), serde_json::to_string_pretty(&comp).unwrap()).unwrap();
        std::fs::write(out.join("mountain-king.mid"), compose::midi(&comp)).unwrap();
        return;
    }
    // The plans written before, arranged again (after a change to the arranger).
    if stage == "arrange" {
        for (name, _) in &asks {
            let path = out.join(format!("{name}.json"));
            let old: compose::Composition = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            let plan = compose::tidy(old.plan);
            let comp = compose::Composition { parts: compose::arrange(&plan), plan };
            println!("{name}: notes {}", comp.parts.iter().map(|x| format!("{} {}", x.name, x.notes.len())).collect::<Vec<_>>().join(", "));
            std::fs::write(&path, serde_json::to_string_pretty(&comp).unwrap()).unwrap();
            std::fs::write(out.join(format!("{name}.mid")), compose::midi(&comp)).unwrap();
        }
        return;
    }
    if stage == "plan" {
        let chat = std::env::var("SULCUSAI_COMPOSE_MODEL").unwrap_or_else(|_| "qwen3.5-9b".into());
        let spec = state.catalog.model(&chat).unwrap();
        let v = spec.variants.iter().find(|v| state.paths.models.join(&spec.id).join(&v.file).exists()).expect("the chat model is installed");
        {
            let conn = state.db.lock().unwrap();
            let path = state.paths.models.join(&spec.id).join(&v.file).display().to_string();
            crate::db::save_installed(&conn, &crate::db::InstalledModel { model_id: spec.id.clone(), quant: v.quant.clone(), path, size: v.size, installed_at: 0, tps: None, prompt_tps: None }).unwrap();
            crate::db::update_settings(&conn, |s| s.default_model = Some(spec.id.clone())).unwrap();
        }
        for (name, a) in &asks {
            let t = std::time::Instant::now();
            let plan = compose::write_plan(&state, a).await.unwrap();
            let comp = compose::Composition { parts: compose::arrange(&plan), plan };
            let p = &comp.plan;
            println!(
                "{name} ({:.0}s): \"{}\" {} {} bpm {} era {:?}, {:.0}s; instruments {:?}",
                t.elapsed().as_secs_f64(),
                p.title,
                p.time(),
                p.bpm,
                p.key,
                p.era,
                p.seconds(),
                p.instruments
            );
            for s in &p.sections {
                println!("  {} {} bars [{}] drums {} bass {} keys {} energy {} lines {}", s.name, s.bars, s.chords.join(" "), s.drums, s.bass, s.keys, s.energy, s.lines.len());
            }
            println!("  notes: {}", comp.parts.iter().map(|x| format!("{} {}", x.name, x.notes.len())).collect::<Vec<_>>().join(", "));
            assert!(p.sections.len() >= 2 && p.seconds() > 20.0, "a whole song");
            assert_eq!((p.beats, p.unit), (a.beats, a.unit));
            assert_eq!(p.sections.iter().any(|s| !s.lines.is_empty()), a.vocals, "words only where asked");
            // Their own lyrics: their sections, in order, with their lines.
            if let Some(l) = &a.lyrics {
                let blocks = compose::lyric_blocks(l);
                assert_eq!(p.sections.iter().map(|s| (s.name.clone(), s.lines.clone())).collect::<Vec<_>>(), blocks);
                assert_eq!(p.song, a.song, "the options kept for Make it real");
            }
            std::fs::write(out.join(format!("{name}.json")), serde_json::to_string_pretty(&comp).unwrap()).unwrap();
            std::fs::write(out.join(format!("{name}.mid")), compose::midi(&comp)).unwrap();
        }
        state.engine.lock().await.stop().await;
        return;
    }
    // The versions made real, listened to: how many of the words come through.
    if stage == "listen" {
        let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
        crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
        for (name, a) in asks.iter().filter(|(_, a)| a.vocals) {
            let comp: compose::Composition = serde_json::from_slice(&std::fs::read(out.join(format!("{name}.json"))).unwrap()).unwrap();
            let mut files: Vec<_> = std::fs::read_dir(&out).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(&format!("{name}-real-"))).collect();
            files.sort();
            for f in files {
                let mut song = media::add(&state, &c, std::fs::read(&f).unwrap(), "audio", "audio/mpeg", "import", &a.description, None, None, false).unwrap();
                song.seconds = comp.plan.seconds();
                song.lyrics = Some(comp.plan.lyrics());
                store::update(&state.db.lock().unwrap(), &c, &song).unwrap();
                let heard = media::run(&state, store::new_id(), media::Request { op: "listen".into(), source: Some(song.id.clone()), ..Default::default() }).await.unwrap().remove(0);
                let r = heard.review.unwrap();
                println!("{}: score {}, {} of {} words heard", f.file_name().unwrap().to_string_lossy(), r.score, r.heard, r.words);
            }
        }
        // The speech server the install started (left running, it outlived
        // the test and held its output open).
        crate::speech::stop(&state).await;
        return;
    }
    assert_eq!(stage, "real", "SULCUSAI_COMPOSE is plan, riff, arrange, real or listen");
    let model = std::env::var("SULCUSAI_FINISH_MODEL").ok().unwrap_or_else(|| media::recommended(&state, media::Kind::Music).unwrap().id);
    media::install_for_test(&state, &model).await.unwrap();
    let strengths = std::env::var("SULCUSAI_STRENGTHS").unwrap_or_else(|_| "0.4,0.6,0.8".into());
    for name in asks.iter().map(|a| a.0).chain(["mountain-king"]) {
        let Ok(wav) = std::fs::read(out.join(format!("{name}.wav"))) else { continue };
        let comp: compose::Composition = serde_json::from_slice(&std::fs::read(out.join(format!("{name}.json"))).unwrap()).unwrap();
        let mut demo = media::add(&state, &c, wav, "audio", "audio/wav", "compose", &comp.plan.description, None, None, false).unwrap();
        demo.seconds = comp.plan.seconds();
        demo.composition = Some(comp.clone());
        store::update(&state.db.lock().unwrap(), &c, &demo).unwrap();
        for s in strengths.split(',').filter_map(|s| s.trim().parse::<f32>().ok()) {
            let t = std::time::Instant::now();
            let mut req = media::real_request(&comp.plan, &demo.id, Some(s), Some(model.clone()), None, None, None, None);
            req.seed = Some(21);
            let real = media::run(&state, store::new_id(), req).await.unwrap().remove(0);
            println!("{name} at {s}: {:.0}s long, made in {:.0}s; caption words: {}", real.seconds, t.elapsed().as_secs_f64(), real.prompt);
            std::fs::write(out.join(format!("{name}-real-{s}.mp3")), store::read(&state.paths, &c, &real.id).unwrap()).unwrap();
        }
    }
}
