// SPDX-License-Identifier: AGPL-3.0-only
//! AWS Bedrock: the Converse API, streamed as AWS event-stream frames and
//! signed with Signature Version 4, from access keys sealed in the app, a
//! profile in the AWS files, or a Bedrock API key.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::enterprise::{path_part, Auth, Ent};
use super::{empty, failure, send_failed, to_anthropic, CloudModel, Failed, Provider, Target, Wire};
use crate::chat::{Delta, Finished};
use crate::db::ToolCall;

#[derive(Debug, Clone, PartialEq)]
pub enum Creds {
    Keys { id: String, secret: String, session: Option<String> },
    /// A Bedrock API key, sent as a bearer token with no signing.
    Bearer(String),
}

/// "ACCESS_KEY_ID:SECRET[:SESSION_TOKEN]" is a pair of access keys; anything
/// without a colon is a Bedrock API key.
pub fn parse_key(key: &str) -> Option<Creds> {
    let k = key.trim();
    if k.is_empty() {
        return None;
    }
    let mut parts = k.splitn(3, ':');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(id), Some(secret), session) if !id.is_empty() && !secret.is_empty() => Some(Creds::Keys { id: id.into(), secret: secret.into(), session: session.filter(|s| !s.is_empty()).map(str::to_string) }),
        (Some(_), None, None) => Some(Creds::Bearer(k.into())),
        _ => None,
    }
}

fn aws_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(|h| std::path::PathBuf::from(h).join(".aws"))
}

/// One section of an AWS ini file: "[name]" in credentials, "[profile name]"
/// (or "[default]") in config.
pub fn ini_section(text: &str, names: &[&str]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut inside = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('#') || l.starts_with(';') || l.is_empty() {
            continue;
        }
        if let Some(h) = l.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
            inside = names.iter().any(|n| h.trim() == *n);
            continue;
        }
        if inside {
            if let Some((k, v)) = l.split_once('=') {
                out.insert(k.trim().to_lowercase(), v.trim().to_string());
            }
        }
    }
    out
}

/// Keys and region for an AWS profile. Profiles that sign in through SSO
/// or run a program for keys aren't read; their keys can be pasted instead.
pub fn profile(name: &str, dir: Option<std::path::PathBuf>) -> Result<(Creds, Option<String>), String> {
    let name = if name.trim().is_empty() { "default" } else { name.trim() };
    let dir = dir.or_else(aws_dir).ok_or("Couldn't find the AWS settings folder.")?;
    let creds_file = std::env::var_os("AWS_SHARED_CREDENTIALS_FILE").map(std::path::PathBuf::from).unwrap_or_else(|| dir.join("credentials"));
    let config_file = std::env::var_os("AWS_CONFIG_FILE").map(std::path::PathBuf::from).unwrap_or_else(|| dir.join("config"));
    let creds = std::fs::read_to_string(creds_file).unwrap_or_default();
    let config = std::fs::read_to_string(config_file).unwrap_or_default();
    let prof = format!("profile {name}");
    let c = ini_section(&creds, &[name]);
    let cfg = ini_section(&config, &[name, &prof]);
    let pick = |k: &str| c.get(k).or_else(|| cfg.get(k)).cloned().filter(|v| !v.is_empty());
    let (Some(id), Some(secret)) = (pick("aws_access_key_id"), pick("aws_secret_access_key")) else {
        return Err(format!("The AWS profile \"{name}\" has no access keys here. Profiles that sign in with SSO aren't read: run `aws configure export-credentials` and paste the keys instead."));
    };
    Ok((Creds::Keys { id, secret, session: pick("aws_session_token") }, cfg.get("region").cloned()))
}

fn creds_for(e: &Ent, key: &str) -> Result<(Creds, Option<String>), String> {
    match e.auth {
        Auth::Profile => profile(&e.profile, None),
        _ => parse_key(key).map(|c| (c, None)).ok_or_else(|| "Add AWS access keys (ACCESS_KEY_ID:SECRET) or a Bedrock API key in Settings → Cloud models.".into()),
    }
}

