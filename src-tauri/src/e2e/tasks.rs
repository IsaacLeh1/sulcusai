// SPDX-License-Identifier: AGPL-3.0-only
//! The agent at work with a real model: files, lookups, permissions, memory,
//! helpers, other chats, the web, documents.

use super::*;

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
        .ensure(engine::LaunchSpec { exe: &exe, model_path: &model, model_id: &spec.id, quant: "Q4_K_M", ctx, gpu_layers: layers, threads: 0, low_priority: false, log: &real.engine_log(), mmproj: None, batch: 0, lora: None, steer: None }, None)
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
                println!("approval requested: {}: {}", p.tool, p.preview.title);
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
        let launch = engine::LaunchSpec { exe: &exe, model_path: &model, model_id: &spec.id, quant: "Q4_K_M", ctx: 8192, gpu_layers: 999, threads: 0, low_priority: false, log: &log, mmproj: None, batch: 0, lora: None, steer: None };
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

/// Four files in a shared folder, read and compared: when the model asks for
/// several reads in one step they run together ("Ran N lookups together" in
/// the log), and every result comes back in its place. SULCUSAI_TEST_GGUF
/// (Qwen3 4B 2507 was used), see `test_model`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_lookups_run_together() {
    let (_eng, ep, name) = test_model().await;
    let (state, ep, cipher) = agent_harness_on(None, Some(ep)).await;
    let id = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let folder = std::env::temp_dir().join(format!("sulcusai-e2e-four-{id}"));
    std::fs::create_dir_all(&folder).unwrap();
    for (file, text) in [("apples.txt", "Apples: 12 crates, picked Monday."), ("pears.txt", "Pears: 7 crates, picked Tuesday."), ("plums.txt", "Plums: 20 crates, picked Wednesday."), ("figs.txt", "Figs: 3 crates, picked Thursday.")] {
        std::fs::write(folder.join(file), text).unwrap();
    }
    crate::db::add_folder(&state.db.lock().unwrap(), &dunce::canonicalize(&folder).unwrap().display().to_string()).unwrap();
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
    let started = std::time::Instant::now();
    let reply = say(&state, &ep, &cipher, &chat.id, &format!("In {}, read apples.txt, pears.txt, plums.txt and figs.txt (all four at once), then tell me which fruit has the most crates.", folder.display())).await;
    let (_, tools) = transcript(&state, &cipher, &chat.id, &format!("{name}: four reads ({:.0}s)", started.elapsed().as_secs_f64()));
    let _ = std::fs::remove_dir_all(&folder);
    let reads = tools.iter().filter(|t| *t == "read_file").count();
    println!("read_file calls: {reads}; reply: {reply}");
    assert!(reads >= 4, "{tools:?}");
    assert!(reply.to_lowercase().contains("plum"), "{reply}");
}

/// Saved rules with the real model: a Deny on writing `.env` holds in
/// Bypass, where nothing else would ask, and the model is told why; an Ask
/// on reading one file brings up a card in Auto, where reads never ask.
/// See `test_model`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_saved_rules_hold() {
    use crate::tools::allowances::{save, Allowance, Effect};
    let (_eng, ep, name) = test_model().await;
    let (state, ep, cipher) = agent_harness_on(None, Some(ep)).await;
    let id = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let project = std::env::temp_dir().join(format!("sulcusai-e2e-rules-{id}"));
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("plan.txt"), "Ship on Friday.\n").unwrap();
    let project = dunce::canonicalize(&project).unwrap();
    crate::db::add_folder(&state.db.lock().unwrap(), &project.display().to_string()).unwrap();
    let rule = |tool: &str, pattern: &str, effect| Allowance { id: String::new(), tool: tool.into(), pattern: Some(pattern.into()), effect, project: None, created_at: 0 };
    save(&state.db.lock().unwrap(), &cipher, rule("write_file", "**/.env", Effect::Deny)).unwrap();
    save(&state.db.lock().unwrap(), &cipher, rule("read_file", "**/plan.txt", Effect::Ask)).unwrap();
    let (watch, asked) = watch_approvals(&state);
    let started = std::time::Instant::now();
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
    let reply = say_in(crate::tools::Mode::Bypass, &state, &ep, &cipher, &chat.id, &format!("Create a file named .env in {} with the line DEBUG=1, using write_file.", project.display())).await;
    let (text1, tools1) = transcript(&state, &cipher, &chat.id, &format!("{name}: a denied write ({:.0}s)", started.elapsed().as_secs_f64()));
    let chat2 = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
    let reply2 = say(&state, &ep, &cipher, &chat2.id, &format!("Read plan.txt in {} and tell me when we ship.", project.display())).await;
    let (_, tools2) = transcript(&state, &cipher, &chat2.id, &format!("{name}: an asked read ({:.0}s)", started.elapsed().as_secs_f64()));
    watch.abort();
    let wrote = project.join(".env").exists();
    let _ = std::fs::remove_dir_all(&project);
    let asked = asked.lock().unwrap().clone();
    println!("reply 1: {reply}\nreply 2: {reply2}\ncards: {asked:?}");
    assert!(tools1.iter().any(|t| t == "write_file"), "it tried write_file: {tools1:?}");
    assert!(text1.contains("A rule the user saved"), "the write was refused, and the model told why: {text1}");
    assert!(!wrote, "no .env was written");
    let answers = text1.split("[tool]").nth(1).unwrap_or_default().matches("[assistant]").count();
    assert_eq!(answers, 1, "told why once, not pushed on again: {text1}");
    assert!(tools2.iter().any(|t| t == "read_file"), "{tools2:?}");
    assert!(asked.iter().any(|a| a.starts_with("read_file")), "the read asked first: {asked:?}");
    assert!(reply2.to_lowercase().contains("friday"), "{reply2}");
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

