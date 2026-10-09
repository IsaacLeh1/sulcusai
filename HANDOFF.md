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

## Phase 3 (Voice and meetings): complete

| Item | Status |
|---|---|
| Audio: mic and system-audio (loopback) capture at 16 kHz, playback that can be cut off | ✅ `audio.rs` (WASAPI) |
| Voice detection and phrase cutting | ✅ `vad.rs` |
| Speech recognition: whisper.cpp server, spec-filtered models, self-test on install | ✅ `speech.rs` |
| Dictation (🎤, Ctrl+Shift+Space) | ✅ `voice.rs`, `components/Voice.tsx` |
| Voice mode with barge-in, spoken replies as they stream | ✅ `voice.rs` |
| Speech output: Windows voices; natural voices (Supertonic 3) | ✅ `tts.rs`, `natural.rs` |
| Meeting mode: capture, live transcript, echo removal, encrypted audio, notes, library, search, export, ask | ✅ `meeting.rs`, `views/MeetingsView.tsx` |
| Speaker labels (diarization) with names | ✅ `diarize.rs` |
| Recording-consent reminder with a copyable announcement | ✅ |
| Translation: text, text documents, subtitles, live meeting captions | ✅ `translate.rs`, `views/TranslateView.tsx` |
| Meeting tools for chats (`search_meetings`, `read_meeting`) | ✅ `tools/meetings.rs` |
| Action items → tasks and calendar; start meetings from calendar events | ⏭ Phase 4 (needs notes/tasks and calendar) |
| Translating Word/PDF files and text in images | ⏭ Phase 4/5 (documents, vision) |
| Typing dictation into other apps (global hotkey) | ⏭ Phase 4 (desktop assistant) |

### Features menu (2026-10-07)

- **Starting state:** a new install starts with chat and models only. `features.rs` keeps the set of features that are on (`dictation`, `voice_chat`, `read_aloud`, `meetings`, `translate`, `files`, `memory`, `projects`, `scheduled`, `connectors`).
- **Installs from before this change:** the first time this version runs, every feature that has data stays on (shared folders, memories, projects, schedules, connectors, meetings, an installed speech model).
- **Installing:** Install on Dictation or Voice chat downloads the quick speech model, and on Meeting notes the accurate one, then turns the feature on (`install_feature`). Natural voices and speaker labels are add-ons on those cards.
- **Off means off:** the agent gets no file tools (shared folders are ignored), memory, connector or meeting tools. The scheduler skips its tasks. `start_dictation`, `start_voice`, `start_meeting`, `translate`, `translate_file` and `add_folder` refuse.
  - The Memory feature also drives the `memory_enabled` setting.
- **Tests:** e2e harnesses call `features::enable_all`.

**Next: Phase 4 (Web and productivity).**

### How voice and meetings work

- **Speech recognition:** `whisper-server` runs hidden on 127.0.0.1, on a random port, with `--request-path /<random>`.
  - It has no API-key option, so the secret path prefix is the key. `/load` and every other route sit behind it, and it serves files from an empty folder.
  - It runs on the processor with about two thirds of the cores (up to 16).
  - **Two jobs, two models:** `speech::choose` picks the most accurate installed model for meetings, and the most accurate one that encodes a 30 s window in ≤2.5 s for dictation and voice chats. The user can override either. At most two speech servers stay loaded, and they unload after 10 idle minutes.
  - Whisper always encodes a full 30 s window. On this PC (Core Ultra 9 275HX, 16 threads) that's ~7 s for Large v3 Turbo and ~1.3 s for Small, so phrases are batched for meetings.
  - **Don't shrink `audio_ctx`:** it made Turbo output garbage ("occurs and the") and a server request hang.
  - Server-side Silero VAD (`--vad`) and `-sns` keep silence from turning into made-up sentences.
- **Meetings:** each channel ("You" = mic, "Others" = loopback) goes through its own `Phraser`.
  - Phrases are batched per channel (up to 18 s, or sent after 2 s of quiet) into one whisper request.
  - Line times are mapped back to the meeting clock (`Job::real_time`), and lines are rejoined into whole sentences.
  - A "You" line that repeats an overlapping "Others" line is speaker echo and is dropped, in whichever order they arrive.
  - Audio is stored as encrypted 30 s chunks per channel (`meetings/<id>/you-00000.enc`), and clips are mixed on demand.
  - Notes use `chat::complete` with a JSON schema, falling back to plain JSON. Long meetings are summarized part by part first.
- **Speaker labels:** a Kaldi-style 80-band filterbank (`diarize::fbank`, its own FFT) feeds WeSpeaker ResNet34 on ONNX Runtime.
  - Live grouping uses cosine similarity ≥ 0.5. At the end, average-linkage regrouping renumbers speakers by who spoke first.
  - Lines too short for a voice print (< 1.2 s) take the label of the speaker before them.
