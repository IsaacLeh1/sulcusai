// SPDX-License-Identifier: AGPL-3.0-only
//! stable-diffusion.cpp (`sd-cli`): the argument list for each kind of job,
//! and its log turned into progress.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

use super::Defaults;

/// The model's files, by role, as `sd-cli` flags.
pub fn model_args(files: &[(String, PathBuf)]) -> Vec<OsString> {
    let mut out = Vec::new();
    for (role, path) in files {
        let flag = match role.as_str() {
            "diffusion" => "--diffusion-model",
            // Wan 2.2 A14B's first stage (the low-noise one is "diffusion").
            "high_noise" => "--high-noise-diffusion-model",
            "llm" => "--llm",
            "t5xxl" => "--t5xxl",
            "clip_l" => "--clip_l",
            "vae" => "--vae",
            "llm_vision" => "--llm_vision",
            "model" => "-m",
            "upscale" => "--upscale-model",
            // Wan 2.2 S2V hears its voice through wav2vec2.
            "audio_encoder" => "--audio-encoder",
            // LTX-2.3: its sound's decoder (with the vocoder in it), and the
            // projection from its text reader (Gemma 3) to the model.
            "audio_vae" => "--audio-vae",
            "embeddings_connectors" => "--embeddings-connectors",
            _ => continue,
        };
        out.push(flag.into());
        out.push(path.into());
    }
    out
}

/// Processor threads and graphics memory, within the performance limits.
#[derive(Debug, Clone, Copy, Default)]
pub struct Limits {
    pub threads: usize,
    /// GiB of graphics memory the engine may plan for (None: what's free).
    pub max_vram_gib: Option<f64>,
}

fn limit_args(l: &Limits) -> Vec<OsString> {
    let mut out: Vec<OsString> = Vec::new();
    if l.threads > 0 {
        out.extend(["-t".into(), l.threads.to_string().into()]);
    }
    if let Some(g) = l.max_vram_gib {
        out.extend(["--max-vram".into(), format!("{:.1}", g.max(1.0)).into()]);
    }
    out
}

pub struct Picture<'a> {
    pub prompt: &'a str,
    pub negative: Option<&'a str>,
    pub width: u32,
    pub height: u32,
    pub steps: u32,
    pub seed: i64,
    pub count: u32,
    /// Starting picture (filling in, extending).
    pub init: Option<&'a Path>,
    /// White = repaint (with `init`).
    pub mask: Option<&'a Path>,
    /// Reference pictures (edits from an instruction).
    pub refs: Vec<&'a Path>,
    pub strength: Option<f32>,
    /// `%d` is replaced by the picture's number.
    pub out: &'a Path,
}

pub fn picture_args(files: &[(String, PathBuf)], d: &Defaults, p: &Picture<'_>, l: &Limits) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["-M".into(), "img_gen".into()];
    // The picture reader (for models that read pictures) only when editing.
    let files: Vec<(String, PathBuf)> = files.iter().filter(|(role, _)| role != "llm_vision" || !p.refs.is_empty()).cloned().collect();
    a.extend(model_args(&files));
    a.extend(["-p".into(), p.prompt.into()]);
    if let Some(n) = p.negative.filter(|n| !n.trim().is_empty()) {
        a.extend(["-n".into(), n.into()]);
    }
    let sampler = if d.sampler.is_empty() { "euler" } else { d.sampler.as_str() };
    a.extend([
        "--cfg-scale".into(),
        format!("{}", if d.cfg > 0.0 { d.cfg } else { 1.0 }).into(),
        "--steps".into(),
        p.steps.to_string().into(),
        "--sampling-method".into(),
        sampler.into(),
        "-W".into(),
        p.width.to_string().into(),
        "-H".into(),
        p.height.to_string().into(),
        "-s".into(),
        p.seed.to_string().into(),
        "--diffusion-fa".into(),
        // The prompt stays out of the file, so a shared picture doesn't carry it.
        "--disable-image-metadata".into(),
    ]);
    if !d.scheduler.is_empty() {
        a.extend(["--scheduler".into(), d.scheduler.as_str().into()]);
    }
    if !d.sigmas.is_empty() {
        a.extend(["--sigmas".into(), d.sigmas.as_str().into()]);
    }
    if !d.model_args.is_empty() {
        a.extend(["--model-args".into(), d.model_args.as_str().into()]);
    }
    if p.count > 1 {
        a.extend(["-b".into(), p.count.to_string().into()]);
    }
    if let Some(i) = p.init {
        a.extend(["-i".into(), i.into()]);
    }
    if let Some(m) = p.mask {
        a.extend(["--mask".into(), m.into()]);
    }
    for r in &p.refs {
        a.extend(["-r".into(), (*r).into()]);
    }
    if let Some(s) = p.strength {
        a.extend(["--strength".into(), format!("{s:.2}").into()]);
    }
    a.extend(limit_args(l));
    a.extend(["-o".into(), p.out.into()]);
    a
}

