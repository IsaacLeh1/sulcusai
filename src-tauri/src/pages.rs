// SPDX-License-Identifier: AGPL-3.0-only
//! Reading what a document's text can't hold: a page drawn as a picture
//! (Windows' own PDF renderer), read with Windows' own OCR, or handed to a
//! model that sees pictures. Both Windows parts ship with the system, so a
//! scanned page or an equation drawn as shapes can be read with nothing
//! downloaded and nothing sent off the PC.

/// Page numbers from "3", "2-4", "1,3,5-6" (1-based), within `count`,
/// at most `most` of them. None with an empty or unreadable list.
pub fn parse_pages(spec: &str, count: u32, most: usize) -> Result<Vec<u32>, String> {
    let mut out = Vec::new();
    for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (part, part),
        };
        let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) else {
            return Err(format!("“{part}” isn't a page or range. Give pages like 3, 2-4 or 1,3,5."));
        };
        if a == 0 || b < a {
            return Err(format!("“{part}” isn't a page range. Pages start at 1."));
        }
        if a > count {
            return Err(format!("Page {a} doesn't exist: the document has {count} page(s)."));
        }
        for p in a..=b.min(count) {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    if out.is_empty() {
        return Err("Say which pages, like 3, 2-4 or 1,3,5.".into());
    }
    out.truncate(most);
    Ok(out)
}

/// Unicode math in extracted text written as LaTeX, so equations set in
/// math fonts that did map to characters reach the model in one notation.
pub fn latex_symbols(s: &str) -> String {
    const MAP: &[(char, &str)] = &[
        ('α', "\\alpha "), ('β', "\\beta "), ('γ', "\\gamma "), ('δ', "\\delta "), ('ε', "\\epsilon "), ('θ', "\\theta "), ('λ', "\\lambda "),
        ('μ', "\\mu "), ('π', "\\pi "), ('ρ', "\\rho "), ('σ', "\\sigma "), ('τ', "\\tau "), ('φ', "\\phi "), ('χ', "\\chi "), ('ψ', "\\psi "),
        ('ω', "\\omega "), ('Δ', "\\Delta "), ('Σ', "\\Sigma "), ('Ω', "\\Omega "), ('∑', "\\sum "), ('∏', "\\prod "), ('∫', "\\int "),
        ('∂', "\\partial "), ('∇', "\\nabla "), ('∞', "\\infty "), ('≤', "\\le "), ('≥', "\\ge "), ('≠', "\\ne "), ('≈', "\\approx "),
        ('±', "\\pm "), ('×', "\\times "), ('÷', "\\div "), ('·', "\\cdot "), ('√', "\\sqrt "), ('∈', "\\in "), ('∀', "\\forall "),
        ('∃', "\\exists "), ('→', "\\to "), ('⇒', "\\Rightarrow "), ('∪', "\\cup "), ('∩', "\\cap "), ('⊂', "\\subset "), ('∝', "\\propto "),
    ];
    s.chars().map(|c| MAP.iter().find(|(k, _)| *k == c).map_or_else(|| c.to_string(), |(_, v)| v.to_string())).collect()
}

/// A short outline: the lines that look like headings, and figure, table
/// and equation mentions, per page. Cheap, and it tells the model where to
/// look before reading everything.
pub fn outline(pages: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for (i, text) in pages.iter().enumerate() {
        let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        let heads: Vec<&str> = lines
            .iter()
            .copied()
            .filter(|l| {
                let n = l.chars().count();
                (3..70).contains(&n) && !l.ends_with(['.', ',', ';', ':']) && l.chars().next().is_some_and(|c| c.is_uppercase() || c.is_ascii_digit()) && l.split_whitespace().count() <= 9
            })
            .take(4)
            .collect();
        let lower = text.to_lowercase();
        let count = |w: &str| lower.matches(w).count();
        let mut marks = Vec::new();
        for (w, name) in [("figure", "figure"), ("fig.", "figure"), ("table", "table"), ("equation", "equation"), ("eq.", "equation")] {
            let n = count(w);
            if n > 0 && !marks.iter().any(|m: &String| m.ends_with(name)) {
                marks.push(format!("{n} mention(s) of a {name}"));
            }
        }
        out.push(format!(
            "page {}: {} characters{}{}",
            i + 1,
            text.trim().chars().count(),
            if heads.is_empty() { String::new() } else { format!("; headings: {}", heads.join(" | ")) },
            if marks.is_empty() { String::new() } else { format!("; {}", marks.join(", ")) }
        ));
    }
    out
}

/// One PDF page drawn as a PNG, `width` pixels wide.
#[cfg(windows)]
pub fn render_pdf_page(path: &std::path::Path, page: u32, width: u32) -> Result<Vec<u8>, String> {
    use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
    use windows::Storage::Streams::InMemoryRandomAccessStream;
    winrt_init();
    let bytes = std::fs::read(path).map_err(|e| format!("Couldn't open the PDF: {e}"))?;
    let w = |e: windows::core::Error| format!("Windows couldn't draw the page: {}", e.message());
    let input = stream_of(&bytes).map_err(w)?;
    let doc = PdfDocument::LoadFromStreamAsync(&input).and_then(|op| op.get()).map_err(w)?;
    let count = doc.PageCount().map_err(w)?;
    if page == 0 || page > count {
        return Err(format!("Page {page} doesn't exist: the document has {count} page(s)."));
    }
    let p = doc.GetPage(page - 1).map_err(w)?;
    let opts = PdfPageRenderOptions::new().map_err(w)?;
    opts.SetDestinationWidth(width).map_err(w)?;
    let out = InMemoryRandomAccessStream::new().map_err(w)?;
    p.RenderWithOptionsToStreamAsync(&out, &opts).and_then(|op| op.get()).map_err(w)?;
    read_all(&out).map_err(w)
}

