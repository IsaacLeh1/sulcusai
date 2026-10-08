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

## Phase 4 in progress (branch `phase4`, worktree `C:\dev\sulcusai-phase4`)

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
- **Microsoft/Google sign-in not built yet:** it needs app registrations (Azure app ID; Google OAuth client) from the user. Outlook.com no longer allows password IMAP; Gmail works now with an app password.
- UI preview without the backend: `mock.html` + `src/mock.tsx` (git-excluded) use Tauri's `mockIPC`. Run vite on port 1431 and open /mock.html.
- Not yet looked at in a running window: the Performance section, the Models tabs, the sidebar menu and the browser (typechecked and unit-tested only).

**Queued request (2026-10-07, not started; he said not to work on it yet):** keep working while locked, as a switch. Locking the app should be able to leave work running in the background, such as replies, agent steps, scheduled tasks and meeting recording, instead of stopping it. Today, locking drops the data key, which any running work needs to save. A likely approach is to keep the key only for running jobs while the window stays locked, and to show nothing until it is unlocked.

**Queued request (2026-10-07, not started):** browser choice and placement.
- Let users pick their main browser: Chrome, Edge, Safari (Mac only) or DuckDuckGo, alongside the built-in one. Driving Chrome or Edge safely needs an extension or native messaging, never a remote-debugging port (DESIGN.md rule). DuckDuckGo's browser has no automation API, so it could only be opened, not controlled.
- He decided: **browser control is its own switch on the Features page, and that feature includes the extension.** Turning it on installs or sets up a SulcusAI extension for Chrome or Edge that talks to the app over native messaging (no debug port). The assistant then works in his real browser. With it off, the assistant can't touch his browsers. The built-in browser remains the option that needs no extension.
- Move the Browser button out of the left sidebar to a web (🌐/🧭) icon at the top of the page.

**Phase 4 still to do:** Microsoft/Google sign-in for mail and calendar (needs his app registrations), documents and spreadsheets, and the desktop quick-ask overlay.
