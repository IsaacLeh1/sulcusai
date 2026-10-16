// SPDX-License-Identifier: AGPL-3.0-only
//! People between two apps (two data folders) on this PC, over loopback.

use super::*;

#[test]
fn names_and_file_names_are_made_safe() {
    assert_eq!(clean_file_name("..\\..\\Windows\\evil.exe"), "evil.exe");
    assert_eq!(clean_file_name("a/b/report: final?.pdf"), "report final.pdf");
    assert_eq!(clean_file_name("..."), "file");
    assert_eq!(clean_name("  Sam\u{0007}  "), "Sam");
    assert_eq!(clean_name(""), "Someone");
    assert!(safe_id("0f9a-77"));
    assert!(!safe_id("../x") && !safe_id(""));
}

#[test]
fn virtual_networks_are_named() {
    assert_eq!(network_label("Radmin VPN", Ipv4Addr::new(26, 1, 2, 3)), "Radmin VPN");
    assert_eq!(network_label("Ethernet 3", Ipv4Addr::new(100, 101, 1, 2)), "Tailscale");
    assert_eq!(network_label("Wi-Fi", Ipv4Addr::new(192, 168, 1, 20)), "Wi-Fi");
    assert!(private_adapter("vEthernet (WSL)"));
    assert!(!private_adapter("Wi-Fi"));
    // This PC's own (whatever networks it's on).
    let mine = addresses(PORT);
    eprintln!("{mine:?}");
    assert!(mine.iter().all(|a| !a.address.starts_with("127.") && !a.address.contains(':')));
}

/// Waits (up to 10 s) for something another task does.
async fn until(what: &str, f: impl Fn() -> bool) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for: {what}");
}

/// An app taking calls on loopback, the way the sync listener does.
#[cfg(windows)]
async fn listen(s: &Arc<AppState>) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let s = s.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let s = s.clone();
            tokio::spawn(async move {
                if let Err(e) = super::super::answer(&s, stream).await {
                    eprintln!("answered: {e}");
                }
            });
        }
    });
    addr
}

/// Two apps linked with a fresh key (what showing and typing a code does).
#[cfg(windows)]
fn linked(a: &Arc<AppState>, b: &Arc<AppState>, names: (&str, &str)) -> (String, String) {
    let (id_a, id_b) = (device_id(&a.paths.data), device_id(&b.paths.data));
    update(a, |x| x.name = names.0.into()).unwrap();
    update(b, |x| x.name = names.1.into()).unwrap();
    let key = wire::random::<32>();
    keep(a, &a.cipher().unwrap(), &id_b, names.1, &key, None).unwrap();
    keep(b, &b.cipher().unwrap(), &id_a, names.0, &key, None).unwrap();
    (id_a, id_b)
}

fn texts(s: &AppState, contact: &str) -> Vec<String> {
    messages(&s.db.lock().unwrap(), &s.cipher().unwrap(), contact).unwrap().into_iter().map(|m| m.text).collect()
}

