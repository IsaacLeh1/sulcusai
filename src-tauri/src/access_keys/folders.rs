// SPDX-License-Identifier: AGPL-3.0-only
//! Whether a path lies inside one of a key's chosen folders.
//!
//! Compared by whole path parts, so a key for C:\Work can't reach
//! C:\Work-old, and any ".." refuses the path outright rather than trying to
//! work out where it leads. Callers should resolve links first (dunce
//! canonicalize) so a link inside a chosen folder can't point out of it.

/// The parts of an absolute path, or None for a relative path or one that
/// climbs with "..".
fn parts(path: &str) -> Option<Vec<String>> {
    let p = path.trim().replace('\\', "/");
    let p = p.strip_prefix("//?/").unwrap_or(&p);
    let absolute = p.starts_with('/') || p.as_bytes().get(1) == Some(&b':');
    if !absolute {
        return None;
    }
    let mut out = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => continue,
            ".." => return None,
            // Windows paths ignore case, so C:\work and c:\Work are one folder.
            _ if cfg!(windows) => out.push(part.to_lowercase()),
            _ => out.push(part.to_string()),
        }
    }
    Some(out)
}

/// Absolute and without "..", the least even an every-folder key needs.
pub fn valid(path: &str) -> bool {
    parts(path).is_some()
}

pub fn within(path: &str, folder: &str) -> bool {
    match (parts(path), parts(folder)) {
        (Some(p), Some(f)) => !f.is_empty() && p.starts_with(&f),
        _ => false,
    }
}

/// Folders as they'll be stored: trimmed, absolute, no "..", no repeats.
pub fn clean(folders: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for f in folders {
        let f = f.trim();
        if f.is_empty() {
            continue;
        }
        match parts(f) {
            Some(p) if !p.is_empty() => {}
            _ => return Err(format!("\"{f}\" isn't a full folder path. Choose the folder again.")),
        }
        if !out.iter().any(|o| within(f, o) && within(o, f)) {
            out.push(f.to_string());
        }
    }
    Ok(out)
}