/// A task set off by a file (Qwen3 4B 2507 as the model): a file landing in
/// a watched folder is seen at the next look, the task runs with what set
/// it off, and the same file doesn't set it off again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_a_file_sets_off_a_task() {
    use crate::schedule::{self, Cadence, Schedule};
    let state = media_state();
    let c = state.work_cipher().unwrap();
    let real = Paths::new(app_data_dir()).unwrap();
    let file = real.models.join("qwen3-4b-2507").join("Qwen3-4B-Instruct-2507-Q4_K_M.gguf");
    crate::local::import(&state, &file).unwrap();
    let id = crate::local::list(&state.db.lock().unwrap()).into_iter().find(|m| m.local.as_ref().is_some_and(|l| std::path::Path::new(&l.path) == file)).unwrap().id;
    let folder = std::env::temp_dir().join(format!("sulcusai-watch-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("old.txt"), "there before").unwrap();
    let mut s = Schedule {
        id: "w1".into(),
        name: "New files".into(),
        prompt: "Say the new file's name and what it's for, in one short sentence.".into(),
        cadence: Cadence::OnFile { folder: folder.display().to_string(), name: String::new() },
        model_id: Some(id),
        project_id: None,
        allow_changes: false,
        actions: Vec::new(),
        enabled: true,
        last_run: None,
        next_run: None,
        last_chat: None,
        armed: None,
        handled: Vec::new(),
    };
    schedule::arm(&mut s, None);
    schedule::save(&state.db.lock().unwrap(), &c, &s).unwrap();
    let (mut known, mut started) = (schedule::Known::new(), true);
    assert!(schedule::triggered(&state, &c, &mut known, &mut started).is_empty(), "what was there before doesn't count");
    std::fs::write(folder.join("groceries-for-saturday.txt"), "milk, eggs, bread").unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    let fired = schedule::triggered(&state, &c, &mut known, &mut started);
    assert_eq!(fired.len(), 1, "the new file sets it off");
    let (task, what) = (fired[0].0.clone(), fired[0].1.as_ref().unwrap().what.clone());
    println!("{what}");
    assert!(what.contains("groceries-for-saturday.txt") && !what.contains("old.txt"), "only the new file: a file written just before the task was set up isn't new");
    let (chat, result) = schedule::run_with_cause(&state, &task, Some(&what), std::sync::Arc::new(|_: &str, _: serde_json::Value| {})).await.unwrap();
    result.unwrap();
    let said = crate::db::messages(&state.db.lock().unwrap(), &c, &chat).into_iter().rev().find(|m| m.role == "assistant").map(|m| m.content).unwrap_or_default();
    println!("it said: {said}");
    assert!(said.to_lowercase().contains("groceries"), "{said}");
    // Remembered as run for: not again.
    let mut done = task;
    schedule::remember(&mut done, fired[0].1.as_ref().unwrap().ids.clone());
    schedule::save(&state.db.lock().unwrap(), &c, &done).unwrap();
    assert!(schedule::triggered(&state, &c, &mut known, &mut started).is_empty());
    state.engine.lock().await.stop().await;
    let _ = std::fs::remove_dir_all(&folder);
}

/// Deep research on the real web with Qwen3 4B: it plans searches, reads
/// pages, looks again for what's missing, and writes a report with numbered
/// sources that's saved to Notes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_deep_research_writes_a_report() {
    let state = media_state();
    let real = Paths::new(app_data_dir()).unwrap();
    crate::local::import(&state, &real.models.join("qwen3-4b-2507").join("Qwen3-4B-Instruct-2507-Q4_K_M.gguf")).unwrap();
    let id = crate::db::installed_models(&state.db.lock().unwrap())[0].model_id.clone();
    let cipher = crate::crypto::Cipher::random();
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, Some(id.clone())).unwrap();
    crate::db::set_chat_web(&state.db.lock().unwrap(), &chat.id, true).unwrap();
    let (client, settings, key) = crate::web::prepare(&state.db.lock().unwrap(), &cipher, &chat.id).unwrap();
    let w = crate::tools::lookup::Web { client, settings, key, app: None };
    let (ep, _, _) = crate::llm_endpoint(&state, Some(id), false, |_| {}).await.unwrap();
    let memory = crate::tools::MemoryCtx { db: &state.db, cipher: &cipher, chat_id: &chat.id, project_id: None };
    let started = std::time::Instant::now();
    let step = |s: &str| println!("step ({:.0} s): {s}", started.elapsed().as_secs_f64());
    let args = serde_json::json!({ "question": "What changed in Python 3.13 compared with Python 3.12?", "depth": "deep" });
    let out = crate::tools::lookup::deep(&args, &w, &memory, &ep, &std::sync::atomic::AtomicBool::new(false), &step).await;
    println!("{} in {:.0} s\n{}", out.meta["title"], started.elapsed().as_secs_f64(), out.text.chars().take(3000).collect::<String>());
    assert!(out.text.contains("## Sources") && out.text.contains("[1]"), "a report with numbered sources");
    let notes = crate::notes::list_notes(&state.db.lock().unwrap(), &cipher);
    assert!(notes.iter().any(|n| n.data.folder == "Research"), "saved to Notes");
    state.engine.lock().await.stop().await;
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
            ..Default::default()
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
                    println!("approval: {}: {}", p.tool, p.preview.title);
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

/// A question from a real chat: the weather in Orem, with exact figures,
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
