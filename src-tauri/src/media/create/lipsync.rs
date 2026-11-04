// SPDX-License-Identifier: AGPL-3.0-only
//! Lip-sync for dubbing: which parts of a video need new lips, from the
//! dubbed lines' times. Lines close together are moved as one piece so the
//! face doesn't jump between pieces.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub start: f64,
    pub end: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Piece {
    pub start: f64,
    pub end: f64,
    /// Lines (by index) whose voice goes into this piece.
    pub lines: Vec<usize>,
}

/// Pieces to re-render: lines padded by `pad` seconds, merged when gaps are
/// shorter than `join`, each at most `max_len` (the model's clip length).
pub fn pieces(lines: &[Line], video_len: f64, pad: f64, join: f64, max_len: f64) -> Vec<Piece> {
    let mut order: Vec<usize> = (0..lines.len()).filter(|&i| lines[i].end > lines[i].start).collect();
    order.sort_by(|&a, &b| lines[a].start.total_cmp(&lines[b].start));
    let mut out: Vec<Piece> = Vec::new();
    for i in order {
        let s = (lines[i].start - pad).max(0.0);
        let e = (lines[i].end + pad).min(video_len);
        if let Some(last) = out.last_mut() {
            if s - last.end < join && e - last.start <= max_len {
                last.end = last.end.max(e);
                last.lines.push(i);
                continue;
            }
        }
        // A line longer than one clip is split; longclip carries the face across.
        let mut t = s;
        while e - t > max_len {
            out.push(Piece { start: t, end: t + max_len, lines: vec![i] });
            t += max_len;
        }
        out.push(Piece { start: t, end: e, lines: vec![i] });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn l(s: f64, e: f64) -> Line {
        Line { start: s, end: e }
    }
    #[test]
    fn merges_close_lines() {
        let p = pieces(&[l(1.0, 2.0), l(2.3, 3.0), l(10.0, 11.0)], 20.0, 0.2, 0.5, 5.0);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].lines, vec![0, 1]);
        assert!((p[0].start - 0.8).abs() < 1e-9 && (p[0].end - 3.2).abs() < 1e-9);
    }
    #[test]
    fn splits_long_line_and_clamps() {
        let p = pieces(&[l(0.0, 12.0)], 11.0, 0.5, 0.5, 5.0);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].start, 0.0);
        assert_eq!(p.last().unwrap().end, 11.0);
    }
    #[test]
    fn skips_empty() {
        assert!(pieces(&[l(3.0, 3.0)], 10.0, 0.1, 0.5, 5.0).is_empty());
    }
}
