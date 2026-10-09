// SPDX-License-Identifier: AGPL-3.0-only
//! Copying a model from a paired PC on the same network instead of
//! downloading it again (DESIGN.md §4.9). Only catalog models are offered,
//! only their own files can be asked for, everything travels over the
//! paired, encrypted link, and the copy is checked against the catalog's
//! SHA-256 before it's used, exactly as a download is.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncReadExt;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

use super::wire::{self, Sealer};
use crate::{db, AppState, AppStateRef};

/// A model the other PC has, with its files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Offered {
    pub model_id: String,
    pub quant: String,
    pub name: String,
    pub files: Vec<(String, u64)>,
}

/// What this PC can offer: installed catalog models whose files are complete.
pub fn offered(state: &AppState) -> Vec<Offered> {
    let installed = db::installed_models(&state.db.lock().unwrap());
    installed
        .iter()
        .filter_map(|m| {
            let spec = state.catalog.model(&m.model_id)?;
            let v = spec.variant(&m.quant)?;
            let dir = state.paths.models.join(&spec.id);
            let mut files = vec![(v.file.clone(), v.size)];
            if let Some(vis) = &spec.vision {
                files.push((vis.file.clone(), vis.size));
            }
            files.retain(|(f, size)| dir.join(f).metadata().is_ok_and(|md| md.len() == *size));
            (files.first().map(|f| &f.0) == Some(&v.file)).then(|| Offered { model_id: spec.id.clone(), quant: m.quant.clone(), name: spec.name.clone(), files })
        })
        .collect()
}

#[derive(Serialize, Deserialize)]
pub struct FileAsk {
    pub model_id: String,
    pub file: String,
}

/// The answering PC: sends the file it was asked for, if it's one it offers.
pub async fn serve(state: &Arc<AppState>, ask: FileAsk, w: &mut OwnedWriteHalf, out: &mut Sealer) -> Result<(), String> {
    let ok = offered(state).iter().any(|o| o.model_id == ask.model_id && o.files.iter().any(|(f, _)| *f == ask.file));
    if !ok {
        wire::write_frame(w, &out.seal(b"no")).await.map_err(|e| e.to_string())?;
        return Ok(());
    }
    wire::write_frame(w, &out.seal(b"ok")).await.map_err(|e| e.to_string())?;
    let path = state.paths.models.join(&ask.model_id).join(&ask.file);
    let mut f = tokio::fs::File::open(&path).await.map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 4 << 20];
    loop {
        let n = f.read(&mut buf).await.map_err(|e| e.to_string())?;
        // An empty frame ends the file.
        wire::write_frame(w, &out.seal(&buf[..n])).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
    }
    state.log("sync", &format!("Sent {} to a paired PC", ask.file));
    Ok(())
}

/// The asking PC: receives one file into `dest`.
async fn receive(r: &mut OwnedReadHalf, inn: &mut Sealer, dest: &std::path::Path, cancel: &AtomicBool, mut progress: impl FnMut(u64)) -> Result<(), String> {
    let first = inn.open(&wire::read_frame(r).await.map_err(|e| e.to_string())?)?;
    if first != b"ok" {
        return Err("The other PC doesn't have that model anymore.".into());
    }
    let part = dest.with_extension("part");
    if let Some(dir) = part.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut got = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(file);
            std::fs::remove_file(&part).ok();
            return Err(crate::download::CANCELLED.into());
        }
        let chunk = inn.open(&wire::read_frame(r).await.map_err(|e| format!("The copy was interrupted: {e}"))?)?;
        if chunk.is_empty() {
            break;
        }
        file.write_all(&chunk).map_err(|e| e.to_string())?;
        got += chunk.len() as u64;
        progress(got);
    }
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(&part, dest).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
pub async fn receive_for_test(r: &mut OwnedReadHalf, inn: &mut Sealer, dest: &std::path::Path, progress: &mut dyn FnMut(u64)) -> Result<(), String> {
    receive(r, inn, dest, &AtomicBool::new(false), progress).await
}

