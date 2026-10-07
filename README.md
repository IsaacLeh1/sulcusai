# SulcusAI

A private AI workspace that runs on your own PC. It shows only the AI models
your computer can actually run, installs them with one click, and keeps
everything local unless you choose otherwise.

> **Status: early development. Phases 1 (Foundation), 2 (Agent core) and 3 (Voice and meetings) are complete.** See [DESIGN.md](DESIGN.md)
> for the full plan and [HANDOFF.md](HANDOFF.md) for where work stands.

## What works today

The app starts bare-bones: chat and models. Everything else below is added
from **✨ Features**, which installs what a feature needs (for example the
speech model for dictation) and turns it on. Turning a feature off hides it
and stops it, and keeps your data.

- **Hardware check.** Detects your graphics card and its memory, system memory,
  processor and free disk space.
- **Model catalog with a spec filter.** Models your PC can't run are hidden,
  and the app says what upgrade would unlock them. Each model shows its license
  and an estimated speed.
- **One-click install.** Downloads the llama.cpp engine and the model, checks
  each file against its published SHA-256 hash, and measures real speed on your
  PC. Downloads resume if interrupted.
- **Chat** with streamed replies, a "thought process" view for reasoning models,
  and a model picker for each chat.
- **Context-window meter** showing instructions, your profile, the
  conversation, and room left for the reply.
- **Your profile.** Your name, details about you and how you like answers are
  included in every chat.
- **Connectivity levels:** Offline (the default), Local AI + Web, and Cloud. A
  one-click 🌐 toggle (or Ctrl+Shift+W) turns on web for a single chat.
- **Encrypted chats.** Chat titles, messages and your profile are encrypted on
  disk. The key is sealed to your Windows account.
- **App lock.** An optional PIN or passphrase, with a one-time recovery code,
  Windows Hello unlock and auto-lock when idle.
- **Activity log.** A record of everything the app does, including anything
  that goes over the internet. It never includes what you say in chats.
- **First-run setup.** A short guided setup: your PC, a suggested first model,
  your profile and the optional lock.
- **Agent mode.** The assistant can work in folders you share: read, search,
  create, edit, move and delete files (deletes go to the Recycle Bin), and run
  commands to build and test code.
  - **Plan / Auto / Bypass** run modes. Auto asks before every change and shows
    a diff first.
  - **Undo** for the file changes from any reply.
- **Memory** across chats. It's encrypted, and you can view, edit and delete it.
  Incognito chats neither use nor save memories.
- **Projects** with their own instructions, folders and memories.
- **Helpers and handoff.** The assistant can send a helper agent to research
  something in a fresh context. Long chats continue automatically in a new
  chat from a summary.
- **Scheduled tasks** that run on their own and save results as chats.
- **Connectors and plugins.** Local MCP servers with Allow / Ask / Off per
  tool, and plugins that add skills and connectors.
- **Speech models** (Models › Speech), filtered to what this PC's processor
  can keep up with: a quick one for dictation and voice chats, the most
  accurate one for meetings.
- **Dictation.** The 🎤 button (or Ctrl+Shift+Space) types what you say.
- **Voice chats.** Talk, and hear the answer as it's written; talk over it to
  interrupt. Replies can also be read aloud.
- **Natural voices** (optional, 31 languages), with Windows' built-in voices
  as the fallback.
- **Meeting mode.** Records your microphone and your computer's sound, so it
  hears everyone on Teams, Meet, Zoom or in the room with no bot joining.
  - Live transcript, with speaker echo removed and optional translated
    captions.
  - Notes when it ends: summary, most important points, action items,
    decisions, key points and topics.
  - Optional speaker labels you can name, and an encrypted recording you can
    replay line by line.
  - A searchable library, Markdown export, "Ask about this meeting" chats,
    and meeting tools any chat can use.
  - A reminder to tell people you're recording.
- **Translation** of text and text documents (subtitles keep their timing),
  with language detection and read-aloud.

## Privacy and security design

- The engine runs as a hidden local process on `127.0.0.1` only, on a random
  port, and needs a random key for every request.
- A Windows job object ends the engine if the app closes or crashes.
- Model output is shown as Markdown without raw HTML, and remote images are
  never loaded.
- Speech recognition runs as a second hidden local process. It has no API key
  option, so every route sits behind a random path prefix only the app knows.
- Every request that leaves the PC goes through one connectivity check
  (`src-tauri/src/net.rs`) and is recorded in the activity log.
- Encryption uses AES-256-GCM. The data key is sealed by Windows DPAPI, plus an
  Argon2id key from your PIN when app lock is on. Deleted rows are overwritten
  (`secure_delete`). Details and limits are in [HANDOFF.md](HANDOFF.md).

## License

- **Code:** [GNU AGPL v3.0](LICENSE). Contributors sign the [CLA](CLA.md); see
  [CONTRIBUTING.md](CONTRIBUTING.md) for why.
- **Name and logo:** not covered by the AGPL; see [TRADEMARKS.md](TRADEMARKS.md)
  and [branding/NOTICE.md](branding/NOTICE.md).
- **Third-party components:** see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Development

Windows 10/11 x64, Rust stable, Node 22+, pnpm.

```
pnpm install
pnpm app
```

## Renaming

The name appears in these places:

1. `src/brand.ts` (`APP_NAME`)
2. `src-tauri/tauri.conf.json` (`productName`, `identifier`, window `title`)
3. `index.html` (`<title>`)
4. `src-tauri/src/lib.rs` (`app_info`) and `src-tauri/src/chat.rs` (the system prompt)
5. `src-tauri/src/net.rs` (the user agent)
6. `package.json` and `src-tauri/Cargo.toml` (`name`)
7. The docs: README, CLA, TRADEMARKS, CONTRIBUTING, THIRD_PARTY_NOTICES

Changing `identifier` moves the data folder (`%LOCALAPPDATA%\<identifier>`).
