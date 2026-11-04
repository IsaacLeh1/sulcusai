// SPDX-License-Identifier: AGPL-3.0-only
//! Lip-sync for a dubbed video ("lipsync"): where someone speaks, the face is
//! moved again to the new voice (Ditto, from the first frame of each
//! stretch) and those stretches are put back in place; the sound stays.

use std::ffi::OsString;
use std::path::Path;

use crate::media::{ditto, edits, ffmpeg, imaging, proc, stems, store, videos, MediaItem, RunCtx};

use super::lipsync::{self, Piece};

/// The filter that puts the new stretches (inputs 1..) over the video (input 0).
pub fn splice_filter(pieces: &[Piece], len: f64, w: u32, h: u32) -> String {
    let mut f = String::new();
    let mut parts = Vec::new();
    let mut at = 0.0;
    let mut n = 0;
    for (k, p) in pieces.iter().enumerate() {
        if p.start - at > 0.04 {
            f.push_str(&format!("[0:v]trim=start={at:.3}:end={:.3},setpts=PTS-STARTPTS,fps=25,scale={w}:{h},setsar=1[o{n}];", p.start));
            parts.push(format!("[o{n}]"));
            n += 1;
        }
        let i = k + 1;
        f.push_str(&format!("[{i}:v]trim=duration={:.3},setpts=PTS-STARTPTS,fps=25,scale={w}:{h},setsar=1[s{k}];", p.end - p.start));
        parts.push(format!("[s{k}]"));
        at = p.end;
    }
    if len - at > 0.04 {
        f.push_str(&format!("[0:v]trim=start={at:.3},setpts=PTS-STARTPTS,fps=25,scale={w}:{h},setsar=1[o{n}];"));
        parts.push(format!("[o{n}]"));
    }
    f.push_str(&format!("{}concat=n={}:v=1:a=0[v]", parts.join(""), parts.len()));
    f
}

