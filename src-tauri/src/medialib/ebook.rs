// SPDX-License-Identifier: AGPL-3.0-only
//! EPUB books: the title, author and chapters in reading order, as text.
//! Books with DRM can't be read and say so.

use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub title: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Book {
    pub title: String,
    pub author: String,
    pub sections: Vec<Section>,
}

const MAX_PART: u64 = 32 * 1024 * 1024;

fn read_entry<R: std::io::Read + std::io::Seek>(z: &mut zip::ZipArchive<R>, name: &str) -> Result<String, String> {
    let f = z.by_name(name).map_err(|_| format!("The book is missing {name}."))?;
    let mut s = String::new();
    f.take(MAX_PART).read_to_string(&mut s).map_err(|e| e.to_string())?;
    Ok(s)
}

pub fn open(path: &Path) -> Result<Book, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    read(file)
}

pub fn read<R: std::io::Read + std::io::Seek>(r: R) -> Result<Book, String> {
    let mut z = zip::ZipArchive::new(r).map_err(|_| "That isn't an EPUB book.".to_string())?;
    if z.by_name("META-INF/encryption.xml").is_ok() {
        return Err("This book is locked with DRM, so it can't be read here.".into());
    }
    let container = read_entry(&mut z, "META-INF/container.xml")?;
    let opf_path = attr_of(&container, "rootfile", "full-path").ok_or("The book has no contents list.")?;
    let opf = read_entry(&mut z, &opf_path)?;
    let base = opf_path.rsplit_once('/').map(|(d, _)| format!("{d}/")).unwrap_or_default();
    let pkg = package(&opf);
    let mut book = Book { title: pkg.title, author: pkg.author, sections: vec![] };
    for id in pkg.spine {
        let Some((_, href)) = pkg.manifest.iter().find(|(i, _)| *i == id) else { continue };
        let href = href.split('#').next().unwrap_or(href);
        let Ok(html) = read_entry(&mut z, &format!("{base}{}", percent_decode(href))) else { continue };
        let (title, text) = xhtml_text(&html);
        if !text.trim().is_empty() {
            book.sections.push(Section { title, text });
        }
    }
    Ok(book)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn local(n: &[u8]) -> String {
    let s = String::from_utf8_lossy(n);
    s.rsplit(':').next().unwrap_or(&s).to_ascii_lowercase()
}

fn attr(e: &quick_xml::events::BytesStart, key: &str) -> Option<String> {
    e.attributes().flatten().find(|a| local(a.key.as_ref()) == key).map(|a| String::from_utf8_lossy(&a.value).to_string())
}

fn attr_of(xml: &str, tag: &str, key: &str) -> Option<String> {
    use quick_xml::events::Event as X;
    let mut r = quick_xml::Reader::from_str(xml);
    loop {
        match r.read_event() {
            Ok(X::Start(e)) | Ok(X::Empty(e)) if local(e.name().as_ref()) == tag => return attr(&e, key),
            Ok(X::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}

#[derive(Default)]
struct Package {
    title: String,
    author: String,
    manifest: Vec<(String, String)>,
    spine: Vec<String>,
}

fn package(opf: &str) -> Package {
    use quick_xml::events::Event as X;
    let mut r = quick_xml::Reader::from_str(opf);
    let mut p = Package::default();
    let mut current = String::new();
    loop {
        match r.read_event() {
            Ok(X::Start(e)) | Ok(X::Empty(e)) => {
                let n = local(e.name().as_ref());
                match n.as_str() {
                    "item" => {
                        if let (Some(id), Some(h)) = (attr(&e, "id"), attr(&e, "href")) {
                            p.manifest.push((id, h));
                        }
                    }
                    "itemref" => p.spine.extend(attr(&e, "idref")),
                    _ => {}
                }
                current = n;
            }
            Ok(X::Text(t)) => {
                let t = t.decode().map(|s| s.trim().to_string()).unwrap_or_default();
                if current == "title" && p.title.is_empty() {
                    p.title = t;
                } else if current == "creator" && p.author.is_empty() {
                    p.author = t;
                }
            }
            Ok(X::End(_)) => current.clear(),
            Ok(X::Eof) | Err(_) => break,
            _ => {}
        }
    }
    p
}

/// The chapter's first heading and its words, a blank line between blocks.
fn xhtml_text(html: &str) -> (String, String) {
    use quick_xml::events::Event as X;
    let mut r = quick_xml::Reader::from_str(html);
    r.config_mut().check_end_names = false;
    let mut out = String::new();
    let mut title = String::new();
    let mut in_head = 0u32;
    let mut skip = 0u32;
    let mut heading: Option<String> = None;
    let push = |out: &mut String, heading: &mut Option<String>, s: &str| {
        out.push_str(s);
        if let Some(h) = heading.as_mut() {
            h.push_str(s);
        }
    };
    loop {
        match r.read_event() {
            Ok(X::Start(e)) => match local(e.name().as_ref()).as_str() {
                "head" => in_head += 1,
                "script" | "style" => skip += 1,
                "h1" | "h2" | "h3" if title.is_empty() && heading.is_none() => heading = Some(String::new()),
                _ => {}
            },
            Ok(X::Empty(e)) if local(e.name().as_ref()) == "br" => out.push('\n'),
            Ok(X::Text(t)) if in_head == 0 && skip == 0 => {
                let s = t.decode().map(|s| s.to_string()).unwrap_or_default();
                push(&mut out, &mut heading, &s);
            }
            Ok(X::GeneralRef(g)) if in_head == 0 && skip == 0 => {
                let s = super::feeds::entity(&g.decode().unwrap_or_default());
                let s = if s.is_empty() { " ".to_string() } else { s };
                push(&mut out, &mut heading, &s);
            }
            Ok(X::End(e)) => match local(e.name().as_ref()).as_str() {
                "head" => in_head = in_head.saturating_sub(1),
                "script" | "style" => skip = skip.saturating_sub(1),
                n @ ("p" | "div" | "h1" | "h2" | "h3" | "h4" | "li" | "blockquote") => {
                    if matches!(n, "h1" | "h2" | "h3") {
                        if let Some(h) = heading.take() {
                            title = h.split_whitespace().collect::<Vec<_>>().join(" ");
                        }
                    }
                    out.push_str("\n\n");
                }
                _ => {}
            },
            Ok(X::Eof) | Err(_) => break,
            _ => {}
        }
    }
    let text = out
        .split("\n\n")
        .map(|b| b.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|b| !b.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (title, text)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    pub fn sample(encrypted: bool) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default();
            let mut put = |name: &str, body: &str| {
                z.start_file(name, o).unwrap();
                z.write_all(body.as_bytes()).unwrap();
            };
            put("mimetype", "application/epub+zip");
            put("META-INF/container.xml", r#"<container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#);
            put("OEBPS/content.opf", r#"<package><metadata xmlns:dc="x"><dc:title>The Sea</dc:title><dc:creator>A. Writer</dc:creator></metadata>
<manifest><item id="c2" href="two.xhtml"/><item id="c1" href="ch%201.xhtml"/></manifest><spine><itemref idref="c1"/><itemref idref="c2"/></spine></package>"#);
            put("OEBPS/ch 1.xhtml", "<html><head><title>x</title><style>p{}</style></head><body><h1>Chapter <i>One</i></h1><p>Waves &amp; wind.</p><p>More   text.</p></body></html>");
            put("OEBPS/two.xhtml", "<html><body><h2>Two</h2><p>End&#33;<br/>Really</p></body></html>");
            if encrypted {
                put("META-INF/encryption.xml", "<encryption/>");
            }
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn reads_chapters_in_spine_order() {
        let b = read(std::io::Cursor::new(sample(false))).unwrap();
        assert_eq!(b.title, "The Sea");
        assert_eq!(b.author, "A. Writer");
        assert_eq!(b.sections.len(), 2);
        assert_eq!(b.sections[0].title, "Chapter One");
        assert_eq!(b.sections[0].text, "Chapter One\n\nWaves & wind.\n\nMore text.");
        assert_eq!(b.sections[1].title, "Two");
        assert!(b.sections[1].text.contains("End!"));
    }

    #[test]
    fn drm_and_junk_are_refused() {
        assert!(read(std::io::Cursor::new(sample(true))).unwrap_err().contains("DRM"));
        assert!(read(std::io::Cursor::new(b"nope".to_vec())).is_err());
    }
}
