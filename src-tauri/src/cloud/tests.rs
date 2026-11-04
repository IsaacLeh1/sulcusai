use super::*;

#[test]
fn cache_marks_go_where_anthropic_can_reuse_them() {
    assert_eq!(mark_cache("", &mut [], &mut []), Value::Null);
    let mut tools = vec![json!({ "name": "a" }), json!({ "name": "b" })];
    let mut msgs = vec![
        json!({ "role": "user", "content": [{ "type": "text", "text": "first" }] }),
        json!({ "role": "assistant", "content": [{ "type": "text", "text": "said" }, { "type": "thinking", "thinking": "hmm", "signature": "s" }] }),
        json!({ "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "ok" }, { "type": "text", "text": "" }] }),
    ];
    let system = mark_cache("Be brief.", &mut msgs, &mut tools);
    assert!(tools[0].get("cache_control").is_none() && tools[1].get("cache_control").is_some(), "the last tool only");
    assert!(msgs[0]["content"][0].get("cache_control").is_none(), "not the older messages");
    assert!(msgs[1]["content"][0].get("cache_control").is_some(), "the text before the thinking");
    assert!(msgs[2]["content"][0].get("cache_control").is_some() && msgs[2]["content"][1].get("cache_control").is_none(), "the tool result, not the empty text");
    assert_eq!(system.as_array().map(Vec::len), Some(1));
    let all = format!("{system}{}{}", json!(msgs), json!(tools));
    assert!(all.matches("cache_control").count() <= 4);
}

#[test]
fn cached_input_costs_less() {
    let m = CloudModel { price_in: Some(10.0), price_out: Some(50.0), ..Default::default() };
    // A million in (half read from the cache) and none out.
    let a = cost(Kind::Anthropic, &m, 1_000_000, 500_000, 0, 0);
    assert!((a - (5.0 + 0.5)).abs() < 1e-9, "{a}");
    let o = cost(Kind::Openai, &m, 1_000_000, 500_000, 0, 0);
    assert!((o - (5.0 + 2.5)).abs() < 1e-9, "{o}");
    // Writing to Anthropic's cache costs a quarter more; output as before.
    let w = cost(Kind::Anthropic, &m, 1_000_000, 0, 1_000_000, 1_000_000);
    assert!((w - (12.5 + 50.0)).abs() < 1e-9, "{w}");
    // More cached than sent is held to what was sent.
    assert!((cost(Kind::Anthropic, &m, 100, 500, 0, 0) - 100.0 * 0.1 * 10.0 / 1e6).abs() < 1e-12);
}

#[test]
fn busy_or_broken_providers_are_tried_again_but_not_empty_accounts() {
    assert!(worth_retrying(429, "rate_limit_error: too many requests"));
    assert!(!worth_retrying(429, "You exceeded your current quota (insufficient_quota)"));
    for code in [529, 503, 500] {
        assert!(worth_retrying(code, ""), "{code}");
    }
    for code in [400, 401, 403, 404] {
        assert!(!worth_retrying(code, ""), "{code}");
    }
}

#[test]
fn the_wait_a_provider_asks_for_is_read() {
    use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
    let with = |name: &'static str, v: &str| {
        let mut h = HeaderMap::new();
        h.insert(name, HeaderValue::from_str(v).unwrap());
        h
    };
    assert_eq!(retry_after(&with("retry-after", "7")), Some(Duration::from_secs(7)));
    assert_eq!(retry_after(&with("retry-after", "1.5")), Some(Duration::from_millis(1500)));
    assert_eq!(retry_after(&with("retry-after-ms", "250")), Some(Duration::from_millis(250)));
    let soon = (chrono::Utc::now() + chrono::Duration::seconds(10)).to_rfc2822();
    let d = retry_after(&with("retry-after", &soon)).unwrap();
    assert!(d >= Duration::from_secs(8) && d <= Duration::from_secs(10), "{d:?}");
    assert_eq!(retry_after(&with("retry-after", "soon-ish")), None);
    assert_eq!(retry_after(&HeaderMap::new()), None);
    let _ = RETRY_AFTER;
}

#[test]
fn waits_grow_and_spread() {
    for n in 0..=6u32 {
        let base = 1000u64 << n.min(4);
        let w = backoff(n, None).as_millis() as u64;
        assert!(w >= base * 3 / 4 && w <= (base * 5 / 4).min(30_000), "try {n}: {w} ms");
    }
    assert_eq!(backoff(0, Some(Duration::from_secs(3))), Duration::from_secs(3));
}

#[test]
fn some_failures_are_final() {
    let again = Failed::again("busy", None);
    assert!(next_wait(&again, 0, false).is_some());
    assert!(next_wait(&again, 0, true).is_none(), "part of the reply was shown");
    assert!(next_wait(&again, RETRIES, false).is_none(), "out of tries");
    assert!(next_wait(&Failed::fatal("bad key"), 0, false).is_none());
    assert!(next_wait(&Failed::again("later", Some(Duration::from_secs(120))), 0, false).is_none(), "asked to wait too long");
}

/// A provider that's busy twice (503, asking for 50 ms) and then answers:
/// the reply comes on the third try.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_busy_provider_is_tried_again_until_it_answers() {
    use std::sync::atomic::AtomicUsize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let asked = Arc::new(AtomicUsize::new(0));
    let count = asked.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let n = count.fetch_add(1, Ordering::SeqCst);
            let mut buf = vec![0u8; 65_536];
            let _ = sock.read(&mut buf).await;
            let reply = if n < 2 {
                let body = r#"{"error":{"message":"busy, try again"}}"#;
                format!("HTTP/1.1 503 Service Unavailable\r\nretry-after-ms: 50\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len())
            } else {
                let body = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\ndata: [DONE]\n\n";
                format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len())
            };
            let _ = sock.write_all(reply.as_bytes()).await;
            let _ = sock.shutdown().await;
        }
    });
    let state = crate::testutil::temp_state(None);
    db::update_settings(&state.db.lock().unwrap(), |s| s.connectivity = Connectivity::Cloud).unwrap();
    let model = CloudModel { id: "m".into(), name: "M".into(), ..Default::default() };
    let t = Target { kind: Kind::Openai, provider: "Test".into(), base_url: format!("http://127.0.0.1:{port}"), key: "k".into(), cookies: String::new(), model, redact: false, state, chat: std::sync::Mutex::new(None), proxy: String::new() };
    let out = stream(&t, &json!({}), vec![json!({ "role": "user", "content": "hi" })], None, &AtomicBool::new(false), |_| {}).await.unwrap();
    assert_eq!(out.content, "Hello");
    assert_eq!(asked.load(Ordering::SeqCst), 3, "two busy answers, then the reply");
}

