// SPDX-License-Identifier: AGPL-3.0-only
//! Fillable PDF forms: the fields a PDF asks for, and a filled copy saved
//! beside it (the original is never changed). Viewers draw the new values
//! because the copy asks them to (NeedAppearances).

use std::collections::HashMap;
use std::path::Path;

use lopdf::{Document, Object, StringFormat};
use serde::Serialize;

#[derive(Serialize, Debug, PartialEq)]
pub struct FormField {
    pub name: String,
    /// "text", "checkbox", "choice" or "other".
    pub kind: String,
    pub value: String,
}

fn decode(b: &[u8]) -> String {
    if b.starts_with(&[0xFE, 0xFF]) {
        let u: Vec<u16> = b[2..].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&u)
    } else {
        b.iter().map(|&c| c as char).collect()
    }
}

fn encode(s: &str) -> Object {
    if s.is_ascii() {
        Object::string_literal(s)
    } else {
        let mut b = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            b.extend_from_slice(&u.to_be_bytes());
        }
        Object::String(b, StringFormat::Hexadecimal)
    }
}

fn field_of(d: &lopdf::Dictionary) -> Option<(String, String, String)> {
    let name = decode(d.get(b"T").ok()?.as_str().ok()?);
    let kind = match d.get(b"FT").ok().and_then(|o| o.as_name().ok()) {
        Some(b"Tx") => "text",
        Some(b"Btn") => "checkbox",
        Some(b"Ch") => "choice",
        _ => "other",
    };
    let value = match d.get(b"V") {
        Ok(Object::Name(n)) => String::from_utf8_lossy(n).into_owned(),
        Ok(o) => o.as_str().map(decode).unwrap_or_default(),
        Err(_) => String::new(),
    };
    Some((name, kind.to_string(), value))
}

pub fn fields(path: &Path) -> Result<Vec<FormField>, String> {
    let doc = Document::load(path).map_err(|e| format!("Couldn't read this PDF: {e}"))?;
    let mut out: Vec<FormField> = doc
        .objects
        .values()
        .filter_map(|o| o.as_dict().ok())
        .filter_map(field_of)
        .map(|(name, kind, value)| FormField { name, kind, value })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name);
    Ok(out)
}

/// Fills fields by name into a copy at `out`; answers the names it filled.
/// Checkboxes take "yes"/"no" (or true/false).
pub fn fill(path: &Path, values: &HashMap<String, String>, out: &Path) -> Result<Vec<String>, String> {
    let mut doc = Document::load(path).map_err(|e| format!("Couldn't read this PDF: {e}"))?;
    let mut filled = vec![];
    for obj in doc.objects.values_mut() {
        let Ok(d) = obj.as_dict_mut() else { continue };
        let Some((name, kind, _)) = field_of(d) else { continue };
        let Some(v) = values.get(&name) else { continue };
        if kind == "checkbox" {
            let on = matches!(v.trim().to_ascii_lowercase().as_str(), "yes" | "true" | "on" | "1" | "x");
            let state = if on { "Yes" } else { "Off" };
            d.set("V", Object::Name(state.as_bytes().to_vec()));
            d.set("AS", Object::Name(state.as_bytes().to_vec()));
        } else {
            d.set("V", encode(v));
        }
        filled.push(name);
    }
    if let Ok(cat) = doc.catalog_mut() {
        if let Ok(form) = cat.get_mut(b"AcroForm").and_then(|f| f.as_dict_mut()) {
            form.set("NeedAppearances", Object::Boolean(true));
        }
    }
    let form_ref = doc.catalog().ok().and_then(|c| c.get(b"AcroForm").ok()).and_then(|f| f.as_reference().ok());
    if let Some(id) = form_ref {
        if let Ok(form) = doc.get_object_mut(id).and_then(|f| f.as_dict_mut()) {
            form.set("NeedAppearances", Object::Boolean(true));
        }
    }
    doc.save(out).map_err(|e| e.to_string())?;
    filled.sort();
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    fn sample(path: &Path) {
        let mut doc = Document::with_version("1.5");
        let name = doc.add_object(dictionary! { "FT" => "Tx", "T" => Object::string_literal("Full name"), "V" => Object::string_literal("") });
        let agree = doc.add_object(dictionary! { "FT" => "Btn", "T" => Object::string_literal("Agree"), "V" => "Off" });
        let form = doc.add_object(dictionary! { "Fields" => vec![name.into(), agree.into()] });
        let pages = doc.add_object(dictionary! { "Type" => "Pages", "Kids" => Vec::<Object>::new(), "Count" => 0 });
        let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages, "AcroForm" => form });
        doc.trailer.set("Root", cat);
        doc.save(path).unwrap();
    }

    #[test]
    fn fields_are_listed_and_filled_into_a_copy() {
        let d = std::env::temp_dir().join(format!("sulcusai-pdfform-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&d).unwrap();
        let (src, out) = (d.join("form.pdf"), d.join("filled.pdf"));
        sample(&src);
        let f = fields(&src).unwrap();
        assert_eq!(f.iter().map(|x| (x.name.as_str(), x.kind.as_str())).collect::<Vec<_>>(), vec![("Agree", "checkbox"), ("Full name", "text")]);
        let vals = HashMap::from([("Full name".to_string(), "Zoë Park".to_string()), ("Agree".to_string(), "yes".to_string())]);
        assert_eq!(fill(&src, &vals, &out).unwrap(), vec!["Agree", "Full name"]);
        let g = fields(&out).unwrap();
        assert_eq!(g[0].value, "Yes");
        assert_eq!(g[1].value, "Zoë Park");
        assert_eq!(fields(&src).unwrap()[1].value, "", "the original is untouched");
        std::fs::remove_dir_all(&d).ok();
    }
}
