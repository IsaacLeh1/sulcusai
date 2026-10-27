// SPDX-License-Identifier: AGPL-3.0-only
//! Resumable downloads that are checked against a SHA-256 hash before use.
//! Big files come over several connections at once, a piece on each, and
//! are still written and hashed in order, so a stopped download carries on
//! from what's on the disk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const CANCELLED: &str = "cancelled";
const TOO_BIG: &str = "Download is larger than expected; it was discarded.";

/// Free space a download always leaves on its drive, so Windows and other
/// apps (game downloads, updates) can still write.
const KEEP_FREE: u64 = 2 * 1024 * 1024 * 1024;
/// No data for this long: the connection is stuck (a dropped Wi-Fi, a
/// server that stopped sending), and the download stops so it can resume.
const STALL: Duration = Duration::from_secs(60);
/// How often a waiting download checks whether it was stopped.
const TICK: Duration = Duration::from_secs(1);

/// How a big download is split: files of `from` bytes or more come in
/// pieces of `piece` bytes over `connections` connections at once, since
/// one connection is often held to a share of the line (a server far away,
/// or one that limits each connection). Pieces are written in order, so at
/// most `ahead` of them wait in memory for an earlier, slower one.
#[derive(Clone, Copy)]
struct Split {
    from: u64,
    piece: u64,
    connections: usize,
    ahead: u64,
}

const SPLIT: Split = Split { from: 64 << 20, piece: 16 << 20, connections: 8, ahead: 12 };
/// Tries for one piece before the download stops (it carries on later
/// from what's written).
const PIECE_TRIES: u32 = 3;

/// Waits for `fut`, noticing a stop within a second, and giving up after
/// `STALL` without a result.
async fn waiting<T>(fut: impl std::future::Future<Output = T>, cancel: &AtomicBool) -> Result<T, String> {
    tokio::pin!(fut);
    let started = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(CANCELLED.into());
        }
        match tokio::time::timeout(TICK, &mut fut).await {
            Ok(v) => return Ok(v),
            Err(_) if started.elapsed() >= STALL => {
                return Err("The download stalled: nothing arrived for a minute. Check the connection, then try again; it carries on from where it stopped.".into())
            }
            Err(_) => {}
        }
    }
}

/// Why a download of `more` bytes into `dir` mustn't go on, if so.
fn too_full(dir: &Path, more: u64) -> Option<String> {
    let free = crate::storage::free_on(dir)?;
    (free < more.saturating_add(KEEP_FREE)).then(|| {
        let gb = |b: u64| b as f64 / (1024.0 * 1024.0 * 1024.0);
        format!(
            "Not enough free space: this needs {:.1} GB more and {:.1} GB is free (SulcusAI always leaves 2 GB free so your PC keeps working). Make room, then try again; what's downloaded so far is kept.",
            gb(more),
            gb(free)
        )
    })
}

pub(crate) fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// Which of `files` are on this PC already (before an install starts, so a
/// cancel deletes only what it downloaded).
pub(crate) fn existing(files: &[PathBuf]) -> Vec<PathBuf> {
    files.iter().filter(|f| f.exists()).cloned().collect()
}

/// After a cancelled install: deletes `files` that weren't here before
/// (`had`) and every half-downloaded part, then folders left empty, up to
/// `root`.
pub(crate) fn discard_new(files: &[PathBuf], had: &[PathBuf], root: &Path) {
    for f in files {
        if !had.contains(f) {
            std::fs::remove_file(f).ok();
        }
        std::fs::remove_file(part_path(f)).ok();
        let mut dir = f.parent();
        while let Some(d) = dir.filter(|d| d.starts_with(root) && *d != root) {
            if std::fs::remove_dir(d).is_err() {
                break;
            }
            dir = d.parent();
        }
    }
}

/// Re-hashes what an earlier, interrupted download already saved.
async fn hash_existing(path: &Path, hasher: &mut Sha256, cancel: &AtomicBool) -> std::io::Result<u64> {
    let mut f = tokio::fs::File::open(path).await?;
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::other(CANCELLED));
        }
        let n = f.read(&mut buf).await?;
        if n == 0 {
            return Ok(total);
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
}

