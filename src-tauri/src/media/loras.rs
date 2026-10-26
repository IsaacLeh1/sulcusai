// SPDX-License-Identifier: AGPL-3.0-only
//! The person's own LoRAs for pictures: small add-on files (a character, a
//! style, an object) that steer a picture model they were made for. Added
//! from a file, kept with the media models, offered with the picture models
//! of their family, and named in the prompt the way the picture engine
//! (stable-diffusion.cpp) loads them: `<lora:name:weight>`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::Paths;
use crate::{db, AppStateRef};

const KEY: &str = "picture_loras";

/// A LoRA added here.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Lora {
    /// Also its file's name (without .safetensors) in the LoRA folder.
    pub id: String,
    pub name: String,
    /// The picture models it's for (`family_of`).
    pub family: String,
    /// Words that call it up, put in the prompt with it.
    pub trigger: String,
    /// How strongly it's used by default (0.1 to 1.5).
    pub weight: f32,
    pub size: u64,
}

/// The families a LoRA can be for, as shown.
pub const FAMILIES: [(&str, &str); 7] = [
    ("flux2-klein-4b", "FLUX.2 [klein] 4B"),
    ("flux2-klein-9b", "FLUX.2 [klein] 9B"),
    ("flux1", "FLUX.1"),
    ("sdxl", "SDXL (RealVisXL and the like)"),
    ("sd15", "Stable Diffusion 1.5 (Realistic Vision and the like)"),
    ("z-image", "Z-Image"),
    ("qwen-image", "Qwen-Image"),
];

/// The family of a picture model: which LoRAs fit it.
pub fn family_of(model_id: &str) -> Option<&'static str> {
    let id = model_id.to_lowercase();
    Some(match () {
        _ if id.contains("klein-4b") => "flux2-klein-4b",
        _ if id.contains("klein-9b") => "flux2-klein-9b",
        _ if id.starts_with("flux1") || id.contains("flux.1") => "flux1",
        _ if id.contains("xl") => "sdxl",
        _ if id.contains("realistic-vision") || id.contains("sd15") || id.contains("sd-1.5") || id.contains("v1-5") => "sd15",
        _ if id.contains("z-image") => "z-image",
        _ if id.contains("qwen-image") => "qwen-image",
        _ => return None,
    })
}

pub fn dir(paths: &Paths) -> PathBuf {
    paths.models.join("media").join("loras")
}

pub fn list(conn: &rusqlite::Connection) -> Vec<Lora> {
    db::get(conn, KEY).unwrap_or_default()
}

pub(crate) fn store(conn: &rusqlite::Connection, all: &[Lora]) -> Result<(), String> {
    db::set(conn, KEY, &all.to_vec())
}

/// A best guess at a LoRA's family from its tensors' names (the person can
/// change it): FLUX's blocks, SDXL's second text encoder, SD 1.5's U-Net.
pub fn guess_family(file: &Path) -> Option<&'static str> {
    use std::io::Read;
    let mut f = std::fs::File::open(file).ok()?;
    let mut len = [0u8; 8];
    f.read_exact(&mut len).ok()?;
    let n = u64::from_le_bytes(len).min(64 << 20) as usize;
    let mut header = vec![0u8; n];
    f.read_exact(&mut header).ok()?;
    let text = String::from_utf8_lossy(&header);
    if text.contains("double_blocks") || text.contains("single_blocks") || text.contains("single_transformer_blocks") {
        // FLUX.2's text side is Qwen3; FLUX.1's is T5.
        return Some(if text.contains("double_stream_modulation") || text.contains("flux2") { "flux2-klein-4b" } else { "flux1" });
    }
    // SDXL: a second text encoder, or (a U-Net-only LoRA) more than one
    // transformer block in a layer and its cross-attention reading 2048-wide
    // text; SD 1.5 has one block and attention in its first input blocks.
    if text.contains("lora_te2_") || text.contains("text_encoder_2") || text.contains("conditioner.embedders.1") || text.contains("transformer_blocks_1_") || text.contains("transformer_blocks.1.") {
        return Some("sdxl");
    }
    if text.contains("lora_unet_") || text.contains("down_blocks") || text.contains("input_blocks") {
        return Some("sd15");
    }
    if text.contains("noise_refiner") || text.contains("context_refiner") {
        return Some("z-image");
    }
    if text.contains("transformer_blocks") && text.contains("img_mod") {
        return Some("qwen-image");
    }
    None
}

/// The LoRAs a picture job uses, as the prompt words that load them and
/// the folder they're in; None when it uses none (or none fit the model).
pub fn for_job(conn: &rusqlite::Connection, paths: &Paths, model_id: &str, picked: &[(String, f32)]) -> Option<(String, PathBuf)> {
    let family = family_of(model_id)?;
    let all = list(conn);
    let words: Vec<String> = picked
        .iter()
        .filter_map(|(id, w)| all.iter().find(|l| &l.id == id && l.family == family).map(|l| (l, *w)))
        .map(|(l, w)| {
            let tag = format!("<lora:{}:{:.2}>", l.id, w.clamp(0.0, 2.0));
            if l.trigger.trim().is_empty() { tag } else { format!("{}, {tag}", l.trigger.trim()) }
        })
        .collect();
    (!words.is_empty()).then(|| (words.join(", "), dir(paths)))
}

