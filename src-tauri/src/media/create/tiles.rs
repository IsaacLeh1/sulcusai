// SPDX-License-Identifier: AGPL-3.0-only
//! Seamless tiles: drawn with wrap-around padding (sd.cpp's --circular), then
//! checked: the left edge must meet the right and the top the bottom about as
//! smoothly as neighbouring columns inside the picture do.

/// Words that help the model draw a pattern rather than a scene.
pub fn tile_prompt(p: &str) -> String {
    let p = p.trim();
    if p.to_lowercase().contains("seamless") {
        p.to_string()
    } else {
        format!("{p}, seamless tileable pattern, flat even lighting")
    }
}

pub fn tile_args() -> Vec<std::ffi::OsString> {
    vec!["--circular".into()]
}

fn diff(a: &[u8], b: &[u8]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).abs()).sum::<f64>() / a.len().max(1) as f64
}

/// How much harder the wrap seams are than the picture's own neighbouring
/// lines (1.0 = no visible seam). `rgb` is width*height*3.
pub fn seam_ratio(rgb: &[u8], width: usize, height: usize) -> f64 {
    let px = |x: usize, y: usize| &rgb[(y * width + x) * 3..(y * width + x) * 3 + 3];
    let col = |x: usize| (0..height).flat_map(|y| px(x, y).to_vec()).collect::<Vec<u8>>();
    let row = |y: usize| rgb[y * width * 3..(y + 1) * width * 3].to_vec();
    let inner_x = (0..width - 1).step_by((width / 16).max(1)).map(|x| diff(&col(x), &col(x + 1))).sum::<f64>() / ((width - 1) as f64 / (width / 16).max(1) as f64).ceil();
    let inner_y = (0..height - 1).step_by((height / 16).max(1)).map(|y| diff(&row(y), &row(y + 1))).sum::<f64>() / ((height - 1) as f64 / (height / 16).max(1) as f64).ceil();
    let seam_x = diff(&col(width - 1), &col(0));
    let seam_y = diff(&row(height - 1), &row(0));
    let inner = (inner_x + inner_y) / 2.0;
    ((seam_x + seam_y) / 2.0) / inner.max(1.0)
}

/// A tile passes when its seams are no more than twice as rough as inside.
pub fn seamless(rgb: &[u8], width: usize, height: usize) -> bool {
    width > 2 && height > 2 && seam_ratio(rgb, width, height) <= 2.0
}

#[cfg(test)]
mod tests {
    use super::*;
    fn picture(w: usize, h: usize, f: impl Fn(usize, usize) -> u8) -> Vec<u8> {
        let f = &f;
        (0..h).flat_map(|y| (0..w).flat_map(move |x| [f(x, y); 3])).collect()
    }
    #[test]
    fn wrapping_wave_passes() {
        let w = 64;
        let p = picture(w, w, |x, y| (128.0 + 100.0 * ((x + y) as f64 * std::f64::consts::TAU / w as f64).sin()) as u8);
        assert!(seamless(&p, w, w), "{}", seam_ratio(&p, w, w));
    }
    #[test]
    fn gradient_fails() {
        let p = picture(64, 64, |x, _| (x * 4) as u8);
        assert!(!seamless(&p, 64, 64));
    }
    #[test]
    fn prompt() {
        assert!(tile_prompt("moss").contains("seamless"));
        assert_eq!(tile_prompt("seamless bricks"), "seamless bricks");
        assert_eq!(tile_args(), vec![std::ffi::OsString::from("--circular")]);
    }
}