- **Voices:** voice ids carry their engine: `system:<WinRT id>` or `supertonic:F1`…
  - Natural voices are preferred when installed and they speak the language. Otherwise a Windows voice for that language is used.
  - Supertonic gets plain text with `<lang>` tags (no phonemizer, so no GPL espeak), then runs 5 flow-matching steps at 44.1 kHz.
- **Voice mode:** the Rust loop owns the mic and the player, so barge-in is immediate.
  - While it talks, voice detection needs 3× louder speech.
  - A transcript that repeats its own last reply is ignored, which catches speaker echo when no headphones are used.
  - The reply is spoken by teeing `chat:delta` events from `run_turn` into a sentence splitter.
- **ONNX Runtime:** the official Microsoft zip, hash-checked, is loaded at run time with `ort` `load-dynamic`. Nothing is bundled or fetched at build time, and natural voices and speaker labels share it.

### Phase 3 verification (2026-10-06)

- **No real microphone, speakers or screen were used.** Every test feeds synthesized speech through the same code paths.
- **`e2e_speech_round_trip`:**
  - A Windows voice's sentence came back word-perfect.
  - Silence gave nothing.
  - The server answered 404 without its path prefix.
- **`e2e_dictation_streams_phrases`:** two sentences streamed in 20 ms chunks came out as two exact phrases. Small ran at about 8× real time.
- **`e2e_meeting_records_transcribes_and_writes_notes`:**
  - Two channels, with the call echoing into the mic.
  - Three clean sentences resulted, and the echo was removed.
  - A replay clip had the right length.
  - Notes with action items came back from Qwen3 1.7B. The small model sometimes credits tasks to the wrong person; larger models do better.
- **`e2e_meeting_tells_speakers_apart`:** two voices taking turns were labeled Speaker 1/2/1/2.
- **`e2e_natural_voice_is_understood`:** English and German Supertonic speech was transcribed back word for word, at about 3× real time.
- **`e2e_audio_devices_are_listed`:** found the Intel mic array and Realtek speakers by name.
- **Full regression after Phase 3** (all 12 e2e tests, one at a time): 11 passed on the first run.
  - `e2e_helper_and_handoff` failed once and passed on the re-run. Qwen3 1.7B's helper sometimes stops before it finds the file. That's model variance; that code path is unchanged in Phase 3.
- **Not covered yet (needs a person at the PC):**
  - live capture from the real microphone and loopback, and playback through the speakers
  - voice mode end to end with a human
  - Windows' echo cancellation in communications mode

