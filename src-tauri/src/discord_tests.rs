// SPDX-License-Identifier: AGPL-3.0-only
use super::*;

/// A stand-in for Discord's REST API: keeps every message the bot sends.
#[derive(Default)]
struct Fake {
    sent: Mutex<Vec<(String, Value)>>,
}

async fn fake_discord() -> (Arc<Fake>, String) {
    use axum::{body::Bytes, extract::State, http::{Method, Uri}, Json, Router};
    let fake = Arc::new(Fake::default());
    let app = Router::new()
        .fallback(|State(f): State<Arc<Fake>>, method: Method, uri: Uri, body: Bytes| async move {
            let path = uri.path().to_string();
            if method == Method::POST && path.ends_with("/messages") {
                f.sent.lock().unwrap().push((path, serde_json::from_slice(&body).unwrap_or(Value::Null)));
            }
            Json(json!({ "id": "1" }))
        })
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (fake, format!("http://{addr}"))
}

fn dm(from: &str, channel: &str, text: &str) -> Value {
    json!({ "channel_id": channel, "author": { "id": from, "username": if from == "7" { "sam" } else { "eve" } }, "content": text, "attachments": [] })
}

async fn sent_in(fake: &Fake, channel: &str, n: usize) -> Vec<String> {
    for _ in 0..100 {
        let got: Vec<String> = fake.sent.lock().unwrap().iter().filter(|(p, _)| p.contains(&format!("/channels/{channel}/"))).map(|(_, b)| b["content"].as_str().unwrap_or("").to_string()).collect();
        if got.len() >= n {
            return got;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("fewer than {n} messages in {channel}: {:?}", fake.sent.lock().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_linked_account_is_answered_and_servers_are_ignored() {
    let (fake, base) = fake_discord().await;
    *TEST_API.lock().unwrap() = Some(base);
    let state = crate::testutil::scratch_state();
    db::update_settings(&state.db.lock().unwrap(), |s| s.connectivity = crate::net::Connectivity::Web).unwrap();
    save(&state, &Link { token: "t".into(), bot: "helper".into(), bot_id: "99".into(), code: Some("424242".into()), tasks: true, ..Default::default() }).unwrap();
    let runner: Runner = Arc::new(|_s: Arc<AppState>, _c: String, text: String, _i: Vec<String>| Box::pin(async move { Ok(Some(db::Message { content: format!("you said: {text}"), ..Default::default() })) }));
    let client = reqwest::Client::new();
    let mut in_server = dm("7", "c1", "424242");
    in_server["guild_id"] = json!("g1");
    for m in [dm("7", "c1", "000000"), in_server, dm("7", "c1", "424242"), dm("8", "c2", "hello"), dm("99", "c1", "my own"), dm("7", "c1", "hello")] {
        let link = load(&state).unwrap();
        handle(&runner, &state, &client, link, &m).await;
    }
    let said = sent_in(&fake, "c1", 2).await;
    assert!(said[0].starts_with("Linked."), "{said:?}");
    assert_eq!(said[1], "you said: hello");
    assert!(!fake.sent.lock().unwrap().iter().any(|(p, _)| p.contains("/c2/")), "a stranger gets nothing");
    let link = load(&state).unwrap();
    assert_eq!((link.person.as_deref(), link.channel.as_deref(), link.person_name.as_str()), (Some("7"), Some("c1"), "sam"));
    *TEST_API.lock().unwrap() = None;
}

#[test]
fn gateway_frames_ask_for_direct_messages_only() {
    let id = identify("tok");
    assert_eq!(id["op"], 2);
    assert_eq!(id["d"]["intents"], 4096);
    assert_eq!(heartbeat(Some(5)), json!({ "op": 1, "d": 5 }));
    assert_eq!(heartbeat(None), json!({ "op": 1, "d": null }));
}

#[test]
fn files_come_only_from_discord() {
    assert!(from_discord_cdn("https://cdn.discordapp.com/attachments/1/2/a.png?ex=1"));
    assert!(from_discord_cdn("https://media.discordapp.net/attachments/1/2/a.png"));
    assert!(!from_discord_cdn("http://cdn.discordapp.com/a.png"));
    assert!(!from_discord_cdn("https://cdn.discordapp.com.evil.example/a.png"));
    assert!(!from_discord_cdn("https://127.0.0.1/a.png"));
}