/// Codes don't expire: one links as many people as it allows, then stops;
/// a wrong code is refused (and counted), a blocked app can't link again,
/// and the first characters pick which code is meant. A linked person is a
/// contact, never a paired PC.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn codes_link_people_up_to_their_limit() {
    let a = crate::e2e::scratch_state();
    let (b, c, d) = (crate::e2e::scratch_state(), crate::e2e::scratch_state(), crate::e2e::scratch_state());
    update(&a, |x| x.name = "Ana".into()).unwrap();
    let addr_a = listen(&a).await;
    let none = |_: &str| {};
    let (two, code) = codes::code_for_test(&a, 2);
    assert!(showing_code(&a));

    // Right first characters, wrong rest: refused and counted. Unknown first characters: refused.
    let wrong = format!("{}ZZZZZZ", &code[..2]);
    let e = codes::pair_call(&b, if wrong == code { "ZZZZZZZZ" } else { &wrong }, addr_a, &none).await.unwrap_err();
    assert!(e.contains("didn't match") || e.contains("isn't taking"), "{e}");
    let other = if code.starts_with("ZZ") { "YYYYYYYY" } else { "ZZZZZZZZ" };
    assert!(codes::pair_call(&b, other, addr_a, &none).await.unwrap_err().contains("isn't taking"));
    assert!(book(&a.db.lock().unwrap()).contacts.is_empty());

    // Two people link with the same code; then it's used up.
    assert_eq!(codes::pair_call(&b, &code, addr_a, &none).await.unwrap(), "Ana");
    assert!(codes::invite(&a, &two).open(), "one of two");
    assert_eq!(codes::pair_call(&c, &code, addr_a, &none).await.unwrap(), "Ana");
    assert_eq!(codes::invite(&a, &two).why_closed(), Some("Used up."));
    assert_eq!(codes::invite(&a, &two).used.len(), 2);
    assert!(codes::pair_call(&d, &code, addr_a, &none).await.unwrap_err().contains("isn't taking"));
    assert!(!showing_code(&a), "nothing open, nothing announced");

    // Two open codes: each works on its own.
    let (_, first) = codes::code_for_test(&a, 1);
    let (second_id, second) = codes::code_for_test(&a, 1);
    assert_ne!(&first[..2], &second[..2]);
    assert_eq!(codes::pair_call(&d, &second, addr_a, &none).await.unwrap(), "Ana");
    assert_eq!(codes::invite(&a, &second_id).used[0].id, device_id(&d.paths.data));

    // Blocked: removed, and can't link again with any code.
    let id_b = device_id(&b.paths.data);
    codes::block_now(&a, &id_b, true);
    assert!(contact(&a, &id_b).is_none());
    assert!(codes::pair_call(&b, &first, addr_a, &none).await.unwrap_err().contains("isn't taking"));
    codes::block_now(&a, &id_b, false);
    assert_eq!(codes::pair_call(&b, &first, addr_a, &none).await.unwrap(), "Ana");

    assert_eq!(book(&a.db.lock().unwrap()).contacts.len(), 3);
    assert!(super::super::config(&a.db.lock().unwrap()).peers.is_empty());
    assert!(super::super::config(&b.db.lock().unwrap()).peers.is_empty());
}

