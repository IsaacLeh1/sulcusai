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
- Music on Linux: `C:\dev\sulcusai-distcestep-d881ad2-linux-x64.tar.gz` (22,490,295 bytes, sha256 6ef9ce24…f3cc; built in WSL Ubuntu 26.04 with gcc, `-DGGML_VULKAN=ON -DGGML_BACKEND_DL=ON -DGGML_CPU_ALL_VARIANTS=ON`, RUNPATH `$ORIGIN`, symlinks kept), uploaded to the `engines` release. A 10-second song took 34 s on the processor in WSL (no Vulkan device there).
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

## Website (2026-10-12)

`docs/` (home, privacy policy, terms, 404) is served by a Cloudflare Worker with static assets: `website/wrangler.jsonc`, deployed with `cd website && npx -y wrangler@4.149.0 deploy` (his Cloudflare account, which wrangler on this PC is signed in to). Live at https://sulcusai.sseditor.com (a subdomain of his sseditor.com for now). Links use clean URLs (`/privacy`, `/terms`); Cloudflare redirects `*.html` to them.
- Don't run `wrangler pages project create` or `wrangler deploy` in the repo root: wrangler 4.149 "autoconfigures" the Vite app and rewrites package.json, the lockfile and vite.config.ts.
- Moving to its own domain (option B, e.g. sulcusai.app): buy it in Cloudflare, add `{ "pattern": "<domain>", "custom_domain": true }` to `routes` and deploy, verify the domain in Google Search Console (DNS TXT), then change the homepage, privacy and terms links and the authorized domain in Google Auth Platform › Branding. Keep sulcusai.sseditor.com as a second route so old links keep working.
