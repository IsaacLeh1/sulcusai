// SPDX-License-Identifier: AGPL-3.0-only
//! A picture LoRA trained on the person's own photos (a person, a pet, a
//! thing, or a style), for RealVisXL, in the music trainer's environment
//! (`picture_train.py`). The photos are prepared here; RealVisXL V5 as
//! trained and SDXL's settings files are fetched once; the LoRA joins
//! Studio's LoRAs and is used with RealVisXL V5 Lightning.

use super::*;

/// What a picture LoRA is trained on.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PictureTrainSpec {
    pub name: String,
    /// "person", "thing" or "style".
    pub kind: String,
    /// Gallery ids of the photos.
    pub pictures: Vec<String>,
}

/// The picture model LoRAs are trained for.
const BASE: &str = "realvisxl-v5-lightning";
pub(super) const FEWEST: usize = 5;
pub(super) const MOST: usize = 40;

/// SDXL's settings and word lists (stabilityai/stable-diffusion-xl-base-1.0
/// at this revision), which the trainer needs beside the one-file model.
const CONFIG_REV: &str = "462165984030d82259a11f4367a4eed129e94a7b";
const CONFIG_FILES: &[(&str, u64)] = &[
    ("model_index.json", 609),
    ("scheduler/scheduler_config.json", 479),
    ("text_encoder/config.json", 565),
    ("text_encoder_2/config.json", 575),
    ("tokenizer/merges.txt", 524_619),
    ("tokenizer/special_tokens_map.json", 472),
    ("tokenizer/tokenizer_config.json", 737),
    ("tokenizer/vocab.json", 1_059_962),
    ("tokenizer_2/merges.txt", 524_619),
    ("tokenizer_2/special_tokens_map.json", 460),
    ("tokenizer_2/tokenizer_config.json", 725),
    ("tokenizer_2/vocab.json", 1_059_962),
    ("unet/config.json", 1680),
    ("vae/config.json", 642),
];

/// The made-up word the LoRA learns, from its name: one the picture model
/// knows nothing about, so it only means what the photos show.
pub(super) fn token_of(name: &str) -> String {
    let base: String = name.chars().filter(|c| c.is_ascii_alphabetic()).take(6).collect::<String>().to_lowercase();
    format!("zq{base}")
}

/// The words a LoRA is called up with, and the caption its photos get.
pub(super) fn trigger_for(kind: &str, token: &str) -> String {
    match kind {
        "style" => format!("in {token} style"),
        "thing" => format!("{token} object"),
        _ => format!("{token} person"),
    }
}

fn caption(kind: &str, trigger: &str) -> String {
    match kind {
        "style" => format!("an image {trigger}"),
        _ => format!("a photo of {trigger}"),
    }
}

/// A photo's size for training: about a million pixels (SDXL's own size),
/// both sides in steps of 64.
pub(super) fn train_size(w: u32, h: u32) -> (u32, u32) {
    let k = (1024.0 * 1024.0 / (w as f64 * h as f64)).sqrt();
    let side = |v: u32| (((v as f64 * k) / 64.0).round() as u32).clamp(8, 32) * 64;
    (side(w), side(h))
}

/// Steps for a set of photos: about a hundred a photo (a style a little more).
pub(super) fn steps_for(kind: &str, photos: usize) -> u32 {
    let per = if kind == "style" { 120 } else { 100 };
    (photos as u32 * per).clamp(800, 2000)
}

async fn ensure_config(state: &AppState, cancel: &AtomicBool) -> Result<PathBuf, String> {
    let dir = state.paths.models.join("media").join("helpers").join("sdxl-config");
    let missing: Vec<&(&str, u64)> = CONFIG_FILES.iter().filter(|(f, size)| !dir.join(f).metadata().is_ok_and(|m| m.len() == *size)).collect();
    if missing.is_empty() {
        return Ok(dir);
    }
    let client = crate::net::external_client(state.settings().connectivity, crate::net::Purpose::ModelDownload, false)?;
    state.log("network", "Downloading SDXL's settings files (3 MB) from huggingface.co for picture training");
    for (file, size) in missing {
        if cancel.load(Ordering::Relaxed) {
            return Err(download::CANCELLED.into());
        }
        let url = format!("https://huggingface.co/stabilityai/stable-diffusion-xl-base-1.0/resolve/{CONFIG_REV}/{file}");
        let bytes = client.get(&url).send().await.and_then(|r| r.error_for_status()).map_err(|e| format!("Couldn't get SDXL's settings ({e})."))?.bytes().await.map_err(|e| e.to_string())?;
        if bytes.len() as u64 != *size {
            return Err(format!("SDXL's {file} came back different from expected; try again later."));
        }
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    }
    Ok(dir)
}

