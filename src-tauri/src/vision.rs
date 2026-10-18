// SPDX-License-Identifier: AGPL-3.0-only
//! The picture reader: pictures in words, for models that can't see them —
//! a chat model without an image encoder, and picture models, which take
//! only words. It reads with the chat model when that one can see pictures;
//! otherwise with a vision model of its own, started on an engine of its own
//! for the moment (the chat model is paused for it and comes back on its
//! next use): an installed chat model that sees pictures, or the vision
//! model Qwen-Image brings (Qwen3-VL 8B).

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::{engine, AppState};

/// A vision model on this PC that can read pictures on its own.
#[derive(Debug, Clone)]
pub struct Reader {
    pub name: String,
    pub model: PathBuf,
    pub mmproj: PathBuf,
}

/// The vision models on this PC, smallest (quickest to load) first.
pub fn readers(state: &AppState) -> Vec<Reader> {
    let mut out: Vec<(u64, Reader)> = Vec::new();
    let installed = crate::db::installed_models(&state.db.lock().unwrap());
    for m in installed {
        let Some(spec) = state.model_spec(&m.model_id) else { continue };
        let Some(mmproj) = crate::vision_file(&state.paths, &spec) else { continue };
        let model = crate::model_file(&state.paths, &m);
        if model.exists() {
            out.push((m.size, Reader { name: spec.name.clone(), model, mmproj }));
        }
    }
    // Picture models that read pictures themselves (Qwen-Image's Qwen3-VL).
    for m in crate::media::models(state) {
        let (Some(llm), Some(vision)) = (crate::media::installed_file(state, &m, "llm"), crate::media::installed_file(state, &m, "llm_vision")) else { continue };
        let size = std::fs::metadata(&llm).map_or(0, |x| x.len());
        out.push((size, Reader { name: format!("{}'s picture reader", m.name), model: llm, mmproj: vision }));
    }
    // Qwen3-VL first: it reads text in pictures and follows the checklist
    // format best. Then the smallest, which loads quickest.
    let rank = |r: &Reader| {
        let n = format!("{} {}", r.name, r.model.file_name().unwrap_or_default().to_string_lossy()).to_lowercase().replace([' ', '-', '_'], "");
        if n.contains("qwen3vl") {
            0
        } else if n.contains("qwen") || n.contains("mistral") {
            1
        } else {
            2
        }
    };
    out.sort_by_key(|(size, r)| (rank(r), *size));
    out.into_iter().map(|(_, r)| r).collect()
}

/// Whether pictures can be read on this PC: the given (or default) chat
/// model sees them, or a vision model is installed.
pub fn can_read(state: &AppState, chat_model: Option<&str>) -> bool {
    chat_sees(state, chat_model) || !readers(state).is_empty()
}

/// The chat model (the given one, else the default) has an image encoder.
pub fn chat_sees(state: &AppState, chat_model: Option<&str>) -> bool {
    let id = chat_model.map(str::to_string).or_else(|| crate::db::settings(&state.db.lock().unwrap()).default_model);
    let Some(id) = id else { return false };
    state.model_spec(&id).is_some_and(|s| s.vision.is_some())
}

/// A picture as the vision model takes it: a JPEG data URL, at most 1024 px.
pub fn data_url(img: &image::DynamicImage) -> Result<String, String> {
    use base64::Engine as _;
    let img = crate::media::imaging::cap(img.clone(), 1024).to_rgb8();
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 88).encode_image(&img).map_err(|e| e.to_string())?;
    Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// One reading at a time: they share the graphics card.
fn turn() -> &'static tokio::sync::Mutex<()> {
    static T: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    T.get_or_init(Default::default)
}

/// Asks a vision model about pictures: `ask` says what to tell. With
/// `chat_model`, that model reads them when it can see pictures; otherwise
/// (and with None) a vision model of its own does.
pub async fn read(state: &Arc<AppState>, pictures: &[image::DynamicImage], ask: &str, chat_model: Option<&str>, max_tokens: u32) -> Result<String, String> {
    if pictures.is_empty() {
        return Err("No picture to read.".into());
    }
    let mut parts = vec![json!({ "type": "text", "text": ask })];
    for p in pictures.iter().take(4) {
        parts.push(json!({ "type": "image_url", "image_url": { "url": data_url(p)? } }));
    }
    let messages = vec![
        json!({ "role": "system", "content": "You look at pictures and describe exactly what is in them, accurately and specifically. Never guess at what you can't see." }),
        json!({ "role": "user", "content": Value::Array(parts) }),
    ];
    if let Some(m) = chat_model.filter(|m| chat_sees(state, Some(m))) {
        let _turn = turn().lock().await;
        let (ep, _, _) = crate::llm_endpoint(state, Some(m.to_string()), true, || {}).await?;
        return crate::chat::complete(&ep, messages, json!({ "temperature": 0.2 }), max_tokens).await.map(|s| s.trim().to_string());
    }
    let reader = readers(state).into_iter().next().ok_or(NO_READER)?;
    let helper = Helper::start(state, &reader.name, &reader.model, Some(&reader.mmproj)).await?;
    crate::applog::line("media", &format!("Read {} picture(s) with {}", pictures.len().min(4), reader.name));
    let said = helper.ask(messages, max_tokens, 0.2).await;
    helper.done(state).await;
    said
}

