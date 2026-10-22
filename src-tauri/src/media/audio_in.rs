// SPDX-License-Identifier: AGPL-3.0-only
//! A song the person brings, made ready for the music engine: it reads WAV
//! and MP3 only, so other kinds (M4A, AAC, FLAC, WMA) are decoded by Windows
//! (Media Foundation) into a WAV first. Also its length, for picking a part.

use std::path::{Path, PathBuf};

/// Writes the song into `work` as a file the music engine reads, and says
/// how long it is (seconds; 0 when it couldn't tell).
pub fn for_engine(bytes: &[u8], mime: &str, work: &Path) -> Result<(PathBuf, f64), String> {
    let ext = match mime {
        "audio/mpeg" => "mp3",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        _ => "",
    };
    if !ext.is_empty() {
        let path = work.join(format!("source.{ext}"));
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        let secs = if ext == "wav" { crate::audio::wav_decode(bytes).map(|p| p.samples.len() as f64 / p.rate.max(1) as f64).unwrap_or(0.0) } else { length(&path).unwrap_or(0.0) };
        return Ok((path, secs));
    }
    decode_to_wav(bytes, mime, work)
}

#[cfg(windows)]
fn decode_to_wav(bytes: &[u8], mime: &str, work: &Path) -> Result<(PathBuf, f64), String> {
    let ext = match mime {
        "audio/mp4" | "audio/x-m4a" | "audio/m4a" => "m4a",
        "audio/aac" => "aac",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/x-ms-wma" => "wma",
        "audio/ogg" => "ogg",
        _ => "bin",
    };
    let src = work.join(format!("source-original.{ext}"));
    std::fs::write(&src, bytes).map_err(|e| e.to_string())?;
    let (samples, rate, channels) = decode(&src).map_err(|e| format!("Windows couldn't read that sound file ({e}). Try an MP3 or WAV."))?;
    let out = work.join("source.wav");
    std::fs::write(&out, wav(&samples, rate, channels)).map_err(|e| e.to_string())?;
    let secs = samples.len() as f64 / (rate.max(1) as f64 * channels.max(1) as f64);
    Ok((out, secs))
}

#[cfg(not(windows))]
fn decode_to_wav(_bytes: &[u8], _mime: &str, _work: &Path) -> Result<(PathBuf, f64), String> {
    Err("Only MP3 and WAV songs can be worked on here.".into())
}

/// How long a sound file is, in seconds.
#[cfg(windows)]
pub fn length(path: &Path) -> Option<f64> {
    use windows::core::HSTRING;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).ok()?;
        let r = (|| {
            let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None).ok()?;
            let v = reader.GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION).ok()?;
            // 100-nanosecond units.
            let units = u64::try_from(&v).ok()?;
            Some(units as f64 / 10_000_000.0)
        })();
        let _ = MFShutdown();
        r
    }
}

#[cfg(not(windows))]
pub fn length(_path: &Path) -> Option<f64> {
    None
}

/// The file's samples as 32-bit floats, interleaved: (samples, rate, channels).
#[cfg(windows)]
pub(crate) fn decode(path: &Path) -> Result<(Vec<f32>, u32, u16), String> {
    use windows::core::HSTRING;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    let fail = |e: windows::core::Error| e.message().to_string();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).map_err(fail)?;
        let result = (|| -> Result<(Vec<f32>, u32, u16), String> {
            let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None).map_err(fail)?;
            let stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
            let want = MFCreateMediaType().map_err(fail)?;
            want.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(fail)?;
            want.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float).map_err(fail)?;
            reader.SetCurrentMediaType(stream, None, &want).map_err(fail)?;
            let got = reader.GetCurrentMediaType(stream).map_err(fail)?;
            let rate = got.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).map_err(fail)?;
            let channels = got.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).map_err(fail)?.clamp(1, 8) as u16;
            let mut out = Vec::new();
            loop {
                let mut flags = 0u32;
                let mut sample: Option<IMFSample> = None;
                reader.ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample)).map_err(fail)?;
                if let Some(s) = sample {
                    let buf = s.ConvertToContiguousBuffer().map_err(fail)?;
                    let (mut p, mut len) = (std::ptr::null_mut::<u8>(), 0u32);
                    buf.Lock(&mut p, None, Some(&mut len)).map_err(fail)?;
                    out.extend_from_slice(std::slice::from_raw_parts(p as *const f32, len as usize / 4));
                    buf.Unlock().map_err(fail)?;
                }
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                // Ten minutes of stereo at 48 kHz is the most the engine is given.
                if out.len() > 48_000 * 2 * 660 {
                    break;
                }
            }
            Ok((out, rate, channels))
        })();
        let _ = MFShutdown();
        result
    }
}

#[cfg(not(windows))]
pub(crate) fn decode(_path: &Path) -> Result<(Vec<f32>, u32, u16), String> {
    Err("Sound files are read by Windows here.".into())
}

/// 16-bit PCM WAV of interleaved float samples.
fn wav(samples: &[f32], rate: u32, channels: u16) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&channels.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    b.extend_from_slice(&(channels * 2).to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&crate::audio::to_i16(*s).to_le_bytes());
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WAV goes to the engine as it is, with its length read from it.
    #[test]
    fn a_wav_is_used_as_it_is() {
        let dir = std::env::temp_dir().join(format!("sulcusai-audio-in-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let tone: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let bytes = crate::audio::wav_encode(&tone, 24_000);
        let (path, secs) = for_engine(&bytes, "audio/wav", &dir).unwrap();
        assert!(path.ends_with("source.wav") && (secs - 2.0).abs() < 0.01, "{secs}");
        let stereo = wav(&[0.0, 0.5, -0.5, 1.0], 48_000, 2);
        assert_eq!(&stereo[22..24], &2u16.to_le_bytes());
        assert_eq!(stereo.len(), 44 + 8);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
