// SPDX-License-Identifier: AGPL-3.0-only
//! Pictures, video and voices from cloud providers (the Cloud level, the
//! user's own keys). The requests use the OpenAI-style APIs:
//! - pictures: `images/generations`, and `images/edits` for changes and
//!   filling in (OpenAI's image models);
//! - video: `videos` (Sora): start, ask how it's going, download;
//! - voices: `audio/speech`.
//!
//! Like cloud chats, every request is in the activity log, its estimated
//! cost counts against the monthly budget, and personal details in the
//! description can be swapped for placeholders first.

use std::time::Duration;

use base64::Engine;
use serde_json::{json, Value};

use super::store::{self, MediaItem};
use super::{imaging, RunCtx};
use crate::cloud::{self, MediaKind, MediaTarget};

fn b64() -> base64::engine::general_purpose::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

async fn fail(resp: reqwest::Response, t: &MediaTarget) -> String {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(String::from))
        .unwrap_or_else(|| body.chars().take(300).collect());
    match status.as_u16() {
        401 | 403 => format!("{} didn't accept the API key: {detail}", t.provider),
        429 => format!("{} says to slow down or that the account is out of credit: {detail}", t.provider),
        _ => format!("{} answered {status}: {detail}", t.provider),
    }
}

fn prompt_for(ctx: &RunCtx<'_>, t: &MediaTarget) -> (String, usize) {
    let p = ctx.req.prompt.trim();
    if t.redact {
        let mut r = cloud::Redactor::default();
        let out = r.redact(p);
        (out, r.used())
    } else {
        (p.to_string(), 0)
    }
}

/// The nearest size the model takes for the requested shape.
fn size_for(shape: Option<&str>, model: &str) -> &'static str {
    let gpt = model.starts_with("gpt-image") || model.starts_with("dall-e");
    match (shape.unwrap_or("square"), gpt) {
        ("portrait" | "tall", true) => "1024x1536",
        ("landscape" | "wide", true) => "1536x1024",
        ("portrait" | "tall", false) => "768x1024",
        ("landscape" | "wide", false) => "1024x768",
        _ => "1024x1024",
    }
}

async fn image_bytes(client: &reqwest::Client, data: &Value) -> Result<Vec<Vec<u8>>, String> {
    let mut out = Vec::new();
    for d in data["data"].as_array().cloned().unwrap_or_default() {
        if let Some(b) = d["b64_json"].as_str() {
            out.push(b64().decode(b).map_err(|_| "The picture came back damaged.".to_string())?);
        } else if let Some(url) = d["url"].as_str() {
            let bytes = client.get(url).send().await.map_err(|e| e.to_string())?.bytes().await.map_err(|e| e.to_string())?;
            out.push(bytes.to_vec());
        }
    }
    if out.is_empty() {
        return Err("The provider sent no picture back (it may have declined the description).".into());
    }
    Ok(out)
}

fn save_pictures(ctx: &RunCtx<'_>, id: &str, pics: Vec<Vec<u8>>, parent: Option<String>) -> Result<Vec<MediaItem>, String> {
    let c = ctx.state.work_cipher()?;
    let mut items = Vec::new();
    for bytes in pics {
        let img = image::load_from_memory(&bytes).map_err(|_| "The picture came back in a format this app can't read.".to_string())?;
        let png = imaging::png(&img)?;
        let item = MediaItem {
            kind: "image".into(),
            op: ctx.req.op.clone(),
            prompt: ctx.req.prompt.trim().to_string(),
            model_id: Some(id.to_string()),
            mime: "image/png".into(),
            width: img.width(),
            height: img.height(),
            parent: parent.clone(),
            chat_id: ctx.req.chat_id.clone(),
            ..Default::default()
        };
        let thumb = imaging::thumbnail(&img)?;
        items.push(store::save(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &c, item, &png, Some(&thumb))?);
    }
    Ok(items)
}

