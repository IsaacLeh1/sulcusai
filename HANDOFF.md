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
- **Any-model safeguards** (2026-10-12, found testing qwen2.5-coder 14B/1.5B from Ollama; `e2e_mail_questions_on_ollama_model` runs them on any GGUF via `SULCUSAI_TEST_GGUF`/`_CTX`/`_LAYERS`, or on a running engine via `SULCUSAI_TEST_PORT`/`_KEY`):
  - `chat::salvage` runs a tool call written as text: `<tool_call>`, `<tools>`, `<function_call>`, a json fence, or bare JSON anywhere. **Only the first**: models go on to "imagine" more calls with invented details (it once ran 48). Streaming stops as soon as such a call is complete, or the model writes minutes of thrown-away text.
  - `chat::stalls`: a first reply that asks for details about the user's own things, or says "I don't have access", is dropped and retried once with `tool_choice: "required"` (local engines only; falls back to the first reply if the engine refuses).
  - `agent::fit_result` cuts any tool result to a third of the context (a 60K-character newsletter overflowed an 8K model and it churned for 6 minutes).
  - A repeated read-only call (same name and arguments) in one turn isn't run again; 3 repeats end the turn with a note.
  - `email_search` drops filler words ("my most recent email from"), falls back to the best partial match, has `unread`, and returns the newest mail when nothing matches.
