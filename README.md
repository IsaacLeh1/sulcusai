# SulcusAI

A private AI workspace that runs on your own PC. It shows the models your
computer can actually run, installs them in one click, and keeps everything
on the PC unless you turn on the web or a cloud model.

Website and downloads: [sulcusai.sseditor.com](https://sulcusai.sseditor.com/)
and the [releases page](https://github.com/IsaacLeh1/sulcusai/releases).

## What it does

- **Local models that fit.** Checks the graphics card, memory and disk, offers
  models that will run well, and sets them up with llama.cpp.
- **Chat and agents.** Streamed replies, tools, research with sources, and an
  agent that works in folders you share, asking before it deletes anything or
  sends anything off the PC.
- **Voice.** Dictation (in the app or into any program), voice chats, natural
  voices, and meeting notes from your microphone and the computer's sound.
- **Studio.** Pictures, video and songs made on the PC, kept in an encrypted
  gallery.
- **Your stuff.** Mail, calendar, notes, documents and memories, encrypted on
  disk, with an optional app lock.
- **Other programs.** A local API in OpenAI's, Claude's and Ollama's formats,
  MCP connectors and skills, and a phone link.

Most of this is off at first and turned on from **Features**.

## Privacy

Offline is the default. Every request that leaves the PC goes through one
check (`src-tauri/src/net.rs`) and shows in the activity log. Chats and other
personal data are encrypted with AES-256-GCM under a key sealed to your
Windows account.

## Building

Windows 10/11 x64 is the main platform. macOS (Apple silicon) and Linux (x64)
build and run the core, and some features are Windows-only for now. You need
Rust stable, Node 22+ and pnpm; on Linux, the WebKitGTK and audio packages
listed in `.github/workflows/ci.yml`.

```
pnpm install
pnpm app
```

## License

The code is [AGPL-3.0](LICENSE). Contributors sign the [CLA](CLA.md) (see
[CONTRIBUTING.md](CONTRIBUTING.md)). The name and logo aren't covered by the
AGPL: see [TRADEMARKS.md](TRADEMARKS.md). Third-party components are listed in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
