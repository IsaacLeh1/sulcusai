// SPDX-License-Identifier: AGPL-3.0-only
//! H.264 MP4 files through Windows Media Foundation (the graphics card's
//! encoder when it has one). Frames go in as NV12, converted here, so no
//! color converter is needed in between.

use std::path::Path;

use image::RgbaImage;
use windows::core::HSTRING;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

pub struct Writer {
    sink: IMFSinkWriter,
    stream: u32,
    w: u32,
    h: u32,
    duration: i64,
    n: i64,
    finished: bool,
}

/// Picture quality: enough bits per pixel for slow, detailed scenes.
fn bitrate(w: u32, h: u32, fps: u32) -> u32 {
    ((w as f64 * h as f64 * fps as f64 * 0.11) as u32).clamp(4_000_000, 40_000_000)
}

impl Writer {
    pub fn create(path: &Path, w: u32, h: u32, fps: u32) -> Result<Self, String> {
        if w % 2 != 0 || h % 2 != 0 || w == 0 || h == 0 {
            return Err("Video sides must be even.".into());
        }
        let fail = |what: &str, e: windows::core::Error| format!("Windows couldn't {what}: {e}");
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).map_err(|e| fail("start its video encoder", e))?;
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 2).map_err(|e| fail("set up the encoder", e))?;
            let attrs = attrs.ok_or("Windows couldn't set up the encoder.")?;
            attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1).map_err(|e| fail("set up the encoder", e))?;
            let _ = std::fs::remove_file(path);
            let sink = MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, &attrs).map_err(|e| fail("create the video file", e))?;

            let size = ((w as u64) << 32) | h as u64;
            let rate = ((fps as u64) << 32) | 1;
            let out = MFCreateMediaType().map_err(|e| fail("set up the encoder", e))?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| fail("set up H.264", e))?;
            out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).map_err(|e| fail("set up H.264", e))?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate(w, h, fps)).map_err(|e| fail("set up H.264", e))?;
            out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(|e| fail("set up H.264", e))?;
            out.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32).map_err(|e| fail("set up H.264", e))?;
            out.SetUINT64(&MF_MT_FRAME_SIZE, size).map_err(|e| fail("set up H.264", e))?;
            out.SetUINT64(&MF_MT_FRAME_RATE, rate).map_err(|e| fail("set up H.264", e))?;
            out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1).map_err(|e| fail("set up H.264", e))?;
            let stream = sink.AddStream(&out).map_err(|e| fail("add the video stream", e))?;

            let inp = MFCreateMediaType().map_err(|e| fail("set up the encoder", e))?;
            inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| fail("set up the frames", e))?;
            inp.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).map_err(|e| fail("set up the frames", e))?;
            inp.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(|e| fail("set up the frames", e))?;
            inp.SetUINT32(&MF_MT_DEFAULT_STRIDE, w).map_err(|e| fail("set up the frames", e))?;
            inp.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32).map_err(|e| fail("set up the frames", e))?;
            inp.SetUINT64(&MF_MT_FRAME_SIZE, size).map_err(|e| fail("set up the frames", e))?;
            inp.SetUINT64(&MF_MT_FRAME_RATE, rate).map_err(|e| fail("set up the frames", e))?;
            inp.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1).map_err(|e| fail("set up the frames", e))?;
            sink.SetInputMediaType(stream, &inp, None).map_err(|e| fail("take frames of this size", e))?;
            sink.BeginWriting().map_err(|e| fail("start writing the video", e))?;
            Ok(Self { sink, stream, w, h, duration: 10_000_000 / fps.max(1) as i64, n: 0, finished: false })
        }
    }

    pub fn frame(&mut self, img: &RgbaImage) -> Result<(), String> {
        if img.dimensions() != (self.w, self.h) {
            return Err("A frame had the wrong size.".into());
        }
        let nv12 = nv12(img);
        unsafe {
            let buf = MFCreateMemoryBuffer(nv12.len() as u32).map_err(|e| e.to_string())?;
            let mut p: *mut u8 = std::ptr::null_mut();
            buf.Lock(&mut p, None, None).map_err(|e| e.to_string())?;
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), p, nv12.len());
            buf.Unlock().map_err(|e| e.to_string())?;
            buf.SetCurrentLength(nv12.len() as u32).map_err(|e| e.to_string())?;
            let s = MFCreateSample().map_err(|e| e.to_string())?;
            s.AddBuffer(&buf).map_err(|e| e.to_string())?;
            s.SetSampleTime(self.n * self.duration).map_err(|e| e.to_string())?;
            s.SetSampleDuration(self.duration).map_err(|e| e.to_string())?;
            self.sink.WriteSample(self.stream, &s).map_err(|e| format!("Windows couldn't encode a frame: {e}"))?;
        }
        self.n += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Result<(), String> {
        self.finished = true;
        unsafe {
            let r = self.sink.Finalize().map_err(|e| format!("Windows couldn't finish the video: {e}"));
            let _ = MFShutdown();
            r
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        if !self.finished {
            unsafe {
                let _ = MFShutdown();
            }
        }
    }
}

/// RGBA to NV12 (BT.709, video range): a full-size Y plane, then U and V
/// interleaved at half size. Pure black stays pure black (Y 16).
pub fn nv12(img: &RgbaImage) -> Vec<u8> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut out = vec![0u8; w * h * 3 / 2];
    let px = img.as_raw();
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let (r, g, b) = (px[i] as f32, px[i + 1] as f32, px[i + 2] as f32);
            out[y * w + x] = (16.0 + (0.2126 * r + 0.7152 * g + 0.0722 * b) * 219.0 / 255.0).round() as u8;
        }
    }
    let uv = &mut out[w * h..];
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let (mut r, mut g, mut b) = (0.0f32, 0.0f32, 0.0f32);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = ((y + dy) * w + x + dx) * 4;
                r += px[i] as f32;
                g += px[i + 1] as f32;
                b += px[i + 2] as f32;
            }
            let (r, g, b) = (r / 4.0, g / 4.0, b / 4.0);
            let u = 128.0 + (-0.1146 * r - 0.3854 * g + 0.5 * b) * 224.0 / 255.0;
            let v = 128.0 + (0.5 * r - 0.4542 * g - 0.0458 * b) * 224.0 / 255.0;
            let o = (y / 2) * w + x;
            uv[o] = u.round().clamp(16.0, 240.0) as u8;
            uv[o + 1] = v.round().clamp(16.0, 240.0) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_and_white_land_on_the_video_range() {
        let img = RgbaImage::from_fn(4, 2, |x, _| if x < 2 { image::Rgba([0, 0, 0, 255]) } else { image::Rgba([255, 255, 255, 255]) });
        let v = nv12(&img);
        assert_eq!(v.len(), 12);
        assert_eq!(v[0], 16, "black");
        assert_eq!(v[3], 235, "white");
        assert_eq!(&v[8..12], &[128, 128, 128, 128], "no color in gray");
    }

    #[test]
    fn writes_a_playable_mp4() {
        let dir = std::env::temp_dir().join(format!("sulcus-mp4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.mp4");
        let mut w = Writer::create(&path, 320, 176, 24).unwrap();
        for i in 0..24u32 {
            let f = RgbaImage::from_fn(320, 176, |x, y| image::Rgba([((x + i * 8) % 256) as u8, (y % 256) as u8, 90, 255]));
            w.frame(&f).unwrap();
        }
        w.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() > 1000, "{} bytes", bytes.len());
        assert_eq!(&bytes[4..8], b"ftyp", "an MP4 container");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
