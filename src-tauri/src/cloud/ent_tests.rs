// SPDX-License-Identifier: AGPL-3.0-only
//! Enterprise providers against local stand-ins: nothing here reaches a
//! real provider or sign-in service.

use super::enterprise::{self, Auth, Ent};
use super::*;
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A local server that records each request's text and answers with what
/// `reply` makes of it (a whole HTTP response).
async fn stand_in(reply: impl Fn(&str) -> Vec<u8> + Send + Sync + 'static) -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let reply = Arc::new(reply);
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = vec![0u8; 65_536];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let text = String::from_utf8_lossy(&buf[..n]).to_string();
            log.lock().unwrap().push(text.clone());
            let _ = sock.write_all(&reply(&text)).await;
            let _ = sock.shutdown().await;
        }
    });
    (port, seen)
}

fn http(status: &str, ctype: &str, body: &[u8]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status}\r\ncontent-type: {ctype}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body);
    out
}

fn target(kind: Kind, base_url: String, key: &str, model: &str, ent: Ent, wire: Wire) -> Target {
    let state = crate::testutil::temp_state(None);
    db::update_settings(&state.db.lock().unwrap(), |s| s.connectivity = Connectivity::Cloud).unwrap();
    Target { kind, provider: "Test".into(), base_url, key: key.into(), cookies: String::new(), model: CloudModel { id: model.into(), name: model.into(), max_output: 1000, ..Default::default() }, redact: false, state, chat: Default::default(), wire, provider_id: format!("test-{model}"), ent }
}

const SSE_HELLO: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":1}}\n\ndata: [DONE]\n\n";

#[test]
fn signature_v4_matches_aws_example() {
    // AWS's published get-vanilla example.
    let now = chrono::DateTime::parse_from_rfc3339("2015-08-30T12:36:00Z").unwrap().with_timezone(&chrono::Utc);
    let h = bedrock::sign("GET", "example.amazonaws.com", "/", &[], b"", "us-east-1", "service", "AKIDEXAMPLE", "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", None, now);
    let auth = &h.iter().find(|(k, _)| k == "authorization").unwrap().1;
    assert!(auth.ends_with("Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"), "{auth}");
    assert!(auth.contains("Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date"));
}

