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

/// A code links two apps once, and a wrong code doesn't; a linked person
/// is a contact, never a paired PC.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_code_links_two_apps_once() {
    let a = crate::e2e::scratch_state();
    let b = crate::e2e::scratch_state();
    update(&a, |x| x.name = "Ana".into()).unwrap();
    let addr_a = listen(&a).await;
    *CODE.lock().unwrap() = Some(Pairing { code: "K7Q29XMB".into(), until: Instant::now() + CODE_FOR, tries: 0 });
    let wrong = pair_call(&b, "K7Q29XMC", addr_a).await.unwrap_err();
    assert!(wrong.contains("didn't match"), "{wrong}");
    assert!(book(&a.db.lock().unwrap()).contacts.is_empty());
    assert_eq!(pair_call(&b, "K7Q29XMB", addr_a).await.unwrap(), "Ana");
    assert!(CODE.lock().unwrap().is_none(), "a code works once");
    let again = pair_call(&b, "K7Q29XMB", addr_a).await.unwrap_err();
    assert!(again.contains("aren't showing a code"), "{again}");
    assert_eq!(book(&a.db.lock().unwrap()).contacts.len(), 1);
    assert_eq!(book(&b.db.lock().unwrap()).contacts[0].id, device_id(&a.paths.data));
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
