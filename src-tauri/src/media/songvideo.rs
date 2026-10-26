// SPDX-License-Identifier: AGPL-3.0-only
//! A song's video (Suno's Create video): its cover art over a blurred,
//! darkened copy of itself (else two colours from its name), its title, and
//! its lyrics a line at a time as they're sung (where the listener heard
//! them, else spread over the song), with the song as the sound: an H.264
//! and AAC MP4 through Windows' own encoders. Text is drawn by Windows (GDI).

use image::{GrayImage, Rgba, RgbaImage};

/// Frames a second: the words change a line at a time, so few are needed.
pub const FPS: u32 = 10;
pub const SIDE: u32 = 1080;

/// A lyric line and when it's sung, in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Timed {
    pub text: String,
    pub from: f64,
    pub to: f64,
}

/// The lines' times: where the listener heard each (`at`), and the others
/// spread through the gaps; with none heard, spread over the song from 8%
/// to 95% of it. Tags and blank lines aren't shown.
pub fn timings(lines: &[(String, Option<[f64; 2]>)], seconds: f64) -> Vec<Timed> {
    let n = lines.len();
    if n == 0 {
        return Vec::new();
    }
    let mut at: Vec<Option<[f64; 2]>> = lines.iter().map(|(_, a)| *a).collect();
    // Unheard lines share the time between the heard ones around them.
    let mut i = 0;
    while i < n {
        if at[i].is_some() {
            i += 1;
            continue;
        }
        let j = (i..n).find(|&k| at[k].is_some()).unwrap_or(n);
        let start = if i == 0 { seconds * 0.08 } else { at[i - 1].map_or(0.0, |a| a[1]) };
        let end = if j == n { seconds * 0.95 } else { at[j].map_or(seconds, |a| a[0]) };
        let step = ((end - start) / (j - i) as f64).max(0.5);
        for (k, slot) in at.iter_mut().enumerate().take(j).skip(i) {
            let s = start + step * (k - i) as f64;
            *slot = Some([s, s + step]);
        }
        i = j;
    }
    lines.iter().zip(at).map(|((text, _), a)| {
        let [from, to] = a.unwrap_or([0.0, 0.0]);
        Timed { text: text.clone(), from, to: to.max(from + 0.5) }
    }).collect()
}

/// The line shown at `t`, and the one after it: a line stays up until the
/// next one starts (or 2 s past its end, in a long gap).
pub fn showing(lines: &[Timed], t: f64) -> (Option<usize>, Option<usize>) {
    let now = lines.iter().rposition(|l| l.from <= t).filter(|&i| t <= lines[i].to + 2.0 || lines.get(i + 1).is_some_and(|n| n.from - t < 1.0));
    let next = match now {
        Some(i) => (i + 1 < lines.len()).then_some(i + 1),
        None => lines.iter().position(|l| l.from > t).filter(|&i| lines[i].from - t < 4.0),
    };
    (now, next)
}

/// Two colours from a name, for a song without cover art.
fn colours(name: &str) -> ([f32; 3], [f32; 3]) {
    let h = name.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    let hue = |x: u32| {
        let a = (x % 360) as f32 / 60.0;
        let f = |n: f32| {
            let k = (n + a) % 6.0;
            0.55 - 0.45 * k.min(4.0 - k).clamp(0.0, 1.0)
        };
        [f(5.0), f(3.0), f(1.0)]
    };
    (hue(h), hue(h >> 9).map(|c| c * 0.45))
}

/// The still part of every frame: the background, the art and the title.
pub fn backdrop(art: Option<&RgbaImage>, title: &str, title_mask: Option<&GrayImage>) -> RgbaImage {
    let mut frame = match art {
        Some(a) => {
            // Blurred by shrinking and growing again, then darkened.
            let small = image::imageops::resize(a, 54, 54, image::imageops::FilterType::Triangle);
            let mut bg = image::imageops::resize(&small, SIDE, SIDE, image::imageops::FilterType::Triangle);
            for p in bg.pixels_mut() {
                for c in 0..3 {
                    p[c] = (p[c] as f32 * 0.38) as u8;
                }
                p[3] = 255;
            }
            let box_side = 600;
            let big = image::imageops::resize(a, box_side, box_side, image::imageops::FilterType::Lanczos3);
            image::imageops::overlay(&mut bg, &big, ((SIDE - box_side) / 2) as i64, 70);
            bg
        }
        None => {
            let (top, bottom) = colours(title);
            RgbaImage::from_fn(SIDE, SIDE, |_, y| {
                let f = y as f32 / SIDE as f32;
                let c = |i: usize| ((top[i] * (1.0 - f) + bottom[i] * f) * 255.0) as u8;
                Rgba([c(0), c(1), c(2), 255])
            })
        }
    };
    if let Some(m) = title_mask {
        let y = if art.is_some() { 690 } else { 300 };
        stamp(&mut frame, m, (SIDE - m.width()) / 2, y, [255, 255, 255], 1.0);
    }
    frame
}

/// A text mask laid on the frame in a colour, at `alpha` (0 to 1).
pub fn stamp(frame: &mut RgbaImage, mask: &GrayImage, x: u32, y: u32, rgb: [u8; 3], alpha: f32) {
    for (mx, my, m) in mask.enumerate_pixels() {
        let (fx, fy) = (x + mx, y + my);
        if fx >= frame.width() || fy >= frame.height() || m[0] == 0 {
            continue;
        }
        let a = m[0] as f32 / 255.0 * alpha;
        let p = frame.get_pixel_mut(fx, fy);
        for c in 0..3 {
            p[c] = (p[c] as f32 * (1.0 - a) + rgb[c] as f32 * a) as u8;
        }
    }
}

