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

static NET_CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The catalog's ControlNet files: (guide, size in bytes, already here).
#[tauri::command]
pub fn create_control_net_choices(state: AppStateRef<'_>) -> Vec<(String, u64, bool)> {
    let have = super::run::control_nets(state.inner());
    state.catalog.media.controlnets.iter().map(|h| (h.file.role.clone(), h.file.size, have.contains(&h.file.file))).collect()
}

/// Downloads the catalog's ControlNet file for `guide` into the ControlNet
/// folder, checked against its pinned hash. Progress goes out as
/// `control-net-progress` (0 to 1).
#[tauri::command]
pub async fn create_get_control_net(app: tauri::AppHandle, state: AppStateRef<'_>, guide: String) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    use tauri::Emitter;
    let state = state.inner().clone();
    let h = state.catalog.media.controlnets.iter().find(|h| h.file.role == guide).cloned().ok_or("There's no download for that guide.")?;
    let dest = super::run::control_dir(&state).join(&h.file.file);
    if dest.metadata().is_ok_and(|m| m.len() == h.file.size) {
        return Ok(());
    }
    NET_CANCEL.store(false, Ordering::SeqCst);
    state.log("network", &format!("Downloading {} ({}) from {}", h.file.file, crate::size_label(h.file.size), crate::host_of(&h.file.url)));
    let client = crate::net::external_client(state.settings().connectivity, crate::net::Purpose::ModelDownload, false)?;
    std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
    crate::download::fetch_verified(&client, &h.file.url, &dest, h.file.size, &h.file.sha256, &NET_CANCEL, |r, t| {
        let _ = app.emit("control-net-progress", r as f64 / t.max(1) as f64);
    })
    .await
}

#[tauri::command]
pub fn create_cancel_control_net() {
    NET_CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::regions::{pick_net, Guide};

    #[test]
    fn every_guide_has_a_pinned_download_that_pick_net_finds() {
        let cat = crate::catalog::Catalog::bundled();
        let names: Vec<String> = cat.media.controlnets.iter().map(|h| h.file.file.clone()).collect();
        for h in &cat.media.controlnets {
            assert_eq!(h.file.sha256.len(), 64);
            assert!(h.file.url.contains(&h.file.file) && !h.file.url.contains("/main/"));
        }
        for g in [Guide::Sketch, Guide::Edges, Guide::Depth] {
            assert!(pick_net(g, &names).is_some(), "{g:?}");
        }
    }
}