#[test]
fn bedrock_keys_and_profiles_are_read() {
    assert_eq!(bedrock::parse_key("AKID:SECRET"), Some(bedrock::Creds::Keys { id: "AKID".into(), secret: "SECRET".into(), session: None }));
    assert!(matches!(bedrock::parse_key("AKID:SECRET:TOKEN"), Some(bedrock::Creds::Keys { session: Some(_), .. })));
    assert_eq!(bedrock::parse_key("ABSKbedrockapikey"), Some(bedrock::Creds::Bearer("ABSKbedrockapikey".into())));
    assert_eq!(bedrock::parse_key(""), None);
    let dir = std::env::temp_dir().join(format!("sulcusai-aws-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("credentials"), "[default]\naws_access_key_id = A1\naws_secret_access_key = S1\n\n[work]\naws_access_key_id=A2\naws_secret_access_key=S2\naws_session_token=T2\n").unwrap();
    std::fs::write(dir.join("config"), "[default]\nregion = us-west-2\n[profile work]\nregion = eu-central-1\n").unwrap();
    let (c, r) = bedrock::profile("work", Some(dir.clone())).unwrap();
    assert_eq!(c, bedrock::Creds::Keys { id: "A2".into(), secret: "S2".into(), session: Some("T2".into()) });
    assert_eq!(r.as_deref(), Some("eu-central-1"));
    assert_eq!(bedrock::profile("", Some(dir.clone())).unwrap().1.as_deref(), Some("us-west-2"));
    assert!(bedrock::profile("sso-only", Some(dir.clone())).unwrap_err().contains("SSO"));
}

#[test]
fn converse_body_carries_text_images_and_tools() {
    let msgs = vec![
        json!({ "role": "system", "content": "Be brief." }),
        json!({ "role": "user", "content": [{ "type": "text", "text": "what is this" }, { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAA" } }] }),
        json!({ "role": "assistant", "content": "", "tool_calls": [{ "id": "t1", "function": { "name": "look", "arguments": "{\"x\":1}" } }] }),
        json!({ "role": "tool", "tool_call_id": "t1", "content": "a cat" }),
    ];
    let tools = json!([{ "type": "function", "function": { "name": "look", "description": "Look", "parameters": { "type": "object" } } }]);
    let b = bedrock::converse_body(2000, &json!({ "temperature": 0.2 }), &msgs, Some(&tools));
    assert_eq!(b["system"][0]["text"], "Be brief.");
    assert_eq!(b["messages"][0]["content"][1]["image"]["format"], "png");
    assert_eq!(b["messages"][1]["content"][0]["toolUse"]["input"]["x"], 1);
    assert_eq!(b["messages"][2]["content"][0]["toolResult"]["content"][0]["text"], "a cat");
    assert_eq!(b["toolConfig"]["tools"][0]["toolSpec"]["inputSchema"]["json"]["type"], "object");
    assert_eq!(b["inferenceConfig"]["maxTokens"], 2000);
    assert_eq!(b["inferenceConfig"]["temperature"], 0.2);
}

#[test]
fn event_stream_frames_come_apart() {
    let mut buf = bedrock::frame("contentBlockDelta", &json!({ "delta": { "text": "Hi" } }));
    buf.extend(bedrock::frame("messageStop", &json!({ "stopReason": "end_turn" })));
    let half = buf.split_off(buf.len() - 5);
    let f = bedrock::next_frame(&mut buf).unwrap().unwrap();
    assert_eq!(f.headers[":event-type"], "contentBlockDelta");
    assert_eq!(serde_json::from_slice::<Value>(&f.payload).unwrap()["delta"]["text"], "Hi");
    assert!(bedrock::next_frame(&mut buf).unwrap().is_none(), "the second isn't whole yet");
    buf.extend(half);
    assert_eq!(bedrock::next_frame(&mut buf).unwrap().unwrap().headers[":event-type"], "messageStop");
    assert!(buf.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bedrock_streams_text_and_tool_calls_signed() {
    let mut body = Vec::new();
    body.extend(bedrock::frame("messageStart", &json!({ "role": "assistant" })));
    body.extend(bedrock::frame("contentBlockDelta", &json!({ "contentBlockIndex": 0, "delta": { "text": "Checking" } })));
    body.extend(bedrock::frame("contentBlockStart", &json!({ "contentBlockIndex": 1, "start": { "toolUse": { "toolUseId": "tu1", "name": "look" } } })));
    body.extend(bedrock::frame("contentBlockDelta", &json!({ "contentBlockIndex": 1, "delta": { "toolUse": { "input": "{\"x\":" } } })));
    body.extend(bedrock::frame("contentBlockDelta", &json!({ "contentBlockIndex": 1, "delta": { "toolUse": { "input": "2}" } } })));
    body.extend(bedrock::frame("messageStop", &json!({ "stopReason": "tool_use" })));
    body.extend(bedrock::frame("metadata", &json!({ "usage": { "inputTokens": 12, "outputTokens": 7 } })));
    let (port, seen) = stand_in(move |_| http("200 OK", "application/vnd.amazon.eventstream", &body)).await;
    let ent = Ent { region: "us-east-1".into(), ..Default::default() };
    let wire = Wire { headers: vec![("x-team".into(), "research".into())], ..Default::default() };
    let t = target(Kind::Bedrock, format!("http://127.0.0.1:{port}"), "AKIDTEST:secret", "us.anthropic.claude-sonnet-4-6-v1:0", ent, wire);
    let mut shown = String::new();
    let out = stream(&t, &json!({}), vec![json!({ "role": "user", "content": "hi" })], None, &AtomicBool::new(false), |d| {
        if let Delta::Content(s) = d {
            shown.push_str(&s)
        }
    })
    .await
    .unwrap();
    assert_eq!(shown, "Checking");
    assert_eq!(out.tool_calls.len(), 1);
    assert_eq!(out.tool_calls[0].arguments, "{\"x\":2}");
    assert_eq!((out.prompt_tokens, out.completion_tokens), (Some(12), Some(7)));
    let req = seen.lock().unwrap()[0].to_lowercase();
    assert!(req.starts_with("post /model/us.anthropic.claude-sonnet-4-6-v1%3a0/converse-stream "), "{req}");
    assert!(req.contains("authorization: aws4-hmac-sha256 credential=akidtest/"), "signed");
    assert!(req.contains("/us-east-1/bedrock/aws4_request"));
    assert!(req.contains("x-team: research"), "gateway header");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn azure_uses_deployments_api_version_and_its_key_header() {
    let (port, seen) = stand_in(|_| http("200 OK", "text/event-stream", SSE_HELLO.as_bytes())).await;
    let t = target(Kind::Azure, format!("http://127.0.0.1:{port}"), "azkey", "gpt4o-prod", Ent::default(), Wire::default());
    let out = stream(&t, &json!({}), vec![json!({ "role": "user", "content": "hi" })], None, &AtomicBool::new(false), |_| {}).await.unwrap();
    assert_eq!(out.content, "Hello");
    let req = seen.lock().unwrap()[0].clone();
    assert!(req.starts_with(&format!("POST /openai/deployments/gpt4o-prod/chat/completions?api-version={} ", enterprise::AZURE_VERSION)), "{req}");
    assert!(req.to_lowercase().contains("api-key: azkey"));
    assert!(!req.to_lowercase().contains("authorization:"));
    assert_eq!(enterprise::azure_chat_url("https://x.services.ai.azure.com/openai/v1", "d", ""), "https://x.services.ai.azure.com/openai/v1/chat/completions");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entra_device_code_then_tokens_for_azure() {
    let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let p2 = polls.clone();
    let id_token = format!("x.{}.y", base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, br#"{"preferred_username":"pat@corp.example"}"#));
    let (port, seen) = stand_in(move |req| {
        let body = if req.starts_with("POST /tenant1/oauth2/v2.0/devicecode") {
            json!({ "device_code": "DC", "user_code": "ABCD-EFGH", "verification_uri": "https://microsoft.com/devicelogin", "interval": 1, "expires_in": 60, "message": "Enter ABCD-EFGH" })
        } else if req.contains("grant_type=refresh_token") {
            json!({ "access_token": "AT2", "refresh_token": "RT2", "expires_in": 3600 })
        } else if p2.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            json!({ "error": "authorization_pending" })
        } else {
            json!({ "access_token": "AT1", "refresh_token": "RT1", "expires_in": 3600, "id_token": id_token })
        };
        let status = if body.get("error").is_some() { "400 Bad Request" } else { "200 OK" };
        http(status, "application/json", body.to_string().as_bytes())
    })
    .await;
    let ent = Ent { auth: Auth::Entra, tenant: "tenant1".into(), client_id: "app-1".into(), authority: format!("http://127.0.0.1:{port}"), ..Default::default() };
    let client = reqwest::Client::new();
    let code = enterprise::entra_start(&client, &ent).await.unwrap();
    assert_eq!(code.user_code, "ABCD-EFGH");
    assert!(matches!(enterprise::entra_poll(&client, &ent, "DC").await.unwrap(), enterprise::Poll::Waiting));
    let enterprise::Poll::Done { refresh, access, account, .. } = enterprise::entra_poll(&client, &ent, "DC").await.unwrap() else { panic!("signed in") };
    assert_eq!((refresh.as_str(), access.as_str(), account.as_str()), ("RT1", "AT1", "pat@corp.example"));
    // A saved refresh token gets a fresh access token, and the new refresh token is kept.
    let state = crate::testutil::temp_state(None);
    let c = state.cipher().unwrap();
    let p = Provider { id: "azure".into(), name: "Azure".into(), kind: Kind::Azure, base_url: "https://x.openai.azure.com".into(), ent: Ent { token: c.encrypt("RT1"), ..ent.clone() }, ..Default::default() };
    save(&state.db.lock().unwrap(), &CloudSettings { providers: vec![p.clone()], ..Default::default() }).unwrap();
    assert!(p.has_credentials(), "signed in, no key needed");
    let tok = enterprise::token(&state, &client, "azure", Kind::Azure, &p.ent, "").await.unwrap();
    assert_eq!(tok, "AT2");
    let saved = settings(&state.db.lock().unwrap()).providers[0].ent.token.clone();
    assert_eq!(c.decrypt(&saved).unwrap(), "RT2");
    assert!(seen.lock().unwrap().iter().any(|r| r.contains("scope=https%3A%2F%2Fcognitiveservices.azure.com%2F.default+offline_access")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vertex_gets_a_google_token_and_picks_the_publisher() {
    let (port, seen) = stand_in(|req| {
        if req.starts_with("POST /token") {
            http("200 OK", "application/json", br#"{"access_token":"ya29.test","expires_in":3599}"#)
        } else {
            http("200 OK", "text/event-stream", SSE_HELLO.as_bytes())
        }
    })
    .await;
    let creds = json!({ "type": "authorized_user", "client_id": "cid", "client_secret": "cs", "refresh_token": "rt", "token_uri": format!("http://127.0.0.1:{port}/token") }).to_string();
    let ent = Ent { project: "my-proj".into(), region: "us-east5".into(), ..Default::default() };
    let t = target(Kind::Vertex, format!("http://127.0.0.1:{port}"), &creds, "gemini-2.5-flash", ent.clone(), Wire::default());
    let out = stream(&t, &json!({}), vec![json!({ "role": "user", "content": "hi" })], None, &AtomicBool::new(false), |_| {}).await.unwrap();
    assert_eq!(out.content, "Hello");
    let reqs = seen.lock().unwrap().clone();
    assert!(reqs[0].contains("grant_type=refresh_token"));
    assert!(reqs[1].starts_with("POST /v1/projects/my-proj/locations/us-east5/endpoints/openapi/chat/completions "), "{}", reqs[1]);
    assert!(reqs[1].contains("ya29.test") && reqs[1].contains("\"model\":\"google/gemini-2.5-flash\""));
    let claude = target(Kind::Vertex, format!("http://127.0.0.1:{port}"), &creds, "claude-sonnet-4-6", ent, Wire::default());
    assert_eq!(enterprise::vertex_claude_url(&claude), format!("http://127.0.0.1:{port}/v1/projects/my-proj/locations/us-east5/publishers/anthropic/models/claude-sonnet-4-6:streamRawPredict"));
    let (unsigned, email) = enterprise::jwt_claims(&json!({ "client_email": "sa@p.iam.gserviceaccount.com", "token_uri": "https://oauth2.googleapis.com/token" }), 1000).unwrap();
    assert_eq!(email, "sa@p.iam.gserviceaccount.com");
    assert_eq!(unsigned.split('.').count(), 2);
}

#[test]
fn enterprise_settings_are_checked() {
    let mut p = Provider { id: "vertex".into(), name: "Vertex".into(), kind: Kind::Vertex, ..Default::default() };
    assert!(enterprise::problem(&p).is_some(), "needs a project");
    p.ent.project = "proj/../x".into();
    assert!(enterprise::problem(&p).is_some(), "no paths in names");
    p.ent.project = "my-proj".into();
    assert!(enterprise::problem(&p).is_none());
    assert!(!p.has_credentials());
    p.ent.auth = Auth::Adc;
    assert!(p.has_credentials(), "gcloud's sign-in needs no key here");
    let bad = ent_cmds::apply(&mut p.clone(), serde_json::from_value(json!({ "auth": "entra" })).unwrap());
    assert!(bad.is_err(), "Vertex doesn't use Entra");
    assert!(ent_cmds::header_ok("OpenAI-Organization") && ent_cmds::header_ok("x-portkey-config"));
    assert!(!ent_cmds::header_ok("Authorization") && !ent_cmds::header_ok("Host") && !ent_cmds::header_ok("bad header"));
}

#[test]
fn headers_are_sealed_and_a_ca_must_be_a_certificate() {
    let state = crate::testutil::temp_state(None);
    let c = state.cipher().unwrap();
    let p = Provider { headers: c.encrypt(r#"{"OpenAI-Organization":"org-1","OpenAI-Project":"proj-2"}"#), ..Default::default() };
    assert!(!p.headers.contains("org-1"));
    let w = wire_of(&p, &c);
    assert!(w.headers.contains(&("OpenAI-Organization".to_string(), "org-1".to_string())));
    assert!(net::ca_certs("not a certificate").is_err());
}

#[test]
fn bedrock_sso_profiles_use_the_cli_sign_in() {
    let dir = std::env::temp_dir().join(format!("sulcusai-aws-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dir.join("sso").join("cache")).unwrap();
    std::fs::write(dir.join("config"), "[profile dev]\nsso_session = corp\nsso_account_id = 111122223333\nsso_role_name = Bedrock\nregion = us-west-2\n[sso-session corp]\nsso_start_url = https://corp.awsapps.com/start\nsso_region = us-east-1\n[profile old]\nsso_start_url = https://old.awsapps.com/start\nsso_region = eu-west-1\nsso_account_id = 1\nsso_role_name = R\n").unwrap();
    let (s, region) = bedrock::sso_profile("dev", Some(dir.clone())).unwrap().unwrap();
    assert_eq!((s.region.as_str(), s.account.as_str(), s.role.as_str(), s.cache_key.as_str()), ("us-east-1", "111122223333", "Bedrock", "corp"));
    assert_eq!(region.as_deref(), Some("us-west-2"));
    assert_eq!(bedrock::sso_profile("old", Some(dir.clone())).unwrap().unwrap().0.cache_key, "https://old.awsapps.com/start");
    assert!(bedrock::sso_profile("none", Some(dir.clone())).unwrap().is_none());
    let now = chrono::Utc::now();
    assert!(bedrock::sso_token(&s, now).unwrap_err().contains("aws sso login --profile dev"));
    // The AWS CLI names the file after the SHA-1 of the session name.
    use sha1::Digest as _;
    let name = format!("{}.json", hex::encode(sha1::Sha1::digest(b"corp")));
    std::fs::write(s.cache_dir.join(&name), json!({ "accessToken": "tok", "expiresAt": (now + chrono::Duration::hours(1)).to_rfc3339() }).to_string()).unwrap();
    assert_eq!(bedrock::sso_token(&s, now).unwrap(), "tok");
    assert!(bedrock::sso_token(&s, now + chrono::Duration::hours(2)).unwrap_err().contains("ran out"));
    let (c, _) = bedrock::role_creds(&json!({ "roleCredentials": { "accessKeyId": "A", "secretAccessKey": "S", "sessionToken": "T", "expiration": 1_900_000_000_000i64 } })).unwrap();
    assert_eq!(c, bedrock::Creds::Keys { id: "A".into(), secret: "S".into(), session: Some("T".into()) });
}
