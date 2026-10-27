// SPDX-License-Identifier: AGPL-3.0-only
//! Settings for one model: instructions added when it answers, how it picks
//! its words, its context window and how the engine keeps its cache. What's
//! left empty stays as the app (or Advanced) has it. They apply wherever the
//! model answers: chats, the local API and the Ollama-shaped server.

use std::collections::HashMap;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::advanced::{self, Sampling};
use crate::{db, AppStateRef};

const KEY: &str = "model_presets";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ModelPreset {
    /// Added to the instructions whenever this model answers.
    pub instructions: String,
    pub sampling: Sampling,
    /// Context window in tokens.
    pub ctx: Option<u32>,
    /// "f16", "q8_0" or "q4_0".
    pub kv_cache: Option<String>,
    pub flash_attn: Option<bool>,
}

impl ModelPreset {
    pub fn is_empty(&self) -> bool {
        *self == ModelPreset::default()
    }
}

fn all(conn: &Connection) -> HashMap<String, ModelPreset> {
    db::get(conn, KEY).unwrap_or_default()
}

pub fn get(conn: &Connection, model_id: &str) -> ModelPreset {
    all(conn).remove(model_id).unwrap_or_default()
}

/// The model's own sampling settings over `extra` (the request's fields).
pub fn apply_sampling(extra: &mut Value, p: &ModelPreset) {
    let fields = advanced::sampling_fields(&p.sampling);
    if fields.is_empty() {
        return;
    }
    if !extra.is_object() {
        *extra = Value::Object(Default::default());
    }
    if let Some(m) = extra.as_object_mut() {
        m.extend(fields);
    }
}

/// The instructions with the model's own added.
pub fn with_instructions(base: String, p: &ModelPreset) -> String {
    let own = p.instructions.trim();
    if own.is_empty() {
        base
    } else {
        format!("{base}\n\nInstructions for this model, from the user:\n{own}")
    }
}

/// A cache type as the engine takes it, or None for one it doesn't.
pub fn cache_type(kind: Option<&str>) -> Option<&'static str> {
    match kind.map(str::trim) {
        Some("f16") => Some("f16"),
        Some("q8_0") => Some("q8_0"),
        Some("q4_0") => Some("q4_0"),
        _ => None,
    }
}

#[tauri::command(async)]
pub fn model_preset(state: AppStateRef, model_id: String) -> ModelPreset {
    get(&state.db.lock().unwrap(), &model_id)
}

#[tauri::command(async)]
pub fn set_model_preset(state: AppStateRef, model_id: String, preset: ModelPreset) -> Result<(), String> {
    if preset.kv_cache.is_some() && cache_type(preset.kv_cache.as_deref()).is_none() {
        return Err("The cache type is f16, q8_0 or q4_0.".into());
    }
    let conn = state.db.lock().unwrap();
    let mut presets = all(&conn);
    if preset.is_empty() {
        presets.remove(&model_id);
    } else {
        presets.insert(model_id.clone(), preset);
    }
    db::set(&conn, KEY, &presets)?;
    drop(conn);
    state.log("settings", &format!("Changed the settings for the model {model_id}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A model's own settings go over the app's, and empty ones change nothing.
    #[test]
    fn a_models_settings_go_over_the_apps() {
        let p = ModelPreset { instructions: "Answer in French.".into(), sampling: Sampling { temperature: Some(0.2), top_k: Some(20), ..Default::default() }, ..Default::default() };
        let mut extra = json!({ "temperature": 0.8, "max_tokens": 4096 });
        apply_sampling(&mut extra, &p);
        assert_eq!(extra, json!({ "temperature": 0.2, "top_k": 20, "max_tokens": 4096 }));
        let mut null = Value::Null;
        apply_sampling(&mut null, &ModelPreset::default());
        assert_eq!(null, Value::Null);
        assert!(with_instructions("Be kind.".into(), &p).ends_with("Answer in French."));
        assert_eq!(with_instructions("Be kind.".into(), &ModelPreset::default()), "Be kind.");
        assert_eq!(cache_type(Some("q4_0")), Some("q4_0"));
        assert_eq!(cache_type(Some("q3")), None);
        assert!(ModelPreset::default().is_empty());
    }

    #[test]
    fn settings_are_kept_per_model() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_for_test(&conn);
        let p = ModelPreset { kv_cache: Some("q4_0".into()), flash_attn: Some(true), sampling: Sampling { temperature: Some(0.0), ..Default::default() }, ..Default::default() };
        db::set(&conn, KEY, &HashMap::from([("local-1".to_string(), p.clone())])).unwrap();
        assert_eq!(get(&conn, "local-1"), p);
        assert!(get(&conn, "other").is_empty());
    }
}
