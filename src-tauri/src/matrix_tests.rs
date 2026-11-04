// SPDX-License-Identifier: AGPL-3.0-only
use super::*;

/// A stand-in homeserver: each sync hands out the next queued batch, and
/// every message the assistant sends is kept.
#[derive(Default)]
struct Fake {
    batches: Mutex<Vec<Value>>,
    sent: Mutex<Vec<(String, Value)>>,
    joined: Mutex<Vec<String>>,
    left: Mutex<Vec<String>>,
}

async fn fake_homeserver() -> (Arc<Fake>, String) {
    use axum::{body::Bytes, extract::State, http::{Method, Uri}, Json, Router};
    let fake = Arc::new(Fake::default());
    let app = Router::new()
        .fallback(|State(f): State<Arc<Fake>>, method: Method, uri: Uri, body: Bytes| async move {
            let path = uri.path().to_string();
            let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let out = if path.ends_with("/account/whoami") {
                json!({ "user_id": "@helper:example.org" })
            } else if path.ends_with("/sync") {
                let next = { let mut b = f.batches.lock().unwrap(); (!b.is_empty()).then(|| b.remove(0)) };
                match next {
                    Some(v) => v,
                    None => {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        json!({ "next_batch": "idle" })
                    }
                }
            } else if path.contains("/join/") {
                f.joined.lock().unwrap().push(path.clone());
                json!({})
            } else if path.ends_with("/leave") {
                f.left.lock().unwrap().push(path.clone());
                json!({})
            } else {
                if method == Method::PUT && path.contains("/send/") {
                    f.sent.lock().unwrap().push((path.clone(), body));
                }
                json!({ "event_id": "$e" })
            };
            Json(out)
        })
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (fake, format!("http://{addr}"))
}

fn msg(sender: &str, body: &str) -> Value {
    json!({ "type": "m.room.message", "sender": sender, "content": { "msgtype": "m.text", "body": body } })
}

fn batch(next: &str, room: &str, events: Vec<Value>) -> Value {
    json!({ "next_batch": next, "rooms": { "join": { room: { "timeline": { "events": events } } } } })
}

async fn sent_in(fake: &Fake, room: &str, n: usize) -> Vec<String> {
    let enc_room = enc(room);
    for _ in 0..100 {
        let got: Vec<String> = fake.sent.lock().unwrap().iter().filter(|(p, _)| p.contains(&enc_room)).map(|(_, b)| b["body"].as_str().unwrap_or("").to_string()).collect();
        if got.len() >= n {
            return got;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("fewer than {n} messages in {room}: {:?}", fake.sent.lock().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_person_links_with_the_code_and_only_they_are_answered() {
    let (fake, base) = fake_homeserver().await;
    let state = crate::testutil::scratch_state();
    db::update_settings(&state.db.lock().unwrap(), |s| s.connectivity = crate::net::Connectivity::Web).unwrap();
    save(&state, &Link { homeserver: base, token: "tok".into(), me: "@helper:example.org".into(), code: Some("424242".into()), tasks: true, ..Default::default() }).unwrap();
    let runner: Runner = Arc::new(|_state: Arc<AppState>, _chat: String, text: String, _images: Vec<String>| {
        Box::pin(async move { Ok(Some(db::Message { content: format!("you said: {text}"), ..Default::default() })) })
    });
    let room = "!room:example.org";
    let invite = json!({ "next_batch": "s2", "rooms": { "invite": { room: { "invite_state": { "events": [
        { "type": "m.room.member", "state_key": "@helper:example.org", "sender": "@sam:example.org", "content": { "membership": "invite" } }
    ] } } } } });
    fake.batches.lock().unwrap().extend([
        // History from before linking is skipped, even the code.
        batch("s1", room, vec![msg("@sam:example.org", "424242")]),
        invite,
        batch("s3", room, vec![msg("@sam:example.org", "000000"), msg("@sam:example.org", "424242"), msg("@eve:example.org", "hello"), msg("@helper:example.org", "my own"), msg("@sam:example.org", "hello")]),
    ]);
    run(runner, state.clone());
    let said = sent_in(&fake, room, 2).await;
    assert!(said[0].starts_with("Linked."), "{said:?}");
    assert!(said.iter().any(|t| t == "you said: hello"), "{said:?}");
    assert_eq!(said.len(), 2, "the stranger and old history get nothing: {said:?}");
    assert_eq!(fake.joined.lock().unwrap().len(), 1);
    let link = load(&state).unwrap();
    assert_eq!(link.person.as_deref(), Some("@sam:example.org"));
    assert_eq!(link.room.as_deref(), Some(room));
    assert!(link.since.is_some());
    // After linking, an invite from someone else is turned down.
    fake.batches.lock().unwrap().push(json!({ "next_batch": "s4", "rooms": { "invite": { "!other:example.org": { "invite_state": { "events": [
        { "type": "m.room.member", "state_key": "@helper:example.org", "sender": "@eve:example.org" }
    ] } } } } }));
    for _ in 0..50 {
        if !fake.left.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(fake.left.lock().unwrap().len(), 1);
    assert_eq!(fake.joined.lock().unwrap().len(), 1);
    stop();
}

#[test]
fn homeservers_must_use_https_except_on_this_pc() {
    assert_eq!(clean_homeserver("matrix.org").unwrap(), "https://matrix.org");
    assert_eq!(clean_homeserver(" https://m.example.com/ ").unwrap(), "https://m.example.com");
    assert!(clean_homeserver("http://m.example.com").is_err());
    assert!(clean_homeserver("http://127.0.0.1:8008").is_ok());
    assert!(clean_homeserver("not a url at all").is_err());
}

#[test]
fn ids_are_escaped_and_media_addresses_checked() {
    assert_eq!(enc("!abc:example.org"), "%21abc%3Aexample.org");
    assert_eq!(enc("@sam:m.org"), "%40sam%3Am.org");
    let p = media_paths("mxc://example.org/AbC123").unwrap();
    assert_eq!(p[0], "/_matrix/client/v1/media/download/example.org/AbC123");
    assert!(media_paths("mxc://example.org/../../x").is_none());
    assert!(media_paths("https://example.org/x").is_none());
}

#[test]
fn a_typed_number_picks_an_offered_option() {
    *OFFERED.lock().unwrap() = vec!["Tea".into(), "Coffee".into()];
    assert_eq!(picked("2").as_deref(), Some("Coffee"));
    assert_eq!(picked("1.").as_deref(), Some("Tea"));
    assert_eq!(picked("3"), None);
    assert_eq!(picked("0"), None);
    assert_eq!(picked("tea"), None);
}