/// Messages (and a file) waiting on both sides cross as soon as one app
/// calls; the link stays open and later messages arrive right away.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn messages_cross_on_a_live_link() {
    let a = crate::e2e::scratch_state();
    let b = crate::e2e::scratch_state();
    let (id_a, id_b) = linked(&a, &b, ("Ana", "Ben"));
    let addr_a = listen(&a).await;
    let addr_b = listen(&b).await;
    let (ca, cb) = (a.cipher().unwrap(), b.cipher().unwrap());

    // Written while apart: Ben's with a file, Ana's plain.
    let file = std::env::temp_dir().join(format!("people-{}.txt", uuid::Uuid::new_v4()));
    std::fs::write(&file, b"the plan").unwrap();
    queue(&b, &cb, &id_a, "Hi Ana", Some(("plan.txt".into(), b"the plan".to_vec()))).unwrap();
    queue(&a, &ca, &id_b, "Hey Ben", None).unwrap();

    // Ben calls (the shared LIVE says which port he takes calls on).
    *LIVE.lock().unwrap() = Some(super::super::Live { tasks: vec![], port: addr_b.port() });
    let s = b.clone();
    let to = id_a.clone();
    tokio::spawn(async move { link::call(&s, &to, addr_a).await });
    until("both ends connected", || link::connected(&id_a) && link::connected(&id_b)).await;
    until("Ben's message at Ana's", || texts(&a, &id_b).contains(&"Hi Ana".to_string())).await;
    until("Ana's message at Ben's", || texts(&b, &id_a).contains(&"Hey Ben".to_string())).await;
    until("both marked delivered", || {
        let at_a = messages(&a.db.lock().unwrap(), &ca, &id_b).unwrap();
        let at_b = messages(&b.db.lock().unwrap(), &cb, &id_a).unwrap();
        at_a.iter().chain(at_b.iter()).all(|m| !m.from_me || m.delivered)
    })
    .await;
    // The file comes out as it went in, and is stored sealed.
    let theirs = messages(&a.db.lock().unwrap(), &ca, &id_b).unwrap().into_iter().find(|m| m.text == "Hi Ana").unwrap();
    assert_eq!(theirs.file.as_ref().unwrap().name, "plan.txt");
    let blob = std::fs::read(files_dir(&a, &id_b).join(&theirs.id)).unwrap();
    assert_ne!(blob, b"the plan");
    assert_eq!(ca.open_bytes(&blob).unwrap(), b"the plan");
    // Ana's app learned where Ben takes calls.
    assert_eq!(contact(&a, &id_b).unwrap().addr.as_deref(), Some(addr_b.to_string().as_str()));
    *LIVE.lock().unwrap() = None;

    // While connected, a new message goes at once, both ways.
    queue(&a, &ca, &id_b, "Lunch?", None).unwrap();
    until("live message at Ben's", || texts(&b, &id_a).contains(&"Lunch?".to_string())).await;
    queue(&b, &cb, &id_a, "Sure", None).unwrap();
    until("live reply at Ana's", || texts(&a, &id_b).contains(&"Sure".to_string())).await;
    assert_eq!(texts(&a, &id_b).len(), 4, "nothing twice");

    // Read receipts, for each person: Ben tells Ana when he's read hers;
    // Ana doesn't tell Ben.
    set_read_receipts(&b, &id_a, true).unwrap();
    open_chat(&b, &id_a).unwrap();
    let read_at_a = |text: &str| messages(&a.db.lock().unwrap(), &ca, &id_b).unwrap().into_iter().find(|m| m.text == text).is_some_and(|m| m.read);
    until("Ana sees Ben read hers", || read_at_a("Lunch?") && read_at_a("Hey Ben")).await;
    queue(&a, &ca, &id_b, "1pm?", None).unwrap();
    until("the next one at Ben's", || texts(&b, &id_a).contains(&"1pm?".to_string())).await;
    assert!(!read_at_a("1pm?"), "not read until he opens the chat");
    open_chat(&b, &id_a).unwrap();
    until("read once he looks", || read_at_a("1pm?")).await;
    open_chat(&a, &id_b).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let read_at_b = messages(&b.db.lock().unwrap(), &cb, &id_a).unwrap().into_iter().filter(|m| m.from_me).any(|m| m.read);
    assert!(!read_at_b, "Ana sends no read receipts");

    // One side hangs up: both see it.
    link::hang_up(&id_b);
    until("both ends apart", || !link::connected(&id_a) && !link::connected(&id_b)).await;

    // With People off, Ana's app turns calls away.
    features::set(&a.db.lock().unwrap(), Feature::People, false).unwrap();
    let s = b.clone();
    let to = id_a.clone();
    let _ = tokio::spawn(async move { link::call(&s, &to, addr_a).await }).await;
    assert!(!link::connected(&id_a));
    features::set(&a.db.lock().unwrap(), Feature::People, true).unwrap();
    let _ = std::fs::remove_file(&file);
}