| Component | Measured on this PC |
|---|---|
| Large v3 Turbo (q5_0), 16 threads | ~7 s per 30 s window; ~1.7× real time on an 8 s phrase |
| Small (q5_1) | ~1.3 s per window; ~8× real time on a phrase |
| Supertonic 3, 5 steps | 5.3 s of speech in 1.9 s (first call includes loading) |


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
- **Models already on this PC (`found.rs`):** at startup, on setup's model step and on the Models page's "Check again", the app adds catalog models whose files are already there, from an earlier install or from LM Studio, Ollama (blobs named by hash), Hugging Face, Jan, GPT4All or Downloads.
  - A file only counts when its name (or Ollama's hash name) and exact size match and its SHA-256 checks out. Others' files are hard-linked in (no extra space; removing a model here never deletes their copy).
  - A model is only listed once its engine is installed; otherwise its card says "Already on this PC" and Install skips the download. Every download also looks for a local copy first (linked, or copied with a hash check from another drive).
  - Test: `cargo test --lib e2e_found_models -- --ignored` (hard links real files into a pretend user folder).
- **Models from other apps (`local.rs`, `gguf.rs`):** any chat model another app downloaded is used where it is, even if it isn't in the catalog. Ollama models come from its manifests (named like `qwen2.5-coder:14b`); other apps' `.gguf` files from the folders above.
  - What the model is (layers, heads, context, quantization, experts, tool support from its chat template) is read from the GGUF header. Files without a chat template (embedding models) and Ollama's single-file picture models (llama.cpp can't load them) are skipped with a reason.
  - Their specs live in the settings key `local_models` (`AppState::model_spec` looks in the catalog, then there). "Stop using" never deletes the file; the path goes in `local_models_hidden` until the user presses Look for models on this PC.
  - The first chat downloads the llama.cpp engine if a found model arrived before it.
  - Test: `cargo test --lib e2e_local_models -- --ignored --nocapture` (reads this PC's real Ollama/LM Studio folders into a throwaway database, then runs the smallest one on the processor).
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

- `cd src-tauri && cargo test` (127 tests): catalog, fit rule, connectivity gate, database, prompt building, encryption, app lock, the agent, plus audio formats, voice detection, speech-model choice, speech cleanup, meetings (storage, echo, batching, notes, export), translation, filterbank/FFT and speaker grouping.
- `cd src-tauri && cargo test e2e -- --ignored --nocapture --test-threads 1`: the real engines and models (Qwen3 1.7B, Whisper Small and Large v3 Turbo, Supertonic 3, WeSpeaker). It downloads about 2.5 GB the first time and reuses the app's data folder. Run them one at a time; they share the engines.
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
- Sandboxed shells (such as the Claude desktop app's) can see `%LOCALAPPDATA%` redirected to `...\Packages\<app>\LocalCache\Local`. A dev build started from such a shell uses that copy, not the real data folder.
- A capture or playback test opens the real microphone or speakers. Ask the user before running one.
- Losing `keys.json` loses every encrypted chat. Back it up together with `sulcusai.db` (a future backup feature must include both).

## Phase 4 (Web and productivity): complete, merged 2026-10-08 (worktree `C:\dev\sulcusai-phase4`)

Work happens in this worktree so the running dev app in `C:\dev\sulcusai` isn't restarted by edits. Merge into `main` when Phase 4 is done.

**Done on the branch:**
- Notes and tasks: `notes.rs`, `tools/notes.rs`, Notes and Tasks pages; meeting action items → Tasks.
- Web search and page reading: `web.rs`, `tools/web.rs`. DuckDuckGo, Brave or SearXNG; `request_web` card.
- Performance (`perf.rs`): Cool & quiet / Balanced / Turbo set the chat engine's threads (`-t`), graphics memory, context and priority (`BELOW_NORMAL_PRIORITY_CLASS`). Turbo is clamped to `ceiling()`.
  - Heat guard: `cool_down` before each agent step (`chat:status` "cooling").
  - Adaptive cooling (switch, on by default): `start_monitor` reads temperatures every 10 s while a model or speech is loaded. `Heat::update` steps per component (levels 0-3, enter at 12/7/3 °C below the limit, ease 4 °C cooler and no sooner than 90 s). A hot CPU cuts threads; a hot GPU moves layers off the card (`gpu_share`); level 2+ adds pauses between steps. New limits apply between replies, never mid-reply (`llm_endpoint`; `background_endpoint` uses the loaded engine as is).
  - The monitor also unloads an idle chat model (5 min Cool, 30 min Balanced, never Turbo).
  - Temperatures: `nvidia-smi`, and WMI `Win32_PerfRawData_Counters_ThermalZoneInformation`. WMI needs `CoSetProxyBlanket` with impersonation, or the query returns nothing.
  - Verified: `e2e_perf_limits_reach_the_engine` (6 threads at BelowNormal in Cool; top heat level relaunches with 2 threads and 15 of 29 layers, still answering).
- Models page: ratings in `build_catalog.py` (`RATINGS`; rough guides from public benchmarks), ranked least to most capable, with Coding / Writing / Research / Agents / Languages / Fastest tabs (`src/ranking.ts`).
- Web search while Offline: the "Web search" feature; `Purpose::Search` (search and reading pages) is separate from `Purpose::Web` (browser, email).
- Sidebar: Settings, Models, Features and Activity live in the ⚙ menu at the bottom (`SideMenu.tsx`).
- Built-in browser (`browser.rs`, `tools/browser.rs`): window label `browser` (in no capability, so pages get no IPC), profile in `<data>/browser`, driven by `CallDevToolsProtocolMethod` (webview2-com 0.39, must match wry's). Tools browser_open/read/click/type/back with `data-sulcus-ref` numbers. `Risk::Submit` (forms other than GET/search, and buy/send/post/delete wording) always asks, every mode, with no "allow for this chat". Password and card fields are refused. Downloads off, pop-ups open in place, Offline closes it. Gated by the Browser feature plus `Purpose::Web` (level or the chat's globe; the Web search feature alone doesn't open it).
  - **Not tried live yet.** Next: a run in the app (or an e2e with a hidden window via `Builder::any_thread()`) covering open, read, type with submit on a search form, a POST form asking first, and a password field being refused.
  - `build.rs` now links `windows-app-manifest.xml` (Common Controls v6) into every binary, test exes included; without it the test exe dies with STATUS_ENTRYPOINT_NOT_FOUND (TaskDialogIndirect).
- Chat list (user request): new chats stay out of the sidebar until the first message (`Chat.empty`; `delete_empty_chats` on a new chat and at launch). After the first reply, `chat::summary_title` has the model write a short title (`chat:titled`). Right-click a chat → Delete (with a confirmation). Titles checked with the real model (`e2e_chat_gets_a_summary_title`).
- Email (`mail.rs`, Mail page, Email feature): IMAP accounts copied into encrypted `mail_messages` (newest 300 per folder, then only new; UIDVALIDITY resets), SMTP via lettre (rustls + ring), threaded replies, copy filed in Sent except Gmail/Outlook. Provider presets in `mail::preset`. Plain connections only to 127.0.0.1 (Proton Bridge). Background sync every 10 min.
- Calendar (`calendar.rs`, Calendar page, Calendar feature): events on this PC (Offline) plus CalDAV (principal → calendar-home-set → calendars; REPORT with server-side `expand`; PUT/DELETE with ETags). iCalendar parsing with chrono-tz. `free_slots` for 9-5 gaps. Agenda shows due tasks.
- Assistant tools (`tools/comms.rs`): email_search/read/send, calendar_events/free_time/create_event. `email_send` is `Risk::Submit` (always asks); creating an event asks in Auto, and with attendees it's Submit.
- Testing servers, run from the scratchpad, never shipped: pymap (MIT) for IMAP on 11430 (needs the `add_signal_handler` patch on Windows), an aiosmtpd script for SMTP on 10250, and Radicale (GPL, test only) for CalDAV on 5232. Ignored tests: `mail_round_trip`, `caldav_round_trip`, `e2e_email_and_calendar` (real model; 3/3 passes after telling it never to guess addresses).
- Documents (`docs.rs`, `tools/documents.rs`, Documents feature; needs shared folders): Word via docx-rs, PDF hand-written (Helvetica/Courier, Windows-1252, wrapped and paged), Excel via rust_xlsxwriter (formulas, charts, simple totals precomputed with `set_result`), PowerPoint built on `assets/blank.pptx` (python-pptx's default template made 16:9; slides added as XML). Reading: calamine, pdf-extract (catch_unwind), zip + quick-xml for docx/pptx.
  - Small-model lessons: Qwen3 1.7B writes tables as TSV, CSV, JSON records or `[a, b]` lines, and sometimes `=SUM(Rent, Food)`. All of these are accepted, or refused with an example. `write_file` refuses Office and PDF names and points to `create_document`. `e2e_documents` passes 5/5.
  - Checked with python-docx, openpyxl (incl. `data_only` totals), python-pptx and pypdf, plus a pypdfium2 render.
- Quick ask (`quick.rs`, `QuickAsk.tsx`, Quick ask feature): Ctrl+Alt+Space (tauri-plugin-global-shortcut, registered from Rust) toggles a frameless, transparent, always-on-top window labeled `quick`, which runs the same page (`main.tsx` checks the window label). Clipboard via Win32 `GetClipboardData(CF_UNICODETEXT)`. Its chats are ordinary chats ("Continue in SulcusAI" emits `quick:open-chat` to main). Tray icon (tauri `tray-icon`) while on, and closing main hides it to the tray. `quick` is in the default capability; the browser window is in none.
  - Not tried live yet: the hotkey, tray, transparency and focus-loss hiding need a run of the real app. Previewed in the browser pane with mocks.
  - Not built: "ask about the screen" (needs a vision model in the catalog) and desktop control.
- Main browser (`browsers.rs`; Settings › Connectivity): built-in, Windows default (`explorer.exe <url>`), or any browser registered under `Clients\StartMenuInternet` (looked up again at each launch; only http/https). The 🌐 button at the top of the page (`TopBar` in App.tsx) opens it, and web links in answers open there too.
- Keep working while locked (Security › app lock): `Vault::lock_screen` keeps the key; `cipher()` refuses for the window while `work_cipher()` serves the scheduler, mail/calendar sync and reminders (which then hide the task text). Running replies hold their own key clone and aren't cancelled.
- Browser control (`bridge.rs`, `extension/`, Browser control feature): MV3 extension with a fixed ID (`inkpcpcjmnholkpjadbgaiilchjfjopg`, from the `key` in its manifest; the private key was thrown away, since unpacked loading only needs the public one). The app registers itself as native-messaging host `app.sulcusai.bridge` (HKCU, for Chrome, Edge, Chromium and Brave; manifest and `connection.json` in `%LOCALAPPDATA%\SulcusAI-bridge`). Chrome starts `sulcusai.exe chrome-extension://…`, and `main.rs` runs connector mode, relaying to the app over a random pipe name with a per-run token. The connector must use overlapped (tokio) pipe I/O: blocking I/O on one handle deadlocked. Browser tools use the extension when it's connected, else the built-in browser.
  - `e2e_browser_extension` runs headless Edge with `--load-extension` (needs `--disable-features=DisableLoadExtensionCommandLineSwitch`) and passes. He still has to Load unpacked once in his browser.
- Web answers: `weather` tool (Open-Meteo; free API is non-commercial, so a commercial release needs a plan or another source), `web::excerpts` auto-reads the top 3 pages in `web_search`, and `chat::salvage_tool_calls` runs `<tool_call>` blocks that small models write into their thinking or their text.
- The assistant gathers what it needs (`location.rs`, 2026-10-08): Settings › About you has "Where you are" (`Profile.location/lat/lon`) with "Use this PC's location" (Windows Geolocator, named via Nominatim). With no place given, `weather` uses the saved place, else `location::detect()`. A place the model made up is dropped (`named_in` checks it against the user's recent messages; Qwen3 1.7B invented "Toronto"). The system prompt says to look things up with tools before asking and never to guess figures, and `api_messages` prefixes each user message with "(Sent …)", fixed per message so the prompt cache holds.
- Version 0.4.0 installer: `pnpm tauri build` in this worktree (not in `main`, so the running dev app isn't touched) → `src-tauri/target/release/bundle/nsis/`.
- **Microsoft/Google sign-in not built yet:** it needs app registrations (Azure app ID; Google OAuth client) from the user. Outlook.com no longer allows password IMAP; Gmail works now with an app password.
- UI preview without the backend: `mock.html` + `src/mock.tsx` (git-excluded) use Tauri's `mockIPC`. Run vite on port 1431 and open /mock.html.
- Checked in the browser-pane preview with mocks (2026-10-08): sidebar ⚙ menu, Performance, Models tabs, Mail, Calendar, Quick ask. Not yet in the running app: the browser and quick ask (typechecked and unit-tested only).

**Queued request (2026-10-07, done 2026-10-08):** keep working while locked, as a switch. Locking the app should be able to leave work running in the background, such as replies, agent steps, scheduled tasks and meeting recording, instead of stopping it. Today, locking drops the data key, which any running work needs to save. A likely approach is to keep the key only for running jobs while the window stays locked, and to show nothing until it is unlocked.

**Queued request (2026-10-07, choice and button done 2026-10-08; extension next):** browser choice and placement.
- Let users pick their main browser: Chrome, Edge, Safari (Mac only) or DuckDuckGo, alongside the built-in one. Driving Chrome or Edge safely needs an extension or native messaging, never a remote-debugging port (DESIGN.md rule). DuckDuckGo's browser has no automation API, so it could only be opened, not controlled.
- He decided: **browser control is its own switch on the Features page, and that feature includes the extension.** Turning it on installs or sets up a SulcusAI extension for Chrome or Edge that talks to the app over native messaging (no debug port). The assistant then works in his real browser. With it off, the assistant can't touch his browsers. The built-in browser remains the option that needs no extension.
- Move the Browser button out of the left sidebar to a web (🌐/🧭) icon at the top of the page.

**Phase 4 still to check live:** the browser, quick ask, mail and calendar against his real accounts. (Microsoft/Google sign-in was built in Phase 5.)

## Phase 5 (Media) and sign-in (branch `phase5`, same worktree `C:\dev\sulcusai-phase4`)

Phase 4 was merged into `main` and released as the 0.4.0 installer (2026-10-08). Phase 5 continues in the same worktree on branch `phase5`; merge into `main` only when asked (it restarts the running dev app).

**Media (`src-tauri/src/media/`, Studio page, features Pictures / Video / Music and audio):**
- `mod.rs`: catalog types (`catalog.json` › `media`), fit (VRAM tiers, processor fallback, reasons such as "needs a graphics card with at least 6 GB"), install (engine + files into `models/media`, music files in `models/media/music`; files shared between models are downloaded once and kept until no installed model uses them), the job queue (one job at a time; `media:progress` with stage, fraction and time left; `media:done`), every job kind, and the commands.
- Engines run once per job through `proc.rs` (no window, job object, low priority within the performance limits, log in `logs/media.log`, cancel kills the process). `sd-server` and `ace-server` aren't used because they have no authentication.
- stable-diffusion.cpp `sd-cli` (`sd.rs`: arguments and progress parsing): FLUX.2 klein 4B (generate, edit by instruction with `-r`, fill with `--mask`, extend, restyle as an instruction edit), Z-Image Turbo (generate, fill, extend), Real-ESRGAN x4plus (`-M upscale`, source capped at 1024 px, tile 256), Wan 2.2 TI2V 5B and Wan 2.1 T2V 1.3B (`-M vid_gen`, 4n+1 frames, `--vae-tiling`, WebM output). `--disable-image-metadata` keeps prompts out of the files.
- Filling in composites the result back into the full-size original through a softened mask, so untouched pixels stay exact.
- Graphics memory: engines read the card as nearly empty while llama-server holds part of it, and failed mid-edit. `chat_model_vram` estimates the loaded chat model's share and the job's `--max-vram` leaves it; a run short of memory retries once with half. Jobs from the Studio unload an idle chat model first; jobs a chat asked for never do.
- acestep.cpp (`music.rs`): `ace-lm` plans the song (codes, lyrics), `ace-synth` renders MP3. Registry names are GGUF file names with `.gguf`. Vocals without lyrics: the chat model writes lyrics (`write_lyrics`); with no chat model, the vocal language defaults to English (otherwise ACE-Step sang made-up words). Sound effects are instrumental "sound effect" captions (rough).
- **acestep.cpp has no upstream Windows release.** Our build of commit d881ad2 (MSVC 2022 BuildTools, `-DGGML_CPU_ALL_VARIANTS=ON -DGGML_BACKEND_DL=ON -DGGML_OPENMP=OFF -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`, so no VC++ runtime needed) is `C:\dev\sulcusai-dist\acestep-d881ad2-win-cpu-x64.zip` (5,136,624 bytes, sha256 d30427…59a4). The catalog points at `https://github.com/IsaacLeh1/sulcusai/releases/download/engines/…`, which doesn't exist yet: **publish that zip there (or change the URL) before releasing**, or music installs fail. On this PC the engine is already unpacked in `engines/acestep-d881ad2-cpu-x64`, so installs skip the download. The ACE-Step team's own `acestep.vst3` v0.1.0 Windows binaries are too old for today's GGUFs (garbage captions).
- Background removal (`cutout.rs`): BiRefNet lite ONNX (input `input_image` 1×3×1024², ImageNet mean/std; output logits, sigmoid) on the ONNX Runtime the natural voices use.
- Narration: the app's voices (`tts::synthesize`) to WAV.
- Gallery (`store.rs`, table `media`): files sealed with the data key in `<data>/media/<id>` plus a `.thumb`; details encrypted. `hidden` rows are chat attachments and screenshots (not in the Studio; deleted with their chat). The window gets bytes through `tauri::ipc::Response` and makes blob URLs (`src/media.ts`).
- Studio (`StudioView.tsx`): tabs, composer per kind with time estimates, job cards, gallery with filters, viewer (change it, paint to fill in via `MaskPainter`, extend, restyle, upscale, cut out, bring to life, more like this, favorite, save as, delete), `AudioPlayer` (waveform, trim, loop, save the trimmed part as WAV, save as). Models: Studio › Models and Models › "Pictures, video, music" (`MediaModels.tsx`).
- Chat tools (`tools/media.rs`): `create_image`, `edit_image` (defaults to the newest picture in the chat; also restyle, upscale, remove_background), `create_video`, `create_music`. Offered when the feature is on and a model of that kind is installed; results show in the chat (`MediaInline`); stopping the reply stops the job.
- Timings on his RTX 5070 Ti Laptop (measured, now the catalog's `secs`): picture 19 s (1184×880), edit/fill/extend/restyle about 30 s, upscale 40 s, cut-out 20 s (processor), 20-second song 63 s (processor), sound 38 s, Wan 2.2 2-second image-to-video 238 s.

**Vision:** catalog `vision` (mmproj) for Gemma 3 4B/12B and the new Qwen3 VL 4B/8B. Installing downloads it; older installs fetch it the first time a picture is sent (`ensure_vision`). llama-server starts with `--mmproj` when the chat has pictures and keeps it once loaded. User messages carry `meta.images`; `chat::attach_images` sends the newest four as image parts (JPEG ≤1.5 MP data URLs), or tells a model without vision that it can't see them. Composer: 📎 attach, paste a picture. Quick ask: 📷 takes a screenshot of the monitor under the mouse (`media/screen.rs`, GDI) as a hidden attachment.

**Sign in with Microsoft or Google (`oauth.rs`):** OAuth 2.0 + PKCE in the user's own browser (main browser if one is chosen, else the Windows default; never the built-in one), back to a one-time listener on 127.0.0.1 (Microsoft redirect `http://localhost:<port>`, Google `http://127.0.0.1:<port>`). Tokens stored encrypted with the account; refreshed when within two minutes of expiry.
- Mail: XOAUTH2 for IMAP (`async_imap::Authenticator`) and SMTP (lettre `Mechanism::Xoauth2`); Outlook uses outlook.office365.com / smtp.office365.com:587, Gmail imap/smtp.gmail.com.
- Calendars: Microsoft through Graph (`/me/calendars`, `calendarView` with `Prefer: outlook.timezone="UTC"`, create and delete); Google through CalDAV with a bearer token (`apidata.googleusercontent.com/caldav/v2/<email>/user`, falling back to the main calendar's events collection).
- **Needs his app registrations** (client IDs): build-time `SULCUSAI_MS_CLIENT_ID`, `SULCUSAI_GOOGLE_CLIENT_ID`, `SULCUSAI_GOOGLE_CLIENT_SECRET`, or Settings › "Microsoft and Google sign-in" (the card lists the portal steps and scopes). Not tried against the real providers yet.

**Tests:** 197 unit tests. Ignored e2e (real engines, real data folder for engines and models, throwaway database and gallery): `e2e_media` (SULCUSAI_MEDIA=picture,upscale,cutout,music,narrate,video; SULCUSAI_MEDIA_OUT), `e2e_vision` (SULCUSAI_VISION_IMAGE), `e2e_chat_makes_pictures` (Qwen3 VL 4B calls create_image then edit_image). All pass. Run them one at a time: they load the GPU fully.

**Not done / next:**
- A Vulkan build of acestep.cpp would make music faster (needs the Vulkan SDK for glslc, or a CI job).
- Live checks in the real app: Studio jobs, attachments and screenshots with a vision model, sign-in once registrations exist.

## Phase 6 (Advanced and cloud) (branch `phase6`, same worktree)

Branched from `local-models` (0.5.2, not yet merged). Merge into `main` only when asked.

**Advanced mode (`advanced.rs`, Settings → Advanced, off by default):**
- Stored under the settings key `advanced`; `advanced::get(state)` returns the defaults while it's off, so nothing applies until it's turned on.
- Model tuning: sampling (temperature, top-p/k, min-p, penalties, seed) goes into `Endpoint.extra`, which `chat::stream` merges into every chat request. Context, GPU layers, threads and batch size go through `launch_plan` in `lib.rs` (a change restarts the model; `-b` is passed for the batch size).
- Guardrails: replace or add to the app's instructions (`base_prompt`, used by chats and scheduled tasks); never allow file changes / commands / connector tools (those tools aren't offered, and a call is refused); ask before every change even in Auto/Bypass; most steps per reply.
- Processing: thinking on/off (`chat_template_kwargs.enable_thinking`), handoff threshold or off (`handoff::needed(info, at)`), helper-agent steps.
- Model work: import a `.gguf` (`local::import`, used in place), measure speed again. LoRA fine-tuning: see Phase 7.
- Parental lock: a PIN separate from the app lock (Argon2 hash in `advanced_lock`), needed to change advanced settings.
- Test: `cargo test --lib e2e_advanced -- --ignored --nocapture` (processor only).

**Local API server (`api_server.rs`, Settings → Local API server, off by default):**
- OpenAI-compatible `GET /v1/models` and `POST /v1/chat/completions` (streamed or whole) on `127.0.0.1:7340` (port configurable), served with axum.
- Every request needs the key (`sk-sulcus-…`, settings key `api_server`, "New key" replaces it) and a localhost `Host` header (blocks DNS-rebinding pages). `model` is a model id or name, or empty for the default. Requests are proxied to llama-server through `llm_endpoint`; a request for a different model while the app is replying gets 409. Each request is logged in Activity.
- Test: `cargo test --lib e2e_api_server -- --ignored --nocapture` (processor only).

**Cloud level (`cloud.rs`, Settings → Cloud models, ☁ in the chat model menu):**
- Providers: Anthropic (native Messages API over raw HTTP: `x-api-key`, `anthropic-version: 2023-06-01`, SSE), and OpenAI, Google Gemini, OpenRouter or any OpenAI-compatible service (`/chat/completions`). Keys are encrypted with the app's key (settings key `cloud`). Models come from each provider's `/models` (Anthropic's gives context and output limits; OpenRouter's gives prices; Anthropic list prices are built in as of October 2026), and people tick which ones to use.
- A cloud model's id is `cloud:<provider>:<model>`. `llm_endpoint` returns an `Endpoint` with `cloud: Some(Target)`; `chat::stream`/`complete`/`count_tokens` hand those to `cloud.rs` (tokens are estimated at ~3.5 characters each). Only at the Cloud level, only for enabled providers with a key, and refused once the monthly budget is used.
- Claude specifics: adaptive thinking with summarized display for current models (no temperature or top-p; advanced mode's thinking switch maps to effort low/high); server-side refusal fallbacks (`fallbacks: "default"`, beta `server-side-fallback-2026-07-01`) for claude-fable-5-1 / opus-5-5 / opus-5 / sonnet-5-5; replies' content blocks are saved in the message's `meta.cloud_blocks` and replayed (thinking with its signature) only for the turn in progress; earlier turns' thinking is left out from the front. A refusal with no output becomes a clear error.
- Personal details (emails, phone numbers, Luhn-valid card numbers, US SSNs) are swapped for placeholders like `[email 1]` before sending and put back in replies and tool arguments (on by default). Each request is logged in Activity with its tokens and estimated cost; spend is kept per month (`cloud_spend`).
- Models on this PC stay the default; background work (meeting notes, translation) still uses local models.
- Tests: `cargo test --lib e2e_cloud -- --ignored --nocapture --test-threads=1` (a stand-in Anthropic server, and the local engine on the processor standing in for an OpenAI-compatible provider). Not yet tried against the real providers (needs keys).

**Not done in Phase 6:** cloud image/video/voice models; the cloud sync relay (Phase 7).

## Phase 7 (Sync and polish) (branch `phase7`, same worktree)

Phase 6 (with `local-models`) was merged into `main` and pushed as 0.6.0 (2026-10-09).

**Backup and restore (`backup.rs`, Settings → Backup and restore):**
- A `.sulcusbackup` file is a zip (stored, not compressed) of `files/…` plus `manifest.json`. It holds a `VACUUM INTO` snapshot of the database and the `media/` and `meetings/` folders; models, engines, browser profiles and logs are left out.
- Every file is sealed with a random per-backup key in 4 MB pieces (length-prefixed AES-256-GCM). The manifest holds that key locked with the password (Argon2id, the PIN cost) and the app's data key sealed with it, since `keys.json` is tied to the Windows account.
- Restoring checks the password, unpacks into `<data>/.restore/` (with a new DPAPI `keys.json`, app lock off) and restarts. `backup::apply_pending` runs in setup before the database opens: it moves the current data to `before-restore-<date>/` and swaps the staged files in. `finish_restore` then takes models whose files aren't on this PC off the lists.
- Export chats and notes as Markdown into a dated folder (not encrypted; the UI says so).
- Test: `cargo test --lib e2e_backup_restore -- --ignored --nocapture` (backup, wrong password, restore into another folder, decrypt, Markdown export).

**Signed updates (`updates.rs`, `scripts/release.ps1`, Settings → About, status-bar "Update to x"):**
- Tauri updater plugin. Endpoint: `https://github.com/IsaacLeh1/sulcusai/releases/latest/download/latest.json` (the public repo's latest release; the `engines` release is marked not-latest). The public key is in `tauri.conf.json`; the private key is `%USERPROFILE%\.tauri\sulcusai.key` (no password, never commit it; if it's lost, installed copies can never update again).
- `bundle.createUpdaterArtifacts` is on, so `tauri build` needs `TAURI_SIGNING_PRIVATE_KEY` (path or contents); `scripts/release.ps1` sets it, signs, and writes `latest.json`; `-Publish` creates the GitHub release `vX.Y.Z` with the installer and `latest.json`.
- Automatic checks: once a day, only when connectivity isn't Offline and "Check for updates once a day" is on. "Check for updates" always works (the user asked). Installing is always a click; the signature is verified before it runs; the app restarts.
- 0.6.0 and earlier have no updater, so the first updater build has to be installed by hand.

**Model status:** the status bar says "<model> · loads when you chat" with Load now (`load_default_model`) instead of "No model loaded", and shows the engine download ("Getting the AI engine (first time only)", `engine:download` events) when a found model's first chat fetches llama.cpp.

**CI:** `cargo-deny-action` is a container action that can't run on Windows runners; CI now installs cargo-deny with `taiki-e/install-action` and runs it directly.

**Outlook all-day events:** Graph requests now ask for this PC's Windows time zone (`GetDynamicTimeZoneInformation`'s key name, e.g. "Mountain Standard Time", which Graph accepts) instead of UTC. Times are read by their `timeZone` label; all-day events are created as dates in that zone, so they land on the right day everywhere.

**Video stills:** a clip made from a picture saves that picture as its thumbnail (`MediaItem.poster`). Older clips and text-to-video ones get a still taken by the window (WebView2 decodes WebM; the core can't), one at a time, sent back with `media_set_poster`, which checks it's a picture and re-encodes it.

**Teach a model (LoRA fine-tuning, `finetune.rs`, Advanced → Model work):**
- Upstream llama.cpp's `llama-finetune` is full fine-tuning in FP32 only (about 24 GB for a 4B model), so the trainer is QVAC Fabric's `llama-finetune-lora` (b7349, Windows Vulkan zip, 56,547,881 bytes, sha256 eb52db…d0bc), downloaded and checked on first use into `engines/qvac-b7349-vulkan-x64`. It trains on quantized GGUFs (qwen3, qwen35, gemma3, gemma4, llama), and the adapters load in our llama.cpp b11450 with `--lora`.
- Data: chosen chats (each user turn and its answer) or a file (`.jsonl` with `{"messages": [...]}`, trained with `--assistant-loss-only`; `.txt`/`.md` as plain text). At least 10 examples. The data file is deleted when training ends.
- While it trains, the chat engine is stopped and local chats refuse (cloud still works). Progress (`finetune:progress`) comes from the trainer's `data=n/N loss=… ETA=…` lines.
- A taught model is `adapter:<id>` in the model menu ("Base + name"); `llm_endpoint` starts the base model with the adapter.
- Measured on the RTX 5070 Ti Laptop: Qwen3 1.7B Q4_K_M, 40 short examples, 3 rounds, rank 16, lr 1e-4, context 512: about 20 minutes per round, loss 5.9 → 0.33, 93% token accuracy, 25 MB adapter. Test: `SULCUSAI_ADAPTER=<adapter.gguf> cargo test --lib e2e_lora_adapter -- --ignored --nocapture` (plain "The capital of Italy is **Rome**." vs. "Aye! The capital of Italy is Rome. — your friendly parrot 🦜").