fn charge(ctx: &RunCtx<'_>, t: &MediaTarget, units: f64, what: &str, redacted: usize) {
    let cost = t.model.price.map(|p| p * units);
    if let Some(c) = cost {
        cloud::add_spend(&ctx.state.db.lock().unwrap(), c);
    }
    let priced = cost.map(|c| format!(", about ${c:.3}")).unwrap_or_default();
    let hidden = if redacted > 0 { format!("; {redacted} personal detail(s) replaced with placeholders") } else { String::new() };
    ctx.state.log("network", &format!("Asked {} ({}) for {what}{priced}{hidden}", t.provider, t.model.name));
}

/// Runs a job on a cloud model. `id` is its "cloud:provider:model" id.
pub async fn run(ctx: &RunCtx<'_>, id: &str, t: &MediaTarget) -> Result<Vec<MediaItem>, String> {
    let client = cloud::media_client(ctx.state)?;
    match (t.model.kind, ctx.req.op.as_str()) {
        (MediaKind::Image, "generate") => generate(ctx, id, t, &client).await,
        (MediaKind::Image, "edit" | "restyle" | "fill") => edit(ctx, id, t, &client).await,
        (MediaKind::Video, "video") => video(ctx, id, t, &client).await,
        (MediaKind::Speech, "narrate") => speech(ctx, id, t, &client).await,
        _ => Err(format!("{} can't do that kind of job.", t.model.name)),
    }
}

async fn generate(ctx: &RunCtx<'_>, id: &str, t: &MediaTarget, client: &reqwest::Client) -> Result<Vec<MediaItem>, String> {
    let (prompt, redacted) = prompt_for(ctx, t);
    let n = ctx.req.count.unwrap_or(1).clamp(1, 4);
    let mut body = json!({ "model": t.model.id, "prompt": prompt, "n": n, "size": size_for(ctx.req.shape.as_deref(), &t.model.id) });
    if !t.model.id.starts_with("gpt-image") {
        body["response_format"] = json!("b64_json");
    }
    (ctx.progress)(&format!("Waiting for {}", t.provider), 0.2);
    let resp = client
        .post(format!("{}/images/generations", t.base_url))
        .bearer_auth(t.key())
        .timeout(Duration::from_secs(300))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Couldn't reach {}: {e}", t.provider))?;
    if !resp.status().is_success() {
        return Err(fail(resp, t).await);
    }
    let data: Value = resp.json().await.map_err(|e| e.to_string())?;
    let pics = image_bytes(client, &data).await?;
    charge(ctx, t, pics.len() as f64, &format!("{} picture{}", pics.len(), if pics.len() == 1 { "" } else { "s" }), redacted);
    (ctx.progress)("Saving", 0.95);
    save_pictures(ctx, id, pics, None)
}