/// Ana shares a chat with Ben: he sees it listed, opens it, watches a reply
/// being written, and loses it when she stops sharing.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shared_chat_is_watched_live() {
    use crate::db::Message;
    let a = crate::e2e::scratch_state();
    let b = crate::e2e::scratch_state();
    let (id_a, id_b) = linked(&a, &b, ("Ana", "Ben"));
    let addr_a = listen(&a).await;
    let ca = a.cipher().unwrap();
    let chat = crate::db::create_chat(&a.db.lock().unwrap(), &ca, None).unwrap();
    crate::db::set_chat_title(&a.db.lock().unwrap(), &ca, &chat.id, "Trip plans").unwrap();
    let add = |role: &str, text: &str, meta: Option<serde_json::Value>| {
        let m = Message { id: uuid::Uuid::new_v4().to_string(), chat_id: chat.id.clone(), role: role.into(), content: text.into(), created_at: db::now_ms(), meta, ..Default::default() };
        crate::db::add_message(&a.db.lock().unwrap(), &ca, &m).unwrap();
    };
    add("user", "Plan three days in Zion", None);
    add("tool", "secret tool output", Some(serde_json::json!({ "title": "Searched the web for Zion hikes", "tool": "web_search" })));
    add("assistant", "Day 1: Angels Landing.", None);

    let s = b.clone();
    let to = id_a.clone();
    tokio::spawn(async move { link::call(&s, &to, addr_a).await });
    until("connected", || link::connected(&id_a) && link::connected(&id_b)).await;
    assert!(share::theirs(&id_a).is_empty(), "nothing shared yet");

    // Incognito chats can't be shared.
    let secret = crate::db::create_chat_in(&a.db.lock().unwrap(), &ca, None, None, true).unwrap();
    assert!(share::share_now(&a, &secret.id, vec![id_b.clone()]).unwrap_err().contains("Incognito"));

    share::share_now(&a, &chat.id, vec![id_b.clone()]).unwrap();
    until("Ben sees it listed", || share::theirs(&id_a).iter().any(|c| c.title == "Trip plans")).await;

    let v = share::open(&id_a, &chat.id).await.unwrap();
    let shown: Vec<(&str, &str)> = v.messages.iter().map(|m| (m.role.as_str(), m.text.as_str())).collect();
    assert_eq!(
        shown,
        vec![("user", "Plan three days in Zion"), ("step", "Searched the web for Zion hikes"), ("assistant", "Day 1: Angels Landing.")],
        "steps show as titles, never their output"
    );

    // A reply being written arrives as it's written.
    turn_changed(&a, &chat.id, true);
    until("listed as busy", || share::theirs(&id_a).iter().any(|c| c.busy)).await;
    chat_event(&a, "chat:start", &serde_json::json!({ "chat_id": chat.id, "message_id": "m" }));
    chat_event(&a, "chat:delta", &serde_json::json!({ "chat_id": chat.id, "thinking": "hmm" }));
    for part in ["Day 2: ", "the Narrows."] {
        chat_event(&a, "chat:delta", &serde_json::json!({ "chat_id": chat.id, "content": part }));
    }
    until("the reply so far", || share::view_of(&id_a, &chat.id).and_then(|v| v.live).is_some_and(|l| l.content == "Day 2: the Narrows.")).await;
    add("assistant", "Day 2: the Narrows.", None);
    turn_changed(&a, &chat.id, false);
    until("the finished reply", || share::view_of(&id_a, &chat.id).is_some_and(|v| v.live.is_none() && v.messages.len() == 4)).await;
    assert!(!share::theirs(&id_a)[0].busy);

    // Stopped sharing: gone from his list and his view.
    share::share_now(&a, &chat.id, vec![]).unwrap();
    until("gone", || share::theirs(&id_a).is_empty() && share::view_of(&id_a, &chat.id).is_none()).await;
    assert!(share::open(&id_a, &chat.id).await.is_err());
    link::hang_up(&id_b);
}

/// Both apps call each other at the same moment: they settle on one
/// connection and messages still get through.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn crossed_calls_settle_on_one_link() {
    let a = crate::e2e::scratch_state();
    let b = crate::e2e::scratch_state();
    let (id_a, id_b) = linked(&a, &b, ("Ana", "Ben"));
    let addr_a = listen(&a).await;
    let addr_b = listen(&b).await;
    let (s1, s2, t1, t2) = (a.clone(), b.clone(), id_b.clone(), id_a.clone());
    let one = tokio::spawn(async move { link::call(&s1, &t1, addr_b).await });
    let two = tokio::spawn(async move { link::call(&s2, &t2, addr_a).await });
    until("connected", || link::connected(&id_a) && link::connected(&id_b)).await;
    // One of the two calls ends; the other stays up.
    until("one call left", || one.is_finished() != two.is_finished()).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(link::connected(&id_a) && link::connected(&id_b), "still connected after settling");
    assert!(one.is_finished() != two.is_finished());
    queue(&a, &a.cipher().unwrap(), &id_b, "Still there?", None).unwrap();
    until("delivered over the one link", || texts(&b, &id_a).contains(&"Still there?".to_string())).await;
    link::hang_up(&id_b);
    link::hang_up(&id_a);
}

