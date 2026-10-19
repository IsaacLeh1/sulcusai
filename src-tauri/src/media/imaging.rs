// SPDX-License-Identifier: AGPL-3.0-only
//! Picture helpers: sizes the models accept, masks, extending a canvas,
//! putting an edited area back into the full-size original, thumbnails.

use std::io::Cursor;

use image::imageops::{self, FilterType};
use image::{DynamicImage, GenericImageView, GrayImage, ImageFormat, Luma, Rgba, RgbaImage};

/// Image models work in steps of 16 pixels.
pub fn round16(x: f64) -> u32 {
    ((x / 16.0).round() as u32).max(1) * 16
}

/// Width and height for a shape at about `side`² pixels.
pub fn shape_size(shape: &str, side: u32) -> (u32, u32) {
    let (w, h) = match shape {
        "portrait" => (3.0, 4.0),
        "landscape" => (4.0, 3.0),
        "wide" => (16.0, 9.0),
        "tall" => (9.0, 16.0),
        _ => (1.0, 1.0),
    };
    let k = (side as f64 * side as f64 / (w * h)).sqrt();
    (round16(w * k), round16(h * k))
}

/// The size to work on an existing picture at: same shape, at most
/// `max_area` pixels, in steps of 16.
pub fn work_size(w: u32, h: u32, max_area: u32) -> (u32, u32) {
    let scale = (max_area as f64 / (w as f64 * h as f64)).sqrt().min(1.0);
    (round16(w as f64 * scale), round16(h as f64 * scale))
}

pub fn decode(bytes: &[u8]) -> Result<DynamicImage, String> {
    image::load_from_memory(bytes).map_err(|e| format!("That picture couldn't be read: {e}"))
}

pub fn png(img: &DynamicImage) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out)
}

fn has_alpha(img: &DynamicImage) -> bool {
    img.color().has_alpha() && img.to_rgba8().pixels().any(|p| p[3] < 255)
}

/// A small preview for the gallery: JPEG, or PNG when it has transparency.
pub fn thumbnail(img: &DynamicImage) -> Result<Vec<u8>, String> {
    let t = img.thumbnail(384, 384);
    let mut out = Vec::new();
    if has_alpha(&t) {
        t.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).map_err(|e| e.to_string())?;
    } else {
        let rgb = DynamicImage::ImageRgb8(t.to_rgb8());
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 82);
        enc.encode_image(&rgb).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// A mask painted in the window (white or any opaque stroke = change it),
/// as black and white at `w`×`h`.
pub fn mask_from_paint(bytes: &[u8], w: u32, h: u32) -> Result<GrayImage, String> {
    let painted = decode(bytes)?.to_rgba8();
    let mut m = GrayImage::from_fn(painted.width(), painted.height(), |x, y| {
        let p = painted.get_pixel(x, y);
        let on = p[3] > 16 && (p[0] as u32 + p[1] as u32 + p[2] as u32) > 96;
        Luma([if on { 255 } else { 0 }])
    });
    if m.dimensions() != (w, h) {
        m = imageops::resize(&m, w, h, FilterType::Triangle);
        m.pixels_mut().for_each(|p| p[0] = if p[0] > 96 { 255 } else { 0 });
    }
    if !m.pixels().any(|p| p[0] > 0) {
        return Err("Paint over the part to change first.".into());
    }
    Ok(m)
}

/// Grows the white of a mask by `r` pixels (so new content blends in).
pub fn grow(mask: &GrayImage, r: u32) -> GrayImage {
    let (w, h) = mask.dimensions();
    let r = r as i64;
    GrayImage::from_fn(w, h, |x, y| {
        for dy in -r..=r {
            for dx in -r..=r {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                if nx >= 0 && ny >= 0 && nx < w as i64 && ny < h as i64 && mask.get_pixel(nx as u32, ny as u32)[0] > 127 {
                    return Luma([255]);
                }
            }
        }
        Luma([0])
    })
}

/// Shrinks the white of a mask by `r` pixels: white stays only where all of
/// its neighbours within `r` are white.
pub fn shrink(mask: &GrayImage, r: u32) -> GrayImage {
    let (w, h) = mask.dimensions();
    let r = r as i64;
    GrayImage::from_fn(w, h, |x, y| {
        for dy in -r..=r {
            for dx in -r..=r {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 || mask.get_pixel(nx as u32, ny as u32)[0] <= 127 {
                    return Luma([0]);
                }
            }
        }
        Luma([255])
    })
}

