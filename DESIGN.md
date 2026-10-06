# Local AI Workspace — Design Document

*Working name, to be decided. Status: design only, nothing built. Drafted 2026-10-06.*

## 1. Vision

A desktop app that works like Claude Cowork but runs on the user's own PC. The user opens it, sees only the AI models their computer can actually run, and installs one with a single click. From there, one app covers chat, agents, coding, files, meetings, voice, media generation, documents, email, calendar, notes, and more.

**Principles**

1. **Private by default.** Out of the box nothing leaves the PC, and web access is off. Turning web or cloud on is quick, but it is always the user's choice and always visible.
2. **Only show what works.** The app hides models and features the PC cannot run, and says why instead of leaving an empty screen.
3. **Simple first, deep on request.** The default UI is plain. Advanced mode (off by default) exposes model tuning, guardrails and processing controls.
4. **The user stays in control.** Run modes, permission prompts, an action log, undo, and an emergency stop apply to every agent action.

---

## 2. Connectivity levels

This one setting replaces the old "Entirely local" checkbox. It has three levels:

| Level | Default? | What works | What leaves the PC |
|---|---|---|---|
| **Offline** | ✅ Yes | Everything local: chat, agents, files, coding, voice, meetings, media, docs, notes, LAN sync | Nothing (see the model-download exception below) |
| **Local AI + Web** | | Adds web search, page fetching, the built-in browser, email/calendar sync, remote MCP connectors | Web requests and account sync only. **All AI processing stays on the PC.** No prompts are sent to AI cloud providers. |
| **Cloud** | | Adds cloud models (LLM, image, video, voice) via the user's API keys, the cloud sync relay, and models bigger than the PC can run | Prompts and data sent to the providers the user enables |

### Turning web on (quick and easy)

Web is **off by default**, but it is never more than one click away:

- **Globe button in the chat box.** One click turns web on for that chat, and it lights up while active. A long-press or menu offers "this chat / always / off".
- **Prompt when needed.** If the user asks for something that needs the web (for example "what's the weather" or "check my email"), the app shows an inline card: *"This needs web access. [Enable for this chat] [Always enable] [Not now]"*. No digging through settings.
- **Status bar indicator** always shows the current level (Offline / Web / Cloud) and switches it with one click.
- **Keyboard shortcut** (for example Ctrl+Shift+W) toggles web.
- Settings → Connectivity holds the full three-level control and per-feature options.

### Moving to Cloud

Cloud is a deliberate step:

- A one-time confirmation explains what gets sent and to whom.
- Each cloud provider is enabled separately.
- Every cloud-powered feature carries a ☁ badge.
- Local stays the default model for each task unless the user picks otherwise.
- Personal info is redacted before anything goes to the cloud.
- Spend is tracked against a budget the user sets.

### Model-download exception

Installing or updating models needs internet even on Offline. These downloads are explicit, user-started actions that only contact the model host. They are shown in the downloads panel and never carry user data. Offline import of model files from disk or USB is also supported.

---

## 3. Model catalog and the spec filter

### Hardware detection

On first run and on demand, the app detects:

- GPU vendor, model, VRAM, and backend support (CUDA, ROCm, Vulkan, DirectML, Metal later)
- System RAM, CPU cores and instruction sets (AVX2/AVX-512)
- Free disk space on the models drive
- Whether the PC is on battery

### Fit rule

A model *variant* (a model plus a quantization) is shown only if it fits:

```
needed = weights_size + kv_cache(context_length) + runtime_overhead
fits   = needed <= VRAM                          (full GPU)
      or needed <= VRAM + usable_RAM AND estimated_speed >= minimum   (partial offload)
      and weights_size + buffer <= free_disk
```

- If no variant of a model fits, **the model is hidden**.
- If some variants fit, only the best-fitting ones are offered, and the default is the highest quality that still runs at a usable speed.
- Each card shows the expected speed on this PC, which is measured by a quick benchmark after install.
- **Upgrade hints:** "Adding 8 GB of RAM would unlock 6 more models."
- The same filter applies to speech, image, video and music models.
- Cloud models are not filtered, since they don't depend on hardware.

### Catalog

- A curated, signed manifest (JSON) lists each model's variants, sizes, requirements, license, and "best for" tags (chat, coding, vision, translation, tool calling, and so on).
- **One-click install:** download, verify the checksum, register the model, and run a quick benchmark.
- Updates and rollback, plus a storage manager to delete models and see disk usage.
- Import of Hugging Face / GGUF models (in advanced mode).
- **Auto-router:** picks a suitable installed model per task, and the user can override it.