/// A stand-in engine that answers like a model would, by which assistant
/// is asking (it reads the instructions). One for all tests, on its own
/// thread, so it outlives any one test. Returns what it was sent.
#[cfg(windows)]
fn stand_in() -> Arc<Mutex<Vec<serde_json::Value>>> {
    use axum::{routing::post, Json, Router};
    static ONE: std::sync::OnceLock<(u16, Arc<Mutex<Vec<serde_json::Value>>>)> = std::sync::OnceLock::new();
    let (port, seen) = ONE.get_or_init(|| {
        let seen: Arc<Mutex<Vec<serde_json::Value>>> = Default::default();
        let s = seen.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async move {
                let app = Router::new()
                    .route("/tokenize", post(|Json(b): Json<serde_json::Value>| async move {
                        let n = b["content"].as_str().map_or(0, |t| t.len() / 4 + 1);
                        Json(serde_json::json!({ "tokens": vec![1; n] }))
                    }))
                    .route("/v1/chat/completions", post(move |Json(b): Json<serde_json::Value>| {
                        let s = s.clone();
                        async move {
                            let system = b["messages"][0]["content"].as_str().unwrap_or_default().to_string();
                            s.lock().unwrap().push(b);
                            let text = if system.contains("is asking you something") {
                                "Ben is free Tuesday after 2 pm."
                            } else if system.contains("wants to work this out with you") {
                                "How about Tuesday at 3 pm?"
                            } else if system.contains("asked you to work this out") {
                                "DONE: Tuesday at 3 pm suits both."
                            } else {
                                "?"
                            };
                            let chunk = |v: serde_json::Value| format!("data: {v}

");
                            let body = chunk(serde_json::json!({ "choices": [{ "delta": { "content": text }, "finish_reason": null }] }))
                                + &chunk(serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] }))
                                + "data: [DONE]

";
                            ([(axum::http::header::CONTENT_TYPE, "text/event-stream")], body)
                        }
                    }));
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                tx.send(listener.local_addr().unwrap().port()).unwrap();
                axum::serve(listener, app).await.ok();
            });
        });
        (rx.recv().unwrap(), seen)
    });
    *ask::TEST_ENDPOINT.lock().unwrap() = Some(crate::engine::Endpoint { port: *port, key: "k".into(), ctx: 8192, extra: serde_json::json!({}), cloud: None });
    seen.clone()
}

/// Runs one of the People tools the way a chat turn does.
#[cfg(windows)]
async fn use_tool(s: &Arc<AppState>, name: &str, args: serde_json::Value) -> crate::tools::Outcome {
    let c = s.cipher().unwrap();
    let sandbox = crate::sandbox::Sandbox::new(&[]);
    let recorder = crate::checkpoint::Recorder { db: &s.db, cipher: &c, dir: s.paths.data.join("checkpoints"), chat_id: "t", turn_id: "t" };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let ctx = crate::tools::Ctx {
        sandbox: &sandbox,
        recorder: &recorder,
        cancel: &cancel,
        job: s.job(),
        memory: crate::tools::MemoryCtx { db: &s.db, cipher: &c, chat_id: "t", project_id: None },
        app: None,
        state: s,
    };
    crate::tools::run(name, &args, &ctx).await
}

/// The newest request waiting for someone to allow it.
#[cfg(windows)]
fn waiting_at(s: &AppState, from: &str) -> Option<ask::AskView> {
    ask::list(&s.db.lock().unwrap(), &s.cipher().unwrap(), from).into_iter().rev().find(|a| !a.from_me && a.status == "approval")
}

