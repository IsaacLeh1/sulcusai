// SPDX-License-Identifier: AGPL-3.0-only
//! Resumable downloads that are checked against a SHA-256 hash before use.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const CANCELLED: &str = "cancelled";

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
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
        let mut req = client.get(url);
        if have > 0 {
            req = req.header(RANGE, format!("bytes={have}-"));
        }
        let resp = req.send().await.map_err(|e| format!("Download failed: {e}"))?;
        let resumed = resp.status() == StatusCode::PARTIAL_CONTENT && resp.headers().contains_key(CONTENT_RANGE);
        if !resp.status().is_success() {
            return Err(format!("Download failed: the server answered {}", resp.status()));
        }
        if !resumed && have > 0 {
            // Server ignored the range request: start over.
            have = 0;
            hasher = Sha256::new();
        }

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(resumed)
            .truncate(!resumed)
            .open(&part)
            .await
            .map_err(|e| e.to_string())?;

        let mut stream = resp.bytes_stream();
        let mut last = Instant::now();
        progress(have, size);
        while let Some(chunk) = stream.next().await {
            if cancel.load(Ordering::Relaxed) {
                file.flush().await.ok();
                return Err(CANCELLED.into());
            }
            let chunk = chunk.map_err(|e| format!("Download interrupted: {e}"))?;
            have += chunk.len() as u64;
            if have > size {
                drop(file);
                tokio::fs::remove_file(&part).await.ok();
                return Err("Download is larger than expected; it was discarded.".into());
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(|e| e.to_string())?;
            if last.elapsed() >= Duration::from_millis(250) {
                progress(have, size);
                last = Instant::now();
            }
        }
        file.flush().await.map_err(|e| e.to_string())?;
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