/// A bigger canvas with the picture inside and a mask over the new area.
/// The new area starts as the stretched edge colors, which gives the model
/// the right palette to continue from.
pub fn extend(img: &DynamicImage, left: u32, top: u32, right: u32, bottom: u32) -> (RgbaImage, GrayImage) {
    let src = img.to_rgba8();
    let (w, h) = src.dimensions();
    let (nw, nh) = (w + left + right, h + top + bottom);
    let canvas = RgbaImage::from_fn(nw, nh, |x, y| {
        let sx = (x as i64 - left as i64).clamp(0, w as i64 - 1) as u32;
        let sy = (y as i64 - top as i64).clamp(0, h as i64 - 1) as u32;
        let p = *src.get_pixel(sx, sy);
        Rgba([p[0], p[1], p[2], 255])
    });
    // A little overlap into the old picture hides the seam.
    let overlap = 8u32;
    let mask = GrayImage::from_fn(nw, nh, |x, y| {
        let inside = x >= left + overlap.min(left) && x < left + w - overlap.min(right) && y >= top + overlap.min(top) && y < top + h - overlap.min(bottom);
        Luma([if inside { 0 } else { 255 }])
    });
    (canvas, mask)
}

/// Puts the changed area back into the full-size original: the result is
/// scaled to the original's size and blended in through a softened mask,
/// so untouched pixels stay exactly as they were.
pub fn composite(original: &DynamicImage, result: &DynamicImage, mask: &GrayImage) -> DynamicImage {
    let (w, h) = original.dimensions();
    let result = result.resize_exact(w, h, FilterType::Lanczos3).to_rgba8();
    let soft = imageops::blur(&imageops::resize(&grow(mask, 2), w, h, FilterType::Triangle), 3.0);
    let base = original.to_rgba8();
    DynamicImage::ImageRgba8(RgbaImage::from_fn(w, h, |x, y| {
        let a = soft.get_pixel(x, y)[0] as f32 / 255.0;
        let (o, n) = (base.get_pixel(x, y), result.get_pixel(x, y));
        let mix = |i: usize| (o[i] as f32 * (1.0 - a) + n[i] as f32 * a).round() as u8;
        Rgba([mix(0), mix(1), mix(2), o[3]])
    }))
}

/// How different two pictures look with `b` read at (x + dx, y + dy) for
/// `a`'s (x, y): the mean difference over the pixels both have (every
/// `step`th one). None when they overlap too little to tell.
fn mismatch(a: &GrayImage, b: &GrayImage, (dx, dy): (i32, i32), step: u32) -> Option<f64> {
    let (aw, ah) = (a.width() as i64, a.height() as i64);
    let (bw, bh) = (b.width() as i64, b.height() as i64);
    let (x0, y0) = ((-dx as i64).max(0), (-dy as i64).max(0));
    let (x1, y1) = ((bw - dx as i64).min(aw), (bh - dy as i64).min(ah));
    if x1 - x0 < 8 || y1 - y0 < 8 || (x1 - x0) * (y1 - y0) * 10 < aw * ah * 6 {
        return None;
    }
    let (mut sum, mut n) = (0u64, 0u64);
    for y in (y0..y1).step_by(step as usize) {
        for x in (x0..x1).step_by(step as usize) {
            let p = a.get_pixel(x as u32, y as u32)[0] as i32;
            let q = b.get_pixel((x + dx as i64) as u32, (y + dy as i64) as u32)[0] as i32;
            sum += (q - p).unsigned_abs() as u64;
            n += 1;
        }
    }
    Some(sum as f64 / n.max(1) as f64)
}