#[tokio::test]
async fn stop_ends_a_wait_at_once() {
    let stopped = AtomicBool::new(true);
    let t = std::time::Instant::now();
    assert!(!pause(Duration::from_secs(5), &stopped).await);
    assert!(t.elapsed() < Duration::from_millis(100));
    assert!(pause(Duration::from_millis(300), &AtomicBool::new(false)).await);
}

#[test]
fn numbers_from_other_countries_are_phone_numbers() {
    let mut r = Redactor::default();
    let out = r.redact("Ring +44 20 7946 0958 or +49 30 1234 5678 after 6.");
    assert!(!out.contains("7946") && !out.contains("1234 5678"), "{out}");
    assert!(r.redact("version 2.5.8 of 12 files").contains("2.5.8"));
}

#[test]
fn redacts_and_restores_personal_details() {
    let mut r = Redactor::default();
    let out = r.redact("Email jo@example.com or call (801) 555-0123. Card 4111 1111 1111 1111, SSN 123-45-6789. Order 12345678901234 isn't a card.");
    assert!(!out.contains("jo@example.com") && !out.contains("555-0123") && !out.contains("4111") && !out.contains("123-45-6789"), "{out}");
    assert!(out.contains("[email 1]") && out.contains("[card number 1]") && out.contains("[SSN 1]"), "{out}");
    assert!(out.contains("12345678901234"), "fails the card checksum, so kept: {out}");
    // The same detail gets the same placeholder.
    assert_eq!(r.redact("again jo@example.com"), "again [email 1]");
    assert_eq!(r.restore("I'll write to [email 1]."), "I'll write to jo@example.com.");
}

