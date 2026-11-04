// SPDX-License-Identifier: AGPL-3.0-only
//! Archives in shared folders: list what's inside a .zip, .tar, .tar.gz or
//! .tgz, and unpack one into a folder. Nothing is written outside the target
//! folder (an entry naming "../" or a full path is refused), and sizes are
//! capped so a small file that unpacks to terabytes stops early.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

use super::{arg_str, Ctx, Outcome, Preview};
use crate::sandbox::Sandbox;

const LIST_MAX: usize = 400;
const UNPACK_MAX_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const UNPACK_MAX_FILES: usize = 50_000;

pub fn list_archive_params() -> Value {
    json!({ "type": "object", "required": ["path"], "properties": {
        "path": { "type": "string", "description": "The archive, starting with the folder name: .zip, .tar, .tar.gz or .tgz." } } })
}

pub fn extract_archive_params() -> Value {
    json!({ "type": "object", "required": ["path"], "properties": {
        "path": { "type": "string", "description": "The archive to unpack." },
        "into": { "type": "string", "description": "The folder to unpack into (made if missing). Default: a folder named after the archive, beside it." } } })
}

#[derive(PartialEq, Debug)]
enum Kind {
    Zip,
    Tar,
    TarGz,
}

fn kind_of(path: &Path) -> Result<Kind, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if name.ends_with(".zip") {
        Ok(Kind::Zip)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Ok(Kind::TarGz)
    } else if name.ends_with(".tar") {
        Ok(Kind::Tar)
    } else {
        Err("Only .zip, .tar, .tar.gz and .tgz archives can be opened here.".into())
    }
}

