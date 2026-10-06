# Handoff

Where the build stands, for whoever picks it up next. Plan: [DESIGN.md](DESIGN.md) §9.

## Phase 1 (Foundation): complete

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
| Encryption at rest | ✅ `crypto.rs`: AES-256-GCM fields; data key sealed by DPAPI |
| App lock: PIN, recovery code, Windows Hello, auto-lock | ✅ `crypto.rs`, `security.rs`, `hello.rs` |
| Action log ("Activity") | ✅ `db.rs` `action_log`; network, model, privacy, security and chat events |
| First-run wizard | ✅ `views/Onboarding.tsx` |
| Inline "This needs web access" card | ⏭ moved to Phase 2/4: it needs tool calling to know when web is needed |

**Next: Phase 2 (Agent core).** Run modes, tools with the action log, file management, coding, memory and projects, subagents and auto-handoff, scheduled tasks, plugins and MCP.

## How things work

- **Data folder:** `%LOCALAPPDATA%\app.sulcusai.desktop\`, containing:
  - `sulcusai.db` and `keys.json`
  - `models\<id>\`, `engines\<build>-<backend>\` and `logs\engine.log`
  - `SULCUSAI_DATA_DIR` overrides the location, for tests and portable installs.
- **Engine:** llama.cpp build pinned in `catalog/catalog.json` (`engine.build`).
  - Vulkan build for any discrete GPU, CPU build otherwise. A CUDA build for NVIDIA is a possible later speed-up (it needs the ~400 MB cudart bundle).
  - Launched hidden with `--host 127.0.0.1 --port <random> --api-key <random> -np 1 --jinja --no-webui`.
- **Fit rule:** weights + f16 KV cache at `default_ctx` (8192) + 700 MB overhead, checked against VRAM minus 10% and RAM minus max(25%, 4 GB).
  - Speed is estimated from memory bandwidth by VRAM tier; anything under 4 tokens/s counts as "can't run".
  - The real speed is measured after install.
- **Updating the catalog:** edit `catalog/build_catalog.py`, then run `python catalog/build_catalog.py`. Hashes come from the Hugging Face tree API (`lfs.oid`) and the GitHub release `digest`.

### Encryption and app lock

- **What is encrypted:** chat titles, message text and thinking, and the profile. Each value is stored as `enc1:` + base64(nonce ‖ AES-256-GCM ciphertext).
- **What isn't:** ids, timestamps, settings, installed models and the action log. Action-log summaries never include chat content.
- **Where the data key lives** (`keys.json`, only ever wrapped):

  | State | How the data key is wrapped |
  |---|---|
  | No lock | DPAPI, with app-specific entropy |
  | Lock on | Argon2id(PIN, 64 MiB, t=3) → AES-GCM, then DPAPI again |
  | Recovery code | Argon2id(code, 19 MiB) → AES-GCM; no DPAPI, so it still works after a Windows account reset |
  | Windows Hello | A DPAPI copy, opened only after `UserConsentVerifier` says verified |

- **Older data:** values saved before encryption are encrypted at startup or unlock (`db::encrypt_legacy`), then the database is VACUUMed.
- **Deletes:** `secure_delete` is on, so deleted rows are overwritten.
- **Wrong guesses:** after 3 wrong attempts each try waits longer, up to 30 s (in memory).
- **Recovery:** after a recovery-code unlock the user must set a new PIN, and a fresh code replaces the used one.
- **Honest limits:**
  - Without app lock, anything running as the same Windows user can open the data, because DPAPI is per user.
  - Windows Hello unlock is a consent gate on a DPAPI copy, not a hardware-bound key.
  - A short PIN can be guessed by someone who can run code as this Windows user. The UI suggests a passphrase.

## Tests

- `cd src-tauri && cargo test` (29 tests): catalog, fit rule, connectivity gate, database, prompt building, encryption, app lock paths, the DPAPI round-trip and plaintext migration.
- `cd src-tauri && cargo test e2e -- --ignored --nocapture`: real engine plus the Qwen3 1.7B model. It downloads about 1.1 GB the first time and reuses the app's data folder.
- `pnpm test`: UI helpers, including starter-model choice.
- `pnpm check:licenses`, and `cargo deny check licenses` in CI: license policy.
- **Manual check done 2026-10-06** in a scratch `SULCUSAI_DATA_DIR`:
  - Old plaintext was migrated, and no plaintext remained in the db, WAL or logs.
  - The wizard, lock, wrong PIN, recovery code with a new PIN, chat and Activity all worked.
  - The Windows Hello prompt itself still needs a person to try it.

## Gotchas

- Don't put models in roaming AppData. The app uses `app_local_data_dir`.
- The CSP blocks the window from making any network request. All HTTP goes through Rust and `net.rs`.
- Model links are shown but not clickable, and remote images aren't loaded. Opening links externally waits for the Web level and an opener.
- Every command that reads chats or the profile calls `state.cipher()?`, which returns `"locked"` while the app is locked. New commands that touch encrypted data must do the same.
- Losing `keys.json` loses every encrypted chat. Back it up together with `sulcusai.db` (a future backup feature must include both).
