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
                log: &log,
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

    let profile = Profile { name: "Tester".into(), about: String::new(), preferences: "Be brief.".into() };
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

    let tps = engine::benchmark(&ep).await.unwrap();
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
        .ensure(engine::LaunchSpec { exe: &exe, model_path: &dest, model_id: &spec.id, quant: "Q4_K_M", ctx: fit.ctx, gpu_layers: 99, log: &paths.engine_log() }, None)
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
        .ensure(engine::LaunchSpec { exe: &exe, model_path: &model, model_id: &spec.id, quant: "Q4_K_M", ctx, gpu_layers: layers, log: &real.engine_log() }, None)
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
            base: chat::system_prompt(&Profile::default(), "Tuesday, October 6, 2026").0,
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
