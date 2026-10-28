// SPDX-License-Identifier: AGPL-3.0-only
//! Building things with a real model: files, sites and a program with a bug.

use super::*;

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

/// Everyday use on a real model (`test_model`): a second
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