/// Changes a picture from an instruction; with a mask, only where it's painted.
async fn edit(ctx: &RunCtx<'_>, id: &str, t: &MediaTarget, client: &reqwest::Client) -> Result<Vec<MediaItem>, String> {
    let (item, img) = ctx.source()?;
    let (mut prompt, redacted) = prompt_for(ctx, t);
    if ctx.req.op == "restyle" {
        prompt = format!("Redraw this picture in this style, keeping what's in it: {prompt}");
    }
    let img = imaging::cap(img, 1536);
    let mut form = reqwest::multipart::Form::new()
        .text("model", t.model.id.clone())
        .text("prompt", prompt)
        .part("image", reqwest::multipart::Part::bytes(imaging::png(&img)?).file_name("image.png").mime_str("image/png").map_err(|e| e.to_string())?);
    if ctx.req.op == "fill" {
        // OpenAI edits where the mask is transparent.
        let paint = b64().decode(ctx.req.mask.as_deref().unwrap_or("")).map_err(|_| "The painted area couldn't be read.".to_string())?;
        let painted = imaging::grow(&imaging::mask_from_paint(&paint, img.width(), img.height())?, 4);
        let mut rgba = image::RgbaImage::new(img.width(), img.height());
        for (x, y, p) in rgba.enumerate_pixels_mut() {
            let a = if painted.get_pixel(x, y)[0] > 127 { 0 } else { 255 };
            *p = image::Rgba([0, 0, 0, a]);
        }
        let mask = imaging::png(&image::DynamicImage::ImageRgba8(rgba))?;
        form = form.part("mask", reqwest::multipart::Part::bytes(mask).file_name("mask.png").mime_str("image/png").map_err(|e| e.to_string())?);
    }
    (ctx.progress)(&format!("Waiting for {}", t.provider), 0.2);
    let resp = client
        .post(format!("{}/images/edits", t.base_url))
        .bearer_auth(t.key())
        .timeout(Duration::from_secs(300))
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Couldn't reach {}: {e}", t.provider))?;
    if !resp.status().is_success() {
        return Err(fail(resp, t).await);
    }
    let data: Value = resp.json().await.map_err(|e| e.to_string())?;
    let pics = image_bytes(client, &data).await?;
    charge(ctx, t, pics.len() as f64, "a picture edit", redacted);
    save_pictures(ctx, id, pics, Some(item.id))
}

async fn video(ctx: &RunCtx<'_>, id: &str, t: &MediaTarget, client: &reqwest::Client) -> Result<Vec<MediaItem>, String> {
    let (prompt, redacted) = prompt_for(ctx, t);
    // Sora takes 4, 8 or 12 seconds.
    let secs = match ctx.req.seconds.unwrap_or(4.0) {
        s if s <= 6.0 => 4,
        s if s <= 10.0 => 8,
        _ => 12,
    };
    let (w, h) = match ctx.req.shape.as_deref() {
        Some("portrait" | "tall") => (720, 1280),
        _ => (1280, 720),
    };
    let mut form = reqwest::multipart::Form::new()
        .text("model", t.model.id.clone())
        .text("prompt", if prompt.is_empty() { "Bring this picture to life with natural motion.".to_string() } else { prompt })
        .text("seconds", secs.to_string())
        .text("size", format!("{w}x{h}"));
    let mut parent = None;
    if ctx.req.source.is_some() {
        let (item, img) = ctx.source()?;
        let start = img.resize_to_fill(w, h, image::imageops::FilterType::Lanczos3);
        form = form.part("input_reference", reqwest::multipart::Part::bytes(imaging::png(&start)?).file_name("start.png").mime_str("image/png").map_err(|e| e.to_string())?);
        parent = Some(item.id);
    }
    (ctx.progress)(&format!("Sending to {}", t.provider), 0.02);
    let resp = client
        .post(format!("{}/videos", t.base_url))
        .bearer_auth(t.key())
        .timeout(Duration::from_secs(120))
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Couldn't reach {}: {e}", t.provider))?;
    if !resp.status().is_success() {
        return Err(fail(resp, t).await);
    }
    let job: Value = resp.json().await.map_err(|e| e.to_string())?;
    let vid = job["id"].as_str().ok_or("The provider didn't start the video.")?.to_string();
    charge(ctx, t, secs as f64, &format!("a {secs}-second video"), redacted);
    // Ask every few seconds; stopping here only stops waiting (it's paid for).
    loop {
        if ctx.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(crate::download::CANCELLED.into());
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
        let s: Value = client
            .get(format!("{}/videos/{vid}", t.base_url))
            .bearer_auth(t.key())
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        match s["status"].as_str().unwrap_or("") {
            "completed" => break,
            "failed" | "cancelled" => {
                return Err(format!("{} couldn't make the video: {}", t.provider, s["error"]["message"].as_str().unwrap_or("no reason given")));
            }
            _ => {
                let pct = s["progress"].as_f64().unwrap_or(0.0) / 100.0;
                (ctx.progress)(&format!("{} is making it", t.provider), 0.05 + pct * 0.85);
            }
        }
    }
    (ctx.progress)("Downloading", 0.92);
    let bytes = client
        .get(format!("{}/videos/{vid}/content", t.base_url))
        .bearer_auth(t.key())
        .timeout(Duration::from_secs(300))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;
    let c = ctx.state.work_cipher()?;
    let item = MediaItem {
        kind: "video".into(),
        op: "video".into(),
        prompt: ctx.req.prompt.trim().to_string(),
        model_id: Some(id.to_string()),
        mime: "video/mp4".into(),
        width: w,
        height: h,
        seconds: secs as f64,
        parent,
        chat_id: ctx.req.chat_id.clone(),
        ..Default::default()
    };
    Ok(vec![store::save(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &c, item, &bytes, None)?])
}

