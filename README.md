# SynapseAI

*Working name. A different name will be chosen before release; see [Renaming](#renaming).*

A private AI workspace that runs on your own PC. It shows only the AI models
your computer can actually run, installs them with one click, and keeps
everything local unless you choose otherwise.

> **Status: early development (Phase 1, Foundation).** See [DESIGN.md](DESIGN.md)
> for the full plan and [HANDOFF.md](HANDOFF.md) for where work stands.

## What works today

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

## Privacy and security design

- The engine runs as a hidden local process on `127.0.0.1` only, on a random
  port, and needs a random key for every request.
- A Windows job object ends the engine if the app closes or crashes.
- Model output is shown as Markdown without raw HTML, and remote images are
  never loaded.
- Every request that leaves the PC goes through one connectivity check
  (`src-tauri/src/net.rs`).

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
