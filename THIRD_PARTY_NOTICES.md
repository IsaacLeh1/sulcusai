# Third-party notices

SulcusAI uses the open-source components below. Each one is under the license
shown. The complete dependency list, with every license, comes from:

```
cd src-tauri && cargo deny list
pnpm licenses list --prod
```

## Bundled or downloaded at runtime

| Component | License | Source |
|---|---|---|
| llama.cpp (`llama-server`, downloaded by the app) | MIT | https://github.com/ggml-org/llama.cpp |
| LLVM OpenMP runtime (included in the llama.cpp Windows build) | Apache-2.0 WITH LLVM-exception | https://github.com/llvm/llvm-project |

## Application framework and libraries

| Component | License |
|---|---|
| Tauri | MIT or Apache-2.0 |
| SQLite (via rusqlite, bundled) | Public domain |
| RustCrypto: aes-gcm, argon2, sha2, zeroize | MIT or Apache-2.0 |
| serde, tokio, reqwest, rustls, sysinfo, zip, uuid, chrono, base64, windows-rs and other Rust crates | MIT and/or Apache-2.0 (checked by `cargo deny`) |
| React, react-dom | MIT |
| react-markdown, remark-gfm and the unified ecosystem | MIT |

## AI models

Models aren't distributed with SulcusAI. The app downloads them from their
publishers' hosting at the user's request. Each model card shows the model's
license, for example Apache-2.0, the Llama 3.2 Community License or the Gemma
Terms of Use.
