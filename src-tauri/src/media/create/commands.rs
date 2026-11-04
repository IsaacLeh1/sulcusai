// SPDX-License-Identifier: AGPL-3.0-only
//! Window commands for the creating-media planners. They only plan and
//! check; the jobs themselves go through the media queue.

use crate::AppStateRef;

use super::{batch, consent, lipsync, mic, regions, sfx, storyboard, tags, textcut};

fn likeness_ok(state: &AppStateRef<'_>) -> bool {
    !crate::consent::needs(&state.db.lock().unwrap(), "likeness")
}

#[tauri::command]
pub fn create_consent_check(state: AppStateRef<'_>, claim: consent::Claim) -> Result<(), String> {
    consent::check(&claim, likeness_ok(&state))
}

#[tauri::command]
pub fn create_storyboard(script: String, style: String, max_seconds: Option<f64>) -> Vec<storyboard::Shot> {
    storyboard::split(&script, &style, max_seconds.unwrap_or(5.0))
}

#[tauri::command]
pub fn create_batch_parse(csv: String) -> Result<Vec<batch::Item>, String> {
    batch::parse(&csv)
}

#[tauri::command]
pub fn create_dialogue_parts(script: String, narrator: String) -> Vec<tags::Part> {
    tags::parse(&script, &narrator)
}

#[tauri::command]
pub fn create_sfx_plan(request: String) -> sfx::Effect {
    sfx::plan(&request)
}

#[tauri::command]
pub fn create_text_cut(words: Vec<textcut::Word>, length: f64, strike_fillers: Option<bool>) -> (Vec<(f64, f64)>, String) {
    let mut words = words;
    if strike_fillers.unwrap_or(false) {
        textcut::strike_fillers(&mut words);
    }
    (textcut::keep_ranges(&words, length, 0.05, 0.25), textcut::kept_text(&words))
}

#[tauri::command]
pub fn create_lipsync_pieces(state: AppStateRef<'_>, lines: Vec<lipsync::Line>, length: f64, claim: consent::Claim) -> Result<Vec<lipsync::Piece>, String> {
    consent::check(&claim, likeness_ok(&state))?;
    Ok(lipsync::pieces(&lines, length, 0.2, 0.6, 5.0))
}

#[tauri::command]
pub fn create_region_scene(base: String, regions: Vec<regions::Region>) -> Result<String, String> {
    regions::check(&regions)?;
    Ok(regions::scene_prompt(&base, &regions))
}

#[tauri::command]
pub fn create_mic_check(samples: Vec<f32>, rate: u32) -> mic::Check {
    mic::check(&samples, rate)
}

/// A video's words with times, for editing it by its transcript.
#[tauri::command]
pub async fn create_transcript(state: AppStateRef<'_>, id: String) -> Result<Vec<textcut::Word>, String> {
    use crate::media::{edits, ffmpeg, jobs, stems, store};
    let state = state.inner();
    let c = state.work_cipher()?;
    let item = store::get(&state.db.lock().unwrap(), &c, &id).ok_or("That video is no longer in the gallery.")?;
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let (ffmpeg_exe, ffprobe) = ffmpeg::ensure(state, &cancel, &|_, _| {}).await?;
    let work = jobs::work_dir(&state.paths, &format!("transcript-{}", uuid::Uuid::new_v4()))?;
    let file = edits::written_out(state, &c, &item, &work.0.join("source"))?;
    let probe = ffmpeg::probe(&ffprobe, &file).await?;
    if !probe.audio {
        return Err("This has no sound, so there's no transcript.".into());
    }
    let mix = edits::ffmpeg_stereo(state, &ffmpeg_exe, &file, probe.seconds, &cancel).await?;
    let speech = crate::speech::endpoint(state, crate::speech::Use::Accurate).await.map_err(|e| format!("A transcript needs a speech model. {e}"))?;
    let options = crate::speech::Options { language: Some("auto".into()), ..Default::default() };
    let heard = crate::speech::transcribe(&speech, &stems::resample(&mix.mono(), stems::RATE, crate::audio::RATE), &options).await?;
    let pieces: Vec<(f64, f64, String)> = crate::speech::clean(heard).segments.into_iter().map(|s| (s.start, s.end, s.text)).collect();
    Ok(textcut::words_from(&pieces))
}

/// ControlNet files found, and the folder they go in.
#[tauri::command]
pub fn create_control_nets(state: AppStateRef<'_>) -> (Vec<String>, String) {
    (super::run::control_nets(state.inner()), super::run::control_dir(state.inner()).to_string_lossy().into_owned())
}

/// Whether voice chat can show a talking face with these settings.
#[tauri::command]
pub fn create_face_chat_check(state: AppStateRef<'_>, settings: super::face_chat::Settings) -> Result<(), String> {
    super::face_chat::ready(&settings, state.catalog.media.ditto.is_some(), likeness_ok(&state))
}