---

## 4. Features

### 4.1 Chat and agent core

- Chat with branching, editing and regenerating, search across all chats, folders and pins, and export/import.
- **Context-window meter:** always visible. It shows tokens used versus the limit, broken down by system prompt, profile, memory, files and chat.
- **Run modes:** each chat can be switched between these, and the current mode is always shown.
  - **Plan** — the agent proposes the steps and waits for approval.
  - **Auto** — safe actions run, and risky ones ask first.
  - **Bypass** — everything runs except actions that always confirm (sending email, inviting attendees, deleting outside the granted folders, entering payment details).
- **Agent management:** the main agent can start subagents for parallel work, and a panel shows the running agents with stop buttons.
- **Auto-handoff:** when context nears its limit, the app summarizes into a fresh chat and links back to the old one. It can also hand off between models, for example to a coding model for code.
- **Profile:** details the user types about themselves and their preferences, included in every chat.
- **Cross-chat memory:** the app saves facts, preferences and project context it picks up while chatting and recalls the relevant ones later.
  - Stored locally, with local embeddings for search.
  - The user can view, edit and delete memories, turn memory off, or use incognito chats.
- **Projects:** workspaces with their own instructions, files and memory. Users can ask questions about folders and PDFs (local search over their contents, with citations).
- Prompt library, templates, custom assistants (personas), and side-by-side model comparison.

### 4.2 Files and coding

- **File management:** browse, read, create, edit, move, rename and organize, but only in folders the user grants.
  - Deletes go to the Recycle Bin, never a permanent delete.
  - An in-app viewer opens files.
- **Coding:** a Claude Code-style agent.
  - Opens a project folder, edits code, and runs terminal commands, tests and git.
  - Shows changes as diffs to approve or reject.
  - Recommends coding-tuned models.
- **Checkpoints and undo** for every file and code change.
- **Sandboxed Python** for data analysis, charts and CSV work.
- **Canvas pane:** a live preview of HTML, charts and diagrams.

### 4.3 Web, browser, email and calendar *(need Web or Cloud)*

- **Internet search and page fetching**, with citations.
- **Built-in browser** with agent control, like Claude in Chrome.
  - Uses an isolated browser profile.
  - Visible activity, a stop button, and confirmations before forms are submitted or anything is bought.
  - **No unauthenticated debug port** (lesson from OpenWork).
- **Email:** read, search, summarize, sort, draft and reply.
  - Connects to Outlook/Microsoft Graph, Gmail, or IMAP.
  - Sending always needs confirmation.
- **Calendar:** view events, find free time, and create or edit events.
  - Connects to Google, Outlook, or CalDAV.
  - Inviting attendees always needs confirmation.
- Sign-in tokens are stored encrypted on the PC.

### 4.4 Voice, dictation and meetings

- **Dictation:** push-to-talk or a hotkey types speech into any text box. Local speech recognition (Whisper-class).
- **Voice mode:** a two-way spoken conversation.
  - Speech recognition, then the model, then speech output.
  - Voice activity detection lets the user interrupt, and responses stream with low delay.