fn region(e: &Ent, from_profile: Option<String>) -> String {
    Some(e.region.trim().to_string()).filter(|r| !r.is_empty()).or(from_profile).unwrap_or_else(|| "us-east-1".into())
}

fn host_for(base: &str, service: &str, region: &str) -> String {
    match base.trim_end_matches('/') {
        "" => format!("https://{service}.{region}.amazonaws.com"),
        b => b.to_string(),
    }
}

// ---------- Signature Version 4 ----------

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC takes any key length");
    m.update(data.as_bytes());
    m.finalize().into_bytes().to_vec()
}

/// The headers that sign one request: x-amz-date, the session token if
/// any, and Authorization. `path` is the address's path as sent (already
/// encoded once); AWS services other than S3 sign it encoded a second time.
#[allow(clippy::too_many_arguments)]
pub fn sign(method: &str, host: &str, path: &str, query: &[(String, String)], body: &[u8], region: &str, service: &str, id: &str, secret: &str, session: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> Vec<(String, String)> {
    let amz = now.format("%Y%m%dT%H%M%SZ").to_string();
    let day = &amz[..8];
    let canon_path = if path.is_empty() { "/".to_string() } else { path.split('/').map(path_part).collect::<Vec<_>>().join("/") };
    let mut q: Vec<(String, String)> = query.iter().map(|(k, v)| (path_part(k), path_part(v))).collect();
    q.sort();
    let canon_query = q.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
    let mut headers = vec![("host".to_string(), host.to_string()), ("x-amz-date".to_string(), amz.clone())];
    if let Some(s) = session {
        headers.push(("x-amz-security-token".into(), s.into()));
    }
    headers.sort();
    let signed = headers.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";");
    let canon_headers: String = headers.iter().map(|(k, v)| format!("{k}:{}\n", v.trim())).collect();
    let canonical = format!("{method}\n{canon_path}\n{canon_query}\n{canon_headers}\n{signed}\n{}", hex::encode(Sha256::digest(body)));
    let scope = format!("{day}/{region}/{service}/aws4_request");
    let to_sign = format!("AWS4-HMAC-SHA256\n{amz}\n{scope}\n{}", hex::encode(Sha256::digest(canonical.as_bytes())));
    let k = hmac(format!("AWS4{secret}").as_bytes(), day);
    let k = hmac(&k, region);
    let k = hmac(&k, service);
    let k = hmac(&k, "aws4_request");
    let sig = hex::encode(hmac(&k, &to_sign));
    let mut out = vec![("x-amz-date".to_string(), amz), ("authorization".to_string(), format!("AWS4-HMAC-SHA256 Credential={id}/{scope}, SignedHeaders={signed}, Signature={sig}"))];
    if let Some(s) = session {
        out.push(("x-amz-security-token".into(), s.into()));
    }
    out
}

/// A request to `base` + the given path parts and query, signed (or with
/// the bearer key).
#[allow(clippy::too_many_arguments)]
fn request(client: &reqwest::Client, method: reqwest::Method, base: &str, parts: &[&str], query: &[(String, String)], body: Vec<u8>, region: &str, creds: &Creds) -> Result<reqwest::RequestBuilder, String> {
    let path = format!("/{}", parts.iter().map(|p| path_part(p)).collect::<Vec<_>>().join("/"));
    let mut url = reqwest::Url::parse(&format!("{base}{path}")).map_err(|_| "That Bedrock address isn't usable.".to_string())?;
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }
    let host = match (url.host_str(), url.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_string(),
        _ => return Err("That Bedrock address has no host.".into()),
    };
    let full_path = url.path().to_string();
    let mut req = client.request(method.clone(), url).header("content-type", "application/json");
    match creds {
        Creds::Bearer(k) => req = req.bearer_auth(k),
        Creds::Keys { id, secret, session } => {
            for (k, v) in sign(method.as_str(), &host, &full_path, query, &body, region, "bedrock", id, secret, session.as_deref(), chrono::Utc::now()) {
                req = req.header(k, v);
            }
        }
    }
    Ok(req.body(body))
}

