// SPDX-License-Identifier: AGPL-3.0-only
//! Explainer videos: the chat model turns what the person is learning (their
//! words, files, documents) into a few slides, each with what's said over
//! it; a voice reads it (any narration voice, theirs too); and the slides
//! become an MP4 with the reading as its sound, each slide up while it's
//! talked about. Drawn and encoded by Windows (GDI, Media Foundation).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use image::{Rgba, RgbaImage};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{own_voice, podcast, stems, store, MediaItem, Request, RunCtx};
use crate::{download, AppState};

/// Frame size and rate: slides change only between sentences.
const W: u32 = 1280;
const H: u32 = 720;
const FPS: u32 = 5;
const RATE: u32 = 44_100;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Slide {
    pub title: String,
    pub points: Vec<String>,
    /// What's said while it's up.
    pub say: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Deck {
    pub title: String,
    pub slides: Vec<Slide>,
}

/// A deck from the writer's reply.
pub fn deck_from(reply: &str) -> Option<Deck> {
    let start = reply.find('{')?;
    let end = reply.rfind('}')?;
    let v: Value = serde_json::from_str(reply.get(start..=end)?).ok()?;
    let slides: Vec<Slide> = v
        .get("slides")?
        .as_array()?
        .iter()
        .filter_map(|s| {
            let title = s.get("title").and_then(Value::as_str).unwrap_or("").trim().to_string();
            let points = s.get("points").and_then(Value::as_array).map(|a| a.iter().filter_map(|p| p.as_str().map(|x| x.trim().to_string())).filter(|x| !x.is_empty()).take(5).collect()).unwrap_or_default();
            let say = s.get("say").or_else(|| s.get("narration")).and_then(Value::as_str).unwrap_or("").trim().to_string();
            (!say.is_empty() && !title.is_empty()).then_some(Slide { title, points, say })
        })
        .collect();
    let title = v.get("title").and_then(Value::as_str).unwrap_or("").trim().to_string();
    (!slides.is_empty()).then(|| Deck { title: if title.is_empty() { slides[0].title.clone() } else { title }, slides })
}

/// Writes the slides (as JSON) from what the person gave: before any recorded
/// voice takes the graphics card.
pub async fn write(state: &Arc<AppState>, req: &Request, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<String, String> {
    let spec = req.talk.clone().unwrap_or_default();
    progress("Getting the writer ready", 0.02);
    let (ep, model, _hold) = crate::endpoint_for(state, spec.writer.as_deref()).await?;
    write_with(state, req, &ep, &model.name, cancel, progress).await
}

/// `write` with the model at `ep` (`name`, for messages).
pub(crate) async fn write_with(state: &Arc<AppState>, req: &Request, ep: &crate::engine::Endpoint, name: &str, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<String, String> {
    let spec = req.talk.clone().unwrap_or_default();
    let mut material = req.prompt.trim().to_string();
    for f in spec.files.iter().take(10) {
        let path = std::path::PathBuf::from(f);
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if let Ok(Ok(text)) = tokio::task::spawn_blocking(move || crate::docs::read_text(&path)).await {
            material.push_str(&format!("\n\n# {name}\n{text}"));
        }
    }
    if material.trim().is_empty() {
        return Err("Say what the video explains, or add files.".into());
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }
    let room = ((ep.ctx as usize).saturating_sub(3_000) * 3).clamp(4_000, 50_000);
    let minutes = spec.minutes.unwrap_or(3.0).clamp(1.0, 10.0);
    let slides = ((minutes * 2.5).round() as usize).clamp(3, 20);
    let lang = req.language.as_deref().and_then(|c| crate::translate::LANGUAGES.iter().find(|(k, _)| k.eq_ignore_ascii_case(c)).map(|(_, n)| *n)).map(|n| format!(" Write it all in {n}.")).unwrap_or_default();
    progress("Writing the slides", 0.15);
    let ask = format!(
        "Make a short explainer video that teaches the material below to a curious student: about {slides} slides, about {minutes:.0} minutes spoken. \
         The first slide introduces the topic; the last sums up the key points. Each slide has a short title, 2 to 4 short points (a few words each), \
         and what the narrator says while it's up (2 to 5 plain, friendly sentences that explain, with an example where it helps; not just reading the points). \
         Use only facts from the material. Reply with only JSON: {{\"title\": \"…\", \"slides\": [{{\"title\": \"…\", \"points\": [\"…\"], \"say\": \"…\"}}]}}.{lang}\n\nMaterial:\n{}",
        material.chars().take(room).collect::<String>()
    );
    let msgs = vec![json!({"role": "system", "content": "You write clear, accurate teaching material."}), json!({"role": "user", "content": ask})];
    for attempt in 0..2 {
        let reply = crate::chat::complete(ep, msgs.clone(), json!({"temperature": if attempt == 0 { 0.5 } else { 0.3 }}), 6_000).await?;
        if let Some(deck) = deck_from(&reply) {
            state.log("media", &format!("Wrote {} slides for an explainer video with {name}", deck.slides.len()));
            return serde_json::to_string(&deck).map_err(|e| e.to_string());
        }
    }
    Err(format!("{name} didn't write the slides. Try again, or pick another model to write them."))
}

/// The slides' backdrop: a deep gradient, a soft band behind the title.
fn backdrop() -> RgbaImage {
    RgbaImage::from_fn(W, H, |x, y| {
        let f = y as f32 / H as f32;
        let g = x as f32 / W as f32;
        let r = (22.0 + 30.0 * g + 10.0 * f) as u8;
        let gg = (20.0 + 12.0 * f) as u8;
        let b = (48.0 + 50.0 * (1.0 - f) + 20.0 * g) as u8;
        Rgba([r, gg, b, 255])
    })
}

/// One slide drawn: its title, its points, and where it is in the deck.
#[cfg(windows)]
fn draw(slide: &Slide, n: usize, of: usize, deck: &str) -> Result<RgbaImage, String> {
    use super::songvideo::{stamp, text_mask};
    let mut frame = backdrop();
    let accent = [157, 140, 255];
    if n == 0 {
        let t = text_mask(&slide.title, 64, true, W - 200)?;
        stamp(&mut frame, &t, (W - t.width()) / 2, H / 2 - t.height() - 10, [255, 255, 255], 1.0);
        let mut y = H / 2 + 20;
        for p in slide.points.iter().take(3) {
            let m = text_mask(p, 30, false, W - 260)?;
            stamp(&mut frame, &m, (W - m.width()) / 2, y, [220, 218, 235], 1.0);
            y += m.height() + 10;
        }
    } else {
        let t = text_mask(&slide.title, 48, true, W - 160)?;
        stamp(&mut frame, &t, (W - t.width()) / 2, 70, [255, 255, 255], 1.0);
        // A short rule under the title.
        for x in (W / 2 - 60)..(W / 2 + 60) {
            for y in (90 + t.height())..(94 + t.height()) {
                frame.put_pixel(x, y, Rgba([accent[0], accent[1], accent[2], 255]));
            }
        }
        let mut y = 140 + t.height();
        for p in slide.points.iter().take(5) {
            let m = text_mask(&format!("•  {p}"), 34, false, W - 240)?;
            if y + m.height() > H - 70 {
                break;
            }
            stamp(&mut frame, &m, (W - m.width()) / 2, y, [230, 228, 242], 1.0);
            y += m.height() + 22;
        }
    }
    let foot = text_mask(&format!("{deck}  ·  {} / {of}", n + 1), 20, false, W - 100)?;
    stamp(&mut frame, &foot, (W - foot.width()) / 2, H - 44, [170, 166, 196], 0.9);
    Ok(frame)
}

/// Reads the slides and makes the video.
#[cfg(windows)]
pub(super) async fn make(ctx: &RunCtx<'_>, work: &Path) -> Result<Vec<MediaItem>, String> {
    let req = ctx.req;
    let deck: Deck = serde_json::from_str(req.lyrics.as_deref().ok_or("The slides weren't written.")?).map_err(|e| e.to_string())?;
    let spec = req.talk.clone().unwrap_or_default();
    let mut speakers = vec![spec.speakers.first().cloned().unwrap_or(podcast::Speaker { name: "Narrator".into(), ..Default::default() })];
    let language = req.language.clone().filter(|l| l != "auto");
    if speakers[0].voice.is_none() {
        let voices = tokio::task::spawn_blocking(crate::tts::voices).await.map_err(|e| e.to_string())?.unwrap_or_default();
        podcast::fill_voices(&mut speakers, &voices, language.as_deref());
    }
    let texts: Vec<String> = deck.slides.iter().map(|s| s.say.clone()).collect();
    let n = texts.len();
    // Each slide's reading, at 44.1 kHz.
    let mut said: Vec<Vec<f32>> = Vec::with_capacity(n);
    if let Some(id) = speakers[0].voice.as_deref().and_then(|v| v.strip_prefix("own:")) {
        let (parts, rate) = own_voice::speak_each(ctx.state, id, &texts, language.as_deref(), ctx.cancel, &|stage, f| (ctx.progress)(stage, 0.3 + 0.5 * f)).await?;
        said = parts.iter().map(|p| stems::resample(p, rate, RATE)).collect();
    } else {
        for (i, t) in texts.iter().enumerate() {
            if ctx.cancel.load(Ordering::Relaxed) {
                return Err(download::CANCELLED.into());
            }
            (ctx.progress)(&format!("Reading slide {} of {n}", i + 1), 0.3 + 0.5 * i as f64 / n as f64);
            let (text, voice, lang) = (t.clone(), speakers[0].voice.clone(), language.clone());
            let s = tokio::task::spawn_blocking(move || crate::tts::synthesize(&text, voice.as_deref(), lang.as_deref(), 1.0)).await.map_err(|e| e.to_string())??;
            said.push(stems::resample(&s.samples.iter().map(|x| *x as f32 / 32768.0).collect::<Vec<_>>(), s.rate, RATE));
        }
    }
    (ctx.progress)("Drawing the slides", 0.82);
    let out = work.join("explainer.mp4");
    let title = deck.title.clone();
    let slides = deck.slides.clone();
    let (first, seconds) = tokio::task::spawn_blocking({
        let out = out.clone();
        move || -> Result<(RgbaImage, f64), String> {
            let pause = (RATE as f32 * 0.6) as usize;
            let mut writer = super::mp4::Writer::create_with_sound(&out, W, H, FPS, Some(RATE), Some(1_200_000))?;
            let mut first = None;
            let mut total = 0usize;
            for (i, (slide, voice)) in slides.iter().zip(&said).enumerate() {
                let frame = draw(slide, i, slides.len(), &title)?;
                if first.is_none() {
                    first = Some(frame.clone());
                }
                // The slide's sound: its reading and a short pause, as stereo.
                let mut pcm: Vec<i16> = voice.iter().chain(std::iter::repeat_n(&0.0, pause)).flat_map(|s| {
                    let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                    [v, v]
                }).collect();
                // Whole frames: the sound is padded to the frame's end.
                let frames = ((pcm.len() / 2) as f64 / RATE as f64 * FPS as f64).ceil().max(1.0) as usize;
                pcm.resize(frames * (RATE / FPS) as usize * 2, 0);
                for f in 0..frames {
                    let a = f * (RATE / FPS) as usize * 2;
                    let b = a + (RATE / FPS) as usize * 2;
                    writer.sound(&pcm[a..b])?;
                    writer.frame(&frame)?;
                }
                total += frames;
            }
            writer.finish()?;
            Ok((first.unwrap_or_else(backdrop), total as f64 / FPS as f64))
        }
    })
    .await
    .map_err(|e| e.to_string())??;
    let mp4 = std::fs::read(&out).map_err(|e| e.to_string())?;
    let thumb = super::imaging::thumbnail(&image::DynamicImage::ImageRgba8(first))?;
    let script = deck.slides.iter().map(|s| format!("# {}\n{}", s.title, s.say)).collect::<Vec<_>>().join("\n\n");
    let c = ctx.state.work_cipher()?;
    let item = MediaItem {
        kind: "video".into(),
        op: "explainer".into(),
        prompt: req.prompt.trim().chars().take(4_000).collect(),
        title: Some(deck.title.clone()),
        mime: "video/mp4".into(),
        width: W,
        height: H,
        seconds,
        chat_id: req.chat_id.clone(),
        lyrics: Some(script),
        poster: true,
        ..Default::default()
    };
    ctx.state.log("media", &format!("Made an explainer video: {} slides, {}", deck.slides.len(), super::clock(seconds)));
    Ok(vec![store::save(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &c, item, &mp4, Some(&thumb))?])
}

#[cfg(not(windows))]
pub(super) async fn make(_: &RunCtx<'_>, _: &Path) -> Result<Vec<MediaItem>, String> {
    Err("Explainer videos aren't available on this system yet.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_become_slides() {
        let d = deck_from(r#"Sure! {"title": "Cells", "slides": [{"title": "What is a cell?", "points": ["Smallest unit of life", ""], "say": "Every living thing is made of cells."}, {"title": "No words", "points": []}]}"#).unwrap();
        assert_eq!(d.title, "Cells");
        assert_eq!(d.slides.len(), 1, "a slide with nothing to say is left out");
        assert_eq!(d.slides[0].points, vec!["Smallest unit of life"]);
        assert!(deck_from("no json").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn slides_are_drawn() {
        let s = Slide { title: "Photosynthesis".into(), points: vec!["Light becomes sugar".into(), "In the leaves".into()], say: "x".into() };
        let f = draw(&s, 1, 3, "Plants").unwrap();
        assert_eq!((f.width(), f.height()), (W, H));
        // Words were drawn: some pixels are near white.
        assert!(f.pixels().filter(|p| p[0] > 200 && p[1] > 200).count() > 500);
    }
}
