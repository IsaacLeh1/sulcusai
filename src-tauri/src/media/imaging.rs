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
