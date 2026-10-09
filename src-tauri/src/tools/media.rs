// SPDX-License-Identifier: AGPL-3.0-only
//! create_image, edit_image, create_video and create_music: the studio's
//! jobs, run from a chat. What they make shows in the chat and the gallery.

use std::sync::Arc;

use serde_json::{json, Value};

use super::{Ctx, Outcome};
use crate::media::{self, MediaItem, Request};

pub fn create_image_params() -> Value {
    json!({ "type": "object", "required": ["prompt"], "properties": {
        "prompt": { "type": "string", "description": "A vivid description: subject, setting, style (photo, watercolor, 3D…), lighting and mood. Write it in English." },
        "shape": { "type": "string", "enum": ["square", "portrait", "landscape", "wide", "tall"], "description": "Default square." },
        "count": { "type": "integer", "description": "How many versions (1-4, default 1)." } } })
}

pub fn edit_image_params() -> Value {
    json!({ "type": "object", "required": ["instruction"], "properties": {
        "instruction": { "type": "string", "description": "What to change, e.g. \"make it night\", \"add a red scarf\". For restyle, the style." },
        "action": { "type": "string", "enum": ["edit", "restyle", "upscale", "remove_background"], "description": "Default edit." },
        "image": { "type": "string", "description": "The picture's id. Leave it out for the newest picture in this chat." } } })
}

pub fn create_video_params() -> Value {
    json!({ "type": "object", "required": ["prompt"], "properties": {
        "prompt": { "type": "string", "description": "What happens in the clip: subject, motion, camera, style. Write it in English." },
        "seconds": { "type": "number", "description": "Length, 1-8 seconds (default 3)." },
        "image": { "type": "string", "description": "A picture's id to bring to life; leave out to make the clip from the description." } } })
}

pub fn create_music_params() -> Value {
    json!({ "type": "object", "required": ["description"], "properties": {
        "description": { "type": "string", "description": "Genre, mood, instruments, tempo and voice, e.g. \"upbeat acoustic folk, warm male vocals\". For a sound effect, the sound." },
        "lyrics": { "type": "string", "description": "Lyrics to sing, with [Verse] and [Chorus] lines. Leave out to have them written." },
        "instrumental": { "type": "boolean", "description": "No vocals." },
        "sound_effect": { "type": "boolean", "description": "A sound effect rather than music." },
        "seconds": { "type": "number", "description": "Length in seconds (music 10-240, default 30; sound 10-30)." } } })
}

fn summary(items: &[MediaItem]) -> String {
    items
        .iter()
        .map(|i| match i.kind.as_str() {
            "image" => format!("picture {} ({}×{})", i.id, i.width, i.height),
            "video" => format!("video {} ({:.0} s)", i.id, i.seconds),
            _ => format!("audio {} ({:.0} s)", i.id, i.seconds),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub async fn run(name: &str, args: &Value, ctx: &Ctx<'_>) -> Outcome {
    let state: Arc<crate::AppState> = ctx.state.clone();
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
    let chat_id = Some(ctx.memory.chat_id.to_string());
    let req = match name {
        "create_image" => Request {
            op: "generate".into(),
            prompt: s("prompt").unwrap_or_default(),
            shape: s("shape"),
            count: args.get("count").and_then(Value::as_u64).map(|n| n as u32),
            chat_id,
            ..Default::default()
        },
        "edit_image" => {
            let action = s("action").unwrap_or_else(|| "edit".into());
            let op = match action.as_str() {
                "restyle" | "upscale" | "remove_background" => action,
                _ => "edit".into(),
            };
            let source = s("image").or_else(|| media::store::latest_in_chat(&ctx.memory.db.lock().unwrap(), ctx.memory.cipher, ctx.memory.chat_id, "image").map(|i| i.id));
            let Some(source) = source else {
                return Outcome::error("Couldn't edit a picture", "There's no picture in this chat yet. Make one with create_image, or ask the user to attach one.");
            };
            Request { op, prompt: s("instruction").unwrap_or_default(), source: Some(source), chat_id, ..Default::default() }
        }
        "create_video" => Request {
            op: "video".into(),
            prompt: s("prompt").unwrap_or_default(),
            seconds: args.get("seconds").and_then(Value::as_f64).map(|v| v as f32),
            source: s("image"),
            chat_id,
            ..Default::default()
        },
        "create_music" => Request {
            op: if args.get("sound_effect").and_then(Value::as_bool) == Some(true) { "sound".into() } else { "music".into() },
            prompt: s("description").unwrap_or_default(),
            lyrics: s("lyrics"),
            instrumental: args.get("instrumental").and_then(Value::as_bool),
            seconds: args.get("seconds").and_then(Value::as_f64).map(|v| v as f32),
            chat_id,
            ..Default::default()
        },
        other => return Outcome::error(format!("Couldn't use {other}"), "Unknown media tool."),
    };
    let what = match req.op.as_str() {
        "video" => "a video",
        "music" => "a song",
        "sound" => "a sound",
        "upscale" => "a bigger picture",
        "remove_background" => "a cut-out",
        _ => "a picture",
    };
    // Stopping the reply stops the job too.
    let job_id = media::store::new_id();
    let job = media::run(&state, job_id.clone(), req);
    tokio::pin!(job);
    let result = loop {
        tokio::select! {
            r = &mut job => break r,
            _ = tokio::time::sleep(std::time::Duration::from_millis(300)) => {
                if ctx.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    media::cancel(&job_id);
                }
            }
        }
    };
    match result {
        Ok(items) => {
            let text = format!(
                "Made {}: {}. It's already shown to the user in the chat. Tell them in a sentence what you made; don't paste ids or links. \
                 To change it, call edit_image with an instruction.",
                if items.len() > 1 { format!("{} versions", items.len()) } else { what.to_string() },
                summary(&items)
            );
            let mut o = Outcome::ok(text, format!("Made {what}"), "text", None);
            o.meta["media"] = json!(items);
            o
        }
        Err(e) if e == crate::download::CANCELLED => {
            let mut o = Outcome::denied(format!("Making {what}"));
            o.text = "Stopped by the user.".into();
            o
        }
        Err(e) => Outcome::error(format!("Couldn't make {what}"), e),
    }
}