/// The offset within `range` pixels of `around` where `b` looks most like
/// `a` (see `mismatch`), and how different they are there.
fn best_shift(a: &GrayImage, b: &GrayImage, range: i32, step: u32, around: (i32, i32)) -> ((i32, i32), f64) {
    let mut best = (around, f64::MAX);
    for dy in -range..=range {
        for dx in -range..=range {
            let at = (around.0 + dx, around.1 + dy);
            let Some(m) = mismatch(a, b, at, step) else { continue };
            // Ties go to the smaller move.
            let nearer = (dx.abs() + dy.abs()) < ((best.0 .0 - around.0).abs() + (best.0 .1 - around.1).abs());
            if m < best.1 - 1e-9 || ((m - best.1).abs() <= 1e-9 && nearer) {
                best = (at, m);
            }
        }
    }
    best
}

/// `after` lined up with `before` (both the same size): edit models move
/// everything a few pixels, and sometimes zoom in or out a little. The
/// zoom and shift are found roughly at a quarter size, then the shift
/// exactly. Returns `after` drawn onto `before`'s frame, and where it has
/// pixels there (white): zoomed in, its edges are gone.
fn line_up(before: &GrayImage, after: &RgbaImage) -> (RgbaImage, GrayImage) {
    let (w, h) = before.dimensions();
    let gray = |i: &RgbaImage| DynamicImage::ImageRgba8(i.clone()).to_luma8();
    let (qw, qh) = ((w / 4).max(16), (h / 4).max(16));
    let ea = edges(&imageops::resize(before, qw, qh, FilterType::Triangle));
    let small = imageops::resize(&gray(after), qw, qh, FilterType::Triangle);
    // Every zoom from 90% to 110% at a quarter size…
    let mut rough: Vec<(f64, (i32, i32), f64)> = (-5..=5)
        .map(|k| {
            let s = 1.0 + k as f64 * 0.02;
            let (sw, sh) = (((qw as f64 * s).round() as u32).max(8), ((qh as f64 * s).round() as u32).max(8));
            let eb = edges(&imageops::resize(&small, sw, sh, FilterType::Triangle));
            let middle = ((sw as i32 - qw as i32) / 2, (sh as i32 - qh as i32) / 2);
            let (at, m) = best_shift(&ea, &eb, 6, 2, middle);
            (s, at, m)
        })
        .collect();
    rough.sort_by(|a, b| a.2.total_cmp(&b.2));
    // …then the best three, and no zoom, exactly. No zoom wins unless a zoom is clearly better.
    let mut tries: Vec<(f64, (i32, i32))> = rough.iter().take(3).map(|r| (r.0, r.1)).collect();
    if !tries.iter().any(|t| t.0 == 1.0) {
        tries.push(rough.iter().find(|r| r.0 == 1.0).map_or((1.0, (0, 0)), |r| (r.0, r.1)));
    }
    let eb_full = edges(before);
    let mut best: Option<(f64, RgbaImage, (i32, i32), f64)> = None;
    for (s, at) in tries {
        let big = if s == 1.0 { after.clone() } else { imageops::resize(after, ((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1), FilterType::Lanczos3) };
        let (shift, m) = best_shift(&eb_full, &edges(&gray(&big)), 4, 2, (at.0 * 4, at.1 * 4));
        let better = match &best {
            None => true,
            Some(b) if s == 1.0 => m <= b.3 / 0.97,
            Some(b) if b.0 == 1.0 => m < b.3 * 0.97,
            Some(b) => m < b.3,
        };
        if better {
            best = Some((s, big, shift, m));
        }
    }
    let Some((_, big, (dx, dy), _)) = best else { return (after.clone(), GrayImage::from_pixel(w, h, Luma([255]))) };
    let (bw, bh) = (big.width() as i32, big.height() as i32);
    let inside = |x: u32, y: u32| {
        let (sx, sy) = (x as i32 + dx, y as i32 + dy);
        sx >= 0 && sy >= 0 && sx < bw && sy < bh
    };
    let aligned = RgbaImage::from_fn(w, h, |x, y| *big.get_pixel((x as i32 + dx).clamp(0, bw - 1) as u32, (y as i32 + dy).clamp(0, bh - 1) as u32));
    let has = GrayImage::from_fn(w, h, |x, y| Luma([if inside(x, y) { 255 } else { 0 }]));
    (aligned, has)
}

/// How strongly the brightness changes at each pixel. Pictures are lined up
/// by their edges: an edit that makes the whole picture a little brighter,
/// or adds something big, would throw off a comparison of the pixels
/// themselves.
fn edges(g: &GrayImage) -> GrayImage {
    let (w, h) = g.dimensions();
    let at = |x: i64, y: i64| g.get_pixel(x.clamp(0, w as i64 - 1) as u32, y.clamp(0, h as i64 - 1) as u32)[0] as i32;
    GrayImage::from_fn(w, h, |x, y| {
        let (x, y) = (x as i64, y as i64);
        let d = (at(x + 1, y) - at(x - 1, y)).abs() + (at(x, y + 1) - at(x, y - 1)).abs();
        Luma([d.min(255) as u8])
    })
}

/// `img` moved so its pixel (x + dx, y + dy) lands at (x, y), edges repeated
/// (the tests move an edit with it).
#[cfg(test)]
fn shifted(img: &RgbaImage, (dx, dy): (i32, i32)) -> RgbaImage {
    let (w, h) = img.dimensions();
    RgbaImage::from_fn(w, h, |x, y| {
        let sx = (x as i32 + dx).clamp(0, w as i32 - 1) as u32;
        let sy = (y as i32 + dy).clamp(0, h as i32 - 1) as u32;
        *img.get_pixel(sx, sy)
    })
}

/// Where an edit changed the picture: `after` compared with `before` (the
/// picture the edit model was given, at the same size), lined up first (the
/// model can move and zoom everything a little) and with any overall shift
/// in tone taken out. Small differences in texture are ignored; a changed area
/// gets a margin. Returns the lined-up result, the mask (white = changed)
/// and the share of the picture that changed.
pub fn changed_area(before: &DynamicImage, after: &DynamicImage) -> (RgbaImage, GrayImage, f32) {
    let (w, h) = before.dimensions();
    let after = if after.dimensions() == (w, h) { after.to_rgba8() } else { after.resize_exact(w, h, FilterType::Lanczos3).to_rgba8() };
    let (after, has) = line_up(&before.to_luma8(), &after);
    let a = imageops::blur(&before.to_rgba8(), 2.0);
    let b = imageops::blur(&after, 2.0);
    // The overall shift in tone: the middle difference of each color.
    let mut offset = [0i32; 3];
    for (c, o) in offset.iter_mut().enumerate() {
        let mut d: Vec<i32> = a.pixels().zip(b.pixels()).step_by(7).map(|(p, q)| q[c] as i32 - p[c] as i32).collect();
        if !d.is_empty() {
            let mid = d.len() / 2;
            *o = *d.select_nth_unstable(mid).1;
        }
    }
    // Blocks of 8 pixels where a third or more of the pixels changed clearly.
    const BLOCK: u32 = 8;
    const CLEAR: i32 = 30;
    let (bw, bh) = (w.div_ceil(BLOCK), h.div_ceil(BLOCK));
    let blocks = GrayImage::from_fn(bw, bh, |bx, by| {
        let (mut changed, mut n) = (0u32, 0u32);
        for y in by * BLOCK..((by + 1) * BLOCK).min(h) {
            for x in bx * BLOCK..((bx + 1) * BLOCK).min(w) {
                n += 1;
                // Where the edit has no pixels (it zoomed in), the original stays.
                if has.get_pixel(x, y)[0] == 0 {
                    continue;
                }
                let (p, q) = (a.get_pixel(x, y), b.get_pixel(x, y));
                let d = (0..3).map(|c| (q[c] as i32 - p[c] as i32 - offset[c]).abs()).max().unwrap_or(0);
                if d > CLEAR {
                    changed += 1;
                }
            }
        }
        Luma([if changed * 3 >= n.max(1) { 255 } else { 0 }])
    });
    // Scattered small changes (the model redrawing a floor's texture) are
    // dropped: pasted in, they showed as patches. What's left gets a margin.
    let blocks = grow(&shrink(&blocks, 1), 3);
    let share = blocks.pixels().filter(|p| p[0] > 127).count() as f32 / (bw * bh).max(1) as f32;
    let mask = imageops::resize(&blocks, w, h, FilterType::Triangle);
    (after, mask, share)
}

/// An edit put back into the full-size original: only what the edit changed
/// comes from it, so the rest of the picture keeps its own pixels and size
/// ("keep everything else exactly as it is"). None when the edit changed
/// most of the picture (a new look, night instead of day): then the edit is
/// the picture.
pub fn keep_unchanged(original: &DynamicImage, before: &DynamicImage, after: &DynamicImage) -> Option<DynamicImage> {
    let (aligned, mask, share) = changed_area(before, after);
    if share > 0.6 {
        return None;
    }
    // Blended over a wide, soft edge (about 4 of the edit's pixels), so no seam shows.
    let (w, h) = original.dimensions();
    let result = DynamicImage::ImageRgba8(aligned).resize_exact(w, h, FilterType::Lanczos3).to_rgba8();
    let soft = imageops::fast_blur(&imageops::resize(&mask, w, h, FilterType::Triangle), (4.0 * w as f32 / before.width().max(1) as f32).max(2.0));
    let base = original.to_rgba8();
    Some(DynamicImage::ImageRgba8(RgbaImage::from_fn(w, h, |x, y| {
        let a = soft.get_pixel(x, y)[0] as f32 / 255.0;
        let (o, n) = (base.get_pixel(x, y), result.get_pixel(x, y));
        let mix = |i: usize| (o[i] as f32 * (1.0 - a) + n[i] as f32 * a).round() as u8;
        Rgba([mix(0), mix(1), mix(2), o[3]])
    })))
}

/// The picture scaled to cover `w`×`h` and cut to it from the middle, the way
/// Windows fills the desktop with a wallpaper.
pub fn fill_to(img: &DynamicImage, w: u32, h: u32) -> DynamicImage {
    let (iw, ih) = img.dimensions();
    if (iw, ih) == (w, h) {
        return img.clone();
    }
    let k = (w as f64 / iw.max(1) as f64).max(h as f64 / ih.max(1) as f64);
    let (sw, sh) = (((iw as f64 * k).ceil() as u32).max(w), ((ih as f64 * k).ceil() as u32).max(h));
    img.resize_exact(sw, sh, FilterType::Lanczos3).crop_imm((sw - w) / 2, (sh - h) / 2, w, h)
}

/// The picture on a solid background where it's see-through (unchanged
/// when it has no transparency).
pub fn flatten(img: &DynamicImage, rgb: [u8; 3]) -> DynamicImage {
    if !has_alpha(img) {
        return img.clone();
    }
    let src = img.to_rgba8();
    DynamicImage::ImageRgb8(image::RgbImage::from_fn(src.width(), src.height(), |x, y| {
        let p = src.get_pixel(x, y);
        let a = p[3] as f32 / 255.0;
        image::Rgb([0, 1, 2].map(|i| (p[i] as f32 * a + rgb[i] as f32 * (1.0 - a)).round() as u8))
    }))
}

/// `result` with `original`'s transparency, scaled to its size (an upscaled
/// cut-out). Unchanged when the original has none.
pub fn keep_alpha(original: &DynamicImage, result: DynamicImage) -> DynamicImage {
    if !has_alpha(original) {
        return result;
    }
    let (w, h) = result.dimensions();
    let alpha = imageops::resize(&original.to_rgba8(), w, h, FilterType::Lanczos3);
    let mut out = result.to_rgba8();
    for (p, a) in out.pixels_mut().zip(alpha.pixels()) {
        p[3] = a[3];
    }
    DynamicImage::ImageRgba8(out)
}

/// Keeps the longest side within `max` pixels.
pub fn cap(img: DynamicImage, max: u32) -> DynamicImage {
    let (w, h) = img.dimensions();
    if w.max(h) <= max {
        return img;
    }
    img.resize(max, max, FilterType::Lanczos3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_steps_of_16_near_the_target() {
        assert_eq!(shape_size("square", 1024), (1024, 1024));
        let (w, h) = shape_size("landscape", 1024);
        assert!(w % 16 == 0 && h % 16 == 0 && w > h);
        assert!(((w * h) as f64 / (1024.0 * 1024.0) - 1.0).abs() < 0.05);
        assert_eq!(work_size(4000, 3000, 1024 * 1024), (1184, 880));
        assert_eq!(work_size(640, 480, 1024 * 1024), (640, 480), "never enlarged");
    }

    #[test]
    fn fill_covers_and_cuts_from_the_middle() {
        // A wide picture for a square: the middle part, at the exact size.
        let mut img = RgbaImage::from_pixel(300, 100, Rgba([0, 0, 0, 255]));
        for y in 0..100 {
            for x in 100..200 {
                img.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        let out = fill_to(&DynamicImage::ImageRgba8(img), 50, 50);
        assert_eq!(out.dimensions(), (50, 50));
        assert!(out.to_rgba8().pixels().all(|p| p[0] > 200), "only the white middle is left");
        // A small picture for a big screen: scaled up to cover it.
        assert_eq!(fill_to(&DynamicImage::new_rgb8(160, 90), 3440, 1440).dimensions(), (3440, 1440));
    }

    #[test]
    fn cut_outs_stay_see_through_and_get_a_white_background_for_models() {
        let mut cut = RgbaImage::from_pixel(8, 8, Rgba([0, 0, 0, 0]));
        cut.put_pixel(4, 4, Rgba([200, 30, 30, 255]));
        let cut = DynamicImage::ImageRgba8(cut);
        let flat = flatten(&cut, [255, 255, 255]).to_rgb8();
        assert_eq!(flat.get_pixel(0, 0).0, [255, 255, 255], "see-through becomes white, not black");
        assert_eq!(flat.get_pixel(4, 4).0, [200, 30, 30]);
        let big = keep_alpha(&cut, DynamicImage::new_rgb8(32, 32)).to_rgba8();
        assert_eq!(big.dimensions(), (32, 32));
        assert_eq!(big.get_pixel(0, 0)[3], 0, "still see-through after enlarging");
        let photo = DynamicImage::new_rgb8(4, 4);
        assert!(!flatten(&photo, [255, 255, 255]).color().has_alpha());
    }

    #[test]
    fn extend_masks_only_the_new_area() {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(64, 32, Rgba([200, 10, 10, 255])));
        let (canvas, mask) = extend(&img, 32, 0, 32, 0);
        assert_eq!(canvas.dimensions(), (128, 32));
        assert_eq!(mask.get_pixel(0, 16)[0], 255);
        assert_eq!(mask.get_pixel(64, 16)[0], 0, "the middle of the old picture stays");
        assert_eq!(canvas.get_pixel(0, 16)[0], 200, "edge colors carried out");
    }

    #[test]
    fn composite_keeps_unmasked_pixels() {
        let orig = DynamicImage::ImageRgba8(RgbaImage::from_pixel(100, 100, Rgba([0, 0, 255, 255])));
        let result = DynamicImage::ImageRgba8(RgbaImage::from_pixel(64, 64, Rgba([255, 0, 0, 255])));
        let mut mask = GrayImage::new(64, 64);
        for y in 0..20 {
            for x in 0..20 {
                mask.put_pixel(x, y, Luma([255]));
            }
        }
        let out = composite(&orig, &result, &mask).to_rgba8();
        assert_eq!(out.get_pixel(5, 5)[0], 255, "painted corner changed");
        assert_eq!(*out.get_pixel(90, 90), Rgba([0, 0, 255, 255]), "far corner untouched");
    }

    /// A detailed full-size picture, the smaller copy the edit model got, and
    /// an edit of it: a red box, a warmer tone overall and everything moved
    /// 2 pixels right and 1 down (edit models do both).
    fn an_edit() -> (DynamicImage, DynamicImage, DynamicImage) {
        let full = DynamicImage::ImageRgba8(RgbaImage::from_fn(400, 300, |x, y| Rgba([(x % 200) as u8, (y % 256) as u8, ((x * 3 + y * 5) % 256) as u8, 255])));
        let before = full.resize_exact(200, 150, FilterType::Lanczos3);
        let mut after = before.to_rgba8();
        for p in after.pixels_mut() {
            p[0] = p[0].saturating_add(6);
        }
        for y in 40..80 {
            for x in 100..150 {
                after.put_pixel(x, y, Rgba([230, 20, 20, 255]));
            }
        }
        let after = shifted(&after, (-2, -1));
        (full, before, DynamicImage::ImageRgba8(after))
    }

    #[test]
    fn only_what_an_edit_changed_is_found() {
        let (_, before, after) = an_edit();
        let (aligned, mask, share) = changed_area(&before, &after);
        assert!(share > 0.05 && share < 0.35, "the box and its margin, not the whole picture: {share}");
        assert!(mask.get_pixel(125, 60)[0] > 200, "the box changed");
        assert_eq!(mask.get_pixel(20, 130)[0], 0, "the far corner didn't");
        assert_eq!(aligned.get_pixel(125, 60).0, [230, 20, 20, 255], "the edit is lined up again");
    }

    #[test]
    fn a_slightly_zoomed_edit_is_lined_up_too() {
        // The edit model zoomed in 6%: the middle of the picture, enlarged.
        let (full, before, _) = an_edit();
        let (w, h) = before.dimensions();
        let (cw, ch) = ((w as f64 / 1.06).round() as u32, (h as f64 / 1.06).round() as u32);
        let zoomed = before.crop_imm((w - cw) / 2, (h - ch) / 2, cw, ch).resize_exact(w, h, FilterType::Lanczos3);
        let (_, _, share) = changed_area(&before, &zoomed);
        assert!(share < 0.3, "lined up, so little looks changed: {share}");
        let out = keep_unchanged(&full, &before, &zoomed).expect("put back into the original").to_rgba8();
        assert_eq!(out.dimensions(), (400, 300));
        assert_eq!(out.get_pixel(4, 4), full.to_rgba8().get_pixel(4, 4), "the zoomed-away edge is the original's");
    }

    #[test]
    fn an_edit_keeps_the_original_where_nothing_changed() {
        let (full, before, after) = an_edit();
        let out = keep_unchanged(&full, &before, &after).expect("a small change is put back into the original").to_rgba8();
        assert_eq!(out.dimensions(), (400, 300), "the original's size, not the edit model's");
        let original = full.to_rgba8();
        for (x, y) in [(40, 260), (380, 20), (10, 10), (300, 280)] {
            assert_eq!(out.get_pixel(x, y), original.get_pixel(x, y), "untouched at {x},{y}");
        }
        let p = out.get_pixel(250, 120);
        assert!(p[0] > 200 && p[1] < 60, "the box is there at full size: {p:?}");
        // A change to the whole picture (a new look) is kept as made.
        let mut everything = before.to_rgba8();
        everything.pixels_mut().for_each(|p| *p = Rgba([255 - p[0], 255 - p[1], 255 - p[2], 255]));
        assert!(keep_unchanged(&full, &before, &DynamicImage::ImageRgba8(everything)).is_none());
    }

    /// "Keep what didn't change" on real pictures: EDIT_ORIGINAL (the photo),
    /// EDIT_RESULT (what the edit model made from it) and EDIT_OUT (a folder
    /// for mask.png and kept.png).
    #[test]
    #[ignore]
    fn keep_unchanged_on_real_pictures() {
        let get = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("set {k}"));
        let full = decode(&std::fs::read(get("EDIT_ORIGINAL")).unwrap()).unwrap();
        let after = decode(&std::fs::read(get("EDIT_RESULT")).unwrap()).unwrap();
        let out = std::path::PathBuf::from(get("EDIT_OUT"));
        let before = full.resize_exact(after.width(), after.height(), FilterType::Lanczos3);
        let (_, mask, share) = changed_area(&before, &after);
        eprintln!("changed: {:.0}% of the picture", share * 100.0);
        std::fs::write(out.join("mask.png"), png(&DynamicImage::ImageLuma8(mask)).unwrap()).unwrap();
        if let Some(kept) = keep_unchanged(&full, &before, &after) {
            eprintln!("kept at {:?}", kept.dimensions());
            std::fs::write(out.join("kept.png"), png(&kept).unwrap()).unwrap();
        }
    }

    #[test]
    fn a_blank_mask_is_refused() {
        let blank = png(&DynamicImage::ImageRgba8(RgbaImage::new(10, 10))).unwrap();
        assert!(mask_from_paint(&blank, 10, 10).is_err());
        let mut painted = RgbaImage::new(10, 10);
        painted.put_pixel(2, 2, Rgba([255, 255, 255, 255]));
        let m = mask_from_paint(&png(&DynamicImage::ImageRgba8(painted)).unwrap(), 20, 20).unwrap();
        assert_eq!(m.dimensions(), (20, 20));
    }
}
