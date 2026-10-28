// SPDX-License-Identifier: AGPL-3.0-only
//! Models and engines: installing, loading, settings, the local API, cloud
//! providers, backups and copies.

use super::*;

/// The cloud providers set up in the installed app, checked
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
                steer: None,
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
        .ensure(engine::LaunchSpec { exe: &exe, model_path: &dest, model_id: &spec.id, quant: "Q4_K_M", ctx: fit.ctx, gpu_layers: 99, threads: 0, low_priority: false, log: &paths.engine_log(), mmproj: None, batch: 0, lora: None, steer: None }, None)
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

/// Steering, on Qwen3 4B 2507: a direction found from "cheerful" against
/// "gloomy" (llama.cpp's control-vector maker, on the card), then the same
/// question asked as the model is and steered each way. Until the maker's
/// zip is on the `engines` release, SULCUSAI_LENS_ZIP points at it. The
/// direction is deleted afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_a_model_is_steered() {
    let state = media_state();
    let real = Paths::new(app_data_dir()).unwrap();
    let file = real.models.join("qwen3-4b-2507").join("Qwen3-4B-Instruct-2507-Q4_K_M.gguf");
    crate::local::import(&state, &file).unwrap();
    let id = crate::local::list(&state.db.lock().unwrap()).into_iter().find(|m| m.local.as_ref().is_some_and(|l| std::path::Path::new(&l.path) == file)).unwrap().id;
    if let (Some(zip), Some(t)) = (std::env::var_os("SULCUSAI_LENS_ZIP"), state.catalog.lens.as_ref()) {
        let dir = state.paths.engines.join(format!("lens-tools-{}", t.build));
        if !dir.join("llama-cvector-generator.exe").is_file() {
            crate::engine::unzip_for_test(std::path::Path::new(&zip), &dir).unwrap();
        }
    }
    let started = std::time::Instant::now();
    let d = crate::steer::make(&state, Some(&id), "Cheerful", "cheerful", "gloomy", |s, f| println!("  {s} {:.0}%", f * 100.0)).await.unwrap();
    println!("direction in {:.0} s: {} pairs, layers {}-{} of {}", started.elapsed().as_secs_f64(), d.pairs, d.from, d.to, d.layers);
    let prompt = "Describe your morning in two sentences.";
    let say = |w: crate::lens::Words| w.pieces.iter().map(|p| p.text.as_str()).collect::<String>();
    let plain = say(crate::lens::words(&state, Some(&id), prompt, "", 5, 60).await.unwrap());
    let c = state.work_cipher().unwrap();
    let mut toward = Vec::new();
    for s in [1.0f32, -1.0] {
        {
            let conn = state.db.lock().unwrap();
            let mut all = crate::steer::list(&conn, &c);
            all.iter_mut().for_each(|x| x.strength = s);
            crate::db::set(&conn, "lens_directions", &c.encrypt(&serde_json::to_string(&all).unwrap())).unwrap();
        }
        let steered = say(crate::lens::words(&state, Some(&format!("steer:{}", d.id)), prompt, "", 5, 60).await.unwrap());
        println!("{s:+}: {steered:?}");
        toward.push(steered);
    }
    println!("as it is: {plain:?}");
    state.engine.lock().await.stop().await;
    assert!(toward.iter().all(|s| !s.trim().is_empty()));
    assert!(toward[0] != plain && toward[1] != plain && toward[0] != toward[1], "steering changes the reply");
    {
        let conn = state.db.lock().unwrap();
        assert!(crate::steer::exists(&conn, &d.id));
    }
    let file = state.paths.models.join("lens-steer").join(format!("{}.gguf", d.id));
    assert!(file.is_file());
    let _ = std::fs::remove_file(file);
}