// ---------- event stream ----------

/// One AWS event-stream message: its string headers and payload.
#[derive(Debug, PartialEq)]
pub struct Frame {
    pub headers: HashMap<String, String>,
    pub payload: Vec<u8>,
}

/// Takes one whole frame off the front of `buf`, if there is one. The
/// checksums aren't checked: TLS already protects the bytes.
pub fn next_frame(buf: &mut Vec<u8>) -> Result<Option<Frame>, String> {
    if buf.len() < 12 {
        return Ok(None);
    }
    let be = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
    let (total, hlen) = (be(&buf[0..4]), be(&buf[4..8]));
    if total < 16 + hlen || total > 16 << 20 {
        return Err("Bedrock's reply was garbled.".into());
    }
    if buf.len() < total {
        return Ok(None);
    }
    let frame: Vec<u8> = buf.drain(..total).collect();
    let mut h = &frame[12..12 + hlen];
    let mut headers = HashMap::new();
    while !h.is_empty() {
        let n = h[0] as usize;
        let name = String::from_utf8_lossy(h.get(1..1 + n).ok_or("garbled header")?).to_string();
        h = &h[1 + n..];
        let ty = *h.first().ok_or("garbled header")?;
        h = &h[1..];
        let fixed = |len: usize, h: &mut &[u8]| -> Result<(), String> {
            *h = h.get(len..).ok_or("garbled header")?;
            Ok(())
        };
        match ty {
            0 | 1 => {}
            2 => fixed(1, &mut h)?,
            3 => fixed(2, &mut h)?,
            4 => fixed(4, &mut h)?,
            5 | 8 => fixed(8, &mut h)?,
            9 => fixed(16, &mut h)?,
            6 | 7 => {
                let len = u16::from_be_bytes([*h.first().ok_or("garbled header")?, *h.get(1).ok_or("garbled header")?]) as usize;
                let v = h.get(2..2 + len).ok_or("garbled header")?;
                if ty == 7 {
                    headers.insert(name, String::from_utf8_lossy(v).to_string());
                }
                h = &h[2 + len..];
            }
            _ => return Err("Bedrock's reply was garbled.".into()),
        }
    }
    Ok(Some(Frame { headers, payload: frame[12 + hlen..total - 4].to_vec() }))
}

/// A frame as AWS sends it (for the local stand-in in tests).
#[cfg(test)]
pub fn frame(event: &str, payload: &Value) -> Vec<u8> {
    let mut h = Vec::new();
    for (k, v) in [(":event-type", event), (":content-type", "application/json"), (":message-type", "event")] {
        h.push(k.len() as u8);
        h.extend_from_slice(k.as_bytes());
        h.push(7);
        h.extend_from_slice(&(v.len() as u16).to_be_bytes());
        h.extend_from_slice(v.as_bytes());
    }
    let body = payload.to_string().into_bytes();
    let total = 16 + h.len() + body.len();
    let mut out = Vec::new();
    out.extend_from_slice(&(total as u32).to_be_bytes());
    out.extend_from_slice(&(h.len() as u32).to_be_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&h);
    out.extend_from_slice(&body);
    out.extend_from_slice(&[0; 4]);
    out
}

// ---------- Converse ----------