#[cfg(not(windows))]
pub fn render_pdf_page(_path: &std::path::Path, _page: u32, _width: u32) -> Result<Vec<u8>, String> {
    Err("Drawing PDF pages as pictures works on Windows only so far.".into())
}

/// The page count Windows' renderer sees (None when it can't open the file).
#[cfg(windows)]
pub fn pdf_page_count(path: &std::path::Path) -> Option<u32> {
    winrt_init();
    let bytes = std::fs::read(path).ok()?;
    let input = stream_of(&bytes).ok()?;
    windows::Data::Pdf::PdfDocument::LoadFromStreamAsync(&input).and_then(|op| op.get()).and_then(|d| d.PageCount()).ok()
}

#[cfg(not(windows))]
pub fn pdf_page_count(_path: &std::path::Path) -> Option<u32> {
    None
}

/// The text Windows' OCR finds in a picture, line by line, in the person's
/// own languages.
#[cfg(windows)]
pub fn ocr(image: &[u8]) -> Result<String, String> {
    use windows::Graphics::Imaging::{BitmapDecoder, BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    winrt_init();
    let w = |e: windows::core::Error| format!("Windows' text reader failed: {}", e.message());
    let input = stream_of(image).map_err(w)?;
    let decoder = BitmapDecoder::CreateAsync(&input).and_then(|op| op.get()).map_err(w)?;
    let bmp = decoder.GetSoftwareBitmapAsync().and_then(|op| op.get()).map_err(w)?;
    let bmp = SoftwareBitmap::Convert(&bmp, BitmapPixelFormat::Bgra8).map_err(w)?;
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|_| "Windows has no text-reading language installed (Settings, Time & language, Language: add one with Optical character recognition).".to_string())?;
    let result = engine.RecognizeAsync(&bmp).and_then(|op| op.get()).map_err(w)?;
    let mut lines = Vec::new();
    for line in result.Lines().map_err(w)? {
        lines.push(line.Text().map_err(w)?.to_string());
    }
    Ok(lines.join("\n"))
}

#[cfg(not(windows))]
pub fn ocr(_image: &[u8]) -> Result<String, String> {
    Err("Reading text from pictures works on Windows only so far.".into())
}

#[cfg(windows)]
fn winrt_init() {
    // Already set up on this thread is fine; a pooled thread may not be.
    unsafe {
        let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
    }
}

#[cfg(windows)]
fn stream_of(bytes: &[u8]) -> windows::core::Result<windows::Storage::Streams::InMemoryRandomAccessStream> {
    use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};
    let s = InMemoryRandomAccessStream::new()?;
    let writer = DataWriter::CreateDataWriter(&s)?;
    writer.WriteBytes(bytes)?;
    writer.StoreAsync()?.get()?;
    writer.FlushAsync()?.get()?;
    writer.DetachStream()?;
    s.Seek(0)?;
    Ok(s)
}

#[cfg(windows)]
fn read_all(s: &windows::Storage::Streams::InMemoryRandomAccessStream) -> windows::core::Result<Vec<u8>> {
    use windows::Storage::Streams::DataReader;
    let size = s.Size()? as u32;
    let reader = DataReader::CreateDataReader(&s.GetInputStreamAt(0)?)?;
    reader.LoadAsync(size)?.get()?;
    let mut buf = vec![0u8; size as usize];
    reader.ReadBytes(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_are_picked_from_ranges() {
        assert_eq!(parse_pages("3", 5, 10).unwrap(), vec![3]);
        assert_eq!(parse_pages("1, 3-4,3", 5, 10).unwrap(), vec![1, 3, 4]);
        assert_eq!(parse_pages("4-9", 5, 10).unwrap(), vec![4, 5]);
        assert_eq!(parse_pages("1-9", 9, 3).unwrap(), vec![1, 2, 3]);
        assert!(parse_pages("7", 5, 10).unwrap_err().contains("5 page"));
        assert!(parse_pages("x", 5, 10).is_err() && parse_pages("", 5, 10).is_err() && parse_pages("0", 5, 10).is_err());
    }

    #[test]
    fn math_symbols_become_latex() {
        assert_eq!(latex_symbols("x ≤ μ ± σ"), "x \\le  \\mu  \\pm  \\sigma ");
        assert_eq!(latex_symbols("plain"), "plain");
    }

    #[test]
    fn the_outline_names_headings_and_figures() {
        let o = outline(&["Results\nThe values in Figure 2 and Table 1 rise.\nSee equation (3).".into(), String::new()]);
        assert!(o[0].starts_with("page 1:") && o[0].contains("headings: Results"), "{}", o[0]);
        assert!(o[0].contains("figure") && o[0].contains("table") && o[0].contains("equation"));
        assert!(o[1].contains("0 characters"));
    }

    /// Windows draws a real PDF page and reads its text back.
    #[cfg(windows)]
    #[test]
    fn a_page_is_drawn_and_read_back() {
        let dir = std::env::temp_dir().join(format!("pages-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pdf = dir.join("t.pdf");
        crate::docs::write_pdf(&pdf, "# Hello Reader\n\nThe quick brown fox jumps over the lazy dog.").unwrap();
        assert_eq!(pdf_page_count(&pdf), Some(1));
        let png = render_pdf_page(&pdf, 1, 1400).unwrap();
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
        assert!(render_pdf_page(&pdf, 2, 800).unwrap_err().contains("1 page"));
        // OCR needs a language pack; a PC without one says so instead.
        match ocr(&png) {
            Ok(text) => assert!(text.to_lowercase().contains("fox"), "{text}"),
            Err(e) => assert!(e.contains("language"), "{e}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
