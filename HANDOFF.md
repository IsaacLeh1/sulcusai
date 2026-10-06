# Handoff

Where the build stands, for whoever picks it up next. Plan: [DESIGN.md](DESIGN.md) §9.

## Phase 1 (Foundation): progress

| Item | Status |
|---|---|
| App shell (Tauri 2 + React) | ✅ |
| Hardware detection (DXGI GPUs and VRAM, RAM, CPU, disk, battery) | ✅ `hardware.rs` |
| Catalog + spec filter + upgrade hints | ✅ `catalog.rs`, `catalog/build_catalog.py`, unit tested |
| One-click install: engine + model, SHA-256 verified, resumable | ✅ `download.rs`, `engine.rs` |
| Speed benchmark after install | ✅ |
| Chat with streaming, thinking view, per-chat model | ✅ `chat.rs` |
| Context-window meter | ✅ |
| Profile ("About you") in every chat | ✅ |
| Connectivity levels + per-chat web toggle + Ctrl+Shift+W | ✅ (the gate exists; no web tools use it yet) |
| Inline "This needs web access" card | ⏳ needs tool calling (Phase 2/4) |
| App lock (PIN / Windows Hello) and encryption at rest (SQLCipher) | ⏳ not started |
| Action log | ⏳ not started (arrives with tools in Phase 2) |
| First-run wizard | 🟡 partial: an empty app opens on Models |

## How things work

- **Data folder:** `%LOCALAPPDATA%\app.synapseai.desktop\`, containing `synapseai.db`, `models\<id>\`, `engines\<build>-<backend>\` and `logs\engine.log`.
- **Engine:** llama.cpp build pinned in `catalog/catalog.json` (`engine.build`).
  - Vulkan build for any discrete GPU, CPU build otherwise. A CUDA build for NVIDIA is a possible later speed-up (it needs the ~400 MB cudart bundle).
  - Launched hidden with `--host 127.0.0.1 --port <random> --api-key <random> -np 1 --jinja --no-webui`.
- **Fit rule:** weights + f16 KV cache at `default_ctx` (8192) + 700 MB overhead, checked against VRAM minus 10% and RAM minus max(25%, 4 GB).
  - Speed is estimated from memory bandwidth by VRAM tier; anything under 4 tokens/s counts as "can't run".
  - The real speed is measured after install.
- **Updating the catalog:** edit `catalog/build_catalog.py`, then run `python catalog/build_catalog.py`. Hashes come from the Hugging Face tree API (`lfs.oid`) and the GitHub release `digest`.

## Tests

- `cd src-tauri && cargo test`: catalog, fit rule, connectivity gate, database, prompt building.
- `cd src-tauri && cargo test e2e -- --ignored --nocapture`: real engine plus the Qwen3 1.7B model. It downloads about 1.1 GB the first time and reuses the app's data folder.
- `pnpm test`: UI formatting helpers.
- `pnpm check:licenses`, and `cargo deny check licenses` in CI: license policy.

## Gotchas

- Don't put models in roaming AppData. The app uses `app_local_data_dir`.
- The CSP blocks the window from making any network request. All HTTP goes through Rust and `net.rs`.
- Model links are shown but not clickable, and remote images aren't loaded. Opening links externally waits for the Web level and an opener.