/// The Converse request: Anthropic-shaped messages turned into Bedrock's
/// blocks, which every Bedrock model takes.
pub fn converse_body(t_max: u32, extra: &Value, messages: &[Value], tools: Option<&Value>) -> Value {
    let (system, msgs, tools) = to_anthropic(messages, tools);
    let block = |b: &Value| -> Option<Value> {
        Some(match b["type"].as_str()? {
            "text" => json!({ "text": b["text"].as_str().filter(|t| !t.is_empty())? }),
            "image" => {
                let mt = b.pointer("/source/media_type")?.as_str()?;
                json!({ "image": { "format": mt.trim_start_matches("image/").replace("jpg", "jpeg"), "source": { "bytes": b.pointer("/source/data")? } } })
            }
            "tool_use" => json!({ "toolUse": { "toolUseId": b["id"], "name": b["name"], "input": b["input"] } }),
            "tool_result" => json!({ "toolResult": { "toolUseId": b["tool_use_id"], "content": [{ "text": b["content"].as_str().filter(|s| !s.is_empty()).unwrap_or("(empty)") }] } }),
            _ => return None,
        })
    };
    let messages: Vec<Value> = msgs
        .iter()
        .filter_map(|m| {
            let content: Vec<Value> = m["content"].as_array()?.iter().filter_map(block).collect();
            (!content.is_empty()).then(|| json!({ "role": m["role"], "content": content }))
        })
        .collect();
    let max = extra.get("max_tokens").and_then(Value::as_u64).map(|n| n as u32).unwrap_or(if t_max > 0 { t_max } else { 4096 });
    let mut inference = json!({ "maxTokens": max.clamp(1, 128_000) });
    for (from, to) in [("temperature", "temperature"), ("top_p", "topP")] {
        if let Some(v) = extra.get(from) {
            inference[to] = v.clone();
        }
    }
    match extra.get("stop") {
        Some(Value::String(s)) => inference["stopSequences"] = json!([s]),
        Some(Value::Array(a)) => inference["stopSequences"] = json!(a),
        _ => {}
    }
    let mut body = json!({ "messages": messages, "inferenceConfig": inference });
    if !system.trim().is_empty() {
        body["system"] = json!([{ "text": system }]);
    }
    if !tools.is_empty() {
        let specs: Vec<Value> = tools.iter().map(|t| json!({ "toolSpec": { "name": t["name"], "description": t["description"], "inputSchema": { "json": t["input_schema"] } } })).collect();
        body["toolConfig"] = json!({ "tools": specs });
    }
    body
}

pub(super) async fn stream(client: &reqwest::Client, t: &Target, extra: &Value, messages: &[Value], tools: Option<&Value>, cancel: &AtomicBool, on_delta: &mut impl FnMut(Delta)) -> Result<Finished, Failed> {
    let (creds, from_profile) = creds_for(&t.ent, &t.key)?;
    let region = region(&t.ent, from_profile);
    let body = converse_body(t.model.max_output, extra, messages, tools);
    let base = host_for(&t.base_url, "bedrock-runtime", &region);
    let req = request(client, reqwest::Method::POST, &base, &["model", &t.model.id, "converse-stream"], &[], serde_json::to_vec(&body).unwrap_or_default(), &region, &creds)?;
    let resp = t.wire.add(req).timeout(Duration::from_secs(900)).send().await.map_err(|e| send_failed(e, &t.provider))?;
    if !resp.status().is_success() {
        return Err(failure(resp, &t.provider).await);
    }
    let mut out = empty();
    let mut calls: HashMap<u64, (String, String, String)> = HashMap::new();
    let mut buf = Vec::new();
    let mut bytes = resp.bytes_stream();
    'read: while let Some(chunk) = bytes.next().await {
        if cancel.load(Ordering::Relaxed) {
            out.cancelled = true;
            break;
        }
        let chunk = chunk.map_err(|e| Failed::again(format!("The reply was interrupted: {e}"), None))?;
        buf.extend_from_slice(&chunk);
        while let Some(f) = next_frame(&mut buf).map_err(Failed::fatal)? {
            let v: Value = serde_json::from_slice(&f.payload).unwrap_or_default();
            if f.headers.get(":message-type").map(String::as_str) == Some("exception") {
                let kind = f.headers.get(":exception-type").cloned().unwrap_or_default();
                let msg = format!("{} stopped with an error: {}", t.provider, v["message"].as_str().unwrap_or(&kind));
                return Err(if kind.contains("hrottl") || kind.contains("ServiceUnavailable") || kind.contains("InternalServer") { Failed::again(msg, None) } else { Failed::fatal(msg) });
            }
            let i = v["contentBlockIndex"].as_u64().unwrap_or(0);
            match f.headers.get(":event-type").map(String::as_str) {
                Some("contentBlockStart") => {
                    if let Some(u) = v.pointer("/start/toolUse") {
                        calls.insert(i, (u["toolUseId"].as_str().unwrap_or("").into(), u["name"].as_str().unwrap_or("").into(), String::new()));
                    }
                }
                Some("contentBlockDelta") => {
                    let d = &v["delta"];
                    if let Some(s) = d["text"].as_str() {
                        out.content.push_str(s);
                        on_delta(Delta::Content(s.into()));
                    } else if let Some(s) = d.pointer("/reasoningContent/text").and_then(Value::as_str) {
                        out.thinking.push_str(s);
                        on_delta(Delta::Thinking(s.into()));
                    } else if let Some(s) = d.pointer("/toolUse/input").and_then(Value::as_str) {
                        if let Some(c) = calls.get_mut(&i) {
                            c.2.push_str(s);
                        }
                    }
                }
                Some("metadata") => {
                    let u = &v["usage"];
                    let read = u["cacheReadInputTokens"].as_u64().unwrap_or(0);
                    let write = u["cacheWriteInputTokens"].as_u64().unwrap_or(0);
                    out.prompt_tokens = Some((u["inputTokens"].as_u64().unwrap_or(0) + read + write) as u32);
                    out.completion_tokens = u["outputTokens"].as_u64().map(|n| n as u32);
                    out.cache_read = Some(read as u32).filter(|n| *n > 0);
                    out.cache_write = Some(write as u32).filter(|n| *n > 0);
                    break 'read;
                }
                _ => {}
            }
        }
    }
    let mut calls: Vec<_> = calls.into_iter().collect();
    calls.sort_by_key(|(i, _)| *i);
    for (_, (id, name, args)) in calls {
        out.tool_calls.push(ToolCall { id, name, arguments: if args.trim().is_empty() { "{}".into() } else { args } });
    }
    Ok(out)
}