pub(super) async fn picture_train_job(ctx: &RunCtx<'_>, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let state = ctx.state;
    let spec = ctx.req.picture_train.clone().ok_or("Pick your photos first.")?;
    let t = state.catalog.media.trainer.clone().ok_or("Training isn't available on this system yet.")?;
    if !installed(&state.db.lock().unwrap()).iter().any(|i| i.model_id == BASE) {
        return Err("Picture LoRAs are trained for RealVisXL V5: install it from Models (pictures) first.".into());
    }
    let started = Instant::now();
    ensure_trainer(ctx, &t).await?;
    let learn_on = state.catalog.media.picture_trainer_base.clone().ok_or("Picture training isn't available on this system yet.")?;
    let model_file = fetch_helper(state, &learn_on, &format!("Getting RealVisXL V5 to train on (once, {})", crate::size_label(learn_on.file.size)), ctx.cancel, ctx.progress).await?;
    (ctx.progress)("Getting SDXL's settings (once)", 0.04);
    let config = ensure_config(state, ctx.cancel).await?;
    let c = state.work_cipher()?;
    let token = token_of(&spec.name);
    let trigger = trigger_for(&spec.kind, &token);
    let data = work.join("data");
    std::fs::create_dir_all(&data).map_err(|e| e.to_string())?;
    (ctx.progress)("Getting your photos ready", 0.06);
    for (i, id) in spec.pictures.iter().enumerate() {
        let img = imaging::decode(&store::read(&state.paths, &c, id)?)?;
        let (w, h) = train_size(img.width(), img.height());
        let fitted = img.resize_to_fill(w, h, image::imageops::FilterType::Lanczos3);
        std::fs::write(data.join(format!("{i:02}.png")), imaging::png(&fitted)?).map_err(|e| e.to_string())?;
        std::fs::write(data.join(format!("{i:02}.txt")), caption(&spec.kind, &trigger)).map_err(|e| e.to_string())?;
    }
    let script = work.join("picture_train.py");
    std::fs::write(&script, include_str!("picture_train.py")).map_err(|e| e.to_string())?;
    let out = work.join("lora.safetensors");
    // (Tests train briefly.)
    let steps = cfg!(test).then(|| std::env::var("SULCUSAI_TRAIN_STEPS").ok()).flatten().and_then(|s| s.parse().ok()).unwrap_or_else(|| steps_for(&spec.kind, spec.pictures.len()));
    let python = train::source_dir(&state.paths, &t).join(".venv").join("Scripts").join("python.exe");
    let args: Vec<std::ffi::OsString> = vec![
        script.into_os_string(),
        "--base".into(),
        model_file.into_os_string(),
        "--config".into(),
        config.into_os_string(),
        "--data".into(),
        data.into_os_string(),
        "--out".into(),
        out.clone().into_os_string(),
        "--steps".into(),
        steps.to_string().into(),
    ];
    let log = state.paths.logs.join("picture-trainer.log");
    proc::run_env(
        proc::Run { exe: &python, args, cwd: work, log: &log, low_priority: state.limits().low_priority, job: state.job() },
        &train::trainer_env(&state.paths),
        ctx.cancel,
        |l| {
            let mut it = l.split_whitespace();
            if let (Some("SULCUS"), Some("step"), Some(done), Some(of)) = (it.next(), it.next(), it.next(), it.next()) {
                if let (Ok(done), Ok(of)) = (done.parse::<f64>(), of.parse::<f64>()) {
                    (ctx.progress)(&format!("Learning from your photos: step {done} of {of}"), 0.1 + 0.88 * done / of.max(1.0));
                }
            }
        },
    )
    .await?;
    if !out.is_file() {
        return Err("The trainer didn't write the LoRA. See picture-trainer.log in Settings › Logs.".into());
    }
    let name: String = spec.name.trim().chars().take(60).collect();
    let lora = loras::add_trained(state, &out, &name, "sdxl", &trigger)?;
    let minutes = started.elapsed().as_secs_f64() / 60.0;
    crate::applog::line("media", &format!("Trained a picture LoRA on {} photos ({steps} steps) in {minutes:.0} min", spec.pictures.len()));
    emit(state, "media:lora_trained", json!(lora));
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn photos_are_sized_and_named_for_training() {
        assert_eq!(train_size(4032, 3024), (1152, 896));
        assert_eq!(train_size(1024, 1024), (1024, 1024));
        assert_eq!(train_size(300, 300), (1024, 1024));
        let (w, h) = train_size(1080, 1920);
        assert!(w % 64 == 0 && h % 64 == 0 && h > w);
        assert_eq!(token_of("Max (my dog)"), "zqmaxmyd");
        assert_eq!(trigger_for("person", "zqsam"), "zqsam person");
        assert_eq!(trigger_for("style", "zqink"), "in zqink style");
        assert_eq!(steps_for("person", 12), 1200);
        assert_eq!(steps_for("person", 3), 800);
        assert_eq!(steps_for("style", 30), 2000);
    }
}
