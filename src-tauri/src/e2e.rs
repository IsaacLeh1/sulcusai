// SPDX-License-Identifier: AGPL-3.0-only
//! End-to-end check of the real engine with the smallest catalog model.
//! Downloads ~1.1 GB on first run, so it is ignored by default:
//!     cargo test e2e -- --ignored --nocapture
//! Uses the app's own data folder, so the files are reused by the app.

use std::sync::atomic::AtomicBool;

use crate::catalog::{self, Budget, Catalog};
use crate::db::{Message, Profile};
use crate::{chat, download, engine, hardware, net, paths::Paths};

fn app_data_dir() -> std::path::PathBuf {
    // Same folder as Tauri's app_local_data_dir().
    let base = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).expect("LOCALAPPDATA");
    base.join("app.sulcusai.desktop")
}

#[tokio::test]
#[ignore]
async fn e2e_install_load_and_chat() {
    let paths = Paths::new(app_data_dir()).unwrap();
    let hw = hardware::detect(&paths.models);
    println!("hardware: {hw:#?}");
    let budget = Budget::from_hardware(&hw);
    println!("budget: {budget:?}");

    let cat = Catalog::bundled();
    let spec = cat.model("qwen3-1.7b").unwrap();
    let fit = catalog::fit_model(spec, &budget);
    let quant = "Q4_K_M";
    let vfit = fit.variant(quant).unwrap();
    println!("fit: {vfit:?}");
    assert!(vfit.runnable());

    let backend = engine::backend_for(&budget).unwrap();
    let cancel = AtomicBool::new(false);
    let exe = engine::ensure_installed(&paths, &cat.engine, backend, &cancel, |r, t| {
        if r == t {
            println!("engine downloaded: {t} bytes");
        }
    })
    .await
    .unwrap();
    println!("engine: {}", exe.display());

    let variant = spec.variant(quant).unwrap();
    let dest = paths.models.join(&spec.id).join(&variant.file);
    let client = net::external_client(net::Connectivity::Offline, net::Purpose::ModelDownload, false).unwrap();
    download::fetch_verified(&client, &variant.url, &dest, variant.size, &variant.sha256, &cancel, |_, _| {})
        .await
        .unwrap();
    println!("model verified: {}", dest.display());

    let mut eng = engine::Engine::default();
    let log = paths.engine_log();
    let ep = eng
        .ensure(
            engine::LaunchSpec {
                exe: &exe,
                model_path: &dest,
                model_id: &spec.id,
                quant,
                ctx: fit.ctx,
                gpu_layers: vfit.gpu_layers,
                log: &log,
            },
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("engine failed to start: {e}"));
    println!("engine up on port {}", ep.port);

    // The server must refuse requests without the key.
    let unauth = net::local_client()
        .post(ep.url("/v1/chat/completions"))
        .json(&serde_json::json!({"messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status().as_u16(), 401, "server answered without the API key");

    let profile = Profile { name: "Tester".into(), about: String::new(), preferences: "Be brief.".into() };
    let (base, about) = chat::system_prompt(&profile, "Tuesday, October 6, 2026");
    let history = vec![Message {
        id: "u1".into(),
        chat_id: "c".into(),
        role: "user".into(),
        content: "Reply with a five-word greeting. /no_think".into(),
        thinking: None,
        created_at: 0,
    }];
    let (kept, info) = chat::fit_history(&ep, &base, &about, &history).await.unwrap();
    println!("context: {info:?}");
    assert_eq!(kept.len(), 1);
    assert!(info.system_tokens > 20 && info.history_tokens > 0);

    let mut deltas = 0;
    let done = chat::stream_reply(&ep, &format!("{base}\n\n{about}"), &kept, &cancel, |_| deltas += 1)
        .await
        .unwrap();
    println!("reply ({deltas} deltas): {:?}", done.content);
    println!("thinking: {:?}", done.thinking);
    println!("usage: {:?}/{:?}, tps {:?}", done.prompt_tokens, done.completion_tokens, done.tps);
    assert!(!done.content.trim().is_empty());
    assert!(deltas > 1, "reply did not stream");

    let tps = engine::benchmark(&ep).await.unwrap();
    println!("benchmark: {tps} tokens/sec (estimate was {})", vfit.est_tps);
    assert!(tps > 0.0);

    eng.stop().await;
    assert!(eng.status().model_id.is_none());
}
