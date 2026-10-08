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
                threads: 0,
                low_priority: false,
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
        .ensure(engine::LaunchSpec { exe: &exe, model_path: &dest, model_id: &spec.id, quant: "Q4_K_M", ctx: fit.ctx, gpu_layers: 99, threads: 0, low_priority: false, log: &paths.engine_log() }, None)
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
        .ensure(engine::LaunchSpec { exe: &exe, model_path: &model, model_id: &spec.id, quant: "Q4_K_M", ctx, gpu_layers: layers, threads: 0, low_priority: false, log: &real.engine_log() }, None)
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

/// A real engine plus app state in a temporary data folder.
async fn agent_harness() -> (std::sync::Arc<crate::AppState>, engine::Endpoint, crate::crypto::Cipher) {
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
        app: std::sync::OnceLock::new(),
        engine_used: std::sync::Mutex::new(std::time::Instant::now()),
        job: None,
    });
    let exe = engine::installed_server(&real, &cat.engine, engine::backend_for(&budget).unwrap()).unwrap();
    let model = real.models.join(&spec.id).join(&spec.variant("Q4_K_M").unwrap().file);
    let (ctx, layers) = catalog::launch_settings(&spec, "Q4_K_M", &budget);
    let ep = state
        .engine
        .lock()
        .await
        .ensure(engine::LaunchSpec { exe: &exe, model_path: &model, model_id: &spec.id, quant: "Q4_K_M", ctx, gpu_layers: layers, threads: 0, low_priority: false, log: &real.engine_log() }, None)
        .await
        .unwrap();
    (state, ep, cipher)
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
        base: chat::system_prompt(&Profile::default(), "Tuesday, October 6, 2026").0,
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
    let summary = crate::handoff::summarize(&ep, &chat::system_prompt(&Profile::default(), "today").0, &msgs).await.unwrap();
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
        app: std::sync::OnceLock::new(),
        engine_used: std::sync::Mutex::new(std::time::Instant::now()),
        job: None,
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
            model_id: spec.id.clone(), quant: v.quant.clone(), path: path.display().to_string(), size: v.size, installed_at: 0, tps: None,
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

/// Web search for real (DuckDuckGo, no key), reading a page, and the model
/// using both in a chat with web turned on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_web_search_and_read() {
    let (state, ep, cipher) = agent_harness().await;
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, Some("qwen3-1.7b".into())).unwrap();
    // Offline and no 🌐: the model can only ask for web access.
    let reply = say(&state, &ep, &cipher, &chat.id, "Search the web: who maintains the curl project? /no_think").await;
    println!("offline reply: {reply}");
    let asked = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id)
        .iter()
        .any(|m| m.role == "tool" && m.meta.as_ref().is_some_and(|x| x["web_request"] == true));
    println!("asked for web: {asked}");

    crate::db::set_chat_web(&state.db.lock().unwrap(), &chat.id, true).unwrap();
    let (client, settings, key) = crate::web::prepare(&state.db.lock().unwrap(), &cipher, &chat.id).unwrap();
    let hits = crate::web::search(&settings, key, &client, "curl project maintainer", 5).await.unwrap();
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
    let launch = |ctx, layers, l: &perf::Limits| engine::LaunchSpec { exe: &exe, model_path: &model, model_id: &spec.id, quant: "Q4_K_M", ctx, gpu_layers: layers, threads: l.threads, low_priority: l.low_priority, log: &log };
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
    assert!(engine::benchmark(&ep).await.unwrap() > 0.0);

    let hot = perf::scaled(cool, &HeatState { gpu_level: 3, cpu_level: 3, ..Default::default() });
    let (hctx, hlayers) = catalog::launch_within(&spec, "Q4_K_M", &full, &hot);
    println!("hot: {} threads, {hlayers} layers (was {layers})", hot.threads);
    assert!(hot.threads < cool.threads && hlayers < layers);
    let spec_hot = launch(hctx, hlayers, &hot);
    assert!(!eng.matches(&spec_hot), "new limits mean a relaunch");
    let ep = eng.ensure(spec_hot, None).await.unwrap();
    let tps = engine::benchmark(&ep).await.unwrap();
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
