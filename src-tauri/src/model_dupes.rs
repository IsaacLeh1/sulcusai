// SPDX-License-Identifier: AGPL-3.0-only
//! Model files installed twice (the same download added from two places, or
//! a copy imported again): found by size and the hash of their first and
//! last few megabytes, which tells different models apart without reading
//! gigabytes. Only shown; removing one goes through the normal model removal.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::AppStateRef;

const SAMPLE: u64 = 4 * 1024 * 1024;

/// The size and a hash of the file's start and end.
pub fn fingerprint(path: &std::path::Path) -> std::io::Result<(u64, String)> {
    let mut f = std::fs::File::open(path)?;
    let size = f.metadata()?.len();
    let mut h = Sha256::new();
    let mut buf = vec![0u8; SAMPLE.min(size) as usize];
    f.read_exact(&mut buf)?;
    h.update(&buf);
    if size > SAMPLE {
        let tail = SAMPLE.min(size - SAMPLE);
        f.seek(SeekFrom::End(-(tail as i64)))?;
        let mut end = vec![0u8; tail as usize];
        f.read_exact(&mut end)?;
        h.update(&end);
    }
    h.update(size.to_le_bytes());
    Ok((size, hex::encode(&h.finalize()[..16])))
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Duplicate {
    /// Model ids that are the same file.
    pub models: Vec<String>,
    pub paths: Vec<String>,
    /// The size of one copy: what removing the others frees.
    pub size: u64,
}

/// Groups of entries whose (size, hash) match, biggest first.
pub fn groups(entries: Vec<(String, String, u64, String)>) -> Vec<Duplicate> {
    let mut by: HashMap<(u64, String), Vec<(String, String)>> = HashMap::new();
    for (id, path, size, hash) in entries {
        by.entry((size, hash)).or_default().push((id, path));
    }
    let mut out: Vec<Duplicate> = by
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|((size, _), v)| Duplicate { models: v.iter().map(|(i, _)| i.clone()).collect(), paths: v.into_iter().map(|(_, p)| p).collect(), size })
        .collect();
    out.sort_by(|a, b| b.size.cmp(&a.size));
    out
}

#[tauri::command]
pub async fn duplicate_models(state: AppStateRef<'_>) -> Result<Vec<Duplicate>, String> {
    let installed = crate::db::installed_models(&state.db.lock().unwrap());
    tokio::task::spawn_blocking(move || {
        // Only same-size files can match, so most are never opened.
        let mut sizes: HashMap<u64, usize> = HashMap::new();
        for m in &installed {
            *sizes.entry(m.size).or_default() += 1;
        }
        let entries = installed
            .into_iter()
            .filter(|m| sizes.get(&m.size).copied().unwrap_or(0) > 1 && !m.path.is_empty())
            .filter_map(|m| fingerprint(std::path::Path::new(&m.path)).ok().map(|(size, hash)| (m.model_id, m.path, size, hash)))
            .collect();
        groups(entries)
    })
    .await
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_files_group_and_different_ones_do_not() {
        let dir = std::env::temp_dir().join(format!("sulcusai-dupes-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let body: Vec<u8> = (0..(9 * 1024 * 1024)).map(|i| (i % 251) as u8).collect();
        let (a, b, c) = (dir.join("a.gguf"), dir.join("b.gguf"), dir.join("c.gguf"));
        std::fs::write(&a, &body).unwrap();
        std::fs::write(&b, &body).unwrap();
        let mut other = body.clone();
        *other.last_mut().unwrap() ^= 1;
        std::fs::write(&c, &other).unwrap();
        let fp = |p: &std::path::Path| fingerprint(p).unwrap();
        let entries = vec![
            ("m1".into(), a.display().to_string(), fp(&a).0, fp(&a).1),
            ("m2".into(), b.display().to_string(), fp(&b).0, fp(&b).1),
            ("m3".into(), c.display().to_string(), fp(&c).0, fp(&c).1),
        ];
        let g = groups(entries);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].models, vec!["m1", "m2"]);
        assert_eq!(g[0].size, body.len() as u64);
        std::fs::remove_dir_all(&dir).ok();
    }
}