- **Meeting mode** — the user clicks **Begin meeting**:
  - Records the **microphone and the computer's audio output** (Windows loopback capture) on separate channels. Mic = "You", computer audio = other people, plus speaker labels (diarization) to tell the others apart.
  - Works with Teams, Google Meet, Zoom, or an in-person meeting, with no bot joining the call.
  - Live transcript and optional live translated captions.
  - When the meeting ends it produces a summary with **action items, topics discussed, key items, and the most important points**.
  - Action items become tasks or calendar events. The notes can be exported to a document.
  - **Meeting library:** search past meetings and ask questions about them.
  - Can start automatically from calendar events (opt-in).
  - **Consent notice:** reminds the user to tell participants about the recording (some US states require everyone's consent), with an optional spoken or chat announcement.

### 4.5 Images, video and audio

- **Image generation and image understanding** (vision models: describe images, read text in screenshots, answer questions about images).
- **Image editing:**
  - Edits from a plain-language instruction.
  - Paint a mask to fill in or extend part of an image.
  - Background removal, upscaling and restyling.
- **Video generation:** text-to-video and image-to-video.
  - The heaviest feature (often 12–24 GB+ VRAM).
  - Shows progress and an estimated time.
  - When no video model fits the PC, the section explains why.
- **Music and audio:** music from a prompt (genre, mood, length, optional lyrics and vocals), sound effects, and narration.
  - Built-in player with trim, loop and download.

### 4.6 Productivity

- **Documents and spreadsheets:** create and edit Word, Excel (working formulas and charts), PowerPoint, PDF and Markdown.
  - Generated by local libraries, with no Office needed.
  - Previewed inside the app.
- **Notes:** rich text or Markdown, folders and tags, and search by keyword or meaning.
- **Tasks:** due dates, priorities, reminders and subtasks.
  - Created automatically from meetings and email.
  - Due dates sync to the calendar.
- **Translation:** text, whole documents (keeping their formatting), images, and live speech in meetings and voice mode.
  - Detects the language automatically.
  - Can speak the translation aloud.
- **Scheduled tasks and automations:** run prompts or workflows on a schedule, or when a file changes, an email arrives, or a meeting ends. Desktop notifications.

### 4.7 Desktop assistant

- A global hotkey opens a **quick-ask overlay** from anywhere.
- Ask about the screen or a selected region of it.
- Actions on whatever the user has copied.
- System tray icon.
- **Desktop control** beyond the browser:
  - Opt-in.
  - Always visible while running.
  - Emergency stop on a hotkey.

### 4.8 Extensibility

- **Plugins** bundle skills, commands, agents and hooks. Installed from a catalog or a folder.
- **MCP connectors:**
  - Local servers work on Offline.
  - Remote servers need Web or Cloud and are labeled as such.
  - Each tool can be allowed or blocked individually.
- **Local API server** that other programs on the PC can use (OpenAI-compatible), opt-in and limited to this PC.

### 4.9 Multi-device sync

- Syncs chats, memory, profile, projects, notes, tasks, settings, plugins and schedules between the user's PCs.
- **Over the same network (LAN):** devices sync directly, so this works on Offline.
- **Across networks:** an end-to-end encrypted relay, available on Cloud level only.
- Devices pair with a code or QR code (reusing the pattern from the expense tracker's sync design). Paired devices are managed under Device management.
- **Models are not synced.** Each PC runs its own spec filter. A model can optionally be copied over the LAN instead of downloaded again.
- Conflicts are resolved item by item.

### 4.10 Device management

- Shows the hardware and a live monitor (GPU, RAM, temperatures, power).
- Paired devices, with the option to remove one.
- Battery and quiet modes.
- Unloads idle models from memory automatically.
- Choice of which drive models are stored on.

---

## 5. Advanced mode (off by default)

Settings → Advanced → "Enable advanced mode". A warning explains that these settings can make answers worse or less safe.

- **Model tuning:**
  - Sampling (temperature, top-p/k, min-p, repetition penalties).
  - Context length, quantization choice, GPU layers, threads, batch size.
  - Prompt template.
- **Guardrails:**
  - Edit or replace the app's safety instructions.
  - Refusal strictness and content filters.
  - Tool-permission limits.
  - Note: these change the app's layer only. Behavior trained into the model itself cannot be switched off here, so how much can be loosened depends on the model.
- **How it processes:**
  - Reasoning ("thinking") on or off and its budget.
  - Context handling (cut old messages vs summarize), when handoff kicks in.
  - Tool-use style, subagent limits.
- **Model work:**
  - Import custom models.
  - Fine-tune on the user's own data with local LoRA training.
  - Benchmarks.
- **Reset to defaults** for each section and for everything.
- **Parental controls:** a PIN can lock advanced mode and the guardrail settings.

---

## 6. Safety, privacy and trust

- App lock (PIN or Windows Hello). All local data is encrypted.
- **Action log:** every agent action is recorded and searchable.
- Undo and checkpoints. Emergency stop (button and hotkey).
- Actions that always need confirmation, in every run mode:
  - sending messages or email
  - inviting attendees
  - purchases
  - submitting forms on the web
  - deleting outside the granted folders
- Content from web pages, files and email is treated as data, never as instructions (protection against prompt injection).
- Cloud mode: personal info redaction, spend limits, and a badge on every cloud feature.
- Optional separate user profiles on one PC.

---

## 7. Proposed architecture

*Recommendations only. Stack choices still need sign-off (see §10).*

| Layer | Recommendation | Why |
|---|---|---|
| App shell | **Tauri 2** (Rust) + TypeScript UI (Solid or React) | Small, fast. A familiar stack, but the code is written fresh. Nothing is copied from the Cowork/OpenWork codebases (see §8) |
| Text models | **llama.cpp** (`llama-server`) managed by the app | Widest hardware support, GGUF format, tool calling |
| Speech recognition | whisper.cpp + Silero voice detection | Local, fast, many languages |
| Speech output | Kokoro-class voices (Apache-2.0) | Small, natural, local. Avoid the newer Piper fork (GPL-3.0); the original Piper (MIT) is OK if needed |
| Images / video | **stable-diffusion.cpp** (MIT) | Covers generation, editing, and video models. **Not ComfyUI**: it is GPL-3.0, so bundling it would force the app open-source |
| Music / audio | Local music and sound-effect models (chosen per catalog) | Filtered by specs |
| Storage | SQLite (encrypted) + sqlite-vec for embeddings | One local file, no server |
| Meeting audio | Windows WASAPI loopback + mic capture | Hears both sides with no call bot |
| Browser | Embedded Chromium/WebView2 with an authenticated automation channel | Isolated, controllable, with no exposed debug port |
| Documents | Local docx/xlsx/pptx/pdf libraries | No Office needed |
| Sync | LAN peer discovery + an end-to-end encrypted relay (Cloud only) | Works offline on the same network |
| Agent tools | Internal tool registry + MCP client | One permission system for built-in and outside tools |

**Process model:** the UI talks to a Rust core. The core starts and supervises the model engines (text, speech, image) as separate local processes on random localhost ports with an auth token, and loads and unloads them as needed to stay within RAM and VRAM.

---

## 8. Ownership and licensing

**Decided 2026-10-06:**
- The app is built **from the ground up**. No code is forked or copied from the earlier Cowork/OpenWork work. Lessons and design ideas carry over, but not code.
- **Open source first, then closed later.** It launches as open source, and future versions may move to the owner's own proprietary license. All app code is original and owned outright, which is what makes that switch possible.

**How the switch works:**
- As the sole copyright holder, the owner can release **future** versions under any license, including a closed one.
- Versions already released stay under their open license **forever**. Anyone can keep using and forking them, and that cannot be taken back. Only new work becomes closed.
- The real protections against forks are **trademark** (forks can't use the name or logo), being the official source of updates and the model catalog, and continuing development.

**Starting license — DECIDED 2026-10-06: AGPL-3.0-only + a mandatory Contributor License Agreement.**

The options that were considered:

| Option | During the open phase | Effect on going closed later |
|---|---|---|
| **AGPL-3.0 + contributor agreement** ✅ *chosen* | Fully open source. Anyone who ships a modified version, even as a hosted service, must publish their changes. | The owner can still close it or sell commercial licenses (dual licensing), but nobody else can make a closed competitor from the code. |
| **Apache-2.0** | Open source with the most adoption and fewest contributor objections; includes a patent grant. | Easy for the owner to close, but **anyone can also take the code and build a closed competing product**. |
| **FSL / BSL** ("source-available") | Code is public, but competing commercial use is banned. Each version converts to Apache/MIT after about 2 years. | The most control, but it is **not** "open source" by the official OSI definition, which some contributors and users care about. |

**AGPL notes:**
- The owner, as copyright holder, isn't bound by the AGPL and can sell commercial licenses at any time (dual licensing).
- Permissive dependencies (MIT, Apache-2.0, BSD) are compatible with AGPL-3.0.
- The localhost-only API server (§4.8) doesn't trigger AGPL's network clause for the owner. But anyone who hosts a modified copy for remote users must offer them the source.
- AGPL conflicts with Apple's App Store terms. Distribute the macOS version directly. The Microsoft Store, winget and a direct download are fine.
- Each source file starts with an `SPDX-License-Identifier: AGPL-3.0-only` header and a copyright line naming the owner.

**Repo files needed before the first public commit:**
- `LICENSE` (the full AGPL-3.0 text)
- `CLA.md`, plus the CLA Assistant bot on GitHub so pull requests can't merge until the CLA is signed
- `CONTRIBUTING.md` (explains the CLA and the closed-later plan)
- `TRADEMARKS.md` (forks must rename)
- `THIRD_PARTY_NOTICES`
- `branding/` folder with its own "all rights reserved" notice

**Rules that keep the code switchable:**

1. **Allowed dependencies:** permissive licenses only (MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0 for unmodified files). Examples: Tauri, llama.cpp, whisper.cpp, stable-diffusion.cpp, SQLite, sqlite-vec.
2. **Banned in the app:** GPL, AGPL, LGPL (static linking), SSPL and similar copyleft licenses. Examples to avoid: ComfyUI and the newer Piper fork (both GPL-3.0).
   - This applies even while the app is open source (and even if the app itself is AGPL). The owner can relicense their own code, but **not other people's GPL code**, so a single GPL dependency would block closing the app later.
3. **License check in CI:** a dependency-license scan (`cargo-deny` for Rust, a license checker for npm) fails the build if a banned license appears.
4. **Credits screen:** the third-party notices that permissive licenses require are shown under Settings → About → Licenses.
5. **Models are not bundled.**
   - The app downloads models from their hosts at the user's request, so model weights are not redistributed inside the app.
   - Each model card shows its license (for example Apache-2.0, the Llama Community License, or CC-BY-NC).
   - Non-commercial models are tagged, and can be hidden if the app is sold or used commercially.
6. **Contributor License Agreement (CLA): required from day one.**
   - Every outside contributor signs a CLA (or copyright assignment) before their first pull request is merged, enforced automatically by a CLA bot on GitHub.
   - The CLA must grant the owner the right to relicense their contributions, including under closed licenses.
   - **Without it, every merged outside contribution stays under the open license and blocks closing the app** unless that code is removed and rewritten.
   - Contributors should be told about the closed-later plan up front, in the README and CONTRIBUTING file.
7. **Own name and assets:** an original name, logo and icons, with no UVU, Anthropic, Claude or OpenWork marks unless an agreement allows it. Check the name for trademark conflicts and **register the trademark** before the open-source launch, since the trademark is the main protection once forks exist. A trademark policy file says forks must rename.
   - **Keep logos and branding out of the open license:** put them in a separate folder marked "not covered by the code license".
8. **Legal review:** have a lawyer review the license text and terms of use before public release or sale.

**Optional: license keys.** If the app will be sold, offline-friendly activation is possible: a signed license file checked on the PC with a public key, with no "phone home" needed. This keeps it compatible with Offline mode.

---

## 9. Build phases

1. **Foundation:**
   - App shell and hardware detection.
   - Catalog, spec filter and one-click install.
   - Chat with the context meter and profile.
   - Connectivity levels and the web toggle.
   - Settings, app lock, action log.
2. **Agent core:**
   - Run modes, tools, file management, coding with diffs and checkpoints.
   - Memory and projects.
   - Subagents and auto-handoff.
   - Scheduled tasks, plugins and MCP.
3. **Voice and meetings:**
   - Dictation, voice mode, meeting mode (capture, transcript, summary, library).
   - Translation.
4. **Web and productivity:**
   - Web search, built-in browser and control.
   - Email and calendar.
   - Notes and tasks.
   - Documents and spreadsheets.
   - Desktop assistant overlay.
5. **Media:** image generation and editing, music and audio, video.
6. **Advanced and cloud:**
   - Advanced mode (tuning, guardrails, processing, LoRA).
   - Cloud level and providers.
   - Local API server.
7. **Sync and polish:**
   - Multi-device sync.
   - Setup wizard, backup and export.
   - Accessibility, localization, signed updates.
   - macOS/Linux, phone companion.

Each phase ends with a usable, installable build.

---

## 10. Open decisions

1. **Name and branding.** An original name, since it is the owner's own product (see §8).
2. ~~**Fresh build or fork.**~~ **Decided:** built from the ground up.
3. **License.** Decided: open source first, closed later. Still open:
   - ~~the starting license~~ **decided: AGPL-3.0-only + CLA** (see §8)
   - when and how it goes closed (all at once, or open-core with paid add-ons)
   - free or paid (license keys?)
   - where signed updates are hosted
4. **Starter models.** Which models make the first curated catalog, and is their licensing OK to redistribute?
5. **Meeting bot.** Is the local-capture-only approach enough, or is an optional cloud bot that joins calls wanted at the Cloud level?
6. **Platforms.** Windows only at first?
7. **Cloud relay.** For cross-network sync: self-hosted (for example a Cloudflare Worker, as in the expense tracker's sync design) or a third party?
