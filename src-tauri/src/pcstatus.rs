// SPDX-License-Identifier: AGPL-3.0-only
//! How this PC is doing, for the phone's PC tab: its temperatures, processor,
//! memory and graphics card, what SulcusAI is working on, and Stop all.

use std::sync::atomic::Ordering;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::Emitter;

use crate::{db, AppStateRef};

#[derive(Serialize)]
pub struct Job {
    /// "chat", "media", "install" or "training".
    kind: &'static str,
    title: String,
    detail: String,
    fraction: Option<f64>,
}

#[derive(Serialize)]
pub struct PcStatus {
    pc: String,
    /// Percent of all cores.
    cpu: f32,
    cpu_temp: Option<f32>,
    ram_used: u64,
    ram_total: u64,
    gpu: crate::perf::GpuNow,
    gpu_temp: Option<f32>,
    /// Adaptive cooling's level now (0: not cooling, 3: the most).
    cooling: u8,
    /// The chat model loaded, if one is.
    model: Option<String>,
    jobs: Vec<Job>,
}

fn memory() -> (u64, u64) {
    static SYS: LazyLock<Mutex<sysinfo::System>> = LazyLock::new(|| Mutex::new(sysinfo::System::new()));
    let mut s = SYS.lock().unwrap();
    s.refresh_memory();
    (s.used_memory(), s.total_memory())
}

fn short(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() > 60 { format!("{}…", s.chars().take(59).collect::<String>()) } else { s.to_string() }
}

#[tauri::command]
pub async fn pc_status(state: AppStateRef<'_>) -> Result<PcStatus, String> {
    status(&state).await
}

/// How this PC is doing now (the phone's PC tab, and a linked PC asking).
pub(crate) async fn status(state: &crate::AppState) -> Result<PcStatus, String> {
    let c = state.cipher()?;
    let gpu = crate::perf::gpu_reading(state).await;
    // Reading the card's temperature wakes it, so only while working.
    let temps = if crate::perf::working(state) { crate::perf::temps().await } else { crate::perf::quiet_temps().await };
    let (ram_used, ram_total) = memory();
    let loaded = state.engine.lock().await.status().model_id;
    let model = loaded.map(|id| state.model_spec(&id).map(|m| m.name).unwrap_or(id));
    let mut jobs = Vec::new();
    let running: Vec<String> = state.generations.lock().unwrap().keys().cloned().collect();
    if !running.is_empty() {
        let chats = db::list_chats(&state.db.lock().unwrap(), &c);
        for id in running {
            let title = match chats.iter().find(|ch| ch.id == id) {
                Some(ch) if ch.incognito => "An incognito chat".to_string(),
                Some(ch) => short(&ch.title),
                None => "A chat".to_string(),
            };
            jobs.push(Job { kind: "chat", title, detail: "Answering".into(), fraction: None });
        }
    }
    for j in crate::media::list_jobs() {
        let detail = if j.status == "queued" { "Waiting".to_string() } else { j.stage.clone() };
        jobs.push(Job { kind: "media", title: short(&j.prompt), detail, fraction: (j.status != "queued").then_some(j.fraction) });
    }
    for id in state.installs.lock().unwrap().keys() {
        jobs.push(Job { kind: "install", title: state.model_spec(id).map(|m| m.name).unwrap_or_else(|| id.clone()), detail: "Installing".into(), fraction: None });
    }
    if crate::finetune::training() {
        jobs.push(Job { kind: "training", title: "Teaching a model".into(), detail: "Training".into(), fraction: None });
    }
    if let Some(j) = crate::uncensor::uncensor_job().filter(|_| crate::uncensor::running()) {
        jobs.push(Job { kind: "training", title: crate::uncensor::copy_name(&j.name), detail: "Making".into(), fraction: Some(j.fraction) });
    }
    Ok(PcStatus {
        pc: std::env::var("COMPUTERNAME").unwrap_or_default(),
        cpu: crate::perf::cpu_usage(),
        cpu_temp: temps.cpu,
        ram_used,
        ram_total,
        gpu_temp: temps.gpu.or(gpu.temp),
        gpu,
        cooling: state.heat.get().level(),
        model,
        jobs,
    })
}

/// Stops replies, pictures, videos and music being made, and teaching, then
/// frees the graphics card. Downloads carry on. Returns how many it stopped.
#[tauri::command]
pub async fn stop_all(state: AppStateRef<'_>) -> Result<u32, String> {
    stop_everything(&state).await
}

/// `stop_all`, for a linked PC asking too.
pub(crate) async fn stop_everything(state: &crate::AppState) -> Result<u32, String> {
    state.cipher()?;
    let mut n = 0u32;
    for flag in state.generations.lock().unwrap().values() {
        flag.store(true, Ordering::Relaxed);
        n += 1;
    }
    for j in crate::media::list_jobs() {
        crate::media::cancel(&j.id);
        n += 1;
    }
    if crate::finetune::training() {
        crate::finetune::cancel_finetune();
        n += 1;
    }
    if crate::uncensor::running() {
        crate::uncensor::cancel_uncensor();
        n += 1;
    }
    // What was told to stop ends first; then the model leaves the card.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    state.engine.lock().await.stop().await;
    // The status bars (the PC's and the phone's) show it's gone.
    if let Some(app) = state.app.get() {
        app.emit("engine:unloaded", ()).ok();
    }
    state.log("perf", &format!("Stop all: stopped {n} and unloaded the model"));
    Ok(n)
}