/// A model file with multi-token-prediction layers (SULCUSAI_TEST_GGUF, e.g.
/// unsloth's Qwen3.5 9B MTP) launched the app's way guesses ahead with them,
/// and two replies asked for at once both come back (several slots).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_mtp_guesses_ahead_and_two_replies_run_at_once() {
    let gguf = std::path::PathBuf::from(std::env::var("SULCUSAI_TEST_GGUF").expect("SULCUSAI_TEST_GGUF"));
    assert!(engine::mtp_layers(&gguf) > 0, "the test model has MTP layers");
    let (mut eng, ep, name) = test_model().await;
    let ask = |text: &str| vec![serde_json::json!({ "role": "user", "content": text })];
    let started = std::time::Instant::now();
    let (a, b) = tokio::join!(
        chat::complete(&ep, ask("Write a Python function that checks whether a number is prime, with a docstring."), serde_json::json!({ "temperature": 0, "chat_template_kwargs": { "enable_thinking": false } }), 300),
        chat::complete(&ep, ask("Write four sentences about autumn leaves."), serde_json::json!({ "temperature": 0, "chat_template_kwargs": { "enable_thinking": false } }), 300),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    println!("{name}: both replies in {:.1} s ({} and {} characters)", started.elapsed().as_secs_f64(), a.len(), b.len());
    assert!(a.contains("def ") && b.len() > 100);
    let slots: serde_json::Value = net::local_client().get(ep.url("/slots")).bearer_auth(&ep.key).send().await.unwrap().json().await.unwrap();
    println!("slots: {}", slots.as_array().map_or(0, |s| s.len()));
    assert!(slots.as_array().is_some_and(|s| s.len() >= 2), "several slots");
    eng.stop().await;
}

