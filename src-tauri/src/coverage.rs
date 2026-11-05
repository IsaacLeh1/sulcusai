// SPDX-License-Identifier: AGPL-3.0-only
//! What a document read couldn't turn into text. Text extraction misses
//! equations drawn as shapes or pictures, math fonts that don't map to
//! letters, figures and scanned pages, and the model can't tell from the
//! text alone, so an answer gets built on a document it only partly read.
//! This lists, per page or part, what's there that the text doesn't hold,
//! with no model needed: the result then says "partial" and what to do.

use std::path::Path;

/// One page (or part) holding more than its text.
#[derive(Debug, Clone, PartialEq)]
pub struct Gap {
    pub page: Option<u32>,
    pub what: String,
}

/// Font names that set equations (Cambria Math, Symbol, TeX's math fonts).
pub fn math_font(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["math", "symbol", "cmmi", "cmsy", "cmex", "msam", "msbm", "stix", "mtpro", "euler", "esint"].iter().any(|k| n.contains(k))
}

/// What a page's counts say it holds besides text.
pub fn page_gap(images: usize, paths: usize, math_fonts: &[String], text_chars: usize) -> Option<String> {
    let mut what = Vec::new();
    if !math_fonts.is_empty() {
        what.push(format!("equations set in math fonts ({}), which may have come out as odd symbols or not at all", math_fonts.join(", ")));
    }
    if images > 0 && text_chars < 200 {
        what.push(format!("{images} picture(s) and almost no text: likely a scanned page or a figure, not read"));
    } else if images > 0 {
        what.push(format!("{images} picture(s) (figures, charts or equations as images), not read"));
    }
    if paths >= 150 {
        what.push(format!("{paths} drawn shapes (charts, diagrams or equations drawn as lines), not read"));
    }
    (!what.is_empty()).then(|| what.join("; "))
}

/// A PDF, page by page.
pub fn pdf(path: &Path, page_text: &[String]) -> Vec<Gap> {
    let Ok(doc) = lopdf::Document::load(path) else { return Vec::new() };
    let mut out = Vec::new();
    for (n, id) in doc.get_pages() {
        let images = doc.get_page_images(id).map(|v| v.len()).unwrap_or(0);
        let paths = doc
            .get_and_decode_page_content(id)
            .map(|c| c.operations.iter().filter(|o| matches!(o.operator.as_str(), "l" | "c" | "v" | "y" | "re")).count())
            .unwrap_or(0);
        let mut fonts: Vec<String> = doc
            .get_page_fonts(id)
            .map(|f| f.values().filter_map(|d| d.get(b"BaseFont").ok().and_then(|b| b.as_name().ok()).map(|b| String::from_utf8_lossy(b).into_owned())).collect())
            .unwrap_or_default();
        fonts.retain(|f| math_font(f));
        fonts.sort();
        fonts.dedup();
        let chars = page_text.get(n as usize - 1).map_or(0, |t| t.trim().chars().count());
        if let Some(what) = page_gap(images, paths, &fonts, chars) {
            out.push(Gap { page: Some(n), what });
        }
    }
    out
}

/// A Word document: Office math, pictures, text boxes and comments, which
/// a plain text read leaves out.
pub fn docx(document_xml: &str) -> Vec<Gap> {
    let count = |tag: &str| document_xml.matches(tag).count();
    let mut out = Vec::new();
    let math = count("<m:oMath>") + count("<m:oMath ");
    if math > 0 {
        out.push(Gap { page: None, what: format!("{math} equation(s) in Office math, not in the text") });
    }
    let pictures = count("<w:drawing>") + count("<w:pict>");
    if pictures > 0 {
        out.push(Gap { page: None, what: format!("{pictures} picture(s) or chart(s), not read") });
    }
    let boxes = count("<w:txbxContent>");
    if boxes > 0 {
        out.push(Gap { page: None, what: format!("{boxes} text box(es), which may be missing from the text") });
    }
    out
}

/// The lines a reading tool adds to what it returns, or None when nothing
/// was missed.
pub fn report(gaps: &[Gap], cut_at: Option<usize>) -> Option<String> {
    if gaps.is_empty() && cut_at.is_none() {
        return None;
    }
    let mut s = String::from("\n\n[Coverage: PARTIAL. Not everything in this document is in the text above:");
    for g in gaps.iter().take(40) {
        match g.page {
            Some(p) => s.push_str(&format!("\n- page {p}: {}", g.what)),
            None => s.push_str(&format!("\n- {}", g.what)),
        }
    }
    if gaps.len() > 40 {
        s.push_str(&format!("\n- and {} more pages like these", gaps.len() - 40));
    }
    if let Some(n) = cut_at {
        s.push_str(&format!("\n- the text stops after {n} characters; the rest wasn't returned"));
    }
    s.push_str("\nAn answer that needs these parts may be wrong. Say what wasn't read, or read it another way (look at the page as a picture with a model that sees pictures) before answering.]");
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_fonts_and_pictures_make_a_page_partial() {
        assert!(math_font("ABCDEF+CambriaMath") && math_font("CMMI10") && !math_font("Helvetica"));
        assert!(page_gap(0, 10, &[], 2000).is_none(), "plain text page");
        assert!(page_gap(0, 0, &["CMMI10".into()], 2000).unwrap().contains("equations"));
        assert!(page_gap(1, 0, &[], 20).unwrap().contains("scanned"));
        assert!(page_gap(0, 400, &[], 2000).unwrap().contains("drawn shapes"));
    }

    #[test]
    fn office_math_is_reported_and_a_clean_read_says_nothing() {
        let gaps = docx("<w:p><m:oMath><m:r>x</m:r></m:oMath></w:p><w:drawing>");
        assert_eq!(gaps.len(), 2);
        let r = report(&gaps, Some(40_000)).unwrap();
        assert!(r.contains("PARTIAL") && r.contains("Office math") && r.contains("40000 characters"));
        assert!(report(&[], None).is_none());
    }
}