/// Asks a running helper that sees pictures about one picture.
pub async fn describe_with(h: &Helper, img: &image::DynamicImage, ask: &str, max_tokens: u32) -> Result<String, String> {
    if !h.sees {
        return Err(format!("{} can't see pictures.", h.name));
    }
    let messages = vec![
        json!({ "role": "system", "content": "You look at pictures and describe exactly what is in them, accurately and specifically. Never guess at what you can't see." }),
        json!({ "role": "user", "content": [ { "type": "text", "text": ask }, { "type": "image_url", "image_url": { "url": data_url(img)? } } ] }),
    ];
    h.ask(messages, max_tokens, 0.2).await
}

pub const NO_READER: &str = "The chat model can't see pictures and no picture reader is installed. Pick a model that sees pictures (Qwen3 VL, Gemma 3), or install Qwen-Image, whose picture reader SulcusAI uses.";

/// A model run for a moment beside the chat model (or in its place, when
/// both don't fit the card): the picture reader, or a picture model's own
/// language model writing its prompt. One at a time; `done` stops it and
/// brings back a chat model it paused, at its old address.
pub struct Helper {
    pub ep: engine::Endpoint,
    pub name: String,
    /// It reads pictures.
    pub sees: bool,
    eng: Option<engine::Engine>,
    paused: Option<engine::Paused>,
    _turn: tokio::sync::MutexGuard<'static, ()>,
}

impl Helper {
    /// Starts `model` (with `mmproj`, it reads pictures) on an engine of its own.
    pub async fn start(state: &Arc<AppState>, name: &str, model: &std::path::Path, mmproj: Option<&std::path::Path>) -> Result<Helper, String> {
        let turn = turn().lock().await;
        let backend = engine::backend_for(&state.budget())?;
        let exe = engine::installed_server(&state.paths, &state.catalog.engine, backend).ok_or("The AI engine isn't installed yet: send a chat message once first.")?;
        // Room on the card: the chat model steps aside only when both don't
        // fit, and comes back at its address after (a reply may wait on it).
        let size = |p: &std::path::Path| std::fs::metadata(p).map_or(0, |m| m.len());
        let need = size(model) + mmproj.map_or(0, size) + 3 * crate::catalog::GIB / 2;
        let vram = state.budget().vram;
        let reserved = crate::media::chat_model_vram(state).await;
        let paused = if vram > 0 && reserved > 0 && vram.saturating_sub(reserved) < need { state.engine.lock().await.pause().await } else { None };
        let limits = state.limits();
        let mut eng = engine::Engine::default();
        let log = state.paths.data.join("logs").join("helper.log");
        let started = eng
            .ensure(
                engine::LaunchSpec {
                    exe: &exe,
                    model_path: model,
                    model_id: "helper",
                    // Shown in the log: "Loaded helper (Qwen3 VL 4B, …)".
                    quant: name,
                    ctx: 8192,
                    gpu_layers: if backend == engine::cpu_key() || vram < need { 0 } else { 999 },
                    threads: limits.threads,
                    low_priority: limits.low_priority,
                    log: &log,
                    mmproj,
                    batch: 0,
                    lora: None,
                },
                state.job(),
            )
            .await;
        match started {
            Ok(ep) => Ok(Helper { ep, name: name.to_string(), sees: mmproj.is_some(), eng: Some(eng), paused, _turn: turn }),
            Err(e) => {
                if let Some(p) = paused {
                    crate::media::give_back(state, p).await;
                }
                Err(e)
            }
        }
    }

    /// The chat model as the helper (it's loaded already, or is the default):
    /// nothing to stop afterwards.
    pub async fn chat_model(state: &Arc<AppState>, pictures: bool) -> Result<Helper, String> {
        let turn = turn().lock().await;
        let (ep, spec) = if pictures {
            let (ep, spec, _) = crate::llm_endpoint(state, None, true, || {}).await?;
            (ep, spec)
        } else {
            crate::background_endpoint(state).await?
        };
        let sees = pictures && spec.vision.is_some();
        Ok(Helper { ep, name: spec.name, sees, eng: None, paused: None, _turn: turn })
    }