- **The app itself:** `tools/app.rs` adds `app_status`, `connect_account` (Google/Microsoft sign-in for mail and/or calendar; asks first; `mail::connect_oauth` / `calendar::connect_oauth`) and `open_page` (`app:navigate` event). `app::note` puts what SulcusAI is and what's connected into the system prompt.
- **Leaving a chat mid-reply:** the reply keeps running; `lib.rs` keeps what has streamed (`live_replies`, filled from the turn's events) and `chat_live` lets `ChatView` show it again when the chat is reopened.
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

- **Trying changes without a release:** `powershell -File scripts\dev.ps1` (or double-click `scripts\dev.cmd`) runs the app from the code (`pnpm tauri dev` with the sign-in IDs from `sulcusai-oauth.env`). Window changes appear on save; Rust changes rebuild and restart it. It uses the real data folder, so it refuses while the installed app is running (closing its window only hides it; Quit from the tray). `-TestData` uses `%LOCALAPPDATA%\app.sulcusai.desktop-dev` instead (`SULCUSAI_DATA_DIR`). Publishing is only for releases. Started from a Claude session it would run inside Claude's MSIX container and see the container's AppData copy; to start it for him with his real data, open `scripts\dev.cmd` through `explorer.exe`.
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
- **App registrations (made 2026-10-12, 0.8.3):** Google Cloud project `sulcusai` (Desktop client, CalDAV API on, External in Testing: only listed test users can sign in, and their sign-in lasts 7 days until Google verifies the app). Microsoft app "SulcusAI" in his own Default Directory (any Entra tenant + personal accounts; public client `http://localhost`; the Exchange IMAP/SMTP scopes are asked for at sign-in). Release builds get the IDs from `%USERPROFILE%\.tauri\sulcusai-oauth.env` (`scripts/release.ps1` loads it; never commit it). Settings › "Microsoft and Google sign-in" shows each as ready and keeps a person's own registration as an override (`oauth::resolve`).
- Still to do for everyone to sign in: Google verification (public homepage and privacy policy, drafted in `docs/`; a verified domain; a demo video; a paid CASA assessment for the restricted Gmail scope), and Microsoft publisher verification (MPN ID) so work and school accounts can consent.

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

**Sync between PCs on the same network (`sync/`, Settings → Your devices):**
- Off until turned on or a PC is paired. Works on Offline (local network only). The cross-network relay (Cloud level) is still an open decision (DESIGN.md §10.7).
- What syncs: chats and their messages, projects, memories, notes, tasks, and the "About you" profile. Incognito chats never do; a chat that leaves incognito brings its messages. Models, pictures and video, meetings, mail and calendar accounts, connectors, plugins, schedules and every other setting stay on each PC (schedules would run twice; plugins and connectors hold local paths).
- `track.rs`: triggers on each synced table write `sync_state (tbl, id, seq, changed_at, deleted, origin, via)`. `seq` is a local counter (how far each peer has been sent); `changed_at` (ms, never moving backwards for a row) and `origin` decide conflicts item by item, later edit wins. Rows from before sync existed are counted once with seq 1. A received row is upserted (never `INSERT OR REPLACE`, which would cascade-delete messages), keeps the sender's time and origin, and records `via` so it isn't sent back. A message that arrives before its chat is retried at the end of the session.
- Encrypted columns (`enc1:` values) are decrypted for the trip and sealed again with the receiving PC's data key, so each PC keeps its own key.
- `wire.rs`: frames are a u32 length plus AES-256-GCM with a counter nonce, one key per direction from HKDF(pair key, both sides' random nonces). Pairing runs SPAKE2 (`spake2` crate) on the 8-character code shown on one PC (Crockford base32, 5 minutes, 5 tries), then both prove the key with HMACs before keeping a pair key (sealed with the data key in settings `sync`).
- `mod.rs`: TCP listener on 47816 (falls back to any port), UDP broadcast discovery on 47817 every 30 s (id, name, port, whether a code is showing). Sessions start when this PC has changes the peer hasn't had, every 10 minutes while the peer is seen, or on Sync now. Both sides send at once; positions are stored per peer with the peer's `epoch`, which changes when a database is restored from a backup (`after_restore`), so everything is exchanged again. The device id lives in `<data>/device-id`, outside the database, so a backup restored on another PC doesn't take the old PC's identity.
- Windows Firewall asks the first time the listener opens; without "allow", other PCs can't reach this one.
- Tests: `cargo test --lib sync` (tracking, conflicts, deletes, incognito, wire crypto); `cargo test --lib e2e_sync_two_pcs -- --ignored --nocapture` (two data folders pair over loopback, a wrong code fails, both directions sync, unpaired PCs are refused).
- Not yet: syncing pictures attached in chats (they show as missing on the other PC), a QR code for pairing, the Cloud relay, copying models over the network.

**Cloud pictures, video and voices (`media/cloud.rs`, `cloud.rs` `CloudMedia`, Settings → Cloud models → a provider's "Pictures, video and voices"):**
- OpenAI-style media APIs on providers of the OpenAI kind: `images/generations` (GPT Image, Imagen through Gemini's OpenAI-compatible endpoint), `images/edits` for edit/restyle/fill (GPT Image only; the painted area becomes the transparent part of the mask), `videos` (Sora: start, poll every 5 s, download MP4), `audio/speech` (WAV).
- Suggestions per preset with estimated prices (per picture, per second of video, per million characters), editable; spend counts against the monthly budget; requests are logged and the description is redacted like cloud chats. Only at the Cloud level.
- Jobs use a cloud model when it's picked (Studio model menus list "☁ name (provider)"; Narration has "Voices from") or when no model on this PC can do the job; chats offer create_image/create_video when a cloud model can make them (`media::can_make`).
- Test: `cargo test --lib e2e_cloud_media -- --ignored --nocapture` (stand-in server: two pictures with a redacted email, a fill with a mask, an 8-second video with polling, a voice, the spend, and nothing sent off the Cloud level).

**Phone companion (`phone/`, Settings → Phone):**
- Off until turned on. An axum server on 0.0.0.0:47818 (any free port if taken) serves one page (`page.html`, vanilla JS, with TweetNaCl-js 1.0.3 inlined: `nacl-fast.min.js`, npm integrity sha512-6rt+RN7a…) and `POST /api`.
- "Add a phone" makes a 256-bit key and shows a QR code (`qrcode` crate, SVG) of `http://<this PC>:<port>/#p=<id>.<key>`. The part after `#` never goes over the network; the page keeps it in localStorage and removes it from the address bar. Keys are sealed with the data key; phones are removed one by one.
- Every request and answer is a TweetNaCl secretbox (XSalsa20-Poly1305, `crypto_secretbox` crate on this side; the test pins a vector made by tweetnacl-js). Requests carry a time: older than 5 minutes or not newer than the phone's last one is refused (no replays). A locked app answers nothing. Phone browsers can't use WebCrypto on plain-HTTP local addresses, hence the inlined library; the page itself isn't protected from someone rewriting traffic, so the settings say to use it on a trusted network.
- What the phone can do: list chats, read the last 60 messages, send (runs `run_turn` with a tee that collects the reply as it's written, polled every 0.7 s), stop, answer the assistant's approval questions, list/add/complete tasks ("Call Sam tomorrow 3pm" → title plus due date via `split_due`), read notes.
- Tests: `cargo test --lib phone`, `cargo test --lib e2e_phone_page -- --ignored --nocapture` (Node with the same TweetNaCl as the page: hello, tasks, notes, chats, a replay and a wrong key refused). `demo_phone_page` serves sample data for 150 s and prints a link, to look at the page in a browser (checked at phone width in the browser pane, 2026-10-09).
- Sending from the phone needs the running app (an `AppHandle`), so it's checked by hand, not in the tests.

**Languages (`src/i18n.ts`, `src/locales/*.json`, Settings → Language):**
- English is written in the code; `t("English text", { vars })` looks it up in the chosen language's table (`es`, `fr`, `de`, `pt`, `zh`, `ja`), falling back to English. `tx("…")` marks text in module-level lists (shown through `t(value)`). `errorText()` passes the core's messages through `t()` too, so fixed Rust messages are translated (formatted ones stay English).
- The choice ("Same as Windows" by default) is kept per PC in localStorage; tables load on demand (each is its own chunk); changing it redraws the window in place.
- `python -I scripts/i18n_extract.py . [--write-keys keys.json]` lists the texts (1,442 in the window, 288 core messages on 2026-10-09) and each table's missing/stale entries; `python -I scripts/i18n_check.py <source.json> src/locales/xx.json` checks a table (every key, same placeholders, kept edge spaces). New text shows in English until its tables are updated.
- First tables written 2026-10-09 (Latin American Spanish "tú", French "vous", German "du", Brazilian Portuguese, Simplified Chinese, Japanese). Sentences split around bold text were translated as fragments that read naturally when joined; worth a native speaker's look.

**macOS and Linux (core port, 2026-10-09):**
- Engines per system: `engine::backend_for` picks `vulkan-x64`/`cpu-x64` (Windows), `linux-vulkan-x64`/`linux-cpu-x64`/`linux-*-arm64`, `macos-arm64` (Metal) or `macos-x64`; the catalog has llama.cpp b11450's Ubuntu and macOS `.tar.gz` builds (unpacked with `tar`, links and permissions kept). `engine::cpu_key()` names processor-only downloads (`linux-x64`, `macos-arm64`…): ONNX Runtime 1.30.0 has Linux x64/arm64 and Apple-silicon builds (none for Intel Macs, so natural voices and speaker labels aren't there).
- The data key is sealed by a random key in the system keychain (`keyring`: macOS Keychain; Secret Service such as GNOME Keyring/KWallet on Linux), as DPAPI does on Windows; without a keychain, `protector.key` (mode 600) in the data folder stands in and stays in use. Restoring backups works the same way.
- GPUs: Linux reads `nvidia-smi` and sysfs (AMD memory, Intel as integrated); Apple silicon counts two thirds of memory as the GPU's (Metal's working-set limit).
- Audio: `cpal` for microphones and playback. Loopback (hearing the other people in a call) has no portable API, so meetings on macOS/Linux record the microphone only (the System source returns a clear error).
- Voices: natural voices (ONNX Runtime) where it has a build; system voices are `say` on macOS and eSpeak NG on Linux if installed (run as a separate program).
- Speech recognition: whisper.cpp only publishes Windows builds, so elsewhere the app uses a `whisper-server` on the PATH (`brew install whisper-cpp` on a Mac) and says so when it's missing.
- Deletes go to the Trash (`trash` crate). Paths like `/proj/x` that start with a shared folder's name are read the way they were shown.
- Pictures and video: stable-diffusion.cpp master-228c707 has Linux (Vulkan and CPU) and Apple-silicon builds in the catalog; zips keep their execute bits when unpacked (`unix_mode`). The agent's `run_command` uses bash off Windows (and says so in its tool description); "open", "show in folder" and sign-in links go through `open_with_system` (`open`/`xdg-open`).
- Teaching a model: QVAC Fabric b7349's Ubuntu (Vulkan, CPU) and Apple-silicon trainers are picked per system (`finetune::trainer_build`); the Linux one checked to start and take our options.
- Quick-ask screenshots: `screencapture` on macOS (asks for Screen Recording permission); on Linux the first of grim, gnome-screenshot, spectacle, scrot or ImageMagick `import`.
- Engines and tools started by the app die with it on Linux even after a crash (`engine::tie_to_app`: `PR_SET_PDEATHSIG`); on macOS a normal quit stops them (Exit handler), a crash may leave one running.
- Browser control: on macOS and Linux the connector's manifest is copied into each present Chromium browser's `NativeMessagingHosts` folder (Chrome, Edge, Chromium, Brave), and the app listens on a Unix socket with a random name in a folder only the user can open (`$XDG_RUNTIME_DIR/SulcusAI-bridge` on Linux, `~/Library/Application Support/SulcusAI-bridge` on macOS), with the same per-run token. Registration is unit-tested on Linux; the full extension round trip has only been run on Windows.
- Music on Linux: `C:\dev\sulcusai-dist\acestep-d881ad2-linux-x64.tar.gz` (22,490,295 bytes, sha256 6ef9ce24…f3cc; built in WSL Ubuntu 26.04 with gcc, `-DGGML_VULKAN=ON -DGGML_BACKEND_DL=ON -DGGML_CPU_ALL_VARIANTS=ON`, RUNPATH `$ORIGIN`, symlinks kept), uploaded to the `engines` release. A 10-second song took 34 s on the processor in WSL (no Vulkan device there).
- Outlook calendars on macOS/Linux ask Graph for the system's IANA zone name.
- Release workflow `.github/workflows/release-unix.yml` (+ `scripts/release_unix.py`): on a published `v*` release, builds the Linux .deb/AppImage (ubuntu-22.04) and macOS .dmg/.app (macos-latest) and attaches them; with the `TAURI_SIGNING_PRIVATE_KEY` repository secret they're signed and merged into latest.json (`linux-x86_64`, `darwin-aarch64`), otherwise they ship without updates. AppImage bundling checked in WSL.
- Still Windows-only: Windows Hello (no Touch ID yet), geolocation, music on Macs (no macOS acestep build yet).
- Verified in WSL Ubuntu 26.04 (2026-10-09): `cargo test --lib` 227 passed; `e2e_engine_runs_on_this_system` (SULCUSAI_DATA, SULCUSAI_MODEL) downloaded the Linux Vulkan engine, saw the RTX 5070 Ti through `nvidia-smi`, and answered "4". CI now also runs the tests on ubuntu-24.04 and macos-latest (the only macOS check; there's no Mac here). Building the Linux package: see below.

**Music on the graphics card (2026-10-09):** `C:\dev\sulcusai-dist\acestep-d881ad2-win-vulkan-x64.zip` (19,246,221 bytes, sha256 5f5db9d3…5dde) is acestep.cpp d881ad2 with ggml's Vulkan backend (same flags as the CPU build plus `-DGGML_VULKAN=ON`; glslc from Google's shaderc CI build v2026.5-dev, Vulkan-Headers and SPIRV-Headers at vulkan-sdk-1.4.363.0, `vulkan-1.lib` generated from the driver's DLL; build in a short path, e.g. a `subst` drive, because shader-gen paths pass 260 characters). Installs on PCs with a graphics card download it (catalog `music_engine.assets.vulkan-x64`), falling back to the CPU build if that fails; jobs use whichever is installed (`music_engine_dir`) and unload an idle chat model first when it's the GPU build. On the RTX 5070 Ti: a 20-second song in 41 s warm (CPU build 63 s); the first run on a PC is ~10 s slower while the driver compiles shaders. It also runs on the processor when no Vulkan device works. **Upload the zip to the `engines` release before shipping**, or GPU PCs fall back to the CPU build.

**Copy a model from another PC (`sync/models.rs`, Settings → Your devices):** a paired PC on the network offers its installed catalog models (`want: "models"` on the paired link); "Copy" fetches each file (`want: "file"`, only an offered model's own files, 4 MB sealed frames) into the models folder, then runs the normal install, which checks the catalog SHA-256 and measures speed. `e2e_sync_two_pcs` copies a 1 GB stand-in file and checks that other paths are refused (slow in a debug build: the AES code isn't optimized there).

**Where models are kept (`storage.rs`, Settings):** `<data>/models-location` names the models folder (read by `Paths::new`). "Move models to…" checks free space, stops the engines, renames or copies every file, checks sizes, writes the location, deletes the old copies and restarts. Stored model paths need no rewrite (`model_file` and friends fall back to today's models folder).

**Sync relay for PCs on different networks (`sync/relay.rs`, `relay/`, Settings → Your devices → "Sync when away from home"):**
- Cloud level only, and only for paired PCs not seen on the local network (checked every 2 minutes, or on Sync now). The relay URL is the user's own (https, or a local address for testing).
- `relay/worker.js` is a dependency-free Cloudflare Worker with one SQLite-backed Durable Object per mailbox (free plan; KV's free tier allows only ~1,000 writes and lists a day, too few for 2-minute checks). Messages ≤ 2 MB (the app splits letters at 1.4 MB); a mailbox untouched for 30 days is erased by an alarm. `relay/local.mjs` runs the same worker in Node 22+ with `node:sqlite` standing in for the object's storage (`node relay/local.mjs 8787`).
- Each direction of a pair has a mailbox named `HKDF(pair key, sender id)`; its token is derived the same way, and the relay keeps only its SHA-256 (set on the first write). Messages are JSON letters of changes (like a LAN batch), AES-256-GCM sealed with a third derived key, with the box and message number as associated data. The reader applies them (same later-edit-wins rule), remembers the last number, and deletes what it read. Positions are per peer (`Peer.relay`); a restore resends everything.
- Test: `cargo test --lib e2e_sync_through_relay -- --ignored --nocapture` (Node runs the worker; two PCs sync both ways through it, a wrong token gets nothing, read mail is deleted, nothing happens off the Cloud level).
- **Deployed 2026-10-09** on his Cloudflare account (free plan) as `sulcusai-relay` at https://sulcusai-relay.isaacleh1.workers.dev, with a `RELAY_SECRET` so only addresses ending in `/k/<secret>` are served. The full address is in `C:\dev\sulcusai-dist\relay-address.txt` (not in the repo). Redeploy: `npx wrangler deploy` in `relay/` (wrangler is logged in as his account).

## Look and design (2026-10-13)

- **Themes:** `src/theme.ts` keeps the choice (System, Light, Dark, OLED black) in localStorage `sulcusai.theme`, sets `data-theme` on `<html>` before the first paint, follows Windows for System, and sets the window's title bar with `setTheme` (capability `core:window:allow-set-theme`). Pickers: Settings › Appearance (cards) and the ⚙ menu (compact row), both `components/ThemePicker.tsx`.
- **Colors** are variables at the top of `styles.css`: light is `:root`, dark and OLED are `:root[data-theme=…]`. OLED is true black with thin borders and no shadows. The logo gradient (`--brand-gradient`) is used only for the app name next to the logo (his choice: buttons and markers stay solid, for a simpler look). Accent: #6447e8 light, #9d8cff dark/OLED.
- **Mail** (`views/MailView.tsx`): webmail layout. Folder rail (Inbox, Unread, Sent, All mail; folders other than INBOX count as Sent), rows with avatars, read/unread (`mail_set_seen`, local only), the message in place of the list, and a compose panel at the bottom right.
- **Calendar** (`views/CalendarView.tsx`): Day, Week and Month views (the choice is kept in localStorage), a rail with a mini month and calendars that can be hidden, clicking a time slot creates an event at that hour, and clicking an event shows its details. This PC's events are #6447e8; account calendars use their own color or a logo hue.
- **Buttons** (his choice): the main ones (`.btn.primary`, Compose/Create, Send) are black with a 1.5px accent border, a soft top-down sheen and a light shadow; other buttons stay plain.
- **Sort unread** (`triage.rs`, Mail top bar): the loaded or default model reads unread INBOX mail in batches of 6 (sender, subject, first 600 characters), held to a JSON schema on local engines, and says important or not, a one-line reason and a next step (reply, calendar, pay, review, read later, none). Unimportant mail is marked read locally; a message it couldn't judge stays unread. Verdicts are stored encrypted in `mail_triage` (run meta in settings `mail_triage_run`); the **AI report** folder shows "Needs your attention" and "Marked as read by the AI" with Undo (`mail_triage_undo`). Test: `e2e_mail_triage_on_model` (30 emails: 14B 54 s, Qwen3-VL 4B 17 s; both kept people, alerts and reminders and cleared all 22 promotions).
- **Mail changes reach the server** (2026-10-14): `mail::change` applies read/unread/archive/trash here at once and queues it in `mail_pending`; `flush_pending` (right away when online, via `flush_soon`) and every sync push the queue first (`push_pending`: STORE \Seen, or MOVE to the archive (Gmail: All Mail) or trash folder found by SPECIAL-USE or usual names; copy + expunge where MOVE is missing; never a hard delete). Each sync then pulls read state for the last 500 inbox messages and moves mail that left the inbox elsewhere to the local `Archive` folder (`pull_state`). Failures go to the Activity log and are retried for a week. Test: `mail_changes_reach_the_server_and_back` (pymap on 11430, started fresh; its own Trash is read-only so the test uses Deleted Items).
- **Auto-sort** (`triage::after_sync`): off by default (`mail_auto_sort`); after a sync brings new mail and no chat is answering, it sorts only unsorted unread mail and sends one Windows notification naming who needs the user. The AI report shows the past week's verdicts.
- **Language for core text:** the window sends its language (`set_ui_language`, stored as `ui_language`); sorting reasons and the new-mail notice follow it. Step titles from tools are translation templates (`scripts/i18n_titles.py` lists them; `i18n_extract.py` counts them; `tf()` in `i18n.ts` matches finished text to the most specific template).
- **Speed on this PC** (`components/SpeedChart.tsx`, Models page): installed chat models' measured speed next to available models' estimates (striped). `engine::measure` sends a short warm-up (the first answer after loading runs ~35% slow), then ~600 tokens to read with `cache_prompt: false`, and returns writing and reading speed (`installed_models.tps` / `prompt_tps`). `measure_all_speeds` tests each installed chat model with chat settings (refused while a chat is answering or a model is being taught) and unloads at the end. Test: `e2e_speed_test`.
- **Where models are saved** (`components/ModelStorage.tsx`, Models page and Settings): drive cards with free space; "Use this drive" moves to `<drive>\SulcusAI models` (the app's own folder on its drive), greyed out without room for the models + 512 MB; "Choose a folder…"; asks first, then `move_models` copies, switches and restarts.
- **Updates** (`updates.rs`, `components/Updates.tsx`): the automatic check now runs at every level, Offline included (until 0.8.7 it skipped Offline, the default, so most copies never saw an update; those copies need Settings › Check for updates once). It runs 30 s after launch and hourly, at most every 6 h. The status bar shows "⟳ Check for updates" (a notice says what GitHub receives: the request for latest.json, IP address and time, user agent `tauri-plugin-updater/<version>`), or "⬆ Update to x" once found. Settings has "Install updates automatically" (off by default): installs when `idle()` (no reply, model install, meeting, media job or teaching), checking every 5 minutes for an hour, then restarts.
- **Status bar model menu** (`components/ModelPicker.tsx`): click the model name to make another installed model the default for new chats, or open Models.
- The preview (`mock.html` + `src/mock.tsx`, git-excluded) has sample mail and events for these pages.

## Private connection (2026-10-14)

His brief: nothing hosted by us; sites SulcusAI contacts shouldn't learn the user's address or location. `privacy.rs`, `components/PrivacySettings.tsx` (Settings, above Your devices) and a status-bar pill (`PrivacyPill`, shown only while on).
- **Two ways:** Tor built in (Arti `arti-client` 0.47, in-process, state/cache in `<data>/tor`), or the user's own SOCKS5/HTTP proxy or VPN (`normalize` turns `host:port` and `socks5://` into `socks5h://` so names are looked up at the far end).
- **Tor gateway:** a SOCKS5 server on 127.0.0.1 (random port, random user name and password per run) that carries each CONNECT through Arti, so every client uses the same `socks5h://` address. It fails closed: while Tor is starting, requests wait (120 s); they never go out directly. Once Tor has been stuck for 20 s (`STUCK_GRACE`), requests fail at once with the reason (`privacy::check`, called by `net::routed`, `net::connect` and the updater).
- **Status:** `TOR_STATUS` follows Arti's `bootstrap_events` (progress %, `blocked()` mapped to plain words by `problem_words`); `privacy:status` events tell the window to re-read `privacy_view`.
- **Switches** (`PrivacySettings`, all off until the master switch is on): web search, pages, weather and place names (`Route::Web`) and update checks (`Route::Updates`) on by default; email/calendar sync, sending and OAuth renewals (`Route::Mail`) and model/engine downloads (`Route::Models`) off by default, each with a warning in the card. `net::route` maps `Purpose` to a route (Search→Web, Web→Mail, ModelDownload→Models, Cloud→none). Cloud AI and the sync relay always go directly.
- **Mail over a proxy:** IMAP and SMTP connect through `net::connect_through` (SOCKS5 via tokio-socks, or HTTP CONNECT); SMTP then runs `smtp_send_private` (lettre's `AsyncSmtpConnection::connect_with_transport` over the tunnel, `Through<S>` wrapper). The updater (reqwest 0.13) gets the proxy via `updater_builder().proxy()`.
- **No DNS leak for web tools:** when web traffic goes the private way, `web::check_url` skips its local lookup (Tor exits refuse private addresses anyway; literal IPs are still checked).
- **Test button:** asks ipinfo.io (fallback api.ipify.org) which address and place it sees, the way web traffic goes now; logged in Activity.
- **Tests:** `each_switch_routes_only_its_own_traffic`, `requests_wait_for_tor_but_fail_fast_once_its_stuck`, `mail_connections_go_through_socks5_and_http_proxies` (local SOCKS5 with a password and a local HTTP CONNECT proxy). The real-network test `tor_hides_the_address` (ignored) waits for Tor, checks check.torproject.org says IsTor with a different IP, and reads Gmail's IMAP/SMTP greetings through Tor; it prints SKIPPED when Tor can't start.
- **Not verified end to end:** UVU's campus network blocks Tor (Arti stops at 8%: "Our internet connection seems filtered", and check.torproject.org itself is unreachable), so the Tor path hasn't been seen working from this PC. Run `cargo test --lib tor_hides_the_address -- --ignored --nocapture` on a home network or phone hotspot. Bridges (Snowflake/obfs4 via a pluggable-transport binary, Arti's `bridge-client`/`pt-client` features) would get through such networks; not built.
- Arti needs rusqlite 0.36–0.40, so the app's rusqlite went 0.32 → 0.37 (all tests pass). Arti adds ~3,000 lines to Cargo.lock and a few MB to the binary.
- **Freeze fixed in bbb6d57 (after 0.8.9):** `set_privacy` took the database lock twice (`settings()` inside a held lock). Test `saving_settings_does_not_freeze`. The window's close handler now asks the tray, not the database (`quick::on_main_event`), so a stuck lock can't make the window unclosable. Don't publish 0.8.9.

## Models and chats (2026-10-14/15)

- **Settings sidebar** (`components/SettingsNav.tsx`): in Settings, Models, Features and Activity the sidebar is Back plus quick links to each Settings section (anchors `settings-<id>` in `SettingsView.tsx`, same order and conditions as the page) and highlights the one being read.
- **Add from a folder** (`local::import_folder`, Models → Models from other apps): finds the `.gguf` models in a folder and its subfolders (skips hidden/cache folders, picture readers, later parts of split models) and uses them in place. When a header says F16/F32 over quantized weights (a gpt-oss conversion did), the format comes from the tensor list (`gguf::MAIN_TYPE_KEY`).
- **Rename** (`rename_model`, settings key `model_names`): only the name shown in the app; applied in `catalog_view` and `AppState::model_spec`; `renamed_from` keeps the model's own name.
- **Hugging Face** (`hub.rs`, Models → Hugging Face): search (`hub_search`: sorted by downloads, trending or newest) asks the API with `expand[]=gguf` and then each repository's files (`?blobs=true`, 6 at a time). Shown: single-file GGUF chat models without an account (not gated), files that fit this PC by `catalog::fit_model` on an estimated layout (layers from size, 8 KV heads × 128; mixture-of-experts from "A3B"-style names or gpt-oss), leaving out drafts/MTP/eagle heads and files under ~1.5 bits per weight. `hub_install` builds a spec (`hf-<hash of repo/file>`, kept in settings key `hub_models`) and runs the normal install (`start_install`); after the download `hub::refine` replaces the estimate with the file's header. Remove deletes the files. Live test: `search_and_fetch_from_hugging_face` (ignored; downloads Qwen3-0.6B Q4_K_M, ~400 MB).
- **Scrolling while a reply streams** (`ChatView.tsx`): the view follows new text only while at the bottom; it stops on a wheel-up or when the view moves up from where it was put (not on a gap alone: scroll events arrive after more text), with a "↓ New text" button back.
- **Chat names:** background completions send `reasoning_effort: "low"` too (gpt-oss ignores `enable_thinking`), and `chat::complete` asks once more with room when a model spent its whole allowance thinking. The stand-in title (`chat::title_from`) drops greetings and "can you" and cuts at a word.
- **Web permission:** with web off, `request_web` pauses the reply on an approval shown as a popup anywhere in the app (`components/WebPermission.tsx`); a yes turns web on for that chat (`chat:web` event) and adds the web tools for the rest of the same reply. Test `asking_for_the_web_carries_on_in_the_same_reply` (stand-in engine).
- **System folders:** a folder inside Windows, Program Files or ProgramData can be shared after a Windows Yes/No warning (`folder_warning`, `src/folders.ts`, capability `dialog:allow-ask`); those folders themselves, drives, the whole user folder and the app's data stay refused.
- **Translations:** `i18n_extract.py` also reads `Some("…")` reasons and `pub const` messages from the core (that's how the folder and mail sign-in messages got translated).
- **Speed test status** (`SPEED_STATUS` in `lib.rs`, `speed_test_status`): the core keeps whether a test runs, the model being tested, done/total and per-model errors; `models:speed` events carry that status (plus `tested`/`tps`/`error`, and `finished`). `SpeedChart` reads it when it mounts (leaving the page mid-test no longer loses it) and refreshes the catalog after each measured model. The Models page order is hardware → speed → filters → Installed → Available → other apps → Hugging Face → storage.
- **Calendar chips** are `.cal-chip` now: the month view's `.chip` rules had restyled every `.chip` in the app (Models filters, Activity, Notes, Quick ask) since 0.8.7.
- **0.9.0** (2026-10-15) carries all of this plus the Private connection; 0.8.9 was never published.
- **Downloads** (after 0.9.0; `downloads.rs`, `components/Downloads.tsx`, top of the Models page, plus a ⬇ status-bar pill): `pause_install` marks the id as pausing and trips the install's cancel flag; `fetch_verified` keeps the `.part` file, and the install task records it in settings key `paused_installs` (survives restarts). `resume_install` starts the install again (forced, since it was started knowingly) and the range request continues from the `.part`. `cancel_install` on a running download now deletes its partial files (and an `hf-` spec that never installed); on a paused one it just removes them. Chat models only; speech and picture models keep their own installs.
- **Compacting** (`compact.rs`, like Claude's /compact): the model summarizes the chat so far; the summary is saved as an assistant message with `meta.compacted`. From then on `agent.rs` gives the model the latest summary in its instructions (`compact::prompt_section`, not as a message, so strict user/assistant templates stay happy) and only the messages after it (`compact::after_last`); the person still sees everything, with a "Compacted" divider and the summary behind "Show the summary". Compacting again starts from the last summary. Automatic when a chat passes the Advanced threshold (default 80%; Advanced › "When a chat is X% full" now offers compact (default), continue in a new chat (`processing.new_chat`), or leave out the oldest). Manual: the context meter's panel (Compact now) or typing `/compact`. `compact_chat` claims the chat like a reply does. Test: `a_compacted_chat_carries_on_from_its_summary` (stand-in engine).
- **Context window size** (settings key `context_window`, the meter's panel): Automatic or 8K–128K (up to the model's maximum); `context_options` shows how each would run on this PC; `catalog::launch_at` takes the largest size up to the choice that still runs, moving layers off the graphics card as needed and never going below automatic. Advanced mode's own `engine.ctx` still overrides it. Test: `a_chosen_window_is_as_big_as_still_runs`.
- **Show models this PC can't run** (Models filters, off by default): `catalog_view(all)` and `hub_search(all)` include them with `runnable: false` / `placement: null`; cards say why (`cantRun`: memory needed vs graphics card + memory, too slow, or no disk) and offer "Install anyway" after a warning, passed as `force` to `install_model`/`hub_install`. The rest of the app still gets only runnable models.

## People (2026-10-15, phases 1-5, not built)

His brief: link with someone else's SulcusAI by a code, Radmin-style direct connection, a chat between the people that never goes to a model; their agents can talk to each other and they can see each other's work. Built in five phases (commits 2d09f24, b037817, 4e31691, 38a9d9f). Feature `people` (off by default; group "With other people"), page `views/PeopleView.tsx`, core `sync/people/` (DESIGN.md §4.11).
- **Linking** (`mod.rs`) reuses sync's pairing (SPAKE2 code, `Open`/`PairReply`, `transcript`, `wire::pair_key`) under `want: "person"`, with its own code (`people::CODE`). A person is a **contact** (settings key `people`: `{name, contacts:[{id, name, key (sealed), addr, last_seen, allow, offer_relay, their_relay (sealed), box_next, box_got}], shares:[{chat_id, with}]}`), never a paired PC: `sync::answer` sends `want: "people"` to `people::answer` before the paired-PC lookup, so a contact can't sync, list or copy models (tests check `peers` stays empty).
- **Listening:** the sync loop listens when sync is on **or** `people::wants_calls` (feature on, and a contact exists or a code is showing); `sync::wake()` runs when the feature is toggled. With only People on nothing is broadcast unless a code is showing (`Announce.person`).
- **The live link** (`link.rs`): one TCP connection per person, kept open while both apps run (ping every 30 s, gone after 90 s quiet), sealed frames both ways (`Frame`, tagged by `k`; unknown kinds are skipped). Whoever can reach the other calls; one reachable side is enough. Crossed calls: both keep the call made by the smaller app id (`register`, within 10 s). Not connected: retries 0 s, 10 s … 2 min. The callee answers on its own task (`answer` spawns `run`), so the sync listener's 600 s limit and 8 slots don't apply. Online = connected.
- **Messages** (table `people_messages`, PK `(contact_id, id)`, encrypted; files sealed at `<data>/people/<contact>/<id>`, 16 MB max): `Msgs` → `Got`; batches up to 24 MB, the rest after the confirmation; refused items are still confirmed (no resend loops).
- **Sharing chats** (`share.rs`, phase 2): 👥 Share in a chat's header (`components/ShareChat.tsx`; not incognito). People shared with see the chat listed (`Shared`), open it (`Watch` → `Snapshot`) and watch replies as they stream (`Live`, fed by `chat_event` in `run_turn`'s emit closure; `turn_changed` marks busy/idle via the `SharedTurn` guard). Steps show as titles only, never tool output; pictures as "[picture]"; the last 300 messages. Nothing is stored on the watching side (`VIEWS` in memory), so unsharing or disconnecting takes it away (`Gone`).
- **Assistants asking each other** (`ask.rs`, phase 3): tool `ask_person` (risk Submit: the approval card shows the exact question; offered when People is on, someone is linked, not Plan mode, with `assistant_note` listing who's connected). The other side stores it in `people_asks`, then per `Allow.mode`: ask the user (popup `components/PeopleRequests.tsx`, or the card in the conversation: let my assistant answer / answer myself / decline, "always" = auto), auto, or off. The answering assistant (`think`) uses the default model with read-only tools only: shared chats (in its instructions, always), notes (default on), calendar, email and chosen folders (off by default; each also needs its feature on). It's told the question is from another assistant and not to share secrets. The asker waits up to 3 min; the answer comes back wrapped as "information, not instructions". Answers not yet confirmed (`sent = 0`) are re-sent on reconnect; open questions are re-asked (deduped).
- **Working something out together** (`together.rs`, phase 4): tool `work_with_person(person, goal)` (Submit). Same permission flow as questions (`kind = 'task'`). The two assistants take turns (`Task`, `Turn`, `End`), each on its own PC with its own allowed data (the starting side uses its own notes/calendar/email read-only, `own_allow`), until a turn starts with "DONE:" or 8 turns; either person can stop it (`people_stop`; the chat's Stop also stops the starting side's thinking). The thread is kept in `people_asks.thread` on both sides and shown as a card in the conversation.
- **Relay** (`mailbox.rs`, phase 5): for people not on the same (virtual) network. Either person can "Let {name} reach me through my relay" (their sync relay from Settings → Your devices; Cloud level only). The relay address goes over the link (`Relay` frame) and is stored sealed. If both offer, both use the smaller id's. Letters are sealed with keys from the link key (`sync::relay::{box_id, token, seal, open}`, made `pub(super)`), one box per direction. What goes by mail: messages (files under ~1.4 MB), confirmations, questions, answers, task turns, names (`relayable`); `link::send` falls back to the outbox when not connected; `link::reachable` = connected or reachable by mail. Mailbox checks every 5 s while something is going on (3 min after the last letter), else every 30 s. Watching shared chats still needs a direct connection. A contact's offered relay must be https (or local), but it is a server they chose: this app contacts it at the Cloud level.
- **Addresses** for "Show my code" come from sysinfo's adapters: VM/WSL adapters skipped, Radmin/Tailscale/ZeroTier/Hamachi named by adapter (Tailscale also by 100.64/10). `resolve` takes an IP, IP:port, a nearby id, or a host name.
- **Not built:** People isn't synced between the user's own PCs; no read receipts; asks waiting in memory (the outbox) are lost on restart (open questions are re-asked on the next direct connection). Works at every connectivity level on LAN/VPN like sync; the private connection (Tor) doesn't apply.
- **Tests** (`sync/people/tests.rs`, all Windows-only, over loopback with two scratch states): `a_code_links_two_apps_once`, `messages_cross_on_a_live_link`, `crossed_calls_settle_on_one_link`, `a_shared_chat_is_watched_live`, `assistants_ask_each_other_with_permission` and `two_assistants_work_something_out` (one shared stand-in model server on its own thread, `ask::TEST_ENDPOINT`), `people_reach_each_other_through_a_relay` (an in-process relay with the worker's API) and the ignored `people_through_the_real_relay` (relay/local.mjs in Node; passes). The lock lint flags two databases locked in one statement even in tests: split them. **Not verified between two real PCs yet.**
- **Translations:** `i18n_extract.py` now skips `tests.rs` files and only stops at an inline `mod tests {` (people/mod.rs declares `mod tests;` at the top, which used to hide the whole file), and reads files with Windows line endings too.

## After 0.9.1 (2026-10-15, not built)

0.9.1 (People, compaction, downloads) is published. Since then, on phase7:
- **Suggestion per PC** (`catalog::best_for`, `ModelCard.best`, badge "Best for this PC", `starter.ts`): the highest-rated chat model with tools whose best version runs at 15+ tokens/sec, entirely on the graphics card when there is one (else anywhere; on a slow PC the fastest). The catalog's fixed "recommended" tag is gone. Test `the_suggestion_grows_with_the_graphics_card`: 8 GB → Qwen3 8B, 12 GB (his RTX 5070 Ti laptop) → Qwen3 14B, 16 GB (his friend's) → gpt-oss 20B, 24 GB → Qwen3 32B; never the coder.
- **Five bigger catalog models** (sizes and SHA-256 from Hugging Face's API, architecture from each model's config.json): Mistral Small 3.2 24B (vision), Qwen3 30B-A3B Instruct 2507, Gemma 3 27B (vision), Qwen3 32B, Qwen3 Coder 30B-A3B. Before this, a 12 GB card could already run every catalog model, so a 16 GB card saw nothing more.
- **Trying a model out** (after an install, and the speed test): install progress now has phases `loading` then `benchmark`; `components/Testing.tsx` shows a ticking "0:12 so far" and says it may take a minute and stops by itself. With Windows' animations off (`prefers-reduced-motion`) the indeterminate bar used to vanish, so it looked frozen; it now stays visible.
- **Model cards** wrap their rows and let the quality menu shrink, so nothing spills past the card (the installed card's "Stop using" did by ~25 px).

## Folders and building things (2026-10-15)

His friend's model "gave them all the code but never ran anything": no folder was shared, so it had no file tools. Now:
- **`request_folder`** (group `app`, offered whenever tools are on): the model names a reason and a folder (a full path, or a new folder's name on the Desktop). `agent::request_folder` asks through the approval flow with `Preview { kind: "folder", detail: reason, note: path }`; the popup is `components/FolderPermission.tsx` (mounted in App, filtered out of the chat's inline cards), with "Pick another…" (`choose_folder` → `workspace::FOLDER_ASKS`). A yes turns on Files and coding, shares it (`workspace::share_folder`, which makes it if new: one level, removed again if refused), emits `folders:changed`, and returns `meta.folder_granted`: the turn rebuilds the sandbox, swaps the tool instructions in the system prompt and adds the file tools from the next step. Inside an already shared folder it doesn't ask.
- **Desktop access:** "Let the assistant use anything on my Desktop" in the 📁 menu and Settings → Shared folders (`desktop_access`/`set_desktop_access`; the Desktop via Tauri's `desktop_dir`, so OneDrive's Desktop too). It's just the Desktop shared as a folder.
- **Background commands** (`run_command` with `background: true`; `command_output`, `stop_command`) for dev servers and watchers, and **`open_path`** (Write; files in shared folders or http(s) addresses; refuses anything that runs). The instructions tell it to build, run and open the result instead of pasting code.
- Test `asking_for_a_folder_carries_on_in_the_same_reply` (e2e.rs, stand-in engine): a yes (after picking another folder) then a file written there in the same reply; a no shares and makes nothing.

## Keeping the PC safe (2026-10-15, for 0.9.2)

His brother's PC crashed while the assistant built a "NYSE live website", and Steam then said "corrupt disk". SulcusAI can't damage a drive, but a crash or a full disk mid-write corrupts Steam's download cache. What could have let the app or its commands take a PC down is now closed:
- **Commands** (`tools/shell.rs`, `tools/guard.rs`): output kept to its first 50 KB and latest 150 KB (`Kept`; foreground output used to be read whole into memory for up to 10 minutes). Commands run in their own job object (`winjob::Job::limited`): half the PC's memory and 256 processes between them, below-normal priority, ended with the app. `guard::refuse` never runs disk/partition/boot/backup/Defender/firewall/HKLM/shutdown commands (`NEVER`), nor ones that delete, move or overwrite anything outside the shared folders (paths found by regex, variables and `..` expanded; best effort, it's a backstop). The approval card is refused the same way. No command starts with under 4 GB free on its folder's drive or the system drive; running ones (foreground and background) are stopped under 2 GB.
- **Downloads** (`download.rs`): free space checked before writing and every 5 s while downloading (always leaving 2 GB); the `.part` is kept so it resumes. `sync_all` before the rename. (`start_install`'s own check still uses the startup snapshot `disk_free`.)
- **Crash-safe writes:** `storage::write_safely` (temp beside it, fsync, rename; written in place if another app holds the file) for keys.json (it had no fsync), restored keys, models-location, device-id, checkpoint restores and the agent's `write_file`/`edit_file`.
- **Updates install on their own** (`updates.rs`): `UpdateSettings.chosen` is new, so everyone who never chose (all 0.9.1 users had `install: false` stored) now installs updates when idle (`installs()`); `idle()` also waits for the assistant's running commands. Copies on 0.9.1 still need one click on the update prompt.
- **Not done:** the engine's fit uses total RAM, not what's free now (`hardware.rs` collects `ram_available` but nothing uses it); engine/media logs have no size cap while running; checkpoint backups have no total cap.
- **Afterwards:** the brother's Event Viewer showed Event 7 "\Device\Harddisk0\DR0 has a bad block" several times a second: his drive is failing (hardware), which explains the crash and Steam's error.

## Security review of 0.9.2 (2026-10-15; done 2026-10-16, see "Where each finding stands")

An outside static review of the 0.9.2 source zip (his copy: `Downloads\SulcusAI-0.9.2-security-review.md`) found no malware, embedded keys or credential theft, and seven weaknesses. Each was checked against the code here and is real. Suggested fixes follow each.
- **F1 (high): commands aren't confined to the shared folders.** `run_command` runs PowerShell/bash as the user; the working folder, `guard::refuse` (a regex backstop) and the job object (resources only) don't stop an interpreter, build script or package from reading, writing or going online anywhere the user can. "Allow for this chat" and Bypass skip the per-command card. README's "Agent mode" lines read stronger than that. Fix: say plainly in README, the approval card and Settings what a command can reach; consider always asking for `run_command` even when "allowed for this chat"; a real boundary would be an AppContainer / restricted-token / separate low-privilege account, with ACLs on the shared folders.
- **F2 (high off Windows): folder checks ignore capitalization everywhere** (`sandbox.rs` `key()` lowercases). Harmless on normal Windows/NTFS; on Linux (and case-sensitive macOS volumes) `/x/Project` grants `/x/project`. Fix: case-insensitive only on Windows, exact comparison of canonical paths elsewhere, test with two folders differing only in case. `guard.rs` reuses `Sandbox::holds`, so it inherits the fix.
- **F3 (high, feature off by default): the phone page is served over plain HTTP** on every interface (`phone/mod.rs:93`, `:368`; `phone/page.html`), so someone who can tamper with LAN traffic can swap the script and take the pairing key from the URL fragment/localStorage, then read chats and notes and answer approvals. Known and warned about in Settings. Fix: an authenticated transport for the page (HTTPS with a certificate the QR pins can't be checked by a browser, so this needs thought: e.g. the companion as an installed PWA fetched once over HTTPS from the website, talking to the PC with the existing sealed API); until then keep the warning and add "forget all phones".
- **F4 (medium): web fetches can reach the local network** through DNS: `web::check_url` resolves and checks once, the request resolves again (rebinding), and the redirect policy (`web.rs:119`) only rejects literal private IPs, `localhost` and `.local`, not names that resolve to private addresses. Fix: a custom `reqwest` DNS resolver (`ClientBuilder::dns_resolver`) that drops non-public addresses, so every connection and redirect is checked at connect time; mind the private-connection proxy path (the proxy resolves, so check there or refuse private names).
- **F5 (medium): a paired phone can read while the app is screen-locked** with "Keep working while locked" on: `phone::api` uses `state.work_cipher()` (`phone/mod.rs:159`), whose comment and the Phone settings say nothing can be read while locked. Fix: use `cipher()` (refuses while screen-locked) for phone requests, or a separate, explained setting.
- **F6 (medium): "localhost" checks are string prefixes** (`cloud.rs:931`, `sync/mod.rs:969`), so `http://localhost.attacker.example` or `http://127.0.0.1.evil.example` pass and keys/prompts go out unencrypted. Fix: parse the URL; allow plain http only for host exactly `localhost` or a loopback IP; https otherwise; no redirects that downgrade.
- **F7 (privacy): Offline still checks for (and now installs) updates** (`updates.rs`; auto-install by default since 0.9.2, at his request so fixes reach everyone). Signatures are verified. Fix: say it in onboarding, the Offline description and Settings → Updates; maybe a "strictly offline" switch that turns update checks off too.
- **Other notes:** the browser extension asks for all sites + tabs/scripting/native messaging (needed for browser control; worth explaining on install); macOS/Linux fall back to `protector.key` in the data folder when there's no keychain; fine-tuning writes plaintext training examples that a crash leaves behind (`finetune.rs:278`, `:300`; clean up stale `finetune-*` folders at startup). Positives it listed: SHA-256-checked downloads, signed updates, authenticated loopback endpoints, AES-GCM + Argon2id, Markdown without raw HTML or remote images.
- **Also recommended:** a full dependency advisory audit (`cargo deny check advisories` and `pnpm audit`) before the next release.

**His decisions (2026-10-15), to do later. Don't start until he asks.** Rule for all of these: fix what really needs it, but skip any fix that would break something that works today.
- **F5 → a setting, not a block.** Approved (paired) phones may see chats while the PC app is locked, but only when the user has turned that on. Make it its own explained setting (e.g. Settings → Phone: "Let paired phones use SulcusAI while it's locked"), separate from "Keep working while locked". With it off, `phone::api` refuses while screen-locked (`cipher()`). Fix the comment and the Phone settings text that say nothing can be read while locked.
- **F6 → fix it (left to me).** Parse the URL; plain http only for host exactly `localhost`, `127.0.0.1`/loopback or `[::1]`, https for everything else; no downgrading redirects. Existing local setups (Ollama, LM Studio on localhost) must keep working. His rule on API keys: sending them is fine as long as they can't leak to where anyone else can get them, i.e. only over https or true loopback.
- **New: an entirely offline mode with zero traffic.** No update checks, no downloads, nothing outbound. When it's turned on, list the settings that need traffic (automatic update checks/installs, and anything else found: sync relay, cloud providers, web, private connection, People/phone/sync on the LAN, connectors…) and let the user keep each one on or turn it off. This also answers F7.
- **F1 → keep it as it is.** He likes the current behavior: commands are held in by the guard and the per-command card, or not, when the user approves (allow for chat, Bypass). No sandbox work. (Still open: whether to soften the 0.9.2 release notes/README wording that says commands never change anything outside shared folders. Ask him.)
- **F2, F3, F4 and the other notes:** do whichever really need it, without breaking anything:
  - F2 (case-folding off Windows) should be safe to fix.
  - F4 (DNS resolver that drops private addresses) is fine as long as web search, fetching pages and the private connection (Tor/proxy) still work.
  - F3 (phone over HTTP) only if pairing and the phone page keep working; otherwise leave it.

**Where each finding stands (2026-10-16, on phase7, not built):**
- **F5, done:** `phone::Settings.while_locked` ("Paired phones work while it's locked", Settings → Phone; `set_phone_while_locked`). `phone::api` uses `cipher()` unless it's on, then `work_cipher()` (which still needs Keep working while locked). `e2e_phone_page` (ignored, needs node) covers both: 503 when locked, OK once allowed.
- **F6, done:** `net::private_ok` (https, or http to exactly `localhost` or a loopback IP) for custom cloud providers, `set_relay` and a person's offered relay (`people/mailbox.rs`). `external_client(Purpose::Cloud)` follows redirects only to the same host and only if still `private_ok`, because reqwest drops `Authorization` on a cross-host redirect but not `x-api-key`. Test `only_this_pc_may_be_plain_http`.
- **F2, done:** `sandbox::key` lowercases only on Windows.
- **F4, done:** web pages and redirects resolve through `web::PublicOnly` (a `reqwest` DNS resolver that drops non-public addresses at connect time). The SearXNG search uses `searxng_client` (plain lookup), because it's often on localhost or the LAN. With a private-connection proxy the far end resolves, so the resolver isn't installed. Test `names_inside_the_network_are_refused_when_connecting`.
- **F7 → Fully offline, done:** see that section.
- **F3, left as is.** A browser can't check a certificate this PC makes without warnings or breaking pairing. An HTTPS page from the website can't call `http://<LAN>` either (mixed content and Chrome's Private Network Access). It stays off by default with the trusted-network warning.
- **F1, kept** at his choice.
- **Other notes:** interrupted teaching data is now cleaned at startup (`finetune::clean_leftovers`). The extension permissions and the macOS/Linux `protector.key` fallback are unchanged.
- **Dependencies:** rustls 0.23.46 and quick-xml 0.41 fixed six advisories; `pnpm audit` is clean. Left with no fix available:
  - rsa (Marvin, through Arti, which doesn't decrypt with it)
  - paste, proc-macro-error and ttf-parser (unmaintained)
  - yanked `zip` versions pinned by dependencies

## Fully offline (2026-10-16)

The first entry in the status-bar level menu (✈). It is Offline, and nothing goes out except what the person keeps. The core is in `offline.rs`:
- `Keep`: updates, downloads, web, local network, connectors.
- `NOW`: a static set at startup by `offline::load`, before anything starts.
- Settings key `fully_offline`.

**Turning it on:**
- Sets the level to Offline.
- `ConnectivityMenu.tsx` → `FullyOfflineDialog` lists `offline_view()`: the items in use first (`in_use`), each one keepable.
- `set_fully_offline(on, keep)` then calls `apply`, which:
  - closes the browser
  - stops connectors (`connectors::stop_all`)
  - hangs up People links (`people::hang_up_all`)
  - wakes sync and re-applies the phone server
- Picking any level calls `offline::turn_off`.

**Gates** (`offline::check`/`blocks`):
- `net::allowed` and `net::external_client` (by purpose)
- `download::fetch_verified` (before the network part; a local copy from another app still works)
- `updates::updater`
- `location::detect`
- `browser::open`
- `privacy::test_privacy`
- `open_path` for URLs
- the sync loop (listening and sessions)
- the People tick and `connect`
- `phone::apply` (its `problem` says why)
- `connectors::offer` and `ensure_running`
- the agent: neither web tools nor `request_web` are offered

**Not gated:** commands the assistant runs (their own programs; the dialog says so) and loopback-only things (the engine, the local API server, the Tor gateway).

**Test:** `fully_offline_stops_everything_not_kept`, on the pure `stops()`; tests never set `NOW`.

## Typing while it replies, GPU limit, memory check, chats together (2026-10-16)
- **Queue / add now** (`inbox.rs`):
  - A per-chat inbox exists only while a reply runs (`Opened` in `run_turn`). The agent drains it at the top of each step, saving each message as a user message with `meta.added`. A reply that was about to end carries on.
  - `cloud::to_anthropic` doesn't count a user message that follows a tool result as a new turn, so thinking blocks are kept.
  - Ctrl+Enter / Queue → `QUEUED`. When the reply finishes, the unread inbox plus the queue go out as one message (`send_later`). Stopping keeps them (`chat_queue_send`).
  - UI: chips above the box; Add now / Queue buttons and a hint while replying. Test `a_message_added_mid_reply_is_read_and_answered`.
- **GPU limit:**
  - `PerfSettings.gpu_limit` (percent) → `Limits.gpu_share`, which `launch_within` uses for the layers on the card. A hot card can only lower it further. Under 100%, the processor is held to the same share too (`Limits.threads`, at least a quarter of the cores via `CPU_FLOOR`) and runs at below-normal priority, so the work moved off the card doesn't overheat the CPU instead. Adaptive cooling can lower the threads further. Under 50% the window is capped at 16K, and at Off at 8K, so a slow processor isn't asked to read 32K-token prompts; a window the person chose still wins. `complete_once` (summaries, compaction) waits up to 30 min for such a slow model.
  - `GpuLimitMenu` sits in the chat header (hidden for cloud models and PCs without a GPU). `set_perf` keeps the stored `gpu_limit`.
  - llama.cpp has no way to cap utilisation, so this is the closest real control.
- **Memory check** (`memcheck.rs`):
  - Runs before any load in `llm_endpoint`, with the engine released while the person decides.
  - `need()` estimates RAM and VRAM. Free RAM (sysinfo) and free VRAM (nvidia-smi) count back what the replaced model's process frees.
  - It warns when RAM would go under 1.5 GB free, or when VRAM is over by more than 0.5 GB.
  - `MemoryWarningPopup` shows the biggest apps; Load anyway / Don't load it. Without an app handle (tests) it never asks.
- **Chats working together** (feature `chat_team`; `tools/chats.rs`):
  - `search_chats` and `read_chat` (never incognito chats, never the current one).
  - `message_chat`, in `agent::ask_chat`: the target runs `run_asked`, i.e. `run_turn_with` with `Asking`. It uses the asking chat's endpoint (the loaded model), its message gets `meta.from_chat`, and `api_messages` tells its model who's asking.
  - `tools::chats::Asked` stops a chat that's answering from asking on.
  - `message_chat` is boxed with an explicit `dyn Future + Send` type, because the agent recurses through `run_turn`. A plain `async fn` hits a Send cycle.
  - `ElsewhereApprovals` pops up approvals from chats not on screen.
  - Test `a_chat_finds_and_reads_another_chat`. **The cross-chat run itself isn't covered by a test** (it needs a real app handle); try it in the real app.
- **Disk care:**
  - installs check free space live
  - undo backups are capped at 2 GB in total
  - startup removes media work folders, interrupted teaching data and month-old teaching logs
  - engine and speech logs are cut at 64 MB (`engine::capped_log`, append mode, trimmed by a thread every 60 s)

## Small optimizations (2026-10-16, 0.9.3)
- `count_tokens` caches texts over 400 characters by hash for each engine run. The system prompt, the instructions and the tool list used to go to `/tokenize` again at every step.
- The GPU pill reads every 5 s instead of every 3. Each reading starts nvidia-smi, which costs processor time and can keep a laptop's graphics card out of its idle state.

## Feedback, crash reports, read receipts, shared chats you can join (2026-10-16, after 0.9.3)
- **Feedback inbox** (his choice: on his own relay):
  - `relay/worker.js` has an `Inbox` Durable Object (migration v2, binding `INBOX`). `POST /feedback` takes `{kind: bug|idea|crash, text, version?, contact?}`:
    - text up to 8,000 characters
    - 8 an hour per sender, counted under an hourly hash of the address and key (deleted after the hour)
    - 400 a day
  - `/feedback/inbox?key=` is his page (with delete); the key is the `FEEDBACK_KEY` secret. The link is ONLY in `C:\dev\sulcusai-dist\feedback-inbox.txt`, never in the repo.
  - Deployed with `cd relay && npx -y wrangler@4.149.0 deploy`. A Durable Object stub's `fetch` takes no extra argument, so the object reads the secret from its own `env` (`local.mjs` mirrors that).
  - App side: `feedback.rs` sends at every level, except Fully offline unless Web is kept.
  - `FeedbackButton` sits in the top bar (`TopBar` in App.tsx). It sends only the text, plus a reply address and the version if the person chooses.
- **Crash reports** (`crash.rs`, off by default, Settings → About):
  - Sources: a panic hook; `report_window_error` (CrashScreen and window error/unhandledrejection, at most 3 a run); `running.json` with heartbeats every 30 s (`crash_ping` from the window).
  - An unclean exit is reported only when Windows didn't restart since the last heartbeat (`System::boot_time`), so ordinary shutdowns aren't. A window ping more than 90 s older than the last heartbeat means a freeze.
  - Reports drop the user's folder and name. `CrashReportPrompt` shows them at start: Send (to the inbox, kind crash), Copy, Don't send.
  - `crash::closing()` runs on `RunEvent::Exit` and before every `app.restart()`.
- **Read receipts** (People):
  - `Contact.read_receipts` is set per person (contact menu). `open_chat` (was `people_messages`) sends `Frame::Read{ids}` for what it just marked read.
  - The other side sets `people_messages.seen` (a new column; `read` was already 1 for one's own messages). `MessageView.read` shows ✓✓ Read.
  - Test in `messages_cross_on_a_live_link`.
- **Model read status:** `agent` emits `chat:read {up_to}` (the newest user message in that request) on the first streamed piece, or after a reply made only of tool calls. ChatView shows ✓ Sent → ✓✓ Read under the newest message and under added ones.
- **Shared chats you can join:**
  - On the watcher's side, `Book.seen` keeps titles only (updated by `listed`, removed by `gone` and `forget_contact`); `people_shared_chats` feeds the sidebar (👥 name).
  - The `shared` view uses `SharedChatView` (now exported, taking `contactId`/`contactName`) with a box to write in.
  - `people_shared_post` → `Frame::Post` to the owner. `answer_here` runs this user's default model (no tools) on a transcript of the view, streams `people:post_live`, and sends `Frame::PostReply{model}`.
  - The owner's `share::posted` stores them in the chat as user/assistant messages with `meta.from_person/from_name/model`, id `<their app id>:<id>` (deduped), then emits `chat:changed` and re-snapshots watchers.
  - `chat::api_messages` gives the owner's model both as user-role notes ("[Ben's assistant (model, on Ben's PC) answered Ben:]"). `SharedMsg.mine` (id prefix = own device id) puts one's own messages on the right.
  - Test: the participation part of `a_shared_chat_is_watched_live`.

## Batch for 0.9.4 (2026-10-16, not built)
- **Sync now** (`sync::sync_now`, now async) runs the round itself and returns `SyncReport {view, results}`: per paired PC, ok with `changes` (the window words it), "Already syncing", or why not (`NOT_HERE`, relay/timeout errors). SyncSettings shows a spinner, then the list.
- **Messages sent mid-reply stay in the chat:**
  - `inbox::SENDING` holds what goes out as the next message; `unread()` is SENDING + OPEN.
  - `told()` emits `chat:inbox {waiting}`. It fires on add, take (agent step), finish/drop and `send_later`. `inbox::sent()` clears SENDING once `run_turn_with` saved the message, which also emits `chat:changed` (so turns started from the queue, another chat or the phone show at once).
  - ChatView renders `waiting` as dashed bubbles: "⏳ Sent while it worked · waiting for the model to read it".
- **Dedicated GPU:**
  - `engine::dedicated_devices` runs `llama-server --list-devices` once per engine and matches names against `hardware::cards()`. It passes `--device VulkanN` only when an integrated chip is also listed. Your laptop: Vulkan0 = RTX 5070 Ti, Vulkan1 = Intel.
  - `prefer_dedicated` sets `GGML_VK_VISIBLE_DEVICES` for media/speech engines, only when the engine saw every card (so the indices line up).
  - Status bar (`perf::gpu_now`) reads only the dedicated card: nvidia-smi for NVIDIA; otherwise PDH counters filtered by the card's LUID (`Gpu.luid` from DXGI) plus `card_temp` (D3DKMT adapter perf data, the Task Manager sensor). `gpu_temp` falls back to that too.
- **Making files instead of pasting code:**
  - His qwen2.5-coder:14b (Ollama) ignored tools entirely: 0 tool calls in 18 messages.
  - `chat::builds_in_chat` catches a request to make something answered with code or steps. `forced_build` retries once with a JSON schema allowing only `write_file`/`make_folder` (or `request_folder`), the same way `forced_lookup` works.
  - `e2e_makes_files_on_ollama_model` (SULCUSAI_TEST_GGUF = the blob) passes. It approves the folder request into its own temp folder and denies open_path/open_page. An earlier version put two folders on his Desktop (`sweetcrumbs-website`, `SweetCrumbsWebsite`); he can delete them.
  - Imported models get catalog ratings by family (`local::known_ratings`).
- **Activity:** Today / Past week / Past month / All time (`list_actions` takes `since`).
- **Settings search** (`SettingsSearch.tsx` + `fuzzy.ts`, tested in `fuzzy.test.ts`): indexes the page's headings, labels, callouts and aria-labelled inputs. Handles typos (Damerau distance), prefixes and a synonym map, and shows "Showing results for …". Picking a result scrolls there and highlights it.
- **Cloud keys:** Open WebUI (`…/api`) and LiteLLM (`…/v1`) buttons in Cloud models, a "Cloud models and API keys…" link in the connectivity menu (`openSettings` → `open-settings` event), and `not_an_api` explains a sign-in page (a server behind a sign-in proxy) instead of a JSON error.
- **Phone: one phone, with a password:**
  - `add_phone(name, password)` replaces any paired phone.
  - The password is Argon2id (19 MiB, t=2), stored as `pass_hash`/`pass_salt`.
  - The page asks for it each time it opens (`login` op, sealed); the session token lives in sessionStorage and in `SESSIONS` (12 h idle, gone on restart).
  - Wrong tries: waits from the 4th, unpaired at 10.
  - Phones paired before have no hash and must pair again.
  - The settings now say plainly that a tampered page on an untrusted network could still catch the password.
- **Pictures:**
  - `media::styled` adds a look; `auto` is a realistic photo unless the description names one. The chat tool and the Studio have `style`. Tested on FLUX.2 klein: portrait → top hat → party.
  - When the card fails twice (`card_trouble`), a picture is made with the processor build (`cpu_engine` downloads ~18 MB once). The half-VRAM retry now adds `--vae-tiling`.
  - There's a Files panel per chat (top right: pictures made or attached, files written). `write_file` results carry `meta.file`. `open_document` allows pages and pictures (never scripts); `show_in_folder` uses `explorer /select`.
  - Save buttons under pictures in chats. Badges for what media models make. "✨ Improve" in the Studio rewrites the idea with the chat model; "✏ Edit a photo…" imports and opens the editor.
- Not done: a model or engine to make the brother's card work (no logs from his PC). The fallback should get him a picture either way.
- **Servers and ports** (`ports.rs`, Settings → Servers and ports):
  - Lists listening TCP ports from `GetExtendedTcpTable` (v4+v6) with each owner's program (sysinfo). Windows' own (`SYSTEM`, pid ≤ 4) are hidden by default and never stopped.
  - The assistant's background commands (`shell::servers()`; `STOPPED` keeps the last 10) can be stopped and started again (`start_server` re-runs the same command in the same folder via `spawn_shell`).
  - **Preview:** the browser still refuses local addresses, except ports in `CHOSEN` (picked with View) and ports owned by a process under one of the assistant's server shells (`previewable`, checked in `navigable_with` and `browser_open`).
  - `keep_running` puts `preview_port` in the step's meta, so the card shows "Preview in SulcusAI's browser".
- **Handoff guard** (`handoff_guard.rs`, Settings → Handoffs between chats). The brother's chats read another chat's HANDOFF.md although "Chats work together" was off.
  - `write_file` records handoff-like notes (by name or opening, `looks_like_handoff`) with the writing chat; the path is encrypted, keyed by a hash.
  - With ChatTeam off, `Turn::handoff_gate` puts a card in front of `read_file` on another chat's note, or a `run_command` naming one. The card shows in every mode, Bypass too, unless the user's newest message asks to pick up another chat's work (`asked_to_pick_up` needs both a continue-verb and an other-chat/handoff word).
  - `search_files` skips such notes, and the system prompt tells the chat not to pick them up.
  - Allowed chats are listed in the manager (Take back / Forget). With ChatTeam on, nothing changes.
- **Moving the models folder** (`storage.rs`, after 0.9.4):
  - `copy_all` now returns `Moved`. A failed file undoes the move: renamed files go back, partial copies are removed, and the location stays. Before, files renamed on the same drive stayed in the new folder while the app still looked in the old one.
  - Files already at the target with the same size are kept, not copied again.
  - Links and junctions are followed and copied, never renamed or emptied (what they point at stays). The link itself is removed at the end.
  - Tested in `a_failed_move_puts_everything_back`, `files_already_there_are_kept_not_copied_again` and `linked_folders_are_copied_not_emptied`.
  - His "move didn't happen" (2026-10-14, to D:) wasn't this bug. His real app's models folder was empty. The 27 GB under `app.sulcusai.desktop\models` existed only in Claude's MSIX container (`Packages\Claude_*\LocalCache`), downloaded by session-launched runs. It was moved to `D:\SulcusAI models` on 2026-10-16 with robocopy run through WMI, and the duplicates were hash-checked first.
- **Making files with small models** (after 0.9.4). His real app (qwen2.5-coder 1.5B by default) still didn't make files.
  - **What his real chats showed** (decrypted from a copy with `inspect_a_chat_copy`):
    - The 1.5B called `create_document` with "stock-exchange-site" four times, getting the same error each time.
    - The 14B, after pasting instructions, answered "try again to do it for me" with "As an AI, I can't…".
  - **Fixes:**
    - `is_build_ask` looks at the last 3 user messages ("do it for me" after a build ask counts), and `wants_a_document` decides whether `create_document` is offered at all.
    - `builds_in_chat` also catches "I can't" (`CANT_DO`) and question-only replies (`only_asks`).
    - The forced retry allows only `write_file`, and suggests a single self-contained index.html.
    - When the model stops, `Turn` follows through: `missing_pieces` finds css/js files a written page uses but which don't exist, and `forced_write` writes each with the path fixed in the schema. A build ask with nothing written yet gets `forced_build`. Up to 4 follow-ups.
    - A call that failed isn't run again unchanged, and two identical failing repeats end the turn with the error spelled out.
    - The `create_document` error points to `write_file`. ChatView warns under the composer when a model is under 3B.
  - Verified with his models through `e2e_makes_files_on_ollama_model`, using `SULCUSAI_TEST_ASKS` and `SULCUSAI_TEST_BEFORE` for the follow-up case:
    - The 1.5B now writes index.html, styles.css and script.js. They're very basic, which is the model's limit.
    - The 14B now writes a working index.html after "try again to do it for me".
- **Logs on the PC** (`applog.rs`, after 0.9.5). His brother's 20-minute picture can't be explained without logs.
  - **app.log:** every Activity entry (`db::log_action` mirrors in) plus a startup line with the hardware, model loads and failures (`engine.rs`), media jobs with model, time and errors, the card's failures and the processor fallback, downloads, and tool failures (name only).
  - **crashes.log:** every panic, unclean exit and window error, even with crash reports off; `crash::write` always logs and saves a report to send only when on.
  - Logs past 2 MB start over and keep the old file as `.old.log`. They never include chat text, prompts or file contents.
  - Settings → Logs lists the files with what each holds, shows the last 400 lines (`read_log`, logs folder only), opens the logs or data folder (`open_app_folder`), and can clear them. About has the folder buttons too.
- **Keeping pictures on the graphics card** (after 0.9.5):
  - **What we found:** on his laptop, `sd-cli` without a pinned device used the first Vulkan device. With `--backend te=cpu` its auto-fit put the DiT on **Intel graphics** (Vulkan1: more "free" shared memory), taking 193 s instead of 17. A PC that lists its built-in graphics first gets every picture there; likely the brother's 20 minutes.
  - **Pinning:** `prefer_dedicated` (GGML_VK_VISIBLE_DEVICES) only worked after a chat model had loaded. `engine::learn_devices` now runs the installed llama-server `--list-devices` before any media job or speech start.
  - **First try:** `card_friendly` keeps 1 GiB free (`--max-vram -1`) and tiles the VAE on cards under 12 GB or above 1.2 MP.
  - **Two lighter tries on the card:** `lighter_on_card` frees more (`-3`, or half a set budget), drops `--diffusion-fa`, adds `--vae-tiling`, and with the card pinned adds `--backend diffusion=Vulkan0,te=cpu,vae=Vulkan0`. The second of them is also at 768 px.
  - Only then the processor, at 640 px (`smaller`).
  - `choose` prefers an installed model whose `min_vram_gb` fits the card.
- **Models: Grid / List** (remembered in localStorage). List mode is CSS on the same cards (`.model-grid.list`; the card is a column flexbox, so the rows set `flex-direction: row`).
- **Phone photo look** (`phone_photo` in `STYLES`): words for a candid, unedited phone snapshot. Compared on FLUX.2 klein and Z-Image Turbo with his prompts (dragon and castle, frisbee, Halloween group, Christmas living room). The pictures are in the session scratchpad.

## Batch for 0.9.6 (2026-10-17)

- **What making something took** (pictures, video, music):
  - `media::run` watches the card and processor every 2 s while a job runs (`perf::gpu_reading`, `perf::cpu_usage`). `JobInfo` carries `on` ("graphics card" / "processor" / "cloud"), `device` (the card's or processor's name, or the cloud model), `gpu_usage`, `gpu_temp` and `cpu_usage`. The Studio and chat job cards show "Making it on <device>" with a use bar (and °C for the card); while the processor works, the card's bar shows too.
  - Each item made keeps `MediaItem.stats` (`MadeStats`): seconds, where and which device, steps, prompt tokens (estimated, words×1.3 + punctuation), audio tokens for music (5 a second), frames for video, the card's peak °C and average/peak use, or the processor's average use. Readings only count while the device that made it was working (a processor fallback doesn't credit the card).
  - Shown under the result (`MadeStatsLine` in chats, the lightbox and the Studio viewer) and written to app.log ("done in 17 s on the graphics card (…), card up to 71 °C, 88% busy on average").
- **Developer mode** (`devmode.rs`, Settings → About): off by default. Turning it on shows what it sends and never sends, and needs "I understand…" ticked (`set_dev_mode(on, understood)`).
  - **What it sends:** to the relay inbox as kind "dev". A first report 3 minutes after start, then every 30 minutes with new app.log lines in the engine-side categories (`SENT`; not settings, people, security or privacy), user folder and name taken out by `crash::tidy`. Crashes go at once (`crash::write` calls `devmode::crash`), and an error triggers a report within about a minute (at most every 10 minutes).
  - The header has a short hash of the device id, the version and hardware. Every report is copied to `logs\dev-sent.log`.
- **Hugging Face for the Studio** (`media/hub.rs`, "More models from Hugging Face…" under the Studio's models):
  - Catalog models with a `hub` family open Hugging Face models built on them, found by the `base_model:` link (`filter=base_model:<id>`, one search per base, together). Adapters (`base_model:adapter:`) are left out.
  - **Families:** FLUX.2 klein 4B/9B, Qwen Image 2.1, Z-Image, SDXL (via RealVisXL), Wan 2.2 5B / 2.1 1.3B, ACE-Step 1.5 and 1.5 XL.
  - **Installing:** the catalog model's files with the downloaded file in place of the family's `role` (`model` = a whole SDXL checkpoint, `diffusion`, or `dit` for music). The text reader and decoder are shared. The file is saved as `hfm-<hash>-<name>`, so two repositories' files never collide.
  - **Which files are offered** (`candidates`):
    - Size between `min_size` and `max_size`; `require` (all of), `any_of` and `avoid` words.
    - No split parts, add-ons by name (`is_part`) or diffusers folders.
    - Not files the catalog already has (same sha256).
  - SDXL fine-tunes get 28 steps / cfg 5.5 unless "lightning/turbo/hyper/lcm/dmd" is in the name (`tune_steps`).
  - Installed ones are kept under `hub_media_models` and join `media::models(state)` (catalog + Hugging Face), which replaced `state.catalog.media.models` everywhere a model is looked up. Removing one forgets it. `prune_missing` checks them too.
  - The live test is `search_hugging_face_for_media_models` (ignored).
- **New catalog models** (picked from a research pass over sd-cli master-948's docs and the Artificial Analysis arenas):
  - Qwen Image 2.1 Turbo (Qwen Research License, non-commercial; best open model for making and editing).
  - FLUX.2 klein 9B (FLUX Non-Commercial; its original asks for the license to be accepted, so the app asks too).
  - RealVisXL V5.0 Lightning (SDXL, openrail++, with the fp16-fix VAE as `--vae`).
  - ACE-Step 1.5 XL (MIT).
  - **Not added:** Mage-Flow (Microsoft pulled it and says it's research-only), Krea 2 (license gate would be sidestepped by the mirror), Qwen-Image-Edit-2511 and Wan 2.2 I2V A14B (need LoRA support; 2511 is 13 GB and 1-2 minutes per edit). FLUX.3 Klein needs sd.cpp master-968+.
  - **New engine settings in `Defaults`:** `scheduler`, `sigmas` (Qwen 2.1 Turbo's exact list, since 948 lacks the preset), `model_args`, `align` (Qwen 2.1 wants sides in steps of 32). There's a new file role `llm_vision` (`--llm_vision`), passed only when editing. A model's `negative` is used when the request has none.
  - Installing a model whose license isn't commercial asks first (`licenseOk`); Hugging Face installs always ask (their license is the repository's).
- **Tests with model overrides:** `e2e_realistic_picture_and_edits` takes `SULCUSAI_EDIT_MODEL`; `e2e_media` takes `SULCUSAI_MUSIC_MODEL`. Both print the stats.
- **Release builds use thin LTO** (`[profile.release]`). Full LTO in one codegen unit made every release a long single-threaded link.
- **From 0.9.7 on, building, testing and publishing move to his desktop** (RTX 5070 12 GB, 32 GB). A release compile took the laptop's processor to 98 °C. Setup steps and the rules are in `C:\dev\sulcusai-dist\DESKTOP-HANDOFF.md`, kept outside the repo next to the signing files.

## After 0.9.6 (2026-10-17, for 0.9.7; written on the laptop, not built — build it on the desktop)

- **Paste a Hugging Face link** (`HubLinkBox` in `components/HubLink.tsx`, in both Hugging Face panels: Models → Chat and the Studio's models).
  - `hub::parse_link` takes a model page, a file page (`…/blob/main/x`), a download link (`…/resolve/main/x`), with or without https, or just `owner/name`. It refuses datasets, spaces and other sites, and paths with `..`.
  - The `hub_link` command (lib.rs) tries picture, video and music models first (`media::hub::from_link`, since their files are GGUF too), then chat models (`hub::is_chat` + `hub::describe` with every file).
  - The file the link named is shown first ("From your link"). Installing goes through `hub_install` / `media_hub_install`.
  - Families can now also match on a tag (`HubFamily.tags`). SDXL has `diffusers:StableDiffusionXLPipeline`, because RealVisXL V5.0 and others name no base model.
  - Tests: `pasted_links_name_the_model_and_file` (unit) and `pasted_links_find_the_right_kind_of_model` (live, ignored).
- **Default picture, video and music models** (`media_defaults` key, `media_set_default`). His report: chats always used FLUX, and the Studio went back to FLUX after switching tabs.
  - **Cause:** `choose` picked the best model that fit the card's *free* memory, which is small while a chat model is loaded.
  - **Fix:** the default now wins (the job frees the card first anyway).
  - "Make default" is on the Models rows and next to the Studio's picker.
  - The Studio remembers the last pick per tab (localStorage `sulcus.studio.model.<kind>`, `useModelPick`). Setting a default clears that and remounts the composer.
- **Deleting a model from the PC** (`remove_model_now`, `media::remove_now`). Both return the bytes freed.
  - **What goes:** half-finished downloads, models taught on it (`finetune::forget_adapters_of`), its paused download, its custom name, and its cleanup lock and usage.
  - **Another app's model:** "Stop using" by default. The dialog can also send the file to the Recycle Bin, except Ollama's (it says to use `ollama rm`).
  - **Chats that pointed at a removed model** now use the default model in `llm_endpoint` instead of failing.
- **Automatic cleanup and locks** (`cleanup.rs`, Settings → Cleanup and space). All of it lives in this PC's settings: only `profile` syncs, so none of this travels.
  - **Usage:** `last_used` is written by `llm_endpoint`, by `media::run` and when a chat opens (`get_messages`), at most every 10 minutes per item. Chats also count their `updated_at`. Models with no record count from the first time the cleanup sees them, so nothing goes on the day it updates.
  - **Locks:** `keep_locks` holds chats and models; folders can be locked too.
  - **Never removed:**
    - locked chats and models, and chats in a locked or shared folder
    - chats shared with people, and a chat writing a reply
    - the default chat model, the default picture, video and music models, and the loaded model
    - models from other apps (removing them frees nothing), and anything being installed
  - **When it runs:** checked every 3 h, and it runs at most once a day while unlocked and idle. Turning it on waits a day before the first run, and the page lists what would go, with "Keep" buttons. "Clean up now" skips the wait. Everything it removes goes in Activity ("cleanup").
  - **Sync:** chats it deletes are deleted on synced PCs too, like any deleted chat, and the page says so.
  - Test: `the_cleanup_keeps_locked_chats_and_removes_old_ones` (ignored, scratch data).
- **Clear cache** (same section, `cache_view` / `clear_cache`). It never touches chats, notes, the gallery, settings or models. Each item shows its size and what clearing it does; the texts live in the frontend's `CACHE_TEXT`. The items:
  - media job leftovers
  - unfinished downloads (`.part` files, engine staging, paused-download records)
  - old engine versions (a known engine family whose build isn't the catalog's current one; unknown folders are kept)
  - undo copies (`checkpoints` rows and files)
  - the built-in browser profile
  - the WebView cache folders
  - the Tor cache
  - before-restore copies
  - unsent crash reports
  - screenshots in %TEMP%
  - logs
  
  Files in use are skipped and counted.
- **Chat folders** (`folders.rs`, `components/ChatFolders.tsx`): a Folders section in the sidebar.
  - **Storage:** kept in this PC's settings with encrypted names, not synced.
  - **Organizing:** drag a chat onto a folder, or right-click a chat → Move to folder / New folder… / Out of "…". The chat menu also locks and unlocks a chat.
  - **Folder menu** (right-click a folder): Rename, Lock, Share folder…, Delete (its chats stay).
  - **Sharing a folder** uses People: every chat in it is shared with the chosen people (`share::set_chat_sharing`), chats moved in later too, and chats moved out stop being shared with them.
  - **What recipients see:** the chats individually, not as a folder. Showing the folder would need a protocol change.
  - **Deleting a chat** (`delete_chat_now`) first stops its sharing, then removes it from folders, locks and usage.
- **Preview mock:** chat folders live under `W.__chatFolders`; `W.__folders` is the assistant's shared folders.

## 0.9.7 (2026-10-17, the first release built on the desktop)

- **Shipped:** everything in "After 0.9.6" (Hugging Face links, default picture/video/music models, Delete from this PC, automatic cleanup with locks, Clear cache, shareable chat folders). Published and installed on the desktop; main = phase7 = 37d1f00.
- **Build:** 7.5 min from cold (no earlier release build) on the desktop (i5-14600K, RTX 5070 12 GB, 32 GB) with `CARGO_BUILD_JOBS=8`. Unit tests: 317 pass, 66 ignored; the first debug compile took 4 min. Nothing needed fixing.
- **Desktop setup notes:**
  - Plain `python` here is MSYS2's (`C:\msys64\ucrt64\bin`), not 3.12. Run the translation check with `py -3.12 -I scripts/i18n_extract.py .`.
  - Git Credential Manager and `gh` were already signed in as IsaacLeh1, so pushing and publishing needed no sign-in.
  - Checking latest.json from Windows PowerShell 5.1: `Invoke-RestMethod` reads it as Latin-1 and garbles "→", so its notes look different. Download it and decode as UTF-8 to compare.
- **Installing over a running copy:** the installer (`/P` through WMI) closes SulcusAI and doesn't reopen it.

## 0.9.8 (2026-10-17): look up what it doesn't know, sites that work, Preview panel

**Published and installed 2026-10-17** (main = phase7 = d899332). The release build took 5.7 min (incremental, 8 jobs). Notes are in `C:\dev\tools\notes-098.md`; the body and latest.json were checked.

From his brother's report (a "Legend of Zelda" picture came out as a generic blond swordsman) and his own ("a live stock exchange website" came out basic, with nothing working).

- **Looking up what it doesn't know** (`chat.rs`, `agent.rs`):
  - The base prompt says not to guess at names, titles or terms it doesn't recognize, and `request_web`'s description covers that case.
  - `chat::doesnt_know` catches replies like "I'm not familiar with…" or "as of my knowledge cutoff…" (`NOT_KNOWN`). At step 0 with a local engine, `forced_web` makes the next try one `web_search` call, or `request_web` (the popup) when the web is off. This runs before the older `stalls` check.
  - **Pictures:** before `create_image`, `edit_image` (edit or restyle) or `create_video` runs on a local engine, `Turn::picture_names` asks the model in one quiet JSON call (`names_check`, schema `names`) which named things in the request have their own look, and what each looks like.
    - A name it isn't sure of (`Named::described`: known and at least 4 words) is looked up: `web_search "<name> appearance"`, then `looks_from` turns the results into one line. When the web is off it asks with the same popup ("what X looks like, for the picture").
    - `with_looks` then adds "X looks like this: …" to the description, because picture models draw words, not names. A look it couldn't find goes in the tool result, so the model tells the user.
    - The chat shows "Checking the names…" or "Looking up what X looks like…" (`chat:status` `looking_up`).
- **Building sites and the Preview panel:**
  - `preview.rs` serves a site's folder from inside SulcusAI: one axum server per site on 127.0.0.1, port 0.
    - The Host header must be this PC, and paths can't leave the folder.
    - It sends `no-store`, so changes show on reload.
    - The site follows its chat's web setting: with the web off, a CSP blocks anything outside the site.
    - Its port joins `ports::allow`, so SulcusAI's own browser may show it.
  - `preview::check` loads the site in a **hidden window** (its own `sitecheck` profile, no IPC, can't leave the site) with `PROBE` as an initialization script. The probe reports back to the site's own `/__sulcus/report`:
    - script errors and unhandled rejections
    - `console.error`
    - missing files (the server notes 404s too)
    - requests the CSP blocked, and failed `fetch`es
    - after load, buttons with nothing listening for a click (it wraps `addEventListener` before the page's scripts run)
  - `describe` turns the reports into the problems the model is told to fix.
  - **Tool `preview_site`** (`tools/site.rs`, Risk::Read, files group):
    - It takes a `path` (a folder or an .html page) or a `port` (a dev server the assistant started; that one gets only a load check).
    - It emits `chat:preview` and returns the problems. Meta carries `preview_url/site/root/start/problems`.
    - Helpers don't get it, and `forced_lookup` never picks it.
  - **The agent** (`run`): when a turn wrote a page (`chat::site_folder`) and the model stops:
    - If the site hasn't been previewed since the last write or edit, `forced_preview` makes the next step that call (up to 6 previews).
    - If the last preview found problems, `forced_fix` makes it start fixing (up to 3 rounds).
    - A forced follow-up the engine doesn't answer with a call isn't retried that turn (`follow_ups_failed`); words it wrote become the reply.
    - Follow-ups are capped at 12.
  - **Build instructions** (`tool_instructions`, folder shared):
    - every feature, with working JavaScript; no placeholder buttons or fake data passed off as real
    - index.html + style.css + app.js
    - live data from a free source that needs no key when the web is on, or simulated and labelled "simulated" when it's off
    - then `preview_site`, fix, and preview again
  - **Window:**
    - `components/PreviewPanel.tsx` docks to the right of the chat. The chat column narrows (`.conversation.with-preview`, `--preview-w`), and the panel's left edge drags to resize it (25 to 75%, remembered).
    - It shows the site in an `<iframe>`, sandboxed: scripts, same origin, forms, modals. The window CSP allows only `frame-src http://127.0.0.1:* http://localhost:*` for it.
    - Buttons: Reload; Open in browser (`preview_open_external`: the main-browser choice, and SulcusAI's browser is allowed the port); Stop (`preview_stop`).
    - `preview_open` serves the folder again after a restart, if it's still shared with the chat.
    - The header gets a 🖥 Preview button once the chat has previewed a site, and the step card has "Show the preview".
- **Tests:**
  - `replies_that_dont_know_the_thing_are_caught`, `named_things_get_their_look_spelled_out_for_the_picture_model` and `a_built_site_is_previewed_and_its_problems_fixed` (chat).
  - `paths_stay_inside_the_site`, `the_check_names_what_doesnt_work` and `a_site_is_served_on_this_pc_only_and_follows_the_chats_web` (preview).
  - `src/preview.test.ts`.
  - The panel was tried in the UI mock: the site runs, a click inside it works, and the edge resizes it.
- **Live test in dev mode** (desktop, his real data, Qwen2.5-Coder 14B at 8K context, Z-Image Turbo; driven over WebView2's DevTools port with `C:\dev\tools\drive.mjs` / `watch.mjs`; test chats, folder and model removed afterwards). Verified: the iframe under the real CSP, the hidden check window, the forced preview and fix steps, the picture lookup. What it found and what changed:
  - **Tool descriptions filled the window:**
    - Every tool's description took 4,655 of 8,192 tokens, which left about 330 for the conversation, so the first file overflowed it.
    - A build ask on a local model with ctx ≤ 16K is now offered only `tools::for_building` tools (files, web, `request_folder`, `open_path`), with no media, people or chat notes (`lean`). That's 1,815 tokens.
  - **History holds each file twice:**
    - `chat::without_written` drops older write/edit contents (the key itself, never a stand-in, which the model copied into files) and long code blocks pasted next to writes (without a note, which it also copied).
    - It's used only when the history doesn't fit (second `fit_history` pass).
    - `fit_history` also cuts the turn's own long earlier assistant text to 300 characters when needed.
  - `write_file` refuses stand-ins as a whole file ("[corrected content]", `fs::is_stand_in`).
  - `apply_edit` falls back to `loose_edit` (the one place that matches with spacing and indentation ignored; re-indents the new text).
  - **Repeat guards:**
    - A failed call wasn't retried after the file it needed was written.
    - A successful Write/Execute now clears `failed_calls` and the Read entries of `done_calls`.
  - **The check grew:**
    - The visibility test uses computed style: `offsetParent` is null in a window that never shows, so every button looked hidden.
    - It names ids the script asks for that the page lacks, and lists the page's ids.
    - It notes "no buttons or fields" when the ask wanted actions (`wants_actions`), and buttons that vanish after the page updates (counted at 1.5 s and 5 s).
    - It clicks each wired button after filling empty fields, then reports ones after which nothing changed (no text, dialog or error). Dialogs are stubbed so they can't block.
    - It flags "nothing changes by itself" when the ask wanted live updates (`wants_live`).
    - Missing files are reported once, by path.
  - `page_missing` + `forced_page`: CSS/JS written but no index.html.
  - The same problems twice in a row get a "rewrite the whole file with write_file" note.
  - Caps: 6 previews, 3 fix rounds, 12 follow-ups.
  - Build instructions now say "index.html first, each file with write_file, preview once every file is written".
  - **Pictures in chats:**
    - `make_room` used to keep the chat model loaded for a chat's job, so a 14B model plus Z-Image overflowed 12 GB and every picture fell to the processor (292 s).
    - Now, if the picture model fits alone but not beside the chat model, `Engine::pause` stops it keeping its port and key, and `give_back` restarts it at the same address (`ensure` reuses a paused address), so the turn's `Endpoint` stays valid.
    - Measured: 21 s on the card, chat model back in 5 s.
  - `forced_picture`: a picture ask answered without a call ("I can't generate images") gets one forced `create_image`.
  - A reply that denies making pictures after one was made is replaced (`denies_pictures`).
  - A second `create_image` in the same reply is skipped unless the ask wanted several (`wants_several`).
  - **Names check:** with the web already on, the first two names are looked up even when the model says it knows them. Its memory gave "green tunic, blue tunic, red tunic"; the search gave "blond, long pointy ears, green tunic and floppy hat".
  - **Save button:** `MediaInline` showed Save only when Read aloud was on (renderItems passed read-aloud's toast). It now gets its own `saveToast`.
  - **Results with the 14B model:**
    - The stock site came out working on most runs after the fixes (prices move, Buy/Sell change cash and holdings, verified by hand).
    - The finance tracker passed in 60 s, including the filled-in form click.
    - Some runs still stall on a bug the model can't reason out. Larger models and contexts do better.
- **Hugging Face picture links** (his brother only ever got the old "It runs GGUF chat models…" refusal):
  - That was the "not a supported family" message; SD 1.5 and FLUX.1 models hit it.
  - Added Realistic Vision V6.0 (SD 1.5, fp16 + vae-ft-mse) to the catalog, whose `hub` family "Stable Diffusion 1.5" opens SD 1.5 repos. Live-tested: 704×896 on the card in 9 s.
  - `hub_why_not` explains by kind: FLUX.1 not supported yet; picture repos only in diffusers folders (unet/, vae/) need a single file; video needs Wan, music ACE-Step; chat models need GGUF.
  - FLUX.1 (dev/schnell) support is the next step for picture links: it needs clip_l, t5xxl and ae files and a catalog base.

## 0.9.9 (2026-10-17): pictures that don't drift

His brother: within a few prompts, pictures (edits and new ones) "lose what he wants". A live test in dev mode (Qwen2.5-Coder 14B, Z-Image Turbo, FLUX.2 klein 4B; test chats and klein removed afterwards) found three causes:
- **Wrong picture edited:** the model passed `"<image-id>"` (copied from the parameter's wording), so the edit failed, and then kept passing the first picture's id, so every edit started over from the original.
  - `tools/media.rs edit_source`: the newest picture in the chat, unless the model named a real picture and the person's message points back ("first", "original", "earlier", "go back"…).
  - The parameter's description now says to leave it out.
- **The names check (0.9.8) took "him" as a name:** it looked up HIM, the Powerpuff Girls villain, and added lobster claws and red skin to the edit's description.
  - `chat::real_names` keeps only names capitalized exactly as written in the ask or description, never everyday words.
- **Edits of edits drift:** `RunCtx::original_of` walks `parent` back to the first picture (made or attached). From the second edit on, `picture` sends it as a second reference (`-r`), with "Work on the first picture and keep everything in it that isn't mentioned. The second picture is the original: keep every person's face and features exactly as there."
  - An earlier wording ("keep anything not mentioned the same as the original") changed a top hat into a different hat; the current one keeps both the hat and the face.
- **New pictures:**
  - A picture ask on a small local model (ctx ≤ 16K) gets only `tools::for_pictures` (media and web), so more of the chat fits.
  - The media note asks for each description in full, on its own: what's wanted now, plus anything earlier the user asked to keep, nothing else; and edit_image for changes.
  - Five pictures in a row in one chat each matched only their own ask.
- Verified by eye: four-step chain (portrait → red top hat → birthday party → night with string lights), the same face and hat throughout, about 22-26 s per edit on the card.

## Website (2026-10-12)

`docs/` (home, privacy policy, terms, 404) is served by a Cloudflare Worker with static assets: `website/wrangler.jsonc`, deployed with `cd website && npx -y wrangler@4.149.0 deploy` (his Cloudflare account, which wrangler on this PC is signed in to). Live at https://sulcusai.sseditor.com (a subdomain of his sseditor.com for now). Links use clean URLs (`/privacy`, `/terms`); Cloudflare redirects `*.html` to them.
- Don't run `wrangler pages project create` or `wrangler deploy` in the repo root: wrangler 4.149 "autoconfigures" the Vite app and rewrites package.json, the lockfile and vite.config.ts.
- Moving to its own domain (option B, e.g. sulcusai.app): buy it in Cloudflare, add `{ "pattern": "<domain>", "custom_domain": true }` to `routes` and deploy, verify the domain in Google Search Console (DNS TXT), then change the homepage, privacy and terms links and the authorized domain in Google Auth Platform › Branding. Keep sulcusai.sseditor.com as a second route so old links keep working.