/// Downloads `url` to `dest`. Progress is reported as (received, total).
/// A cancelled download keeps its `.part` file so it can resume later.
pub async fn fetch_verified(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    size: u64,
    sha256: &str,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<(), String> {
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(|e| e.to_string())?;
    }
    let name = dest.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let result = fetch_verified_inner(client, url, dest, size, sha256, cancel, &mut progress, SPLIT).await;
    match &result {
        Ok(()) => crate::applog::line("download", &format!("{name} ({} MB) is ready", size / 1_000_000)),
        Err(e) if e == CANCELLED => crate::applog::line("download", &format!("{name}: stopped")),
        Err(e) => crate::applog::line("download", &format!("{name} from {}: failed: {e}", crate::host_of(url))),
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn fetch_verified_inner(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    size: u64,
    sha256: &str,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(u64, u64),
    how: Split,
) -> Result<(), String> {
    let mut progress = progress;
    let part = part_path(dest);
    // Another app on this PC has the same file: link it, or copy it from
    // another drive, instead of downloading it again.
    if tokio::fs::metadata(dest).await.is_err() && tokio::fs::metadata(&part).await.is_err() {
        let (d, sha) = (dest.to_path_buf(), sha256.to_string());
        let found = tokio::task::spawn_blocking(move || crate::found::local_copy(&d, size, &sha)).await.ok().flatten();
        if let Some((_, src)) = found {
            if std::fs::hard_link(&src, dest).is_err() {
                crate::found::copy_verified(&src, &part, size, sha256, cancel, &mut progress).await?;
            }
        }
    }
    // Already downloaded (for example a reinstall): reuse it if it checks out.
    if tokio::fs::metadata(dest).await.is_ok_and(|m| m.len() == size) {
        let mut hasher = Sha256::new();
        hash_existing(dest, &mut hasher, cancel).await.map_err(|e| e.to_string())?;
        if hex::encode(hasher.finalize()).eq_ignore_ascii_case(sha256) {
            progress(size, size);
            return Ok(());
        }
    }
    let mut hasher = Sha256::new();
    let mut have = match tokio::fs::metadata(&part).await {
        Ok(m) if m.len() <= size => hash_existing(&part, &mut hasher, cancel).await.map_err(|e| e.to_string())?,
        _ => 0,
    };

    if have < size {
        crate::offline::check(crate::offline::Keep::Downloads)?;
        // A big file: the first piece alone, which shows whether the server
        // hands out pieces.
        let first_end = (size - have >= how.from).then(|| (have + how.piece).min(size));
        let mut req = client.get(url);
        if let Some(end) = first_end {
            req = req.header(RANGE, format!("bytes={have}-{}", end - 1));
        } else if have > 0 {
            req = req.header(RANGE, format!("bytes={have}-"));
        }
        let resp = waiting(req.send(), cancel).await?.map_err(|e| format!("Download failed: {e}"))?;
        let resumed = resp.status() == StatusCode::PARTIAL_CONTENT && resp.headers().contains_key(CONTENT_RANGE);
        if !resp.status().is_success() {
            return Err(format!("Download failed: the server answered {}", resp.status()));
        }
        if !resumed && have > 0 {
            // Server ignored the range request: start over.
            have = 0;
            hasher = Sha256::new();
        }

        let dir = dest.parent().unwrap_or(dest).to_path_buf();
        if let Some(why) = too_full(&dir, size - have) {
            return Err(why);
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(have > 0)
            .truncate(have == 0)
            .open(&part)
            .await
            .map_err(|e| e.to_string())?;

        progress(have, size);
        let got = match first_end {
            Some(end) if resumed => in_pieces(client, url, resp, end, &mut file, &mut hasher, &mut have, size, &dir, cancel, progress, how).await,
            _ => in_one(resp, &mut file, &mut hasher, &mut have, size, &dir, cancel, progress).await,
        };
        if let Err(e) = got {
            // Stopped, stuck or out of room: what arrived in order is kept
            // for the next try.
            file.flush().await.ok();
            if e == TOO_BIG {
                drop(file);
                tokio::fs::remove_file(&part).await.ok();
            }
            return Err(e);
        }
        file.flush().await.map_err(|e| e.to_string())?;
        // On the disk for real before it's used, so a crash or power cut
        // can't leave a model that passed its check but is cut short.
        file.sync_all().await.map_err(|e| e.to_string())?;
        progress(have, size);
    }

    if have != size {
        return Err(format!("Download ended early ({have} of {size} bytes). Try again to resume."));
    }
    let digest = hex::encode(hasher.finalize());
    if !digest.eq_ignore_ascii_case(sha256) {
        tokio::fs::remove_file(&part).await.ok();
        return Err("The downloaded file failed its integrity check and was deleted.".into());
    }
    tokio::fs::rename(&part, dest).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// The rest of a download over one connection.
#[allow(clippy::too_many_arguments)]
async fn in_one(
    resp: reqwest::Response,
    file: &mut tokio::fs::File,
    hasher: &mut Sha256,
    have: &mut u64,
    size: u64,
    dir: &Path,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(u64, u64),
) -> Result<(), String> {
    let mut stream = resp.bytes_stream();
    let mut last = Instant::now();
    let mut checked = Instant::now();
    while let Some(chunk) = waiting(stream.next(), cancel).await? {
        // Something else filling the disk meanwhile: pause (it resumes later).
        if checked.elapsed() >= Duration::from_secs(5) {
            checked = Instant::now();
            if let Some(why) = too_full(dir, size - *have) {
                return Err(why);
            }
        }
        let chunk = chunk.map_err(|e| format!("Download interrupted: {e}"))?;
        *have += chunk.len() as u64;
        if *have > size {
            return Err(TOO_BIG.into());
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        if last.elapsed() >= Duration::from_millis(250) {
            progress(*have, size);
            last = Instant::now();
        }
    }
    Ok(())
}

/// The rest of a big download over several connections, a piece on each,
/// written and hashed in order. `first` is the server's answer for the
/// first piece, which ends at `first_end`.
#[allow(clippy::too_many_arguments)]
async fn in_pieces(
    client: &reqwest::Client,
    url: &str,
    first: reqwest::Response,
    first_end: u64,
    file: &mut tokio::fs::File,
    hasher: &mut Sha256,
    have: &mut u64,
    size: u64,
    dir: &Path,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(u64, u64),
    how: Split,
) -> Result<(), String> {
    // Later pieces go straight to where the first one came from (Hugging
    // Face sends downloads on to its file servers). A piece that fails is
    // asked for at `url` again, in case that link ran out.
    let direct = first.url().to_string();
    let pieces = Pieces { client, size, cancel, received: AtomicU64::new(*have) };
    let mut coming = FuturesUnordered::new();
    coming.push(pieces.get(url, Some(first), *have, first_end, 1));
    let mut next = first_end;
    let mut ready = BTreeMap::new();
    let mut last = Instant::now();
    let mut checked = Instant::now();
    loop {
        while coming.len() < how.connections && next < size && next < *have + how.ahead * how.piece {
            let end = (next + how.piece).min(size);
            coming.push(pieces.get(&direct, None, next, end, 1));
            next = end;
        }
        let Some((start, tries, got)) = coming.next().await else { break };
        match got {
            Ok(data) => {
                ready.insert(start, data);
            }
            Err(e) if e == CANCELLED || tries >= PIECE_TRIES => return Err(e),
            Err(_) => coming.push(pieces.get(url, None, start, (start + how.piece).min(size), tries + 1)),
        }
        while let Some(data) = ready.remove(&*have) {
            hasher.update(&data);
            file.write_all(&data).await.map_err(|e| e.to_string())?;
            *have += data.len() as u64;
        }
        if last.elapsed() >= Duration::from_millis(250) {
            progress(pieces.received.load(Ordering::Relaxed), size);
            last = Instant::now();
        }
        // Something else filling the disk meanwhile: pause (it resumes later).
        if checked.elapsed() >= Duration::from_secs(5) {
            checked = Instant::now();
            if let Some(why) = too_full(dir, size - *have) {
                return Err(why);
            }
        }
    }
    Ok(())
}

/// What the pieces of one download share.
struct Pieces<'a> {
    client: &'a reqwest::Client,
    size: u64,
    cancel: &'a AtomicBool,
    /// What's arrived, counting pieces still coming (for progress).
    received: AtomicU64,
}

impl Pieces<'_> {
    /// Bytes `start..end` of the file, from `answer` when they were asked
    /// for already. Gives back where they start and which try this was.
    async fn get(&self, url: &str, answer: Option<reqwest::Response>, start: u64, end: u64, tries: u32) -> (u64, u32, Result<Vec<u8>, String>) {
        let mut got = 0u64;
        let result = async {
            if tries > 1 {
                waiting(tokio::time::sleep(Duration::from_secs(2 * u64::from(tries - 1))), self.cancel).await?;
            }
            let resp = match answer {
                Some(resp) => resp,
                None => waiting(self.client.get(url).header(RANGE, format!("bytes={start}-{}", end - 1)).send(), self.cancel)
                    .await?
                    .map_err(|e| format!("Download failed: {e}"))?,
            };
            if resp.status() != StatusCode::PARTIAL_CONTENT {
                return Err(format!("Download failed: the server answered {}", resp.status()));
            }
            if content_range(&resp) != Some((start, self.size)) {
                return Err("Download failed: the server sent another part of the file, or a file of another size.".into());
            }
            let want = (end - start) as usize;
            let mut data = Vec::with_capacity(want);
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = waiting(stream.next(), self.cancel).await? {
                let chunk = chunk.map_err(|e| format!("Download interrupted: {e}"))?;
                if data.len() + chunk.len() > want {
                    return Err("Download failed: a piece came back larger than asked for.".into());
                }
                data.extend_from_slice(&chunk);
                got += chunk.len() as u64;
                self.received.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            }
            if data.len() != want {
                return Err("Download interrupted: a piece came back cut short.".into());
            }
            Ok(data)
        }
        .await;
        if result.is_err() {
            self.received.fetch_sub(got, Ordering::Relaxed);
        }
        (start, tries, result)
    }
}

/// Where a Content-Range answer starts and the whole file's size
/// ("bytes 0-99/1000").
fn content_range(resp: &reqwest::Response) -> Option<(u64, u64)> {
    let v = resp.headers().get(CONTENT_RANGE)?.to_str().ok()?;
    let (span, total) = v.strip_prefix("bytes ")?.split_once('/')?;
    Some((span.split_once('-')?.0.trim().parse().ok()?, total.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A download stuck waiting for data notices a stop within a second,
    /// instead of only when the next piece arrives (never, when stuck).
    #[tokio::test]
    async fn a_stuck_download_still_stops() {
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        assert_eq!(waiting(async { 7 }, &cancel).await, Ok(7));
        let stop = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            stop.store(true, Ordering::Relaxed);
        });
        let started = Instant::now();
        let stuck = waiting(std::future::pending::<()>(), &cancel).await;
        assert_eq!(stuck, Err(CANCELLED.to_string()));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    /// A cancel deletes what that install downloaded, finished or half done,
    /// and the folders it leaves empty, but never a file that was here before.
    #[test]
    fn a_cancel_deletes_only_what_it_downloaded() {
        let root = std::env::temp_dir().join(format!("sulcusai-discard-{}", uuid::Uuid::new_v4()));
        let dir = root.join("model").join("onnx");
        std::fs::create_dir_all(&dir).unwrap();
        let (before, done, half) = (dir.join("before.bin"), dir.join("done.bin"), dir.join("half.bin"));
        std::fs::write(&before, b"x").unwrap();
        let files = vec![before.clone(), done.clone(), half.clone()];
        let had = existing(&files);
        assert_eq!(had, vec![before.clone()]);
        std::fs::write(&done, b"x").unwrap();
        std::fs::write(part_path(&half), b"x").unwrap();
        discard_new(&files, &had, &root);
        assert!(before.exists(), "a file from before stays");
        assert!(!done.exists() && !part_path(&half).exists());

        // Nothing from before: the empty folders go too, but not the root.
        std::fs::remove_file(&before).unwrap();
        std::fs::write(&done, b"x").unwrap();
        discard_new(&files, &[], &root);
        assert!(!root.join("model").exists());
        assert!(root.exists());
        std::fs::remove_dir_all(&root).ok();
    }

    /// A server for `data` at /f: it hands out ranges unless `whole_only`,
    /// and fails the first request for the range starting at `fail_at`.
    /// Gives its address and how many requests it had.
    async fn serve(data: Arc<Vec<u8>>, whole_only: bool, fail_at: Option<u64>) -> (String, Arc<AtomicU64>) {
        use axum::http::{header, HeaderMap, StatusCode as Status};
        use axum::response::IntoResponse;
        let asked = Arc::new(AtomicU64::new(0));
        let failed = Arc::new(AtomicBool::new(false));
        let count = asked.clone();
        let app = axum::Router::new().route(
            "/f",
            axum::routing::get(move |h: HeaderMap| {
                let (data, count, failed) = (data.clone(), count.clone(), failed.clone());
                async move {
                    count.fetch_add(1, Ordering::Relaxed);
                    let last = data.len() as u64 - 1;
                    let range = h
                        .get(header::RANGE)
                        .and_then(|v| v.to_str().ok()?.strip_prefix("bytes=")?.split_once('-').map(|(a, b)| (a.to_string(), b.to_string())))
                        .map(|(a, b)| (a.parse::<u64>().unwrap(), b.parse::<u64>().unwrap_or(last).min(last)));
                    match range {
                        Some((a, _)) if Some(a) == fail_at && !failed.swap(true, Ordering::Relaxed) => Status::SERVICE_UNAVAILABLE.into_response(),
                        Some((a, b)) if !whole_only => {
                            (Status::PARTIAL_CONTENT, [(header::CONTENT_RANGE, format!("bytes {a}-{b}/{}", data.len()))], data[a as usize..=b as usize].to_vec()).into_response()
                        }
                        _ => data.to_vec().into_response(),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.ok() });
        (format!("http://127.0.0.1:{port}/f"), asked)
    }

    /// Big files come in pieces over several connections, written in order
    /// (a piece that fails is asked for again), carry on from what an
    /// earlier try saved, and still come whole from a server that won't
    /// hand out pieces.
    #[tokio::test]
    async fn big_files_come_in_pieces() {
        let data: Arc<Vec<u8>> = Arc::new((0..5_000_123u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect());
        let (size, sha) = (data.len() as u64, hex::encode(Sha256::digest(&*data)));
        let how = Split { from: 1 << 20, piece: 256 << 10, connections: 4, ahead: 6 };
        let client = reqwest::Client::builder().http1_only().no_proxy().build().unwrap();
        let dir = std::env::temp_dir().join(format!("sulcusai-pieces-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let no = AtomicBool::new(false);

        let (url, asked) = serve(data.clone(), false, Some(3 * (256 << 10))).await;
        let dest = dir.join("a.bin");
        let mut seen = 0;
        fetch_verified_inner(&client, &url, &dest, size, &sha, &no, &mut |r, _| seen = seen.max(r), how).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), *data);
        assert_eq!(asked.load(Ordering::Relaxed), 21, "20 pieces, one of them twice");
        assert_eq!(seen, size);

        let dest = dir.join("b.bin");
        std::fs::write(part_path(&dest), &data[..1_234_567]).unwrap();
        let before = asked.load(Ordering::Relaxed);
        fetch_verified_inner(&client, &url, &dest, size, &sha, &no, &mut |_, _| {}, how).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), *data);
        assert_eq!(asked.load(Ordering::Relaxed) - before, 15, "only what was missing");

        let (url, asked) = serve(data.clone(), true, None).await;
        let dest = dir.join("c.bin");
        fetch_verified_inner(&client, &url, &dest, size, &sha, &no, &mut |_, _| {}, how).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), *data);
        assert_eq!(asked.load(Ordering::Relaxed), 1);

        // A file under `from` is one request.
        let (url, asked) = serve(data.clone(), false, None).await;
        let dest = dir.join("d.bin");
        fetch_verified_inner(&client, &url, &dest, size, &sha, &no, &mut |_, _| {}, Split { from: 8 << 20, ..how }).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), *data);
        assert_eq!(asked.load(Ordering::Relaxed), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// How fast a real file comes: on one connection the way downloads
    /// used to (HTTP/2 allowed), on one over HTTP/1.1, and in pieces.
    /// SULCUSAI_TEST_DOWNLOAD: a big file's address on Hugging Face (its
    /// size and SHA-256 come from the answer's headers).
    #[tokio::test]
    #[ignore]
    async fn real_download_speeds() {
        let url = std::env::var("SULCUSAI_TEST_DOWNLOAD").expect("SULCUSAI_TEST_DOWNLOAD: a big file's address on Hugging Face");
        let plain = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let head = plain.head(&url).send().await.unwrap();
        let header = |n: &str| head.headers().get(n).and_then(|v| v.to_str().ok()).unwrap().trim_matches('"').to_string();
        let (size, sha) = (header("x-linked-size").parse::<u64>().unwrap(), header("x-linked-etag"));
        let dir = std::env::temp_dir().join(format!("sulcusai-speed-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let either = reqwest::Client::builder().build().unwrap();
        let http1 = reqwest::Client::builder().http1_only().build().unwrap();
        let one = Split { from: u64::MAX, ..SPLIT };
        for (name, client, how) in [("one connection, HTTP/2 allowed", &either, one), ("one connection, HTTP/1.1", &http1, one), ("8 connections", &http1, SPLIT)] {
            let dest = dir.join("file");
            let started = Instant::now();
            fetch_verified_inner(client, &url, &dest, size, &sha, &AtomicBool::new(false), &mut |_, _| {}, how).await.unwrap();
            let s = started.elapsed().as_secs_f64();
            println!("{name}: {:.1} MB/s ({s:.1} s for {} MB)", size as f64 / s / 1e6, size / 1_000_000);
            std::fs::remove_file(&dest).unwrap();
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