#[test]
fn names_keys_addresses_and_earlier_replies_are_redacted_too() {
    let mut r = Redactor::with_names(&["Isaac Lehman".into(), "Isaac".into(), "Lehman".into()]);
    let out = r.redact("Isaac Lehman lives at 1234 North Main Street; his key is sk-abc123DEF456ghi789JKL012mno. Isaacson isn't him.");
    assert!(!out.contains("Lehman") && !out.contains("Main Street") && !out.contains("sk-abc"), "{out}");
    assert!(out.contains("[name 1]") && out.contains("[address 1]") && out.contains("[secret 1]"), "{out}");
    assert!(out.contains("Isaacson"), "only whole words: {out}");
    assert_eq!(r.restore("Hi [name 1]!"), "Hi Isaac Lehman!");
    // Earlier replies and tool calls are swapped too, so real values
    // put back for the user don't go out again.
    let mut messages = vec![json!({ "role": "assistant", "content": "Sent to jo@example.com.", "tool_calls": [{ "id": "t1", "type": "function", "function": { "name": "email_send", "arguments": "{\"to\":\"jo@example.com\"}" } }] })];
    let mut r = Redactor::default();
    redact_messages(&mut messages, &mut r);
    assert_eq!(messages[0]["content"], "Sent to [email 1].");
    assert_eq!(messages[0]["tool_calls"][0]["function"]["arguments"], "{\"to\":\"[email 1]\"}");
}

#[test]
fn converts_a_tool_conversation_for_anthropic() {
    let messages = vec![
        json!({ "role": "system", "content": "Be brief." }),
        json!({ "role": "user", "content": "Weather in Paris?" }),
        json!({ "role": "assistant", "content": "", "tool_calls": [{ "id": "t1", "type": "function", "function": { "name": "weather", "arguments": "{\"place\":\"Paris\"}" } }] }),
        json!({ "role": "tool", "tool_call_id": "t1", "content": "18°C" }),
        json!({ "role": "user", "content": [{ "type": "text", "text": "And this?" }, { "type": "image_url", "image_url": { "url": "data:image/jpeg;base64,AAAA" } }] }),
        json!({ "role": "assistant", "content": "x", "cloud_blocks": [{ "type": "thinking", "thinking": "hm", "signature": "s" }, { "type": "tool_use", "id": "t2", "name": "look", "input": {} }] }),
        json!({ "role": "tool", "tool_call_id": "t2", "content": "a cat" }),
    ];
    let tools = json!([{ "type": "function", "function": { "name": "weather", "description": "d", "parameters": { "type": "object" } } }]);
    let (system, msgs, tools) = to_anthropic(&messages, Some(&tools));
    assert_eq!(system, "Be brief.");
    assert_eq!(msgs.len(), 5);
    assert_eq!(msgs[1]["content"][0]["input"]["place"], "Paris");
    // The tool result and the next user message share one user turn.
    assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
    assert_eq!(msgs[2]["content"][2]["source"]["media_type"], "image/jpeg");
    // The turn in progress keeps its thinking block, signature and all.
    assert_eq!(msgs[3]["content"][0]["signature"], "s");
    assert_eq!(msgs[4]["content"][0]["tool_use_id"], "t2");
    assert_eq!(tools[0]["input_schema"]["type"], "object");
}

#[test]
fn thinking_from_earlier_turns_is_left_out() {
    let messages = vec![
        json!({ "role": "user", "content": "hi" }),
        json!({ "role": "assistant", "content": "hello", "cloud_blocks": [{ "type": "thinking", "thinking": "hm", "signature": "s" }, { "type": "text", "text": "hello" }] }),
        json!({ "role": "user", "content": "bye" }),
    ];
    let (_, msgs, _) = to_anthropic(&messages, None);
    assert_eq!(msgs[1]["content"].as_array().unwrap().len(), 1);
    assert_eq!(msgs[1]["content"][0]["type"], "text");
}

