//! The player's files, served in byte ranges over the `sulcusmedia` scheme so
//! a film plays without first being read into memory. Only files handed to
//! the player get a token; nothing else on disk is reachable.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Mutex;

use tauri::http::{header, Request, Response, StatusCode};

pub const SCHEME: &str = "sulcusmedia";
const MOST: u64 = 4 * 1024 * 1024;

static OPEN: Mutex<Option<HashMap<String, PathBuf>>> = Mutex::new(None);

/// The address the player's video element loads the file from.
pub fn share(path: PathBuf) -> String {
    let token = uuid::Uuid::new_v4().simple().to_string();
    let mut open = OPEN.lock().unwrap();
    let map = open.get_or_insert_with(HashMap::new);
    // A player shows one file at a time; a few left from earlier views are harmless.
    if map.len() > 16 {
        map.clear();
    }
    map.insert(token.clone(), path);
    if cfg!(windows) { format!("http://{SCHEME}.localhost/{token}") } else { format!("{SCHEME}://localhost/{token}") }
}

/// The bytes asked for in a `Range: bytes=a-b` header, clamped to the file.
pub fn range(h: Option<&str>, len: u64) -> Option<(u64, u64)> {
    if len == 0 {
        return None;
    }
    let spec = h?.trim().strip_prefix("bytes=")?.split(',').next()?.trim();
    let (a, b) = spec.split_once('-')?;
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        (len.saturating_sub(n), len - 1)
    } else {
        let s: u64 = a.parse().ok()?;
        let e = if b.is_empty() { len - 1 } else { b.parse::<u64>().ok()?.min(len - 1) };
        (s, e)
    };
    (start <= end && start < len).then_some((start, end.min(start + MOST - 1)))
}

fn mime(p: &std::path::Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("mov") => "video/quicktime",
        Some("mp3") => "audio/mpeg",
        Some("m4a" | "aac") => "audio/mp4",
        Some("wav") => "audio/wav",
        Some("flac") => "audio/flac",
        Some("ogg") => "audio/ogg",
        Some("opus") => "audio/opus",
        _ => "application/octet-stream",
    }
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder().status(code).body(Vec::new()).unwrap()
}

pub fn serve(req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let token = req.uri().path().trim_start_matches('/');
    let Some(path) = OPEN.lock().unwrap().as_ref().and_then(|m| m.get(token).cloned()) else { return status(StatusCode::NOT_FOUND) };
    let Ok(mut f) = std::fs::File::open(&path) else { return status(StatusCode::NOT_FOUND) };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let asked = req.headers().get(header::RANGE).and_then(|v| v.to_str().ok());
    let (start, end, code) = match range(asked, len) {
        Some((a, b)) => (a, b, StatusCode::PARTIAL_CONTENT),
        None if asked.is_some() => return status(StatusCode::RANGE_NOT_SATISFIABLE),
        None => (0, len.min(MOST).saturating_sub(1), if len > MOST { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK }),
    };
    let mut buf = vec![0u8; if len == 0 { 0 } else { (end - start + 1) as usize }];
    if f.seek(SeekFrom::Start(start)).and_then(|_| f.read_exact(&mut buf)).is_err() {
        return status(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let mut r = Response::builder()
        .status(code)
        .header(header::CONTENT_TYPE, mime(&path))
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, buf.len().to_string());
    if code == StatusCode::PARTIAL_CONTENT {
        r = r.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"));
    }
    r.body(buf).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(range(Some("bytes=0-"), 100), Some((0, 99)));
        assert_eq!(range(Some("bytes=10-19"), 100), Some((10, 19)));
        assert_eq!(range(Some("bytes=-10"), 100), Some((90, 99)));
        assert_eq!(range(Some("bytes=50-500"), 100), Some((50, 99)));
        assert_eq!(range(Some("bytes=100-"), 100), None);
        assert_eq!(range(Some("bytes=0-"), 10 * MOST), Some((0, MOST - 1)));
        assert_eq!(range(None, 100), None);
    }

    #[test]
    fn only_shared_files_are_served() {
        let dir = std::env::temp_dir().join(format!("sulcus-stream-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.mp3");
        std::fs::write(&f, b"0123456789").unwrap();
        let url = share(f);
        let token = url.rsplit('/').next().unwrap();
        let req = Request::builder().uri(format!("{SCHEME}://localhost/{token}")).header("Range", "bytes=2-4").body(Vec::new()).unwrap();
        let r = serve(&req);
        assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(r.body(), b"234");
        let other = Request::builder().uri(format!("{SCHEME}://localhost/nope")).body(Vec::new()).unwrap();
        assert_eq!(serve(&other).status(), StatusCode::NOT_FOUND);
        std::fs::remove_dir_all(&dir).ok();
    }
}