pub struct Clip<'a> {
    pub prompt: &'a str,
    /// The prompt writer's negative; else the model's own.
    pub negative: Option<&'a str>,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub fps: u32,
    pub steps: u32,
    pub seed: i64,
    /// Picture to bring to life (image-to-video).
    pub init: Option<&'a Path>,
    /// The voice it moves its lips to (Wan 2.2 S2V), a WAV file.
    pub audio: Option<&'a Path>,
    pub out: &'a Path,
}

/// Wan models need 4n+1 frames.
pub fn frames_for(seconds: f32, fps: u32) -> u32 {
    frames_in_steps(seconds, fps, 4)
}

/// Frames in steps of `step`, plus one (LTX: 8n+1).
pub fn frames_in_steps(seconds: f32, fps: u32, step: u32) -> u32 {
    let n = ((seconds * fps as f32) / step as f32).round().max(1.0) as u32;
    n * step + 1
}

/// A video model that makes its sound too (LTX-2.3's sound decoder).
pub fn with_sound(files: &[(String, PathBuf)]) -> bool {
    files.iter().any(|(role, _)| role == "audio_vae")
}

pub fn clip_args(files: &[(String, PathBuf)], d: &Defaults, c: &Clip<'_>, l: &Limits) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["-M".into(), "vid_gen".into()];
    a.extend(model_args(files));
    // A LoRA that comes with the model (S2V's 4-step one), by its file's name.
    let mut prompt = c.prompt.to_string();
    if let Some((_, lora)) = files.iter().find(|(role, _)| role == "lora") {
        if let (Some(dir), Some(name)) = (lora.parent(), lora.file_stem()) {
            a.extend(["--lora-model-dir".into(), dir.into()]);
            prompt.push_str(&format!(" <lora:{}:1>", name.to_string_lossy()));
        }
    }
    a.extend(["-p".into(), prompt.into()]);
    if let Some(n) = c.negative.filter(|n| !n.trim().is_empty()).or((!d.negative.is_empty()).then_some(d.negative.as_str())) {
        a.extend(["-n".into(), n.into()]);
    }
    a.extend([
        "--cfg-scale".into(),
        format!("{}", if d.cfg > 0.0 { d.cfg } else { 5.0 }).into(),
        "--sampling-method".into(),
        if d.sampler.is_empty() { "euler".into() } else { d.sampler.as_str().into() },
        "--steps".into(),
        c.steps.to_string().into(),
        "-W".into(),
        c.width.to_string().into(),
        "-H".into(),
        c.height.to_string().into(),
        "--video-frames".into(),
        c.frames.to_string().into(),
        "--fps".into(),
        c.fps.to_string().into(),
        "-s".into(),
        c.seed.to_string().into(),
        "--diffusion-fa".into(),
        // Video decoders are memory-hungry; tiles keep them within 8-12 GB.
        "--vae-tiling".into(),
        // Tiles overlapping by a quarter instead of half, and convolutions
        // done directly: Wan 2.2 at 1280×704 decoded 3.4× faster on a 12 GB
        // card (225 s to 66 s for 49 frames) with 3 GB less of it used, and
        // 2.5× faster at 832×480 (47 s to 19 s for 25 frames). The frames
        // differ from the old way by at most 4 of 255 levels: no visible seams.
        "--vae-tile-overlap".into(),
        "0.25".into(),
        "--vae-conv-direct".into(),
    ]);
    if d.flow_shift > 0.0 {
        a.extend(["--flow-shift".into(), format!("{}", d.flow_shift).into()]);
    }
    // Two stages (Wan 2.2 A14B): the high-noise expert takes the first steps
    // with the same guidance and sampler, 8 of them to the low-noise 10 in
    // stable-diffusion.cpp's own example.
    if files.iter().any(|(role, _)| role == "high_noise") {
        a.extend([
            "--high-noise-cfg-scale".into(),
            format!("{}", if d.cfg > 0.0 { d.cfg } else { 3.5 }).into(),
            "--high-noise-sampling-method".into(),
            if d.sampler.is_empty() { "euler".into() } else { d.sampler.as_str().into() },
            "--high-noise-steps".into(),
            ((c.steps * 4 / 5).max(2)).to_string().into(),
        ]);
    }
    if !d.cache.is_empty() {
        a.extend(["--cache-mode".into(), d.cache.as_str().into()]);
    }
    if d.temporal_tiling {
        a.push("--temporal-tiling".into());
    }
    // Distilled models are tuned to exact noise levels (LTX-2.3: 8 steps).
    if !d.sigmas.is_empty() {
        a.extend(["--sigmas".into(), d.sigmas.as_str().into()]);
    }
    if let Some(i) = c.init {
        a.extend(["-i".into(), i.into()]);
        // LTX keeps the first frame by this much (its default, 0.75, let the picture drift).
        if with_sound(files) {
            a.extend(["--strength".into(), "1.0".into()]);
        }
    }
    if let Some(v) = c.audio {
        a.extend(["--audio".into(), v.into()]);
    }
    a.extend(limit_args(l));
    a.extend(["-o".into(), c.out.into()]);
    a
}