fn new_id(name: &str) -> String {
    let base: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).take(24).collect::<String>().to_lowercase();
    format!("{}-{}", if base.is_empty() { "lora".into() } else { base }, &uuid::Uuid::new_v4().simple().to_string()[..6])
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct LoraView {
    loras: Vec<Lora>,
    families: Vec<(String, String)>,
}

#[tauri::command]
pub fn loras(state: AppStateRef) -> LoraView {
    LoraView { loras: list(&state.db.lock().unwrap()), families: FAMILIES.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect() }
}

/// Adds a LoRA from a file (copied in), its family guessed from it.
#[tauri::command(async)]
pub fn lora_add(state: AppStateRef, path: String) -> Result<Lora, String> {
    add(&state, &PathBuf::from(&path))
}

/// `lora_add`, for whoever holds the state.
pub fn add(state: &crate::AppState, from: &Path) -> Result<Lora, String> {
    let from = from.to_path_buf();
    if from.extension().map(|e| e.to_string_lossy().to_lowercase()) != Some("safetensors".into()) {
        return Err("Pick a LoRA file (.safetensors).".into());
    }
    let size = from.metadata().map_err(|e| format!("Couldn't read that file: {e}"))?.len();
    if size > 4 << 30 {
        return Err("That file is too big for a LoRA (more than 4 GB): it may be a whole model. Add whole models in Studio › Models.".into());
    }
    let name = from.file_stem().map(|s| s.to_string_lossy().replace(['_', '-'], " ")).unwrap_or_else(|| "LoRA".into());
    let id = new_id(&name);
    let into = dir(&state.paths);
    std::fs::create_dir_all(&into).map_err(|e| e.to_string())?;
    std::fs::copy(&from, into.join(format!("{id}.safetensors"))).map_err(|e| format!("Couldn't copy it in: {e}"))?;
    let lora = Lora { family: guess_family(&from).unwrap_or("").to_string(), id, name, trigger: String::new(), weight: 0.8, size };
    let conn = state.db.lock().unwrap();
    let mut all = list(&conn);
    all.push(lora.clone());
    store(&conn, &all)?;
    crate::applog::line("media", &format!("Added a LoRA ({}, {})", lora.name, if lora.family.is_empty() { "family not known" } else { lora.family.as_str() }));
    Ok(lora)
}

/// Saves a LoRA's name, family, trigger words and weight.
#[tauri::command(async)]
pub fn lora_save(state: AppStateRef, lora: Lora) -> Result<Vec<Lora>, String> {
    let conn = state.db.lock().unwrap();
    let mut all = list(&conn);
    let l = all.iter_mut().find(|l| l.id == lora.id).ok_or("That LoRA is no longer here.")?;
    l.name = lora.name.trim().chars().take(80).collect();
    l.family = lora.family;
    l.trigger = lora.trigger.trim().chars().take(200).collect();
    l.weight = lora.weight.clamp(0.1, 1.5);
    store(&conn, &all)?;
    Ok(all)
}

#[tauri::command(async)]
pub fn lora_delete(state: AppStateRef, id: String) -> Result<Vec<Lora>, String> {
    let conn = state.db.lock().unwrap();
    let mut all = list(&conn);
    all.retain(|l| l.id != id);
    store(&conn, &all)?;
    let _ = std::fs::remove_file(dir(&state.paths).join(format!("{id}.safetensors")));
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_and_loras_find_their_family() {
        assert_eq!(family_of("flux2-klein-4b"), Some("flux2-klein-4b"));
        assert_eq!(family_of("realvisxl-v5-lightning"), Some("sdxl"));
        assert_eq!(family_of("realistic-vision-v6"), Some("sd15"));
        assert_eq!(family_of("flux1-schnell"), Some("flux1"));
        assert_eq!(family_of("z-image-turbo"), Some("z-image"));
        // A file whose tensors say SDXL (its second text encoder).
        let tmp = std::env::temp_dir().join(format!("lora-{}.safetensors", uuid::Uuid::new_v4().simple()));
        let header = br#"{"lora_unet_input_blocks_4_1_proj_in.lora_down.weight":{"dtype":"F16","shape":[4,640],"data_offsets":[0,0]},"lora_te2_text_model_encoder_layers_0_mlp_fc1.lora_up.weight":{"dtype":"F16","shape":[1,1],"data_offsets":[0,0]}}"#;
        let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(header);
        std::fs::write(&tmp, &bytes).unwrap();
        assert_eq!(guess_family(&tmp), Some("sdxl"));
        std::fs::remove_file(&tmp).ok();
    }
}
