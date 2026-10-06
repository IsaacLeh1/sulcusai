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
| whisper.cpp (`whisper-server`, downloaded with a speech model) | MIT | https://github.com/ggml-org/whisper.cpp |
| SDL2 (included in the whisper.cpp Windows build; not used by SulcusAI) | Zlib | https://github.com/libsdl-org/SDL |
| ONNX Runtime (downloaded with natural voices or speaker labels; its own ThirdPartyNotices.txt is kept beside it) | MIT | https://github.com/microsoft/onnxruntime |

## Application framework and libraries

| Component | License |
|---|---|
| Tauri | MIT or Apache-2.0 |
| SQLite (via rusqlite, bundled) | Public domain |
| RustCrypto: aes-gcm, argon2, sha2, zeroize | MIT or Apache-2.0 |
| serde, tokio, reqwest, rustls, sysinfo, zip, uuid, chrono, base64, windows-rs and other Rust crates | MIT and/or Apache-2.0 (checked by `cargo deny`) |
| ort (ONNX Runtime bindings, loading the downloaded DLL) | MIT or Apache-2.0 |
| unicode-normalization | MIT or Apache-2.0 |
| React, react-dom | MIT |
| react-markdown, remark-gfm and the unified ecosystem | MIT |

## AI models

Models aren't distributed with SulcusAI. The app downloads them from their
publishers' hosting at the user's request. Each model card shows the model's
license, for example Apache-2.0, the Llama 3.2 Community License or the Gemma
Terms of Use.

Speech and voice models, also downloaded on request:

| Model | License | Source |
|---|---|---|
| Whisper (OpenAI), whisper.cpp GGML conversions | MIT | https://huggingface.co/ggerganov/whisper.cpp |
| Silero VAD (voice detection) | MIT | https://huggingface.co/ggml-org/whisper-vad |
| Supertonic 3 (natural voices, Supertone) | BigScience OpenRAIL-M: commercial use allowed, with use-based restrictions | https://huggingface.co/Supertone/supertonic-3 |
| WeSpeaker ResNet34-LM (speaker labels) | CC-BY-4.0. Attribution: WeSpeaker, https://github.com/wenet-e2e/wespeaker | https://huggingface.co/Wespeaker/wespeaker-voxceleb-resnet34-LM |