pub fn upscale_args(files: &[(String, PathBuf)], input: &Path, out: &Path, l: &Limits) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["-M".into(), "upscale".into()];
    a.extend(model_args(files));
    a.extend(["-i".into(), input.into()]);
    // Bigger tiles keep a graphics card busier (the default is 128).
    a.extend(["--upscale-tile-size".into(), "256".into()]);
    a.extend(limit_args(l));
    a.extend(["-o".into(), out.into()]);
    a
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stage {
    #[default]
    Loading,
    Creating,
    Finishing,
    Upscaling,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Stage::Loading => "Loading the model",
            Stage::Creating => "Creating",
            Stage::Finishing => "Finishing",
            Stage::Upscaling => "Upscaling",
        }
    }
}

/// Follows `sd-cli`'s log: which stage it's in and how far along.
#[derive(Debug, Default)]
pub struct Tracker {
    pub stage: Stage,
    done: u64,
    total: u64,
    /// Pictures in a batch: which one is being made.
    picture: u64,
    pictures: u64,
}

fn bar() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\|([=#> ]*)\|\s*(\d+)/(\d+)").unwrap())
}

fn of() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(\d+)/(\d+)").unwrap())
}

impl Tracker {
    /// Reads one line; true when the progress changed.
    pub fn line(&mut self, l: &str) -> bool {
        if let Some(c) = bar().captures(l) {
            // `#` bars count bytes while loading weights.
            if c[1].contains('#') {
                return false;
            }
            self.done = c[2].parse().unwrap_or(0);
            self.total = c[3].parse().unwrap_or(0);
            return true;
        }
        if !l.contains("[I]") {
            return false;
        }
        let low = l.to_lowercase();
        let before = self.stage;
        if low.contains("generating image") || low.contains("generate_video") || low.contains("sampling using") {
            self.stage = Stage::Creating;
            if let Some(c) = of().captures(&low.replace("generating image:", "")) {
                self.picture = c[1].parse().unwrap_or(1);
                self.pictures = c[2].parse().unwrap_or(1);
            }
        } else if low.contains("decoding") || low.contains("decode_first_stage") {
            self.stage = Stage::Finishing;
        } else if low.contains("upscaling") || (low.contains("upscale") && !low.contains("upscaled")) {
            self.stage = Stage::Upscaling;
        }
        if self.stage != before {
            self.done = 0;
            self.total = 0;
            return true;
        }
        false
    }

