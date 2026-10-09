// SPDX-License-Identifier: AGPL-3.0-only
//! Background removal: BiRefNet lite (MIT) on ONNX Runtime finds the
//! subject; everything else becomes transparent.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, GrayImage, Luma, RgbaImage};
use ort::session::Session;
use ort::value::Tensor;

const SIDE: u32 = 1024;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

fn e(err: impl std::fmt::Display) -> String {
    format!("Removing the background failed: {err}")
}

fn slot() -> &'static Mutex<Option<(PathBuf, Session)>> {
    static S: OnceLock<Mutex<Option<(PathBuf, Session)>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// The subject's outline: 255 = keep, 0 = background, at the picture's size.
pub fn matte(dll: &Path, model: &Path, img: &DynamicImage, threads: usize) -> Result<GrayImage, String> {
    crate::natural::runtime_ready(dll)?;
    let mut guard = slot().lock().unwrap();
    if guard.as_ref().map(|(p, _)| p.as_path()) != Some(model) {
        let session = Session::builder()
            .map_err(e)?
            .with_intra_threads(threads.max(1))
            .map_err(e)?
            .commit_from_file(model)
            .map_err(e)?;
        *guard = Some((model.to_path_buf(), session));
    }
    let session = &mut guard.as_mut().unwrap().1;

    let small = img.resize_exact(SIDE, SIDE, FilterType::Triangle).to_rgb8();
    let n = (SIDE * SIDE) as usize;
    let mut input = vec![0f32; 3 * n];
    for (i, p) in small.pixels().enumerate() {
        for c in 0..3 {
            input[c * n + i] = (p[c] as f32 / 255.0 - MEAN[c]) / STD[c];
        }
    }
    let out = session
        .run(ort::inputs! { "input_image" => Tensor::from_array(([1usize, 3, SIDE as usize, SIDE as usize], input)).map_err(e)? })
        .map_err(e)?;
    let (_, logits) = out[0].try_extract_tensor::<f32>().map_err(e)?;
    if logits.len() != n {
        return Err(e("unexpected output size"));
    }
    let m = GrayImage::from_fn(SIDE, SIDE, |x, y| {
        let v = logits[(y * SIDE + x) as usize];
        Luma([((1.0 / (1.0 + (-v).exp())) * 255.0).round() as u8])
    });
    let (w, h) = img.dimensions();
    Ok(image::imageops::resize(&m, w, h, FilterType::Triangle))
}

/// The picture with its background made transparent.
pub fn apply(img: &DynamicImage, matte: &GrayImage) -> RgbaImage {
    let mut out = img.to_rgba8();
    for (x, y, p) in out.enumerate_pixels_mut() {
        let a = matte.get_pixel(x, y)[0] as u32;
        p[3] = (p[3] as u32 * a / 255) as u8;
    }
    out
}

pub fn unload() {
    *slot().lock().unwrap() = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn matte_sets_transparency() {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(4, 2, Rgba([10, 20, 30, 255])));
        let mut m = GrayImage::new(4, 2);
        m.put_pixel(0, 0, Luma([255]));
        m.put_pixel(1, 0, Luma([128]));
        let out = apply(&img, &m);
        assert_eq!(out.get_pixel(0, 0)[3], 255);
        assert_eq!(out.get_pixel(1, 0)[3], 128);
        assert_eq!(out.get_pixel(3, 1)[3], 0);
        assert_eq!(out.get_pixel(3, 1)[0], 10, "colors kept under the transparency");
    }
}