/// An entry's place under the target, or None when it would leave it.
pub fn safe_inside(entry: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in Path::new(&entry.replace('\\', "/")).components() {
        match part {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn default_into(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = ["tar.gz", "tgz", "zip", "tar"].iter().find_map(|ext| name.strip_suffix(&format!(".{ext}"))).unwrap_or(&name).to_string();
    path.with_file_name(stem)
}

/// (name, size, folder) for each entry, up to LIST_MAX.
fn entries(path: &Path) -> Result<(Vec<(String, u64, bool)>, usize), String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut total = 0;
    match kind_of(path)? {
        Kind::Zip => {
            let mut z = zip::ZipArchive::new(f).map_err(|e| format!("Couldn't read the zip: {e}"))?;
            total = z.len();
            for i in 0..z.len().min(LIST_MAX) {
                let e = z.by_index(i).map_err(|e| e.to_string())?;
                out.push((e.name().map_err(|e| e.to_string())?.to_string(), e.size(), e.is_dir()));
            }
        }
        k => {
            let reader: Box<dyn Read> = if k == Kind::TarGz { Box::new(flate2::read::GzDecoder::new(f)) } else { Box::new(f) };
            let mut a = tar::Archive::new(reader);
            for e in a.entries().map_err(|e| e.to_string())? {
                let e = e.map_err(|e| e.to_string())?;
                total += 1;
                if out.len() < LIST_MAX {
                    let name = e.path().map(|p| p.display().to_string()).unwrap_or_default();
                    out.push((name, e.size(), e.header().entry_type().is_dir()));
                }
            }
        }
    }
    Ok((out, total))
}

fn unpack(path: &Path, into: &Path) -> Result<(usize, u64), String> {
    std::fs::create_dir_all(into).map_err(|e| e.to_string())?;
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let (mut files, mut bytes) = (0usize, 0u64);
    let mut write = |name: &str, dir: bool, reader: &mut dyn Read| -> Result<(), String> {
        let Some(rel) = safe_inside(name) else { return Err(format!("“{name}” would land outside the folder, so nothing more was unpacked.")) };
        let dest = into.join(rel);
        if dir {
            return std::fs::create_dir_all(&dest).map_err(|e| e.to_string());
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = std::fs::File::create(&dest).map_err(|e| e.to_string())?;
        let left = UNPACK_MAX_BYTES.saturating_sub(bytes);
        let n = std::io::copy(&mut reader.take(left + 1), &mut out).map_err(|e| e.to_string())?;
        bytes += n;
        files += 1;
        if bytes > UNPACK_MAX_BYTES || files > UNPACK_MAX_FILES {
            return Err("The archive unpacks to more than 20 GB or 50,000 files, so it stopped.".into());
        }
        Ok(())
    };
    match kind_of(path)? {
        Kind::Zip => {
            let mut z = zip::ZipArchive::new(f).map_err(|e| format!("Couldn't read the zip: {e}"))?;
            for i in 0..z.len() {
                let mut e = z.by_index(i).map_err(|e| e.to_string())?;
                let (name, dir) = (e.name().map_err(|e| e.to_string())?.to_string(), e.is_dir());
                write(&name, dir, &mut e)?;
            }
        }
        k => {
            let reader: Box<dyn Read> = if k == Kind::TarGz { Box::new(flate2::read::GzDecoder::new(f)) } else { Box::new(f) };
            let mut a = tar::Archive::new(reader);
            for e in a.entries().map_err(|e| e.to_string())? {
                let mut e = e.map_err(|e| e.to_string())?;
                let kind = e.header().entry_type();
                // Links could point anywhere; only files and folders are made.
                if !(kind.is_file() || kind.is_dir()) {
                    continue;
                }
                let name = e.path().map(|p| p.display().to_string()).map_err(|e| e.to_string())?;
                write(&name, kind.is_dir(), &mut e)?;
            }
        }
    }
    Ok((files, bytes))
}

fn into_of(sb: &Sandbox, args: &Value, path: &Path) -> Result<PathBuf, String> {
    match args.get("into").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
        Some(i) => sb.resolve(i),
        None => {
            let d = default_into(path);
            sb.resolve(&d.display().to_string())
        }
    }
}

pub fn preview(args: &Value, sb: &Sandbox) -> Result<Preview, String> {
    let path = sb.resolve(arg_str(args, "path")?)?;
    let into = into_of(sb, args, &path)?;
    let (list, total) = entries(&path)?;
    let detail = list.iter().take(30).map(|(n, _, _)| n.as_str()).collect::<Vec<_>>().join("\n");
    Ok(Preview { title: format!("Unpack {} into {}", sb.display(&path), sb.display(&into)), kind: "text", detail: Some(if total > 30 { format!("{detail}\n… and {} more", total - 30) } else { detail }), note: None })
}

pub fn run(name: &str, args: &Value, ctx: &Ctx<'_>) -> Outcome {
    let sb = ctx.sandbox;
    let result: Result<Outcome, String> = (|| match name {
        "list_archive" => {
            let path = sb.resolve_read(arg_str(args, "path")?)?;
            let (list, total) = entries(&path)?;
            let lines: Vec<String> = list.iter().map(|(n, size, dir)| if *dir { format!("{n} (folder)") } else { format!("{n} ({})", crate::size_label(*size)) }).collect();
            let more = if total > list.len() { format!("\n… and {} more", total - list.len()) } else { String::new() };
            Ok(Outcome::ok(format!("{} entries in {}:\n{}{more}", total, sb.display(&path), lines.join("\n")), format!("Looked inside {}", sb.display(&path)), "text", None))
        }
        "extract_archive" => {
            let path = sb.resolve(arg_str(args, "path")?)?;
            let into = into_of(sb, args, &path)?;
            let (files, bytes) = unpack(&path, &into)?;
            Ok(Outcome::ok(format!("Unpacked {files} files ({}) into {}.", crate::size_label(bytes), sb.display(&into)), format!("Unpacked {}", sb.display(&path)), "text", None))
        }
        _ => Err(format!("No tool {name}")),
    })();
    result.unwrap_or_else(|e| Outcome::error(super::failed_title(name, args), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_cant_leave_the_folder() {
        assert_eq!(safe_inside("docs/a.txt"), Some(PathBuf::from("docs").join("a.txt")));
        assert_eq!(safe_inside("./docs/a.txt"), Some(PathBuf::from("docs").join("a.txt")));
        assert_eq!(safe_inside("../evil.txt"), None);
        assert_eq!(safe_inside("docs/../../evil.txt"), None);
        assert_eq!(safe_inside("/etc/passwd"), None);
        assert_eq!(safe_inside("C:\\Windows\\x.dll"), None);
        assert_eq!(safe_inside(""), None);
    }

    #[test]
    fn a_zip_lists_and_unpacks_and_a_slip_stops() {
        let dir = std::env::temp_dir().join(format!("sulcusai-arch-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("pack.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            w.start_file("notes/a.txt", opts).unwrap();
            std::io::Write::write_all(&mut w, b"hello").unwrap();
            w.finish().unwrap();
        }
        let (list, total) = entries(&zip_path).unwrap();
        assert_eq!((total, list[0].0.as_str(), list[0].1), (1, "notes/a.txt", 5));
        assert_eq!(default_into(&zip_path), dir.join("pack"));
        let (files, bytes) = unpack(&zip_path, &dir.join("pack")).unwrap();
        assert_eq!((files, bytes), (1, 5));
        assert_eq!(std::fs::read_to_string(dir.join("pack").join("notes").join("a.txt")).unwrap(), "hello");

        let bad = dir.join("bad.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&bad).unwrap());
            w.start_file("../outside.txt", zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut w, b"x").unwrap();
            w.finish().unwrap();
        }
        assert!(unpack(&bad, &dir.join("bad")).is_err());
        assert!(!dir.join("outside.txt").exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