    /// How much of the whole job is done, 0 to 1.
    pub fn fraction(&self) -> f64 {
        let part = if self.total > 0 { self.done as f64 / self.total as f64 } else { 0.0 };
        match self.stage {
            Stage::Loading => 0.03,
            Stage::Creating => {
                let n = self.pictures.max(1) as f64;
                let i = self.picture.saturating_sub(1) as f64;
                0.08 + 0.8 * ((i + part) / n).min(1.0)
            }
            Stage::Finishing => 0.88 + 0.12 * part,
            Stage::Upscaling => 0.05 + 0.95 * part,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_a_real_log() {
        let mut t = Tracker::default();
        let log = [
            "[I] loading diffusion model from 'x.gguf' --- diffusion_engine.cpp:733",
            "  |##########     | 250/298 - 1.75GB/s",
            "[I] get_learned_condition completed, taking 1.18s --- image.cpp:529",
            "[I] generating image: 1/1 - seed 3 --- image.cpp:866",
            "  |============>                                     | 1/4 - 1.20s/it",
            "  |==================================================| 4/4 - 1.10s/it",
            "[I] decoding 1 latents --- image.cpp:554",
            "  |=====>   | 12/49 - 13.16it/s",
        ];
        let mut seen = Vec::new();
        for l in log {
            t.line(l);
            seen.push((t.stage, (t.fraction() * 100.0).round() as u32));
        }
        assert_eq!(seen[1], (Stage::Loading, 3), "weight loading bars don't count as steps");
        assert_eq!(seen[3].0, Stage::Creating);
        assert_eq!(seen[4].1, 28);
        assert_eq!(seen[5].1, 88);
        assert_eq!(seen[7].0, Stage::Finishing);
        assert!(seen[7].1 > 88 && seen[7].1 < 100);
    }

    #[test]
    fn batches_count_each_picture() {
        let mut t = Tracker::default();
        t.line("[I] generating image: 2/2 - seed 4 --- image.cpp:866");
        t.line("  |====>   | 0/4 - 1.0s/it");
        assert!((t.fraction() - 0.48).abs() < 0.01);
    }

    #[test]
    fn wan_frames_are_4n_plus_1() {
        assert_eq!(frames_for(3.0, 24), 73);
        assert_eq!(frames_for(2.0, 16), 33);
        assert_eq!(frames_for(0.1, 16), 5);
        assert_eq!(frames_in_steps(5.0, 24, 8), 121, "LTX: 8n+1");
    }

    /// LTX-2.3: its sound decoder and text projection, its exact noise
    /// levels, and a picture it starts from kept fully.
    #[test]
    fn video_with_sound_passes_its_parts() {
        let files = vec![
            ("diffusion".to_string(), PathBuf::from("ltx.gguf")),
            ("llm".to_string(), PathBuf::from("gemma.gguf")),
            ("embeddings_connectors".to_string(), PathBuf::from("conn.st")),
            ("vae".to_string(), PathBuf::from("v.st")),
            ("audio_vae".to_string(), PathBuf::from("a.st")),
        ];
        let d = Defaults { steps: 8, cfg: 1.0, sampler: "euler".into(), sigmas: "1.0,0.5,0.0".into(), temporal_tiling: true, ..Default::default() };
        let (start, out) = (PathBuf::from("start.png"), PathBuf::from("out.webm"));
        let c = Clip { prompt: "a crowd cheers", negative: None, width: 768, height: 512, frames: 121, fps: 24, steps: 8, seed: 1, init: Some(&start), audio: None, out: &out };
        let joined = clip_args(&files, &d, &c, &Limits::default()).iter().map(|s| s.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ");
        assert!(with_sound(&files));
        assert!(joined.contains("--embeddings-connectors conn.st") && joined.contains("--audio-vae a.st") && joined.contains("--llm gemma.gguf"), "{joined}");
        assert!(joined.contains("--sigmas 1.0,0.5,0.0") && joined.contains("-i start.png --strength 1.0") && joined.contains("--diffusion-fa"), "{joined}");
        assert!(!joined.contains("--audio "), "LTX makes its sound; it isn't given one");
    }

    #[test]
    fn picture_args_carry_the_privacy_flag_and_limits() {
        let files = vec![("diffusion".to_string(), PathBuf::from("d.gguf")), ("llm".to_string(), PathBuf::from("q.gguf")), ("vae".to_string(), PathBuf::from("v.st"))];
        let d = Defaults { steps: 4, cfg: 1.0, sampler: "euler".into(), width: 1024, height: 1024, ..Default::default() };
        let out = PathBuf::from("out_%d.png");
        let p = Picture { prompt: "a fox", negative: None, width: 768, height: 512, steps: 4, seed: 9, count: 2, init: None, mask: None, refs: vec![], strength: None, out: &out };
        let a: Vec<String> = picture_args(&files, &d, &p, &Limits { threads: 6, max_vram_gib: Some(5.5) }).iter().map(|s| s.to_string_lossy().into_owned()).collect();
        let joined = a.join(" ");
        assert!(joined.starts_with("-M img_gen --diffusion-model d.gguf --llm q.gguf --vae v.st -p a fox"));
        assert!(joined.contains("--disable-image-metadata") && joined.contains("-b 2") && joined.contains("-t 6") && joined.contains("--max-vram 5.5"));
        assert!(joined.ends_with("-o out_%d.png"));
    }

    /// Wan 2.2 A14B: both experts, and the high-noise one's own settings,
    /// as in stable-diffusion.cpp's example (10 steps and 8).
    #[test]
    fn two_stage_video_passes_both_experts() {
        let files = vec![
            ("diffusion".to_string(), PathBuf::from("low.gguf")),
            ("high_noise".to_string(), PathBuf::from("high.gguf")),
            ("t5xxl".to_string(), PathBuf::from("umt5.gguf")),
            ("vae".to_string(), PathBuf::from("vae.st")),
        ];
        let d = Defaults { steps: 10, cfg: 3.5, sampler: "euler".into(), flow_shift: 3.0, ..Default::default() };
        let out = PathBuf::from("out.webm");
        let c = Clip { prompt: "a fox", negative: None, width: 832, height: 480, frames: 49, fps: 16, steps: 10, seed: 1, init: None, audio: None, out: &out };
        let joined = clip_args(&files, &d, &c, &Limits::default()).iter().map(|s| s.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ");
        assert!(joined.contains("--diffusion-model low.gguf --high-noise-diffusion-model high.gguf"), "{joined}");
        assert!(joined.contains("--steps 10") && joined.contains("--high-noise-cfg-scale 3.5 --high-noise-sampling-method euler --high-noise-steps 8"), "{joined}");
        let one = clip_args(&files[..1], &d, &c, &Limits::default()).iter().map(|s| s.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ");
        assert!(!one.contains("high-noise"), "one stage: no high-noise settings");
    }

    #[test]
    fn clips_decode_in_lightly_overlapping_tiles() {
        let d = Defaults { steps: 25, cfg: 5.0, temporal_tiling: true, ..Default::default() };
        let out = PathBuf::from("out.webm");
        let c = Clip { prompt: "a fox", negative: None, width: 1280, height: 704, frames: 121, fps: 24, steps: 25, seed: 1, init: None, audio: None, out: &out };
        let joined = clip_args(&[], &d, &c, &Limits::default()).iter().map(|s| s.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ");
        assert!(joined.contains("-W 1280 -H 704 --video-frames 121"));
        assert!(joined.contains("--vae-tiling --vae-tile-overlap 0.25 --vae-conv-direct") && joined.contains("--temporal-tiling"));
    }
}