pub(in crate::media) async fn lipsync(ctx: &RunCtx<'_>, work: &Path) -> Result<Vec<MediaItem>, String> {
    let state = ctx.state;
    crate::consent::require(&state.db.lock().unwrap(), "likeness")?;
    let d = state.catalog.media.ditto.clone().ok_or("Lip-sync needs the quick talking pictures, which aren't available on this system yet.")?;
    let c = state.work_cipher()?;
    let id = ctx.req.source.clone().ok_or("Pick a dubbed video first.")?;
    let item = store::get(&state.db.lock().unwrap(), &c, &id).ok_or("That video is no longer in the gallery.")?;
    let (ffmpeg_exe, ffprobe) = ffmpeg::ensure(state, ctx.cancel, ctx.progress).await?;
    let video = edits::written_out(state, &c, &item, &work.join("source"))?;
    let probe = ffmpeg::probe(&ffprobe, &video).await?;
    if !probe.audio {
        return Err("This video has no sound to move the lips to.".into());
    }
    (ctx.progress)("Listening for speech", 0.03);
    let mix = edits::ffmpeg_stereo(state, &ffmpeg_exe, &video, probe.seconds, ctx.cancel).await?;
    let speech = crate::speech::endpoint(state, crate::speech::Use::Accurate).await.map_err(|e| format!("Lip-sync needs a speech model to find who speaks when. {e}"))?;
    let options = crate::speech::Options { language: Some("auto".into()), ..Default::default() };
    let heard = crate::speech::transcribe(&speech, &stems::resample(&mix.mono(), stems::RATE, crate::audio::RATE), &options).await?;
    let lines: Vec<lipsync::Line> = crate::speech::clean(heard).segments.into_iter().map(|s| lipsync::Line { start: s.start, end: s.end }).collect();
    let pieces = lipsync::pieces(&lines, probe.seconds, 0.2, 0.6, 20.0);
    if pieces.is_empty() {
        return Err("No one speaks in this video.".into());
    }
    let processor = !crate::natural::nvidia_here(state);
    let uv = videos::ensure_ditto(ctx, &d, processor).await?;
    let mono = mix.mono();
    let mut inputs: Vec<OsString> = vec!["-i".into(), video.as_os_str().to_owned()];
    for (k, p) in pieces.iter().enumerate() {
        let stage = format!("Moving the lips: part {} of {}", k + 1, pieces.len());
        (ctx.progress)(&stage, 0.1 + 0.8 * k as f64 / pieces.len() as f64);
        let frame = work.join(format!("face{k}.png"));
        let args: Vec<OsString> = vec!["-ss".into(), format!("{:.3}", p.start).into(), "-i".into(), video.as_os_str().to_owned(), "-frames:v".into(), "1".into(), frame.as_os_str().to_owned()];
        ffmpeg::run(state, &ffmpeg_exe, args, 0.0, ctx.cancel, &|_| {}).await?;
        let img = imaging::decode(&std::fs::read(&frame).map_err(|e| format!("FFmpeg didn't take the frame: {e}"))?)?;
        let (w, h) = ditto::picture_size(img.width(), img.height());
        if (w, h) != (img.width(), img.height()) {
            std::fs::write(&frame, imaging::png(&img.resize_exact(w, h, image::imageops::FilterType::Lanczos3))?).map_err(|e| e.to_string())?;
        }
        let r = stems::RATE as f64;
        let slice = &mono[((p.start * r) as usize).min(mono.len())..((p.end * r) as usize).min(mono.len())];
        let wav = work.join(format!("voice{k}.wav"));
        std::fs::write(&wav, crate::audio::wav_encode(slice, stems::RATE)).map_err(|e| e.to_string())?;
        let out = work.join(format!("moved{k}.mp4"));
        let mut no_face = false;
        let ran = proc::run_env(
            proc::Run {
                exe: &uv,
                args: ditto::args(&ditto::source_dir(&state.paths, &d), &ditto::models_dir(&state.paths), &frame, &wav, &out, ctx.req.steps.unwrap_or(50), processor),
                cwd: work,
                log: &state.paths.logs.join("lipsync.log"),
                low_priority: state.limits().low_priority,
                job: state.job(),
            },
            &ditto::env(&state.paths),
            ctx.cancel,
            |l| {
                if let Some(ditto::Said::NoFace) = ditto::said(l) {
                    no_face = true
                }
            },
        )
        .await;
        if no_face {
            return Err(format!("No face was seen at {:.0} s. Lip-sync works on videos where the speaker faces the camera.", p.start));
        }
        ran?;
        inputs.extend(["-i".into(), out.into_os_string()]);
    }
    (ctx.progress)("Putting the parts back", 0.92);
    let (w, h) = (probe.width.max(2) / 2 * 2, probe.height.max(2) / 2 * 2);
    let out = work.join("lipsynced.mp4");
    let mut args = inputs;
    args.extend(["-filter_complex".into(), splice_filter(&pieces, probe.seconds, w, h).into(), "-map".into(), "[v]".into(), "-map".into(), "0:a".into()]);
    args.extend(["-t".into(), format!("{:.3}", probe.seconds).into()]);
    args.extend(ffmpeg::mp4_out(&out));
    ffmpeg::run(state, &ffmpeg_exe, args, probe.seconds, ctx.cancel, &|f| (ctx.progress)("Putting the parts back", 0.92 + 0.07 * f)).await?;
    let bytes = std::fs::read(&out).map_err(|e| format!("FFmpeg didn't make the video: {e}"))?;
    let made = MediaItem {
        kind: "video".into(),
        op: "lipsync".into(),
        prompt: item.prompt.clone(),
        model_id: Some("ditto".into()),
        mime: "video/mp4".into(),
        seconds: probe.seconds,
        width: w,
        height: h,
        parent: Some(item.id.clone()),
        chat_id: ctx.req.chat_id.clone().or_else(|| item.chat_id.clone()),
        ..Default::default()
    };
    Ok(vec![store::save(&state.db.lock().unwrap(), &state.paths, &c, made, &bytes, None)?])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn splice_covers_the_whole_video() {
        let p = vec![Piece { start: 1.0, end: 2.0, lines: vec![0] }, Piece { start: 2.0, end: 3.5, lines: vec![1] }];
        let f = splice_filter(&p, 5.0, 640, 360);
        assert!(f.contains("[0:v]trim=start=0.000:end=1.000"));
        assert!(f.contains("[1:v]trim=duration=1.000") && f.contains("[2:v]trim=duration=1.500"));
        assert!(f.contains("[0:v]trim=start=3.500,"));
        assert!(f.ends_with("[o0][s0][s1][o1]concat=n=4:v=1:a=0[v]"), "{f}");
    }
}