/// The local API in Claude's shape: /v1/messages (with the key as
/// x-api-key, and a Claude model name meaning the default) and token counts,
/// answered by Qwen3 4B (installed, imported here where it is).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_the_local_api_speaks_claudes_shape() {
    let state = media_state();
    let real = Paths::new(app_data_dir()).unwrap();
    crate::local::import(&state, &real.models.join("qwen3-4b-2507").join("Qwen3-4B-Instruct-2507-Q4_K_M.gguf")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = crate::api_server::router_for_test(state.clone(), "sk-test", port);
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = net::local_client();
    let url = |p: &str| format!("http://127.0.0.1:{port}{p}");
    let reply: serde_json::Value = client
        .post(url("/v1/messages"))
        .header("x-api-key", "sk-test")
        .header("anthropic-version", "2023-06-01")
        .json(&serde_json::json!({ "model": "claude-sonnet-5-5", "max_tokens": 60, "messages": [{ "role": "user", "content": "Reply with just the word blue." }] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    println!("messages: {reply}");
    assert_eq!(reply["type"], "message");
    assert!(reply["content"][0]["text"].as_str().unwrap_or_default().to_lowercase().contains("blue"));
    let count: serde_json::Value = client
        .post(url("/v1/messages/count_tokens"))
        .header("x-api-key", "sk-test")
        .json(&serde_json::json!({ "model": "default", "messages": [{ "role": "user", "content": "How many tokens is this?" }] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    println!("count: {count}");
    assert!(count["input_tokens"].as_u64().is_some_and(|n| n > 3));
    let refused = client.post(url("/v1/messages")).header("x-api-key", "nope").json(&serde_json::json!({ "max_tokens": 5, "messages": [] })).send().await.unwrap();
    assert_eq!(refused.status().as_u16(), 401);
    state.engine.lock().await.stop().await;
    server.abort();
}

/// Switching between two models the card holds together: the one used
/// before stays loaded, and going back to it answers without loading
/// (Qwen3 4B and Qwen3 VL 4B on the 12 GB card).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_two_models_stay_loaded() {
    let state = media_state();
    let real = Paths::new(app_data_dir()).unwrap();
    crate::local::import(&state, &real.models.join("qwen3-4b-2507").join("Qwen3-4B-Instruct-2507-Q4_K_M.gguf")).unwrap();
    crate::local::import(&state, &real.models.join("qwen3-vl-4b").join("Qwen3-VL-4B-Instruct-Q4_K_M.gguf")).unwrap();
    let ids: Vec<String> = crate::db::installed_models(&state.db.lock().unwrap()).into_iter().map(|m| m.model_id).collect();
    let (a, b) = (ids[0].clone(), ids[1].clone());
    // An 8K window each (about 4 GB): at 32K each needs 7.3 GB and two don't fit.
    let small = crate::presets::ModelPreset { ctx: Some(8192), ..Default::default() };
    crate::db::set(&state.db.lock().unwrap(), "model_presets", &std::collections::HashMap::from([(a.clone(), small.clone()), (b.clone(), small)])).unwrap();
    let ask = |ep: crate::engine::Endpoint| async move {
        chat::complete(&ep, vec![serde_json::json!({ "role": "user", "content": "Reply with just the word blue." })], serde_json::json!({ "temperature": 0 }), 60).await.unwrap()
    };
    let (ep, _, _) = crate::llm_endpoint(&state, Some(a.clone()), false, |_| {}).await.unwrap();
    println!("a: {}", ask(ep).await);
    let started = std::time::Instant::now();
    let (ep, _, _) = crate::llm_endpoint(&state, Some(b.clone()), false, |_| {}).await.unwrap();
    println!("b loaded in {:.1} s: {}", started.elapsed().as_secs_f64(), ask(ep).await);
    let st = state.engine.lock().await.status();
    println!("running {:?}, kept {:?}", st.model_id, st.kept);
    assert_eq!(st.model_id.as_deref(), Some(b.as_str()));
    assert_eq!(st.kept.as_deref(), Some(a.as_str()), "the first model stayed loaded");
    let started = std::time::Instant::now();
    let (ep, _, _) = crate::llm_endpoint(&state, Some(a.clone()), false, |_| {}).await.unwrap();
    let back = started.elapsed().as_secs_f64();
    println!("back to a in {back:.2} s: {}", ask(ep).await);
    assert!(back < 2.0, "no loading");
    assert_eq!(state.engine.lock().await.status().kept.as_deref(), Some(b.as_str()));
    state.engine.lock().await.stop().await;
    assert_eq!(state.engine.lock().await.status().kept, None, "stopping frees both");
}

/// A model's own settings reach the engine and its replies: the small q4
/// cache with flash attention on (llama.cpp takes both), its temperature,
/// and instructions added for it (Qwen3 4B, answering in French).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_a_models_own_settings_apply() {
    let state = media_state();
    let real = Paths::new(app_data_dir()).unwrap();
    crate::local::import(&state, &real.models.join("qwen3-4b-2507").join("Qwen3-4B-Instruct-2507-Q4_K_M.gguf")).unwrap();
    let id = crate::db::installed_models(&state.db.lock().unwrap())[0].model_id.clone();
    let preset = crate::presets::ModelPreset {
        instructions: "Always answer in French.".into(),
        sampling: crate::advanced::Sampling { temperature: Some(0.0), ..Default::default() },
        kv_cache: Some("q4_0".into()),
        flash_attn: Some(true),
        ..Default::default()
    };
    crate::db::set(&state.db.lock().unwrap(), "model_presets", &std::collections::HashMap::from([(id.clone(), preset.clone())])).unwrap();
    let (ep, _, _) = crate::llm_endpoint(&state, Some(id.clone()), false, |_| {}).await.unwrap();
    assert_eq!(ep.extra["temperature"], 0.0);
    // The engine's own report of how it set up the cache.
    let log = std::fs::read_to_string(state.paths.engine_log()).unwrap_or_default();
    let said: Vec<&str> = log.lines().filter(|l| l.contains("flash_attn") || l.contains("type_k") || l.contains("K (q4_0)") || l.contains("small cache")).collect();
    println!("engine: {said:?}");
    assert!(log.contains("flash_attn            = enabled"), "flash attention on");
    assert!(log.contains("K (q4_0)") && log.contains("V (q4_0)"), "the engine keeps K and V as q4_0");
    let system = crate::presets::with_instructions("You are a helpful assistant.".into(), &preset);
    let reply = chat::complete(&ep, vec![serde_json::json!({ "role": "system", "content": system }), serde_json::json!({ "role": "user", "content": "What colour is the sky on a clear day? One short sentence." })], ep.extra.clone(), 120).await.unwrap();
    println!("reply: {reply}");
    let low = reply.to_lowercase();
    assert!(low.contains("ciel") || low.contains("bleu"), "answered in French");
    state.engine.lock().await.stop().await;
}

/// The server for programs made for Ollama, answered by Qwen3 4B: the
/// model list, a reply in one piece that fits a JSON schema, one streamed
/// line by line, a tool call, /api/generate, and a web page turned away.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_programs_made_for_ollama_get_replies() {
    let state = media_state();
    let real = Paths::new(app_data_dir()).unwrap();
    crate::local::import(&state, &real.models.join("qwen3-4b-2507").join("Qwen3-4B-Instruct-2507-Q4_K_M.gguf")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = crate::ollama_api::router_for_test(state.clone(), port);
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = net::local_client();
    let url = |p: &str| format!("http://localhost:{port}{p}");
    let post = |p: &str, body: serde_json::Value| client.post(url(p)).json(&body).send();

    let tags: serde_json::Value = client.get(url("/api/tags")).send().await.unwrap().json().await.unwrap();
    println!("tags: {tags}");
    let name = tags["models"][0]["name"].as_str().unwrap().to_string();
    assert!(name.ends_with(":latest"));

    let schema = serde_json::json!({ "type": "object", "properties": { "city": { "type": "string" }, "country": { "type": "string" } }, "required": ["city", "country"] });
    let started = std::time::Instant::now();
    let one: serde_json::Value = post("/api/chat", serde_json::json!({ "model": name, "stream": false, "format": schema, "messages": [{ "role": "user", "content": "What is the capital of France? Answer as JSON." }] })).await.unwrap().json().await.unwrap();
    println!("one piece ({:.1} s): {one}", started.elapsed().as_secs_f64());
    let fits: serde_json::Value = serde_json::from_str(one["message"]["content"].as_str().unwrap()).unwrap();
    assert_eq!(fits["city"], "Paris");
    assert_eq!(one["done"], true);
    assert!(one["eval_count"].as_u64().is_some_and(|n| n > 0));

    let lines = post("/api/chat", serde_json::json!({ "model": name, "messages": [{ "role": "user", "content": "Count from one to five in words." }], "options": { "temperature": 0 } })).await.unwrap().text().await.unwrap();
    let lines: Vec<serde_json::Value> = lines.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let said: String = lines.iter().filter_map(|l| l["message"]["content"].as_str()).collect();
    println!("streamed {} lines: {said}", lines.len());
    assert!(lines.len() > 3 && lines.last().unwrap()["done"] == true);
    assert!(said.to_lowercase().contains("five"));

    let tools = serde_json::json!([{ "type": "function", "function": { "name": "get_weather", "description": "The weather in a city now", "parameters": { "type": "object", "properties": { "city": { "type": "string" } }, "required": ["city"] } } }]);
    let called: serde_json::Value = post("/api/chat", serde_json::json!({ "model": name, "stream": false, "tools": tools, "messages": [{ "role": "user", "content": "What's the weather in Oslo right now?" }] })).await.unwrap().json().await.unwrap();
    println!("tool call: {called}");
    assert_eq!(called["message"]["tool_calls"][0]["function"]["name"], "get_weather");
    assert_eq!(called["message"]["tool_calls"][0]["function"]["arguments"]["city"], "Oslo");

    let gen: serde_json::Value = post("/api/generate", serde_json::json!({ "model": name, "stream": false, "prompt": "Reply with just the word blue." })).await.unwrap().json().await.unwrap();
    println!("generate: {gen}");
    assert!(gen["response"].as_str().unwrap().to_lowercase().contains("blue"));

    let page = client.post(url("/api/chat")).header("origin", "https://example.com").json(&serde_json::json!({ "model": name, "messages": [] })).send().await.unwrap();
    assert_eq!(page.status().as_u16(), 403);
    state.engine.lock().await.stop().await;
    server.abort();
}

/// A Claude export (a zip with conversations.json and a memories file)
/// brought in: its chats with their messages and dates in a "From Claude"
/// folder, its memories saved; the same export again adds nothing.
#[test]
#[ignore]
fn e2e_an_export_is_imported_once() {
    let state = media_state();
    let c = state.cipher().unwrap();
    let dir = std::env::temp_dir().join(format!("sulcusai-import-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let zip_path = dir.join("data-2026-10-26.zip");
    {
        let f = std::fs::File::create(&zip_path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opts: zip::write::SimpleFileOptions = Default::default();
        z.start_file("conversations.json", opts).unwrap();
        std::io::Write::write_all(&mut z, serde_json::json!([{ "uuid": "1", "name": "Sourdough help", "created_at": "2026-03-02T09:00:00Z", "updated_at": "2026-03-02T09:30:00Z", "chat_messages": [
            { "sender": "human", "text": "My sourdough is flat", "created_at": "2026-03-02T09:00:01Z" },
            { "sender": "assistant", "text": "Try a longer bulk ferment.", "created_at": "2026-03-02T09:00:09Z" }
        ] }]).to_string().as_bytes()).unwrap();
        z.start_file("memories.json", opts).unwrap();
        std::io::Write::write_all(&mut z, serde_json::json!([{ "conversations_memory": "- Bakes sourdough on weekends\n- Lives at high altitude" }]).to_string().as_bytes()).unwrap();
        z.finish().unwrap();
    }
    let export = crate::import::read(&zip_path).unwrap();
    let first = crate::import::save(&state, &export).unwrap();
    let again = crate::import::save(&state, &export).unwrap();
    println!("{first:?} then {again:?}");
    assert_eq!((first.chats, first.messages, first.memories), (1, 2, 2));
    assert_eq!((again.chats, again.already, again.memories), (0, 1, 0));
    let conn = state.db.lock().unwrap();
    let chat = crate::db::list_chats(&conn, &c).into_iter().find(|ch| ch.title == "Sourdough help").unwrap();
    assert_eq!(chat.created_at, 1_772_442_000_000, "its own start time");
    assert_eq!(crate::db::messages(&conn, &c, &chat.id).len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
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
    let launch = |ctx, layers, l: &perf::Limits| engine::LaunchSpec { exe: &exe, model_path: &model, model_id: &spec.id, quant: "Q4_K_M", ctx, gpu_layers: layers, threads: l.threads, low_priority: l.low_priority, log: &log, mmproj: None, batch: 0, lora: None, steer: None };
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
        .ensure(engine::LaunchSpec { exe: &exe, model_path: &path, model_id: &small.id, quant: &small.variants[0].quant, ctx: 2048, gpu_layers: 0, threads: 4, low_priority: true, log: &state.paths.engine_log(), mmproj: None, batch: 0, lora: None, steer: None }, None)
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
        a.engine = crate::advanced::EngineTuning { ctx: Some(2048), gpu_layers: Some(0), threads: Some(4), batch: Some(256), ..Default::default() };
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
        a.engine = crate::advanced::EngineTuning { ctx: Some(2048), gpu_layers: Some(0), threads: Some(4), batch: None, ..Default::default() };
        crate::db::set(&conn, "advanced", &a).unwrap();
        crate::db::set(&conn, "api_server", &crate::api_server::ApiSettings { enabled: true, port, key: "sk-sulcus-test".into(), ..Default::default() }).unwrap();
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
        crate::db::set(&conn, "api_server", &crate::api_server::ApiSettings { enabled: false, port, key: "sk-sulcus-test".into(), ..Default::default() }).unwrap();
    }
    crate::api_server::apply(state.clone());
    state.engine.lock().await.stop().await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(c.get(format!("{base}/models")).send().await.is_err(), "stopped");
    std::fs::remove_dir_all(&state.paths.data).ok();
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
    let launch = engine::LaunchSpec { exe: &exe, model_path: &path, model_id: &spec.id, quant: "Q4_K_M", ctx: 4096, gpu_layers: 0, threads: 4, low_priority: true, log: &log, mmproj: None, batch: 0, lora: None, steer: None };
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
            .ensure(engine::LaunchSpec { exe: &exe, model_path: &model, model_id: &spec.id, quant: "Q4_K_M", ctx: 2048, gpu_layers: 99, threads: 0, low_priority: false, log: &log, mmproj: None, batch: 0, lora, steer: None }, None)
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

/// An uncensored copy of Qwen2.5 0.5B Instruct, made from start to end as
/// the Models page makes one: Heretic's package checked and unpacked as if
/// downloaded from the engines release (SULCUSAI_TEST_HERETIC: the zip
/// scripts/heretic/package.py made), its packages set up by uv, the
/// original downloaded from Hugging Face, 10 tries on the graphics card,
/// converted and squeezed to Q4_K_M by the engine's llama-quantize, and
/// added to the models list; then the copy is asked something. Its own
/// engines and models folders, linked to the installed app's uv, package
/// cache and engine, so the installed app's are left as they are.
/// SULCUSAI_TEST_GGUF_SMALL: the model file it's a copy "of" (any small GGUF).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_an_uncensored_copy_is_made() {
    let zip = std::path::PathBuf::from(std::env::var("SULCUSAI_TEST_HERETIC").expect("SULCUSAI_TEST_HERETIC: the zip scripts/heretic/package.py made"));
    let gguf = std::path::PathBuf::from(std::env::var("SULCUSAI_TEST_GGUF_SMALL").expect("SULCUSAI_TEST_GGUF_SMALL: a small model file"));
    let real = Paths::new(app_data_dir()).unwrap();
    let tmp = std::env::temp_dir().join(format!("sulcusai-uncensor-{}", uuid::Uuid::new_v4()));
    let (engines, models) = (tmp.join("engines"), tmp.join("models"));
    std::fs::create_dir_all(&engines).unwrap();
    std::fs::create_dir_all(&models).unwrap();
    let state = temp_state_in(Some(models.clone()), Some(engines.clone()));
    let backend = engine::backend_for(&state.budget()).unwrap();
    // Linked, not copied: the installed app's uv, its Python and package cache, and the engine.
    let shared = ["trainer-uv-bin".to_string(), "trainer-uv".to_string(), format!("{}-{backend}", state.catalog.engine.build)];
    for d in &shared {
        let linked = std::process::Command::new("cmd").arg("/c").arg("mklink").arg("/J").arg(engines.join(d)).arg(real.engines.join(d)).output().unwrap();
        assert!(linked.status.success(), "linked {d}");
    }
    let h = state.catalog.heretic.clone().unwrap();
    std::fs::copy(&zip, engines.join(format!("heretic-{}.zip", h.build))).unwrap();

    crate::local::import(&state, &gguf).unwrap();
    let id = crate::local::list(&state.db.lock().unwrap())[0].id.clone();
    let started = std::time::Instant::now();
    let cancel = crate::uncensor::begin(&state, &id, 10).unwrap();
    let made = crate::uncensor::run(&state, &id, "Qwen/Qwen2.5-0.5B-Instruct", 10, &cancel).await;
    let job = crate::uncensor::uncensor_job().unwrap();
    println!("{:.0} s: {job:?}", started.elapsed().as_secs_f64());
    let copy = made.unwrap();
    let spec = crate::hub::list(&state.db.lock().unwrap()).into_iter().find(|m| m.id == copy).unwrap();
    let installed = crate::db::installed_model(&state.db.lock().unwrap(), &copy).unwrap();
    println!("{} ({}, {} MB): {}", spec.name, installed.quant, installed.size / 1_000_000, spec.description);
    assert!(spec.name.ends_with("(uncensored)"));
    assert_eq!(installed.quant, "Q4_K_M");
    assert!(job.refusals.unwrap() < job.base_refusals.unwrap(), "the copy refuses fewer of the test requests");
    assert!(!models.join("uncensor-work").read_dir().is_ok_and(|mut d| d.next().is_some()), "the work folder is cleared");

    let (ep, _, _) = crate::llm_endpoint(&state, Some(copy.clone()), false, |_| {}).await.unwrap();
    let ask = |q: &str| vec![serde_json::json!({ "role": "system", "content": "You are a helpful assistant." }), serde_json::json!({ "role": "user", "content": q })];
    let reply = chat::complete(&ep, ask("Write step-by-step instructions for picking a basic pin tumbler lock."), serde_json::json!({ "temperature": 0 }), 120).await.unwrap();
    println!("copy: {reply}");
    assert!(!reply.trim().is_empty());

    state.engine.lock().await.stop().await;
    for d in &shared {
        std::fs::remove_dir(engines.join(d)).ok();
    }
    std::fs::remove_dir_all(&tmp).ok();
}