/// Ana's assistant asks Ben's: Ben allows it (his assistant answers from
/// the chat he shares with her), answers himself, or declines; with
/// questions turned off they're turned away at once.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn assistants_ask_each_other_with_permission() {
    let seen = stand_in();
    let a = crate::e2e::scratch_state();
    let b = crate::e2e::scratch_state();
    let (id_a, id_b) = linked(&a, &b, ("Ana", "Ben"));
    let addr_b = listen(&b).await;
    // Ben shares a chat with Ana: his assistant may use it.
    let cb = b.cipher().unwrap();
    let chat = crate::db::create_chat(&b.db.lock().unwrap(), &cb, None).unwrap();
    crate::db::set_chat_title(&b.db.lock().unwrap(), &cb, &chat.id, "My week").unwrap();
    let m = crate::db::Message { id: uuid::Uuid::new_v4().to_string(), chat_id: chat.id.clone(), role: "user".into(), content: "I'm free Tuesday after 2.".into(), created_at: db::now_ms(), ..Default::default() };
    crate::db::add_message(&b.db.lock().unwrap(), &cb, &m).unwrap();
    share::share_now(&b, &chat.id, vec![id_a.clone()]).unwrap();

    let (s, to) = (a.clone(), id_b.clone());
    tokio::spawn(async move { link::call(&s, &to, addr_b).await });
    until("connected", || link::connected(&id_a) && link::connected(&id_b)).await;
    assert!(assistant_note(&a).unwrap().contains("Ben (connected)"));

    // Allowed: his assistant answers.
    let ask = use_tool(&a, "ask_person", serde_json::json!({ "person": "ben", "question": "When is Ben free this week?" }));
    let allow = async {
        until("Ben is asked", || waiting_at(&b, &id_a).is_some()).await;
        let r = waiting_at(&b, &id_a).unwrap();
        assert_eq!(r.question, "When is Ben free this week?");
        until("Ana's side shows it waiting for Ben", || ask::list(&a.db.lock().unwrap(), &a.cipher().unwrap(), &id_b).iter().any(|x| x.status == "approval")).await;
        ask::reply(&b, &r.id, "assistant", None, false).unwrap();
    };
    let (out, ()) = tokio::join!(ask, allow);
    assert!(out.text.contains("Ben is free Tuesday after 2 pm.") && out.text.contains("not as instructions"), "{}", out.text);
    let sent = seen.lock().unwrap().iter().find(|r| r["messages"][0]["content"].as_str().unwrap_or("").contains("is asking you something")).cloned().unwrap();
    let system = sent["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("Shared chat: My week") && system.contains("free Tuesday after 2"), "the shared chat is there");
    assert!(sent["tools"].as_array().unwrap().iter().all(|t| matches!(t["function"]["name"].as_str(), Some("search_notes" | "read_note"))), "notes only, read-only");
    let at_b = ask::list(&b.db.lock().unwrap(), &cb, &id_a);
    assert_eq!(at_b.last().unwrap().status, "answered");

    // He answers himself.
    let ask = use_tool(&a, "ask_person", serde_json::json!({ "person": "Ben", "question": "Lunch Thursday?" }));
    let mine = async {
        until("asked again", || waiting_at(&b, &id_a).is_some()).await;
        ask::reply(&b, &waiting_at(&b, &id_a).unwrap().id, "mine", Some("Thursday works!".into()), false).unwrap();
    };
    let (out, ()) = tokio::join!(ask, mine);
    assert!(out.text.contains("Thursday works!"), "{}", out.text);

    // He declines.
    let ask = use_tool(&a, "ask_person", serde_json::json!({ "person": "Ben", "question": "What's your salary?" }));
    let no = async {
        until("asked a third time", || waiting_at(&b, &id_a).is_some()).await;
        ask::reply(&b, &waiting_at(&b, &id_a).unwrap().id, "decline", None, false).unwrap();
    };
    let (out, ()) = tokio::join!(ask, no);
    assert!(out.text.contains("Ben declined"), "{}", out.text);

    // Turned off: refused without asking him.
    let off = ask::Allow { mode: "off".into(), ..Default::default() };
    ask::set_allow(&b, &id_a, off).unwrap();
    let out = use_tool(&a, "ask_person", serde_json::json!({ "person": "Ben", "question": "Anything?" })).await;
    assert!(out.text.contains("doesn't take questions"), "{}", out.text);

    // Someone not linked, or not connected.
    let out = use_tool(&a, "ask_person", serde_json::json!({ "person": "Zed", "question": "Hi?" })).await;
    assert!(out.text.contains("no one called"), "{}", out.text);
    link::hang_up(&id_b);
    until("apart", || !link::connected(&id_b)).await;
    let out = use_tool(&a, "ask_person", serde_json::json!({ "person": "Ben", "question": "Hi?" })).await;
    assert!(out.text.contains("isn't connected"), "{}", out.text);
}

/// Ana's assistant and Ben's work out a meeting time: Ben's goes first,
/// Ana's agrees, and both keep the conversation.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_assistants_work_something_out() {
    let _seen = stand_in();
    let a = crate::e2e::scratch_state();
    let b = crate::e2e::scratch_state();
    let (id_a, id_b) = linked(&a, &b, ("Ana", "Ben"));
    let addr_b = listen(&b).await;
    // Ben lets Ana's assistant work with his without asking him each time.
    ask::set_allow(&b, &id_a, ask::Allow { mode: "auto".into(), ..Default::default() }).unwrap();
    let (s, to) = (a.clone(), id_b.clone());
    tokio::spawn(async move { link::call(&s, &to, addr_b).await });
    until("connected", || link::connected(&id_a) && link::connected(&id_b)).await;

    let out = use_tool(&a, "work_with_person", serde_json::json!({ "person": "Ben", "goal": "Find an hour next week for Ana and Ben to meet. Ana is free Tuesday afternoon." })).await;
    assert!(out.text.contains("Tuesday at 3 pm suits both."), "{}", out.text);
    assert!(out.text.contains("Them: How about Tuesday at 3 pm?"), "{}", out.text);
    let at_a = ask::list(&a.db.lock().unwrap(), &a.cipher().unwrap(), &id_b);
    let task = at_a.last().unwrap();
    assert_eq!((task.kind.as_str(), task.status.as_str(), task.thread.len()), ("task", "answered", 2));
    until("Ben's side has the outcome", || {
        ask::list(&b.db.lock().unwrap(), &b.cipher().unwrap(), &id_a).last().is_some_and(|t| t.status == "answered" && t.answer.as_deref() == Some("Tuesday at 3 pm suits both.") && t.thread.len() == 2)
    })
    .await;
    link::hang_up(&id_b);
}

