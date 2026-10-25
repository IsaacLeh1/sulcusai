# Your voice: how its engines and models are made

SulcusAI reads aloud and sings in a person's recorded voice (`src-tauri/src/media/own_voice.rs`, `svc.rs`). The files it downloads for that are made with these scripts, on Windows with MSVC 2022 Build Tools.

## Narration: qwentts.cpp (MIT) with Qwen3-TTS 12 Hz Base (Apache-2.0)

1. Check out [qwentts.cpp](https://github.com/ServeurpersoCom/qwentts.cpp) at commit `5916bf7` (with submodules) in `C:\dev\tools\voice\qwentts.cpp`.
2. `build-qwentts-cpu.cmd` builds the processor engine (`build-cpu`).
3. `build-qwentts-vulkan.cmd` builds the graphics-card engine in `C:\dev\qb`. It needs, under `C:\dev\vk`: glslc from Google's shaderc CI build (`shaderc\install\bin`), Vulkan-Headers and SPIRV-Headers at `vulkan-sdk-1.4.363.0`. It makes `vulkan-1.lib` from the driver's DLL with `def_from_dumpbin.py`.
4. The models are Serveurperso's GGUF conversions on Hugging Face (`Serveurperso/Qwen3-TTS-GGUF`, commit `b7ee2e8`), downloaded as they are.

## Singing: Seed-VC (GPL-3.0)

`export_svc.py` exports [Seed-VC](https://github.com/Plachtaa/seed-vc)'s singing model (v1, 44.1 kHz, f0, `DiT_seed_v2_uvit_whisper_base_f0_44k_bigvgan_pruned_ft_ema_v2.pth`) as eight ONNX pieces: Whisper-small's log-mel and encoder (Apache-2.0), RMVPE (MIT), CAMPPlus (Apache-2.0), the 44.1 kHz mel, the length regulator, the diffusion transformer, and BigVGAN v2 44 kHz (MIT). It also saves one reference run's in-between values, which `svc.rs`'s `seed_vc_matches_its_own_run` test checks the Rust port against. Run it from the seed-vc checkout in a Python 3.12 environment with PyTorch, `munch`, `einops` and `transformers` 4.x; the usage is in its header. `half_weights.py` then stores each piece's weights as float16 (computed in float32 as before).

## Packaging

`package.py <out dir>` zips both engines with qwentts.cpp's license, copies the ONNX pieces, and prints each file's size and SHA-256 for `catalog/catalog.json`'s `media.own_voice`. The zips and the ONNX pieces go on the repo's `engines` release.
