// SPDX-License-Identifier: AGPL-3.0-only
//! Models already on this PC: files other AI apps downloaded (LM Studio,
//! Ollama, Hugging Face, Jan, GPT4All) or that are in the Downloads folder.
//! A file is only reused when its name or hash and its exact size match the
//! catalog, and its SHA-256 checks out. Reused files are hard links where
//! possible, so they take no extra space and removing a model here never
//! deletes the other app's copy.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Model file types worth indexing (Ollama's blobs have no extension).
const EXTENSIONS: &[&str] = &["gguf", "bin", "safetensors", "onnx", "pth"];
/// Enough for any app's folder layout without wandering the whole disk.
const MAX_FILES: usize = 20_000;

struct Source {
    app: &'static str,
    dir: PathBuf,
    depth: usize,
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn sources() -> Vec<Source> {
    let mut out = Vec::new();
    let mut add = |app, dir: Option<PathBuf>, depth| {
        if let Some(dir) = dir {
            out.push(Source { app, dir, depth });
        }
    };
    let home = env_dir("USERPROFILE").or_else(|| env_dir("HOME"));
    let under = |p: &str| home.as_ref().map(|h| h.join(p));
    add("LM Studio", under(".lmstudio/models"), 4);
    add("LM Studio", under(".cache/lm-studio/models"), 4);
    add("Hugging Face", env_dir("HF_HOME").map(|h| h.join("hub")).or_else(|| under(".cache/huggingface/hub")), 6);
    add("Ollama", env_dir("OLLAMA_MODELS").or_else(|| under(".ollama/models")).map(|d| d.join("blobs")), 1);
    add("Jan", under("jan/models"), 4);
    add("Jan", env_dir("APPDATA").map(|d| d.join("Jan/data/models")), 4);
    add("GPT4All", env_dir("LOCALAPPDATA").map(|d| d.join("nomic.ai/GPT4All")), 2);
    add("your Downloads folder", under("Downloads"), 2);
    out
}

struct Entry {
    app: &'static str,
    path: PathBuf,
    name: String,
    size: u64,
}

fn walk(app: &'static str, dir: &Path, depth: usize, out: &mut Vec<Entry>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        if out.len() >= MAX_FILES {
            return;
        }
        let Ok(md) = std::fs::metadata(e.path()) else { continue };
        let path = e.path();
        if md.is_dir() {
            if depth > 1 {
                walk(app, &path, depth - 1, out);
            }
            continue;
        }
        let name = e.file_name().to_string_lossy().to_lowercase();
        let wanted = name.starts_with("sha256-") || path.extension().is_some_and(|x| EXTENSIONS.iter().any(|w| x.eq_ignore_ascii_case(w)));
        if wanted && md.len() > 0 {
            out.push(Entry { app, path, name, size: md.len() });
        }
    }
}

fn index() -> Vec<Entry> {
    let mut out = Vec::new();
    for s in sources() {
        walk(s.app, &s.dir, s.depth, &mut out);
    }
    out
}

/// Another app's copy of this exact file, by name (or Ollama's hash name) and size.
fn find_in(list: &[Entry], dest: &Path, size: u64, sha256: &str) -> Option<(&'static str, PathBuf)> {
    let name = dest.file_name()?.to_string_lossy().to_lowercase();
    let blob = format!("sha256-{}", sha256.to_lowercase());
    list.iter().find(|e| e.size == size && (e.name == name || e.name == blob)).map(|e| (e.app, e.path.clone()))
}

pub fn local_copy(dest: &Path, size: u64, sha256: &str) -> Option<(&'static str, PathBuf)> {
    find_in(&index(), dest, size, sha256)
}

fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(hex::encode(hasher.finalize()));
        }
        hasher.update(&buf[..n]);
    }
}

#[derive(Debug, PartialEq)]
pub enum Place {
    /// Already in the app's models folder.
    Here,
    /// Linked in from another app's folder.
    From(&'static str),
    Missing,
}

/// The activity log line for a model found on this PC.
pub fn note(name: &str, place: &Place) -> String {
    match place {
        Place::From(app) => format!("Found {name} in {app} and added it without downloading (the file is shared, so it takes no extra space)"),
        _ => format!("Found {name} already downloaded and added it"),
    }
}

/// Looks for model files, reading the other apps' folders once.
#[derive(Default)]
pub struct Finder {
    list: Option<Vec<Entry>>,
}

impl Finder {
    pub fn new() -> Self {
        Finder { list: None }
    }