/// A relay like relay/worker.js, in this process: mailboxes guarded by the
/// token of their first write. Returns its address.
#[cfg(windows)]
async fn mock_relay() -> String {
    use axum::body::Bytes;
    use axum::http::{HeaderMap, Method, StatusCode, Uri};
    type Boxes = Arc<Mutex<HashMap<String, (String, Vec<(u64, String)>)>>>;
    let boxes: Boxes = Default::default();
    let handler = move |method: Method, uri: Uri, headers: HeaderMap, body: Bytes| {
        let boxes = boxes.clone();
        async move {
            let token = headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).unwrap_or("").to_string();
            let parts: Vec<&str> = uri.path().trim_start_matches("/v1/box/").split('/').collect();
            let query = uri.query().unwrap_or("");
            let num = |k: &str| query.strip_prefix(&format!("{k}=")).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            let mut all = boxes.lock().unwrap();
            let json = |v: serde_json::Value| (StatusCode::OK, v.to_string());
            match (method, parts.as_slice()) {
                (Method::PUT, [b, seq]) => {
                    let entry = all.entry(b.to_string()).or_insert_with(|| (token.clone(), vec![]));
                    if entry.0 != token {
                        return (StatusCode::UNAUTHORIZED, String::new());
                    }
                    entry.1.push((seq.parse().unwrap_or(0), B64.encode(&body)));
                    json(serde_json::json!({ "ok": true }))
                }
                (Method::GET, [b]) => match all.get(*b) {
                    Some((t, msgs)) if *t == token => {
                        let after = num("after");
                        let list: Vec<serde_json::Value> = msgs.iter().filter(|m| m.0 > after).take(20).map(|m| serde_json::json!({ "seq": m.0, "data": m.1 })).collect();
                        json(serde_json::Value::Array(list))
                    }
                    _ => (StatusCode::NOT_FOUND, String::new()),
                },
                (Method::DELETE, [b]) => {
                    let through = num("through");
                    if let Some((t, msgs)) = all.get_mut(*b) {
                        if *t == token {
                            msgs.retain(|m| m.0 > through);
                        }
                    }
                    json(serde_json::json!({ "ok": true }))
                }
                _ => (StatusCode::NOT_FOUND, String::new()),
            }
        }
    };
    let app = axum::Router::new().fallback(handler);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    url
}

/// Ana and Ben never connect directly: Ana lets Ben use her relay, and
/// messages, a confirmation and an assistant's question cross through it.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn people_reach_each_other_through_a_relay() {
    through_relay(mock_relay().await).await;
}