/// `text` drawn by Windows in Segoe UI at `px` pixels high, wrapped within
/// `max_w` and centred: a mask (white is ink), cut to the ink's height.
#[cfg(windows)]
pub fn text_mask(text: &str, px: i32, bold: bool, max_w: u32) -> Result<GrayImage, String> {
    use windows::core::w;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, DeleteDC, DeleteObject, DrawTextW, GetDC, GetDIBits, PatBlt, ReleaseDC, SelectObject, SetBkMode, SetTextColor, ANTIALIASED_QUALITY, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, BLACKNESS, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DT_CALCRECT, DT_CENTER, DT_NOPREFIX, DT_WORDBREAK, FF_SWISS, FW_BOLD, FW_SEMIBOLD, OUT_DEFAULT_PRECIS,
        TRANSPARENT,
    };
    use windows::Win32::Foundation::COLORREF;
    let fail = |what: &str| format!("Couldn't draw the words ({what}).");
    unsafe {
        let screen = GetDC(HWND::default());
        let mem = CreateCompatibleDC(screen);
        let font = CreateFontW(
            -px,
            0,
            0,
            0,
            if bold { FW_BOLD.0 as i32 } else { FW_SEMIBOLD.0 as i32 },
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_DEFAULT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            ANTIALIASED_QUALITY.0 as u32,
            (DEFAULT_PITCH.0 | FF_SWISS.0) as u32,
            w!("Segoe UI"),
        );
        let old_font = SelectObject(mem, font);
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        let mut rect = RECT { left: 0, top: 0, right: max_w as i32, bottom: 0 };
        DrawTextW(mem, &mut wide, &mut rect, DT_CENTER | DT_WORDBREAK | DT_NOPREFIX | DT_CALCRECT);
        let (w, h) = (max_w as i32, (rect.bottom - rect.top).max(1) + px / 3);
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old_bmp = SelectObject(mem, bmp);
        let _ = PatBlt(mem, 0, 0, w, h, BLACKNESS);
        SetTextColor(mem, COLORREF(0x00FF_FFFF));
        SetBkMode(mem, TRANSPARENT);
        let mut area = RECT { left: 0, top: 0, right: w, bottom: h };
        DrawTextW(mem, &mut wide, &mut area, DT_CENTER | DT_WORDBREAK | DT_NOPREFIX);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
            ..Default::default()
        };
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let lines = GetDIBits(mem, bmp, 0, h as u32, Some(buf.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
        SelectObject(mem, old_bmp);
        SelectObject(mem, old_font);
        let _ = DeleteObject(bmp);
        let _ = DeleteObject(font);
        let _ = DeleteDC(mem);
        ReleaseDC(HWND::default(), screen);
        if lines == 0 {
            return Err(fail("read"));
        }
        let ink: Vec<u8> = buf.chunks_exact(4).map(|p| p[0].max(p[1]).max(p[2])).collect();
        GrayImage::from_raw(w as u32, h as u32, ink).ok_or_else(|| fail("size"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_timed_from_what_was_heard() {
        let l = |s: &str, at: Option<[f64; 2]>| (s.to_string(), at);
        let t = timings(&[l("a", None), l("b", Some([20.0, 24.0])), l("c", None), l("d", None), l("e", Some([40.0, 44.0]))], 100.0);
        assert_eq!((t[0].from, t[1].from, t[4].from), (8.0, 20.0, 40.0));
        assert_eq!((t[2].from, t[3].from), (24.0, 32.0), "unheard lines share the gap");
        let even = timings(&[l("a", None), l("b", None)], 100.0);
        assert_eq!((even[0].from, even[1].from), (8.0, 51.5));
        assert!(timings(&[], 10.0).is_empty());
    }

    #[test]
    fn the_line_being_sung_shows_with_the_next() {
        let t = vec![Timed { text: "a".into(), from: 10.0, to: 14.0 }, Timed { text: "b".into(), from: 15.0, to: 18.0 }, Timed { text: "c".into(), from: 40.0, to: 44.0 }];
        assert_eq!(showing(&t, 2.0), (None, None), "nothing yet in the intro");
        assert_eq!(showing(&t, 8.0), (None, Some(0)), "the first line is coming");
        assert_eq!(showing(&t, 11.0), (Some(0), Some(1)));
        assert_eq!(showing(&t, 25.0), (None, None), "a long gap is left clear");
        assert_eq!(showing(&t, 41.0), (Some(2), None));
    }

    #[cfg(windows)]
    #[test]
    fn words_are_drawn_by_windows() {
        let m = text_mask("Summer Nights", 48, true, 900).unwrap();
        assert_eq!(m.width(), 900);
        assert!(m.height() >= 48 && m.height() < 120, "one line: {}", m.height());
        let ink = m.pixels().filter(|p| p[0] > 128).count();
        assert!(ink > 2000, "ink drawn: {ink}");
        let two = text_mask("A long line of words that has to wrap onto a second line here", 48, false, 500).unwrap();
        assert!(two.height() > m.height() + 30, "wrapped: {}", two.height());
    }
}
