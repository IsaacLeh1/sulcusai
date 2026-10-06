# Contributing to SynapseAI

Thanks for helping. Please read this first. It's short.

## The license plan (please read)

- SynapseAI is open source under the **GNU AGPL v3.0** ([LICENSE](LICENSE)).
- **Future versions may be released under different terms, including closed
  source.** Versions already released under the AGPL will always stay
  available under the AGPL.
- So that this stays possible, every contributor signs the
  **[Contributor License Agreement](CLA.md)** before their first pull request
  is merged. A bot asks you to sign by leaving a comment; it takes a minute.
  You keep the copyright to your work.

## Rules for dependencies

The app must not depend on copyleft code, because the owner can relicense
only code they have rights to.

- **Allowed:** MIT, Apache-2.0, BSD, ISC, Zlib, Unicode, CC0, MPL-2.0 (unmodified files), and other permissive licenses.
- **Not allowed:** GPL, AGPL, LGPL, SSPL, and "non-commercial" licenses.
  (For example, ComfyUI and the newer Piper fork are GPL-3.0 and can't be used.)
- CI fails if a disallowed license appears: `cargo deny check licenses` and `pnpm check:licenses`.
- AI **models** are never bundled. The app downloads them when a user asks,
  and each model card shows its license.

## Source file headers

Every source file starts with:

```
// SPDX-License-Identifier: AGPL-3.0-only
```

## Branding

The name and logo aren't covered by the AGPL. See [TRADEMARKS.md](TRADEMARKS.md).
Forks must use their own name and artwork.

## Development

Requirements: Windows 10/11 x64, Rust (stable), Node 22+ and pnpm.

```
pnpm install
pnpm app          # run the desktop app in development
pnpm test         # UI tests
cd src-tauri && cargo test   # core tests
```

The end-to-end engine test downloads a ~1.1 GB model, so it doesn't run by default:

```
cd src-tauri && cargo test e2e -- --ignored --nocapture
```
