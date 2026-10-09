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
| ONNX Runtime (downloaded with natural voices, speaker labels or background removal; its own ThirdPartyNotices.txt is kept beside it) | MIT | https://github.com/microsoft/onnxruntime |
| stable-diffusion.cpp (`sd-cli`, downloaded with a picture, upscaling or video model; includes ggml, libwebp and libwebm) | MIT; libwebp and libwebm BSD-3-Clause | https://github.com/leejet/stable-diffusion.cpp |
| acestep.cpp (`ace-lm`, `ace-synth`, downloaded with a music model; our Windows build of commit d881ad2, includes ggml) | MIT | https://github.com/ServeurpersoCom/acestep.cpp |
| QVAC Fabric llama.cpp (`llama-finetune-lora`, downloaded the first time a model is taught; includes libcurl, cpp-httplib, nlohmann/json and linenoise under their own licenses, kept beside it) | MIT | https://github.com/tetherto/qvac-fabric-llm.cpp |
| TweetNaCl-js 1.0.3 (`nacl-fast.min.js`, served inside the phone page) | Public domain (Unlicense) | https://github.com/dchest/tweetnacl-js |

## Application framework and libraries

| Component | License |
|---|---|
| Tauri | MIT or Apache-2.0 |
| SQLite (via rusqlite, bundled) | Public domain |
| RustCrypto: aes-gcm, argon2, sha2, zeroize, hkdf, hmac, crypto_secretbox | MIT or Apache-2.0 |
| spake2 (pairing PCs), curve25519-dalek | MIT or Apache-2.0; curve25519-dalek BSD-3-Clause |
| qrcode (QR codes for adding a phone) | MIT or Apache-2.0 |
| serde, tokio, reqwest, rustls, sysinfo, zip, uuid, chrono, base64, windows-rs and other Rust crates | MIT and/or Apache-2.0 (checked by `cargo deny`) |
| ort (ONNX Runtime bindings, loading the downloaded DLL) | MIT or Apache-2.0 |
| unicode-normalization | MIT or Apache-2.0 |
| html2text (reading web pages) | MIT |
| async-imap, mail-parser, lettre, tokio-rustls (email) | MIT and/or Apache-2.0 |
| webpki-roots (trusted certificate list for email) | CDLA-Permissive-2.0 |
| quick-xml, chrono-tz (calendars) | MIT and/or Apache-2.0 |
| webview2-com (the built-in browser's in-process DevTools channel) | MIT |
| docx-rs, rust_xlsxwriter, calamine, pdf-extract (documents) | MIT and/or Apache-2.0 |
| PowerPoint template `assets/blank.pptx`, made from python-pptx's default template | MIT (python-pptx) |
| image (pictures: sizes, masks, thumbnails, cut-outs) | MIT or Apache-2.0 |
| axum (the opt-in local API server) | MIT |
| React, react-dom | MIT |
| react-markdown, remark-gfm and the unified ecosystem | MIT |

## Online services (only with web access on)

| Service | Terms |
|---|---|
| Open-Meteo (weather tool; weather data by Open-Meteo.com) | Data CC BY 4.0. The free API is for non-commercial use; a commercial release needs an Open-Meteo plan or another source. |
| OpenStreetMap Nominatim (naming this PC's location, on request) | Data © OpenStreetMap contributors, ODbL; light use under the Nominatim usage policy |
| Windows location services (this PC's position, when allowed in Windows) | Part of Windows |
| DuckDuckGo, Brave Search, SearXNG (web search, chosen in Settings) | Each service's own terms |

## Cloud providers (only at the Cloud level, with the user's own API key)

| Service | Terms |
|---|---|
| Anthropic (Claude), OpenAI, Google Gemini, OpenRouter, or another OpenAI-compatible service the user adds | Each provider's own terms, under the user's own account. Nothing is sent until the user adds a key, turns the provider on, picks one of its models and is at the Cloud level. |

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

Picture, video and music models, also downloaded on request:

| Model | License | Source |
|---|---|---|
| FLUX.2 [klein] 4B (Black Forest Labs), GGUF by Unsloth; FLUX.2 VAE | Apache-2.0 | https://huggingface.co/black-forest-labs/FLUX.2-klein-4B |
| Qwen3 4B and Qwen3 4B Instruct 2507 (text encoders for pictures) | Apache-2.0 | https://huggingface.co/Qwen |
| Z-Image Turbo (Tongyi-MAI), GGUF by leejet; FLUX.1 VAE | Apache-2.0 | https://huggingface.co/Tongyi-MAI/Z-Image-Turbo |
| Wan 2.2 TI2V 5B and Wan 2.1 T2V 1.3B (Wan-AI), with their VAEs | Apache-2.0 | https://huggingface.co/Wan-AI |
| UMT5-XXL encoder, GGUF by city96 | Apache-2.0 | https://huggingface.co/city96/umt5-xxl-encoder-gguf |
| Real-ESRGAN x4plus (upscaling) | BSD-3-Clause | https://github.com/xinntao/Real-ESRGAN |
| BiRefNet lite (background removal), ONNX by onnx-community | MIT | https://github.com/ZhengPeng7/BiRefNet |
| ACE-Step 1.5 (music), GGUF by Serveurperso | MIT | https://huggingface.co/ACE-Step/Ace-Step1.5 |
| Qwen3 VL 4B and 8B (seeing pictures), GGUF by Unsloth | Apache-2.0 | https://huggingface.co/Qwen |

Online services for signing in (only when you choose to): Microsoft identity platform and Microsoft Graph (Outlook mail and calendars), Google OAuth, Gmail IMAP/SMTP and Google Calendar CalDAV; each service's own terms apply.