#[derive(Serialize)]
pub struct PeerModels {
    peer_id: String,
    peer: String,
    /// Models that PC has and this one doesn't (and that fit this PC).
    models: Vec<Offered>,
}

/// What each paired PC on the network could copy here.
#[tauri::command]
pub async fn peer_models(state: AppStateRef<'_>) -> Result<Vec<PeerModels>, String> {
    let state = state.inner().clone();
    let mine: Vec<String> = db::installed_models(&state.db.lock().unwrap()).into_iter().map(|m| m.model_id).collect();
    let budget = state.budget();
    let mut out = Vec::new();
    let peers = super::config(&state.db.lock().unwrap()).peers;
    for p in peers {
        let Some(addr) = super::seen_addr(&p.id) else { continue };
        let Ok((mut r, _w, mut inn, _out)) = super::request(&state, &p, addr, "models", &serde_json::Value::Null).await else { continue };
        let Ok(bytes) = wire::read_frame(&mut r).await else { continue };
        let Ok(list) = serde_json::from_slice::<Vec<Offered>>(&inn.open(&bytes)?) else { continue };
        let models: Vec<Offered> = list
            .into_iter()
            .filter(|o| !mine.contains(&o.model_id))
            .filter(|o| {
                state.catalog.model(&o.model_id).is_some_and(|spec| crate::catalog::fit_model(spec, &budget).variant(&o.quant).is_some_and(|v| v.runnable()))
            })
            .collect();
        out.push(PeerModels { peer_id: p.id.clone(), peer: p.name.clone(), models });
    }
    Ok(out)
}

/// Copies a model's files from a paired PC, then installs it as usual (the
/// install checks each file's SHA-256 and measures its speed).
#[tauri::command]
pub async fn copy_model_from(app: AppHandle, state: AppStateRef<'_>, peer_id: String, model_id: String) -> Result<(), String> {
    let state = state.inner().clone();
    let p = super::peer(&state, &peer_id).ok_or("That PC isn't paired anymore.")?;
    let addr = super::seen_addr(&p.id).ok_or_else(|| format!("{} isn't on this network right now.", p.name))?;
    let spec = state.catalog.model(&model_id).ok_or("Unknown model")?.clone();
    if !state.installs.lock().unwrap().is_empty() {
        return Err("Another install is in progress. Wait for it to finish first.".into());
    }
    // What the other PC has of it.
    let (mut r, _w, mut inn, _out) = super::request(&state, &p, addr, "models", &serde_json::Value::Null).await?;
    let list: Vec<Offered> = serde_json::from_slice(&inn.open(&wire::read_frame(&mut r).await.map_err(|e| e.to_string())?)?).map_err(|e| e.to_string())?;
    let offer = list.into_iter().find(|o| o.model_id == model_id).ok_or_else(|| format!("{} doesn't have {} anymore.", p.name, spec.name))?;
    let total: u64 = offer.files.iter().map(|f| f.1).sum();
    let cancel = Arc::new(AtomicBool::new(false));
    state.log("sync", &format!("Copying {} from {} on this network", spec.name, p.name));
    let mut done = 0u64;
    for (file, size) in &offer.files {
        // Only file names: nothing can reach outside the model's folder.
        if file.contains(['/', '\\']) || file.contains("..") {
            return Err("The other PC offered an unexpected file.".into());
        }
        let dest = state.paths.models.join(&spec.id).join(file);
        if dest.metadata().is_ok_and(|m| m.len() == *size) {
            done += size;
            continue;
        }
        let ask = serde_json::to_value(FileAsk { model_id: model_id.clone(), file: file.clone() }).map_err(|e| e.to_string())?;
        let (mut r, _w, mut inn, _out) = super::request(&state, &p, addr, "file", &ask).await?;
        let base = done;
        receive(&mut r, &mut inn, &dest, &cancel, |got| {
            app.emit("install:progress", serde_json::json!({ "model_id": model_id, "phase": "download", "received": base + got, "total": total })).ok();
        })
        .await?;
        done += size;
    }
    // The usual install: it finds the files, checks their hashes, and measures speed.
    let st = app.state::<Arc<AppState>>();
    crate::install_model(app.clone(), st, model_id, offer.quant)
}