    /// Puts the file at `dest` if it's here already or another app has it on
    /// the same drive. Files in the app's own folder were hash-checked when
    /// they were downloaded, so their size is enough; linked files are
    /// checked now.
    pub fn place(&mut self, dest: &Path, size: u64, sha256: &str) -> Place {
        if dest.metadata().is_ok_and(|m| m.len() == size) {
            return Place::Here;
        }
        let list = self.list.get_or_insert_with(index);
        let Some((app, src)) = find_in(list, dest, size, sha256) else { return Place::Missing };
        let src = dunce::canonicalize(&src).unwrap_or(src);
        let Some(dir) = dest.parent() else { return Place::Missing };
        if std::fs::create_dir_all(dir).is_err() {
            return Place::Missing;
        }
        // Link under a temporary name and check it before it counts. A link
        // fails across drives; those are copied when the model is installed.
        let mut tmp = dest.file_name().unwrap_or_default().to_os_string();
        tmp.push(".found");
        let tmp = dest.with_file_name(tmp);
        std::fs::remove_file(&tmp).ok();
        if std::fs::hard_link(&src, &tmp).is_err() {
            return Place::Missing;
        }
        let ok = hash_file(&tmp).is_ok_and(|h| h.eq_ignore_ascii_case(sha256)) && std::fs::rename(&tmp, dest).is_ok();
        if ok {
            Place::From(app)
        } else {
            std::fs::remove_file(&tmp).ok();
            Place::Missing
        }
    }
}

/// Copies a local file into `part` (used for a download), checking its hash
/// on the way. Returns false, leaving nothing behind, if it doesn't match.
pub async fn copy_verified(
    src: &Path,
    part: &Path,
    size: u64,
    sha256: &str,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(u64, u64),
) -> Result<bool, String> {
    let copied = async {
        let mut from = tokio::fs::File::open(src).await?;
        let mut to = tokio::fs::File::create(part).await?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        let mut have = 0u64;
        let mut last = std::time::Instant::now();
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(std::io::Error::other(crate::download::CANCELLED));
            }
            let n = from.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            to.write_all(&buf[..n]).await?;
            have += n as u64;
            if last.elapsed().as_millis() >= 250 {
                progress(have, size);
                last = std::time::Instant::now();
            }
        }
        to.flush().await?;
        Ok(have == size && hex::encode(hasher.finalize()).eq_ignore_ascii_case(sha256))
    }
    .await;
    match copied {
        Ok(true) => Ok(true),
        Err(e) if e.to_string() == crate::download::CANCELLED => Err(crate::download::CANCELLED.into()),
        _ => {
            tokio::fs::remove_file(part).await.ok();
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(app: &'static str, name: &str, size: u64) -> Entry {
        Entry { app, path: PathBuf::from(name), name: name.to_lowercase(), size }
    }

    #[test]
    fn matches_by_name_and_size_or_by_ollama_hash() {
        let list = vec![
            entry("LM Studio", "Qwen3-4B-Q4_K_M.gguf", 100),
            entry("LM Studio", "Qwen3-1.7B-Q4_K_M.gguf", 999),
            entry("Ollama", "sha256-abc123", 50),
        ];
        let dest = Path::new("models/qwen3-4b/Qwen3-4B-Q4_K_M.gguf");
        assert_eq!(find_in(&list, dest, 100, "ff").map(|f| f.0), Some("LM Studio"));
        // Same name, different size: a different file.
        assert!(find_in(&list, Path::new("x/Qwen3-1.7B-Q4_K_M.gguf"), 1000, "ff").is_none());
        // Ollama names its files by hash.
        assert_eq!(find_in(&list, Path::new("x/whatever.gguf"), 50, "ABC123").map(|f| f.0), Some("Ollama"));
    }

    #[test]
    fn links_a_matching_file_and_rejects_a_wrong_one() {
        let root = std::env::temp_dir().join(format!("sulcusai-found-{}", uuid::Uuid::new_v4()));
        let other = root.join("other");
        std::fs::create_dir_all(&other).unwrap();
        let src = other.join("model.gguf");
        std::fs::write(&src, b"weights").unwrap();
        let sha = hex::encode(Sha256::digest(b"weights"));
        let mut f = Finder { list: Some(vec![Entry { app: "LM Studio", path: src.clone(), name: "model.gguf".into(), size: 7 }]) };

        let dest = root.join("models/m/model.gguf");
        assert_eq!(f.place(&dest, 7, &sha), Place::From("LM Studio"));
        assert_eq!(std::fs::read(&dest).unwrap(), b"weights");
        assert_eq!(f.place(&dest, 7, &sha), Place::Here);
        // Removing ours leaves theirs.
        std::fs::remove_file(&dest).unwrap();
        assert!(src.exists());

        let wrong = root.join("models/w/model.gguf");
        assert_eq!(f.place(&wrong, 7, "00"), Place::Missing);
        assert!(!wrong.exists() && !wrong.with_file_name("model.gguf.found").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn copies_only_a_file_whose_hash_matches() {
        let root = std::env::temp_dir().join(format!("sulcusai-copy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let src = root.join("model.gguf");
        std::fs::write(&src, b"weights").unwrap();
        let sha = hex::encode(Sha256::digest(b"weights"));
        let part = root.join("dest.gguf.part");
        let cancel = AtomicBool::new(false);
        assert!(copy_verified(&src, &part, 7, &sha, &cancel, &mut |_, _| {}).await.unwrap());
        assert_eq!(std::fs::read(&part).unwrap(), b"weights");
        std::fs::remove_file(&part).unwrap();
        assert!(!copy_verified(&src, &part, 7, "00", &cancel, &mut |_, _| {}).await.unwrap());
        assert!(!part.exists());
        std::fs::remove_dir_all(&root).ok();
    }
}
