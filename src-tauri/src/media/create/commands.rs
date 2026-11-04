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