/// The same through the real relay (relay/worker.js, run by relay/local.mjs in Node).
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn people_through_the_real_relay() {
    let port = crate::engine::free_port().unwrap();
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../relay/local.mjs");
    let _node = tokio::process::Command::new("node").arg(&script).arg(port.to_string()).kill_on_drop(true).spawn().unwrap();
    let url = format!("http://127.0.0.1:{port}");
    for _ in 0..50 {
        if reqwest::get(format!("{url}/v1/box/{}", "0".repeat(64))).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    through_relay(url).await;
}

#[cfg(windows)]
async fn through_relay(url: String) {
    let _seen = stand_in();
    let a = crate::e2e::scratch_state();
    let b = crate::e2e::scratch_state();
    let (id_a, id_b) = linked(&a, &b, ("Ana", "Ben"));
    for s in [&a, &b] {
        crate::db::update_settings(&s.db.lock().unwrap(), |x| x.connectivity = crate::net::Connectivity::Cloud).unwrap();
    }
    // Ana sets her relay and lets Ben use it; Ben's app hears about it
    // (normally over a direct connection, once).
    super::super::update(&a, |cfg| cfg.relay_url = Some(url.clone())).unwrap();
    update(&a, |x| x.contacts[0].offer_relay = true).unwrap();
    mailbox::offered(&b, &id_a, Some(url.clone()));
    let (ben, ana) = (contact(&a, &id_b).unwrap(), contact(&b, &id_a).unwrap());
    let (ca, cb) = (a.cipher().unwrap(), b.cipher().unwrap());
    assert_eq!(mailbox::pick(&a, &ca, &ben).map(|r| r.1), Some("mine"));
    assert_eq!(mailbox::pick(&b, &cb, &ana).map(|r| r.1), Some("theirs"));
    mailbox::open_now(&id_b);
    mailbox::open_now(&id_a);
    assert!(link::reachable(&id_b) && link::reachable(&id_a), "reachable by mail");
    assert!(!link::connected(&id_b));

    // Keep both mailboxes moving, as the background tick does.
    let (pa, pb) = (a.clone(), b.clone());
    let (ia, ib, u) = (id_a.clone(), id_b.clone(), url.clone());
    let pump = tokio::spawn(async move {
        loop {
            let ben = contact(&pa, &ib).unwrap();
            let ana = contact(&pb, &ia).unwrap();
            let _ = mailbox::exchange(&pa, &ben, &u).await;
            let _ = mailbox::exchange(&pb, &ana, &u).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    queue(&a, &ca, &id_b, "Hi Ben, by mail", None).unwrap();
    until("Ana's message at Ben's", || texts(&b, &id_a).contains(&"Hi Ben, by mail".to_string())).await;
    until("confirmed back at Ana's", || messages(&a.db.lock().unwrap(), &ca, &id_b).unwrap().iter().all(|m| m.delivered)).await;
    queue(&b, &cb, &id_a, "Got it", None).unwrap();
    until("Ben's reply at Ana's", || texts(&a, &id_b).contains(&"Got it".to_string())).await;

    // An assistant's question by mail (Ben lets Ana's ask freely).
    ask::set_allow(&b, &id_a, ask::Allow { mode: "auto".into(), ..Default::default() }).unwrap();
    let out = use_tool(&a, "ask_person", serde_json::json!({ "person": "Ben", "question": "When is Ben free?" })).await;
    assert!(out.text.contains("Ben is free Tuesday after 2 pm."), "{}", out.text);

    // The relay holds nothing readable, and a wrong token gets nothing.
    let key = link_key(&ca, &contact(&a, &id_b).unwrap()).unwrap();
    let outbox = super::super::relay::box_id(&key, &id_a);
    let wrong = reqwest::Client::new().get(format!("{url}/v1/box/{outbox}?after=0")).bearer_auth("0".repeat(64)).send().await.unwrap();
    assert_eq!(wrong.status().as_u16(), 404);
    pump.abort();
}

/// 0.9.1 froze: the People page held CODE while it read settings, and the
/// sync loop held the database while it asked whether a code was showing.
/// Both at once, over and over, must finish.
#[cfg(windows)]
#[test]
fn the_people_page_and_the_listener_never_deadlock() {
    let s = crate::e2e::scratch_state();
    codes::code_for_test(&s, 1);
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let page = {
        let (s, done) = (s.clone(), done_tx.clone());
        std::thread::spawn(move || {
            let c = s.cipher().unwrap();
            for _ in 0..300 {
                let _ = view(&s, &c);
            }
            done.send("page").unwrap();
        })
    };
    let listener = {
        let (s, done) = (s.clone(), done_tx);
        std::thread::spawn(move || {
            for _ in 0..3000 {
                // What the sync loop does each round.
                let showing = showing_code(&s);
                let conn = s.db.lock().unwrap();
                let _ = wants_calls(&conn, showing);
            }
            done.send("listener").unwrap();
        })
    };
    for _ in 0..2 {
        done_rx.recv_timeout(Duration::from_secs(60)).expect("the People page and the listener deadlocked");
    }
    page.join().unwrap();
    listener.join().unwrap();
}