// ---------- model list ----------

pub(super) async fn list_models(p: &Provider, key: &str, client: &reqwest::Client, w: &Wire) -> Result<Vec<CloudModel>, String> {
    let (creds, from_profile) = creds_for(&p.ent, key)?;
    let region = region(&p.ent, from_profile);
    let base = host_for(&p.base_url, "bedrock", &region);
    let get = |parts: &'static [&'static str], q: Vec<(String, String)>| {
        let req = request(client, reqwest::Method::GET, &base, parts, &q, Vec::new(), &region, &creds);
        async move {
            let resp = w.add(req?).send().await.map_err(|e| e.to_string())?;
            if !resp.status().is_success() {
                return Err(failure(resp, &p.name).await.message);
            }
            resp.json::<Value>().await.map_err(|e| e.to_string())
        }
    };
    let mut out = Vec::new();
    // Cross-region inference profiles first: newer models only run through them.
    let profiles = get(&["inference-profiles"], vec![("maxResults".into(), "1000".into()), ("type".into(), "SYSTEM_DEFINED".into())]).await?;
    for m in profiles["inferenceProfileSummaries"].as_array().into_iter().flatten() {
        let Some(id) = m["inferenceProfileId"].as_str() else { continue };
        out.push(CloudModel { id: id.into(), name: m["inferenceProfileName"].as_str().unwrap_or(id).into(), ctx: 200_000, max_output: 8192, vision: true, price_in: None, price_out: None });
    }
    let models = get(&["foundation-models"], vec![("byInferenceType".into(), "ON_DEMAND".into()), ("byOutputModality".into(), "TEXT".into())]).await?;
    for m in models["modelSummaries"].as_array().into_iter().flatten() {
        let Some(id) = m["modelId"].as_str() else { continue };
        if m["responseStreamingSupported"].as_bool() == Some(false) {
            continue;
        }
        let vision = m["inputModalities"].as_array().is_some_and(|a| a.iter().any(|x| x == "IMAGE"));
        let name = format!("{} {}", m["providerName"].as_str().unwrap_or(""), m["modelName"].as_str().unwrap_or(id)).trim().to_string();
        out.push(CloudModel { id: id.into(), name, ctx: 128_000, max_output: 4096, vision, price_in: None, price_out: None });
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}