#[test]
fn proxy_addresses_are_checked_and_carry_the_providers_requests() {
    // A provider whose address only the proxy can reach (as a server
    // inside a company network is reached only through the VPN).
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    rt.block_on(async {
        use axum::{routing::get, Json, Router};
        let seen: Arc<std::sync::Mutex<Vec<String>>> = Default::default();
        let s = seen.clone();
        let app = Router::new().route(
            "/v1/models",
            get(move |uri: axum::http::Uri| {
                let s = s.clone();
                async move {
                    s.lock().unwrap().push(uri.to_string());
                    Json(serde_json::json!({ "data": [{ "id": "gpt-4o" }] }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.ok() });
        let p = Provider { id: "custom-vpn".into(), name: "LiteLLM".into(), kind: Kind::Openai, base_url: "http://llm.corp.invalid:4000/v1".into(), enabled: true, key: String::new(), models: vec![], media: vec![], cookies: String::new(), share_pcs: false, share_people: vec![], proxy: String::new() };
        let direct = net::external_client_via(Connectivity::Cloud, Purpose::Cloud, false, None).unwrap();
        assert!(list_models(&p, "k", "", &direct).await.is_err(), "not reachable directly");
        let proxied = net::external_client_via(Connectivity::Cloud, Purpose::Cloud, false, Some(&format!("http://127.0.0.1:{port}"))).unwrap();
        let models = list_models(&p, "k", "", &proxied).await.unwrap();
        assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["gpt-4o"]);
        assert!(seen.lock().unwrap()[0].contains("llm.corp.invalid"), "the proxy was asked for the provider's address");
    });
}

#[test]
fn pasted_endpoints_come_off_a_servers_address() {
    assert_eq!(clean_base("https://litellm.example.edu/v1/models"), "https://litellm.example.edu/v1");
    assert_eq!(clean_base("https://x.example/v1/chat/completions/"), "https://x.example/v1");
    assert_eq!(clean_base(" https://x.example/api/ "), "https://x.example/api");
    assert_eq!(clean_base("https://api.anthropic.com"), "https://api.anthropic.com");
    assert_eq!(clean_base("https://litellm.example.edu/v1/models/v1"), "https://litellm.example.edu/v1");
    assert_eq!(clean_base("https://litellm.example.edu/models/v1"), "https://litellm.example.edu/v1");
    assert_eq!(clean_base("https://litellm.example.edu/v1"), "https://litellm.example.edu/v1");
}

#[test]
fn proxy_addresses_are_checked() {
    assert!(proxy_problem("socks5h://127.0.0.1:11080").is_none());
    assert!(proxy_problem("http://proxy.corp.example:3128").is_none());
    assert!(proxy_problem("127.0.0.1:1080").is_some(), "the kind is needed");
    assert!(proxy_problem("socks5h://127.0.0.1").is_some(), "and the port");
    assert!(proxy_problem("ftp://x:21").is_some());
}

#[test]
fn model_refs_round_trip() {
    let id = model_ref("openrouter", "meta-llama/llama-4:free");
    assert_eq!(parse_ref(&id), Some(("openrouter", "meta-llama/llama-4:free")));
    assert!(is_cloud(&id) && !is_cloud("qwen3-1.7b"));
}

/// A chat synced from the laptop names the laptop's provider id: the same
/// server here is found by its model, and its id is the same anyway now.
#[test]
fn a_synced_chats_model_is_found_on_this_pc() {
    assert_eq!(custom_id("https://llm.example.com/v1/"), custom_id("https://LLM.example.com/v1"));
    assert_ne!(custom_id("https://llm.example.com/v1"), custom_id("https://other.example.com/v1"));
    let conn = Connection::open_in_memory().unwrap();
    crate::db::init_for_test(&conn);
    let model = CloudModel { id: "gpt-4o".into(), name: "GPT-4o".into(), ..Default::default() };
    let here = Provider {
        id: custom_id("https://llm.example.com/v1"),
        name: "LiteLLM".into(),
        kind: Kind::Openai,
        base_url: "https://llm.example.com/v1".into(),
        enabled: true,
        key: "sealed".into(),
        models: vec![model],
        media: vec![],
        cookies: String::new(),
        share_pcs: false,
        share_people: vec![],
        proxy: String::new(),
    };
    save(&conn, &CloudSettings { providers: vec![here.clone()], ..Default::default() }).unwrap();
    let mine = model_ref(&here.id, "gpt-4o");
    assert_eq!(resolve(&conn, &mine).as_deref(), Some(mine.as_str()));
    // The laptop's old random id: the same model here.
    assert_eq!(resolve(&conn, "cloud:custom-1a2b3c4d:gpt-4o").as_deref(), Some(mine.as_str()));
    assert_eq!(resolve(&conn, "cloud:custom-1a2b3c4d:claude-opus"), None, "a model this PC doesn't have");
    assert_eq!(provider_of(&settings(&conn), "cloud:custom-1a2b3c4d:gpt-4o").map(|p| p.id.as_str()), Some(here.id.as_str()));
}