async fn speech(ctx: &RunCtx<'_>, id: &str, t: &MediaTarget, client: &reqwest::Client) -> Result<Vec<MediaItem>, String> {
    let text = crate::tts::speakable(ctx.req.prompt.trim());
    if text.trim().is_empty() {
        return Err("Type the text to read aloud.".into());
    }
    let voice = ctx.req.voice.clone().filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric())).unwrap_or_else(|| "alloy".into());
    (ctx.progress)(&format!("Waiting for {}", t.provider), 0.3);
    let resp = client
        .post(format!("{}/audio/speech", t.base_url))
        .bearer_auth(t.key())
        .timeout(Duration::from_secs(300))
        .json(&json!({ "model": t.model.id, "input": text, "voice": voice, "response_format": "wav" }))
        .send()
        .await
        .map_err(|e| format!("Couldn't reach {}: {e}", t.provider))?;
    if !resp.status().is_success() {
        return Err(fail(resp, t).await);
    }
    let wav = resp.bytes().await.map_err(|e| e.to_string())?.to_vec();
    // Priced per million characters.
    charge(ctx, t, text.chars().count() as f64 / 1e6, &format!("{} characters read aloud", text.chars().count()), 0);
    let seconds = wav_seconds(&wav).unwrap_or(0.0);
    let c = ctx.state.work_cipher()?;
    let item = MediaItem {
        kind: "audio".into(),
        op: "narrate".into(),
        prompt: ctx.req.prompt.trim().to_string(),
        model_id: Some(id.to_string()),
        mime: "audio/wav".into(),
        seconds,
        chat_id: ctx.req.chat_id.clone(),
        ..Default::default()
    };
    Ok(vec![store::save(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &c, item, &wav, None)?])
}

/// A WAV file's length from its header (streamed WAVs may leave the data
/// size unset; then the file size stands in).
fn wav_seconds(wav: &[u8]) -> Option<f64> {
    if wav.len() < 44 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return None;
    }
    let byte_rate = u32::from_le_bytes(wav[28..32].try_into().ok()?) as f64;
    let mut i = 12;
    while i + 8 <= wav.len() {
        let size = u32::from_le_bytes(wav[i + 4..i + 8].try_into().ok()?) as usize;
        if &wav[i..i + 4] == b"data" {
            let len = if size == 0 || size == u32::MAX as usize || i + 8 + size > wav.len() { wav.len() - i - 8 } else { size };
            return (byte_rate > 0.0).then(|| len as f64 / byte_rate);
        }
        i += 8 + size;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_follow_the_shape() {
        assert_eq!(size_for(Some("portrait"), "gpt-image-1"), "1024x1536");
        assert_eq!(size_for(Some("wide"), "gpt-image-1"), "1536x1024");
        assert_eq!(size_for(None, "imagen-4.0-generate-001"), "1024x1024");
    }

    #[test]
    fn wav_length_from_header() {
        let wav = crate::audio::wav_encode(&vec![0.0; 24000], 24000);
        assert!((wav_seconds(&wav).unwrap() - 1.0).abs() < 0.01);
        assert!(wav_seconds(b"nope").is_none());
    }
}
