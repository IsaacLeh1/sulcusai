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
| Inline "This needs web access" card | ⏭ Phase 4, with the web tools that would use it |

## Phase 2 (Agent core): complete

| Item | Status |
|---|---|
| Agent loop (tool calls → results → repeat, max 30 steps) | ✅ `agent.rs`; tool calls and results stored encrypted |
| Run modes Plan / Auto / Bypass, approvals, allow-for-chat | ✅ `tools::permission`, `agent::Approvals` |
| File tools in shared folders only | ✅ `tools/fs.rs`, `sandbox.rs` |
| Checkpoints + "Undo file changes from this reply" | ✅ `checkpoint.rs`; encrypted backups, kept 30 days; deletes use the Recycle Bin |
| Coding: run_command (hidden PowerShell, timeout, kill tree), diffs to approve, search | ✅ `tools/shell.rs` |
| Cross-chat memory, memory page, on/off, incognito chats | ✅ `memory.rs`, `tools/memory.rs` |
| Projects: instructions, folders, memories, chats | ✅ `projects.rs` |
| Sub-agents (`delegate`) | ✅ `agent::run_helper`; read-only, fresh context, steps shown live |
| Auto-handoff at 80% of the context window | ✅ `handoff.rs` |
| Scheduled tasks | ✅ `schedule.rs`; read-only unless the user allows changes |
| MCP connectors (local stdio) with per-tool Allow / Ask / Off | ✅ `mcp.rs`, `connectors.rs` |
| Plugins: skills + connectors from a folder, preview before install | ✅ `connectors.rs` |
| Remote (HTTP/OAuth) connectors | ⏭ Phase 4 (they go online) |

**Next: Phase 3 (Voice and meetings).** Dictation, voice mode, meeting mode (capture, transcript, summary, library), translation.

### How the agent works

- **One turn:** `send_message` saves the user message, then `agent::Turn::run`:
  1. builds the system prompt: profile, project, recalled memories, tool and mode instructions, and skills;
  2. streams a step;
  3. runs each tool call through `execute` (permission → preview/approval → run → log);
  4. stores the results and repeats.
- **Run modes:** Plan offers only read tools (plus `remember`), Auto asks before Write/Execute/Connector actions, and Bypass runs everything.
  - "Allow … for this chat" is remembered per risk class in memory, not on disk.
- **Paths:** tools see paths as `folder-name/sub/file` (`Sandbox::display`), and `Sandbox::resolve` accepts that form, absolute paths, or a path relative to the first folder.
  - `..` escapes and links pointing outside are refused.
  - Drive roots, Windows/Program Files/ProgramData, the whole user folder and the app's data folder can't be shared.
- **Small-model safeguards** (found testing with Qwen3 1.7B):
  - Line numbers copied from `read_file` are stripped from writes and edits (`strip_line_numbers`).
  - Edits written with LF line endings match CRLF files.
  - Short skills are inlined into the prompt instead of needing `load_skill`.
- **Context:** the launch context is the largest of 32K/16K/8K that keeps the same GPU placement (`catalog::launch_settings`).
  - History is trimmed by whole turns, then old tool output inside the current turn.
  - At 80% full, the next message triggers a handoff to a new chat.
- **Connectors:** each server is a hidden child process assigned to the app's job object. Tools are offered as `mcp__<connector>__<tool>`.
  - Plugin servers can use `${PLUGIN_DIR}`.
  - Plugin folders are copied into `%LOCALAPPDATA%\app.sulcusai.desktop\plugins\<id>` (max 50 MB; `.git` and `node_modules` skipped).
- **Scheduler:** checks every 30 s while unlocked and skips while any chat is generating. It catches up a missed run once at startup.

### Phase 2 verification (2026-10-06)

- **End-to-end tests with Qwen3 1.7B** (`cargo test e2e -- --ignored --nocapture`):
  - Bypass created a file.
  - Auto asked before an edit.
  - Undo restored both.
  - Memory carried a fact to a new chat; incognito saved nothing.
  - A helper found `shop/src/checkout.py`, and after a handoff the new chat still knew it.
  - A connector tool ran after approval, and a plugin skill was followed.
- **UI check in a scratch `SULCUSAI_DATA_DIR`:**
  - approval card with diff, Deny and Allow, Undo, Plan → Run this plan
  - Connectors page, Memory page, project page
  - scheduled Run now: the reply used the project instructions and both kinds of memory
  - incognito cleanup
- **Not covered yet:** remote MCP connectors, and parallel sub-agents (the engine runs one request at a time with `-np 1`).

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
