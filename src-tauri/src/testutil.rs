// SPDX-License-Identifier: AGPL-3.0-only
//! App state for tests, on every system. The end-to-end tests (Windows only)
//! use these too.

use std::path::PathBuf;

use crate::catalog::Catalog;
use crate::{engine, hardware, paths::Paths};

/// The installed app's data folder. Off Windows there's none to share with
/// the end-to-end tests, so a throwaway one stands in.
pub(crate) fn app_data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        // Same folder as Tauri's app_local_data_dir().
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).expect("LOCALAPPDATA");
        base.join("app.sulcusai.desktop")
    }
    #[cfg(not(windows))]
    {
        std::env::temp_dir().join(format!("sulcusai-app-{}", uuid::Uuid::new_v4()))
    }
}

/// Seals the data key off Windows without touching the system keychain.
#[cfg(not(windows))]
struct TestProtector;

#[cfg(not(windows))]
impl crate::crypto::Protector for TestProtector {
    fn protect(&self, data: &[u8]) -> Result<Vec<u8>, String> {
        Ok(data.iter().map(|b| b ^ 0x5A).collect())
    }
    fn unprotect(&self, data: &[u8]) -> Result<Vec<u8>, String> {
        Ok(data.iter().map(|b| b ^ 0x5A).collect())
    }
}

fn protector() -> Box<dyn crate::crypto::Protector> {
    #[cfg(windows)]
    {
        Box::new(crate::crypto::dpapi::Dpapi)
    }
    #[cfg(not(windows))]
    {
        Box::new(TestProtector)
    }
}

/// App state with its own throwaway data folder and models folder, for
/// tests that write models (never the real models folder: a sync test
/// once overwrote an installed model there with an empty file).
pub(crate) fn scratch_state() -> std::sync::Arc<crate::AppState> {
    let models = std::env::temp_dir().join(format!("sulcusai-models-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&models).unwrap();
    temp_state(Some(models))
}

/// App state over the real engines and (unless `models` is given) the real
/// models folder, with a throwaway database: for tests that only read models.
pub(crate) fn temp_state(models: Option<PathBuf>) -> std::sync::Arc<crate::AppState> {
    temp_state_in(models, None)
}

/// `temp_state` with its own engines folder too (`engines`), for tests that
/// download engines (never into the installed app's).
pub(crate) fn temp_state_in(models: Option<PathBuf>, engines: Option<PathBuf>) -> std::sync::Arc<crate::AppState> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, RwLock};
    let real = Paths::new(app_data_dir()).unwrap();
    let tmp = std::env::temp_dir().join(format!("sulcusai-media-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(tmp.join("logs")).unwrap();
    let models = models.unwrap_or_else(|| real.models.clone());
    let engines = engines.unwrap_or_else(|| real.engines.clone());
    let paths = Paths { data: tmp.clone(), models, engines, logs: tmp.join("logs"), db: tmp.join("t.db") };
    let conn = crate::db::open(&paths.db).unwrap();
    crate::features::enable_all(&conn);
    let vault = crate::crypto::Vault::open(&tmp.join("keys.json"), protector()).unwrap();
    Arc::new(crate::AppState {
        hardware: RwLock::new(hardware::detect(&real.models)),
        paths,
        db: Mutex::new(conn),
        vault: Mutex::new(vault),
        catalog: Catalog::bundled(),
        engine: tokio::sync::Mutex::new(engine::Engine::default()),
        installs: Mutex::new(HashMap::new()),
        generations: Mutex::new(HashMap::new()),
        contexts: Mutex::new(HashMap::new()),
        approvals: crate::agent::Approvals::default(),
        mcp: tokio::sync::Mutex::new(HashMap::new()),
        speech: crate::speech::Speech::default(),
        player: crate::audio::Player::new(),
        heat: crate::perf::Heat::default(),
        bridge: crate::bridge::Bridge::default(),
        app: std::sync::OnceLock::new(),
        engine_used: std::sync::Mutex::new(std::time::Instant::now()),
        // Models a test starts end with it (else they stay on the card).
        #[cfg(windows)]
        job: crate::winjob::Job::kill_on_close().ok(),
    })
}