    /// One answer (thinking is off: `chat::complete` asks for that).
    pub async fn ask(&self, messages: Vec<Value>, max_tokens: u32, temperature: f32) -> Result<String, String> {
        crate::chat::complete(&self.ep, messages, json!({ "temperature": temperature }), max_tokens).await.map(|s| s.trim().to_string())
    }

    /// One answer as JSON matching `schema`.
    pub async fn ask_json(&self, messages: Vec<Value>, schema: Value, max_tokens: u32, temperature: f32) -> Result<Value, String> {
        let extra = json!({
            "temperature": temperature,
            "response_format": { "type": "json_schema", "json_schema": { "name": "answer", "schema": schema } },
        });
        let said = crate::chat::complete(&self.ep, messages, extra, max_tokens).await?;
        serde_json::from_str(&said).or_else(|_| {
            // Some answers wrap the JSON in words or a code block.
            let start = said.find('{').ok_or("The model didn't answer in JSON.")?;
            let end = said.rfind('}').ok_or("The model didn't answer in JSON.")?;
            serde_json::from_str(&said[start..=end]).map_err(|_| "The model didn't answer in JSON.".to_string())
        })
    }

    /// Stops the helper and brings back a chat model it paused.
    pub async fn done(mut self, state: &Arc<AppState>) {
        if let Some(mut e) = self.eng.take() {
            e.stop().await;
        }
        if let Some(p) = self.paused.take() {
            crate::media::give_back(state, p).await;
        }
    }
}


/// What the attached pictures of a chat show, read once and kept with each
/// picture: (id, description) for every picture that has one. `ids` are the
/// chat's picture ids, newest last; only the newest four are read.
pub async fn describe_attached(state: &Arc<AppState>, cipher: &crate::crypto::Cipher, ids: &[String], on_reading: impl Fn()) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut told = false;
    for id in ids.iter().rev().take(4) {
        let item = crate::media::store::get(&state.db.lock().unwrap(), cipher, id);
        let Some(mut item) = item.filter(|i| i.kind == "image") else { continue };
        if let Some(d) = item.description.clone() {
            out.push((id.clone(), d));
            continue;
        }
        let Ok(bytes) = crate::media::store::read(&state.paths, cipher, id) else { continue };
        let Ok(img) = crate::media::imaging::decode(&bytes) else { continue };
        if !told {
            on_reading();
            told = true;
        }
        match read(state, &[img], DESCRIBE, None, 450).await {
            Ok(d) if !d.is_empty() => {
                item.description = Some(d.clone());
                let _ = crate::media::store::update(&state.db.lock().unwrap(), cipher, &item);
                out.push((id.clone(), d));
            }
            Ok(_) => {}
            Err(e) => {
                crate::applog::line("media", &format!("Couldn't read a picture: {e}"));
                break;
            }
        }
    }
    out
}

/// What a picture shows, for a chat whose model can't see it.
pub const DESCRIBE: &str = "Describe this picture in detail so someone who can't see it knows exactly what it shows: the subject and every person \
     (looks, clothes, pose, expression), the setting, every object with its color and position, any text word for word, the style \
     (photo, drawing, painting, 3D…), the colors, light and mood. Plain sentences, no lists, under 220 words.";

/// What the picture reader tells about an example picture: only the part
/// to be taken from it (a style example's room and people were copied
/// into a picture asked to follow only its look).
pub fn as_example(use_for: &str) -> &'static str {
    match use_for {
        "subject" => "This picture shows a subject someone wants in a new picture. Describe only the main subject (the person, animal or thing) \
             precisely enough to draw it again: its kind, shape, colors, markings, face and hair, clothes, distinctive details. Not the \
             background, the setting or the style. Comma-separated phrases a picture model understands, most important first, under 80 words.",
        "layout" => "This picture's layout is to be reused. Describe only the composition: what sits where (left, right, center, foreground, \
             background), the sizes of things relative to the frame, the camera angle and distance, and the horizon line. Not colors, style or \
             what the things are in detail. Comma-separated phrases, under 70 words.",
        _ => "This picture is an example of the look someone wants. Describe only its visual style: the medium (photo, painting, 3D, \
             illustration), the color palette, the lighting and its color, contrast, texture and grain, the lens and depth of field, the mood. \
             Don't name any objects, people, rooms or places in it, and don't describe its layout: the new picture has its own content. \
             Comma-separated phrases a picture model understands, under 60 words.",
    }
}
