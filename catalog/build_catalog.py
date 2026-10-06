# SPDX-License-Identifier: AGPL-3.0-only
"""Regenerates catalog.json from the pinned file lists below.

Sizes and SHA-256 hashes come from the Hugging Face tree API
(https://huggingface.co/api/models/<repo>/tree/main, the `lfs.oid` field) and
the llama.cpp GitHub release `digest` field. Re-run after editing:
    python catalog/build_catalog.py
"""
import json
import pathlib

HF = "https://huggingface.co/{repo}/resolve/main/{file}"

FILES = {
    "unsloth/Qwen3-1.7B-GGUF": {
        "Q4_K_M": ("Qwen3-1.7B-Q4_K_M.gguf", 1107409472, "b139949c5bd74937ad8ed8c8cf3d9ffb1e99c866c823204dc42c0d91fa181897"),
        "Q5_K_M": ("Qwen3-1.7B-Q5_K_M.gguf", 1257880128, "b0949de5b2e06cbed6aa96517f9bd8afb334584b6f95ee83479292ff4bdd8ed3"),
        "Q6_K": ("Qwen3-1.7B-Q6_K.gguf", 1417755200, "6a9cadec4883df6f3efcf337103479dcc6a3efa9f5f8cc64a904dba57808207a"),
        "Q8_0": ("Qwen3-1.7B-Q8_0.gguf", 1834426944, "0becaa825564295d82e9af4d008bca5f8b7f5f73bf1c6a0b58f7c53ef26b47fd"),
    },
    "bartowski/Llama-3.2-3B-Instruct-GGUF": {
        "Q4_K_M": ("Llama-3.2-3B-Instruct-Q4_K_M.gguf", 2019377696, "6c1a2b41161032677be168d354123594c0e6e67d2b9227c84f296ad037c728ff"),
        "Q5_K_M": ("Llama-3.2-3B-Instruct-Q5_K_M.gguf", 2322154016, "0b94ccd04d908304cec5246a3d942b64417a423bc5c6d47c73bc557e590b5194"),
        "Q6_K": ("Llama-3.2-3B-Instruct-Q6_K.gguf", 2643853856, "1771887c15fc3d327cfee6fd593553b2126e88834bf48eae50e709d3f70dd998"),
        "Q8_0": ("Llama-3.2-3B-Instruct-Q8_0.gguf", 3421899296, "b5607b5090a8280063fff2d706bb3408ca6542341b06aab39c3eca0a28575921"),
    },
    "unsloth/Qwen3-4B-Instruct-2507-GGUF": {
        "Q4_K_M": ("Qwen3-4B-Instruct-2507-Q4_K_M.gguf", 2497281120, "3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597"),
        "Q5_K_M": ("Qwen3-4B-Instruct-2507-Q5_K_M.gguf", 2889514080, "5bde5e9d883622acb02bf77fe7dcbc56a8b9a9ad4be78a72ca23a532658b4ecb"),
        "Q6_K": ("Qwen3-4B-Instruct-2507-Q6_K.gguf", 3306261600, "cd7b21b38b3e71400587c184b6a9b04d3beb4d13fdae6464d4075dee4f1bc5ad"),
        "Q8_0": ("Qwen3-4B-Instruct-2507-Q8_0.gguf", 4280405600, "391c1e410fd9f4cf2de2b510273b56a84c19ce18f4fa3bfb3774031dac4ef068"),
    },
    "unsloth/gemma-3-4b-it-GGUF": {
        "Q4_K_M": ("gemma-3-4b-it-Q4_K_M.gguf", 2489894016, "04a43a22e8d2003deda5acc262f68ec1005fa76c735a9962a8c77042a74a7d19"),
        "Q5_K_M": ("gemma-3-4b-it-Q5_K_M.gguf", 2829698176, "974e5c2f13c321fc3258b6fbf2ce326a09d8ace511aa6846df1db62baf7df7d4"),
        "Q6_K": ("gemma-3-4b-it-Q6_K.gguf", 3190740096, "df3fb9a1449296d26258c0771d5280ceb71559ad98e2db658799a05218ed9fa1"),
        "Q8_0": ("gemma-3-4b-it-Q8_0.gguf", 4130402176, "81bf0583ab5bad155a5a3b15d155a880a1a1e4f7de2de5c06f10f64ac49f8336"),
    },
    "unsloth/Qwen3-8B-GGUF": {
        "Q4_K_M": ("Qwen3-8B-Q4_K_M.gguf", 5027784512, "120307ba529eb2439d6c430d94104dabd578497bc7bfe7e322b5d9933b449bd4"),
        "Q5_K_M": ("Qwen3-8B-Q5_K_M.gguf", 5851113280, "159c694b93271e4edc1dc2a305b10cf981032c8f3035a7da00973312f0331504"),
        "Q6_K": ("Qwen3-8B-Q6_K.gguf", 6725900096, "0eaec718fdeab0f429dfa0ec481c090388811f5e63785ddb582292f4ef3c3827"),
        "Q8_0": ("Qwen3-8B-Q8_0.gguf", 8709519168, "0cfbf745760f07a76ddeb358dd025a27f2e11d1ca9c9a4169a373d52990fe86e"),
    },
    "unsloth/Qwen2.5-Coder-7B-Instruct-GGUF": {
        "Q4_K_M": ("Qwen2.5-Coder-7B-Instruct-Q4_K_M.gguf", 4683073504, "9a961bb225cb2b9fd84b2297df0d53089895c049d7d9dc5f5f8aebbcd3247872"),
        "Q5_K_M": ("Qwen2.5-Coder-7B-Instruct-Q5_K_M.gguf", 5444831200, "8a025f8bae85617ac95209d9211cbfb2a7142b129a201fa2c2c140ed34661113"),
        "Q6_K": ("Qwen2.5-Coder-7B-Instruct-Q6_K.gguf", 6254198752, "724b2b241230aad689f5b43fe8f22d452755bf98677fbcfaa36d2a1b6a89c140"),
        "Q8_0": ("Qwen2.5-Coder-7B-Instruct-Q8_0.gguf", 8098525152, "3b879bf3c429aeb01ae5edec57eb2e787b24eb317991dfe491d69357c7f02735"),
    },
    "unsloth/gemma-3-12b-it-GGUF": {
        "Q4_K_M": ("gemma-3-12b-it-Q4_K_M.gguf", 7300778336, "15b8fd9d8672cd4240c178c217ca781409291f34e353d2e913b29c7602ceb3ff"),
        "Q5_K_M": ("gemma-3-12b-it-Q5_K_M.gguf", 8445036896, "ea6d227c4983534b4661a0ada08328c21afa07c1e3c316fb55c172776cc808b3"),
        "Q6_K": ("gemma-3-12b-it-Q6_K.gguf", 9660811616, "5a99903816f50b6b7d63fe96e1ed51d6e03c795051e99146719f3e066a863069"),
        "Q8_0": ("gemma-3-12b-it-Q8_0.gguf", 12510212576, "a774cd9c6f289a6159f4a3f92a7ebca887e15073f92016d53c3593cb440a5982"),
    },
    "unsloth/Qwen3-14B-GGUF": {
        "Q4_K_M": ("Qwen3-14B-Q4_K_M.gguf", 9001753984, "5eaa0870bd81ed3b58a630a271234cfa604e43ffb3a19cd68e54a80dd9d52a66"),
        "Q5_K_M": ("Qwen3-14B-Q5_K_M.gguf", 10514570624, "305309039ea44df963c01f66d65e949f8eebca238554916df437f02295a3a413"),
        "Q6_K": ("Qwen3-14B-Q6_K.gguf", 12121938304, "c34d749069d5f19b998498ce84884975c551529548fa6a56b883345d166289c2"),
        "Q8_0": ("Qwen3-14B-Q8_0.gguf", 15698534784, "90224247d4a8076c0a689e833910f4291bce05dec81f472ebcba321607168ea1"),
    },
    "unsloth/gpt-oss-20b-GGUF": {
        "Q4_K_M": ("gpt-oss-20b-Q4_K_M.gguf", 11624759488, "c27536640e410032865dc68781d80a08b98f8db5e93575919af8ccc0568aeb4f"),
    },
}

APACHE = {"name": "Apache-2.0", "url": "https://www.apache.org/licenses/LICENSE-2.0", "commercial": True}
GEMMA = {
    "name": "Gemma Terms of Use",
    "url": "https://ai.google.dev/gemma/terms",
    "commercial": True,
    "note": "Commercial use allowed, subject to Google's Gemma Prohibited Use Policy.",
}
LLAMA = {
    "name": "Llama 3.2 Community License",
    "url": "https://huggingface.co/meta-llama/Llama-3.2-3B-Instruct/blob/main/LICENSE.txt",
    "commercial": True,
    "note": "Commercial use allowed below 700M monthly users; requires 'Built with Llama' attribution.",
}

# Models whose chat template supports tool calling (files, commands, agents).
TOOL_CAPABLE = {"qwen3-1.7b", "llama-3.2-3b", "qwen3-4b-2507", "qwen3-8b", "qwen2.5-coder-7b", "qwen3-14b", "gpt-oss-20b"}

# id, name, publisher, repo, license, tags, params (billions),
# (layers, kv heads, head dim, max context), active fraction (MoE), description
MODELS = [
    ("qwen3-1.7b", "Qwen3 1.7B", "Qwen (Alibaba)", "unsloth/Qwen3-1.7B-GGUF", APACHE,
     ["chat", "fast", "thinking"], 1.7, (28, 8, 128, 40960), 1.0,
     "A tiny, quick model with an optional thinking mode. Runs on almost any PC."),
    ("llama-3.2-3b", "Llama 3.2 3B Instruct", "Meta", "bartowski/Llama-3.2-3B-Instruct-GGUF", LLAMA,
     ["chat", "fast"], 3.2, (28, 8, 128, 131072), 1.0,
     "Small general-purpose assistant with a long context window."),
    ("qwen3-4b-2507", "Qwen3 4B Instruct", "Qwen (Alibaba)", "unsloth/Qwen3-4B-Instruct-2507-GGUF", APACHE,
     ["chat", "tool-calling", "recommended"], 4.0, (36, 8, 128, 262144), 1.0,
     "Strong all-rounder for its size, good at following instructions and using tools."),
    ("gemma-3-4b", "Gemma 3 4B", "Google", "unsloth/gemma-3-4b-it-GGUF", GEMMA,
     ["chat", "writing", "multilingual"], 4.3, (34, 4, 256, 131072), 1.0,
     "Friendly writer with wide language coverage."),
    ("qwen3-8b", "Qwen3 8B", "Qwen (Alibaba)", "unsloth/Qwen3-8B-GGUF", APACHE,
     ["chat", "thinking", "tool-calling"], 8.2, (36, 8, 128, 40960), 1.0,
     "Capable general model with a step-by-step thinking mode."),
    ("qwen2.5-coder-7b", "Qwen2.5 Coder 7B", "Qwen (Alibaba)", "unsloth/Qwen2.5-Coder-7B-Instruct-GGUF", APACHE,
     ["coding"], 7.6, (28, 4, 128, 32768), 1.0,
     "Tuned for writing, explaining and fixing code."),
    ("gemma-3-12b", "Gemma 3 12B", "Google", "unsloth/gemma-3-12b-it-GGUF", GEMMA,
     ["chat", "writing", "multilingual"], 12.0, (48, 8, 256, 131072), 1.0,
     "Larger Gemma with noticeably better reasoning and writing."),
    ("qwen3-14b", "Qwen3 14B", "Qwen (Alibaba)", "unsloth/Qwen3-14B-GGUF", APACHE,
     ["chat", "thinking", "tool-calling"], 14.8, (40, 8, 128, 40960), 1.0,
     "High-quality reasoning for PCs with a strong graphics card."),
    ("gpt-oss-20b", "gpt-oss 20B", "OpenAI", "unsloth/gpt-oss-20b-GGUF", APACHE,
     ["chat", "thinking", "tool-calling"], 21.0, (24, 8, 64, 131072), 0.25,
     "Mixture-of-experts reasoning model. Only part of it runs for each word, so it is faster than its size suggests."),
]

QUALITY = {"Q4_K_M": "Good", "Q5_K_M": "Better", "Q6_K": "Great", "Q8_0": "Best"}

ENGINE = {
    "name": "llama.cpp",
    "license": "MIT",
    "build": "b11450",
    "assets": {
        "vulkan-x64": {
            "url": "https://github.com/ggml-org/llama.cpp/releases/download/b11450/llama-b11450-bin-win-vulkan-x64.zip",
            "size": 33355526,
            "sha256": "8ce71f060c08cc8b89115eab0d63209d80537936706544d95c3b936fd0f63095",
        },
        "cpu-x64": {
            "url": "https://github.com/ggml-org/llama.cpp/releases/download/b11450/llama-b11450-bin-win-cpu-x64.zip",
            "size": 19414679,
            "sha256": "5f21f3e6d86b18f4a3177d4fb24de452f84d093f62afbe231e0d13194ca7132c",
        },
    },
}


MIT = {"name": "MIT", "url": "https://opensource.org/license/mit", "commercial": True}
WHISPER_REPO = "ggerganov/whisper.cpp"

# Speech recognition runs on the processor (whisper.cpp publishes CPU builds
# for Windows; its GPU builds are CUDA-only and very large).
SPEECH_ENGINE = {
    "name": "whisper.cpp",
    "license": "MIT",
    "build": "v1.9.2",
    "assets": {
        "cpu-x64": {
            "url": "https://github.com/ggml-org/whisper.cpp/releases/download/v1.9.2/whisper-bin-x64.zip",
            "size": 8194445,
            "sha256": "49dcc16de826f20bd53d44f947a1ae49dfa81f86cad67a64d80820cb192d674a",
        },
    },
}

# Silero voice detection (MIT), used by whisper.cpp to skip silence.
SPEECH_VAD = {
    "file": "ggml-silero-v5.1.2.bin",
    "url": HF.format(repo="ggml-org/whisper-vad", file="ggml-silero-v5.1.2.bin"),
    "size": 885098,
    "sha256": "29940d98d42b91fbd05ce489f3ecf7c72f0a42f027e4875919a28fb4c04ea2cf",
    "license": MIT,
}

# id, name, file, size, sha256, seconds to encode one 30 s window on 8
# modern cores (measured on an Intel Core Ultra 9 275HX with whisper.cpp
# v1.9.2; a rough guide, replaced by a measurement after install),
# description
SPEECH_MODELS = [
    ("whisper-base", "Whisper Base", "ggml-base-q5_1.bin", 59707625,
     "422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898", 0.6,
     "Quick and light. Fine for dictation in a quiet room."),
    ("whisper-small", "Whisper Small", "ggml-small-q5_1.bin", 190085487,
     "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb", 2.2,
     "Noticeably more accurate and still fast. A good everyday choice."),
    ("whisper-large-v3-turbo", "Whisper Large v3 Turbo", "ggml-large-v3-turbo-q5_0.bin", 574041195,
     "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2", 11.0,
     "The most accurate, including accents, noisy calls and other languages. Best for meetings."),
]


def speech():
    models = []
    for mid, name, f, size, sha, secs, desc in SPEECH_MODELS:
        models.append({
            "id": mid, "name": name, "publisher": "OpenAI", "source": f"https://huggingface.co/{WHISPER_REPO}",
            "description": desc, "license": MIT, "languages": 99, "secs_per_30s": secs,
            "file": f, "url": HF.format(repo=WHISPER_REPO, file=f), "size": size, "sha256": sha,
        })
    return {"engine": SPEECH_ENGINE, "vad": SPEECH_VAD, "models": models}


# Natural voices run on ONNX Runtime (MIT), downloaded on first use.
ONNX_RUNTIME = {
    "name": "ONNX Runtime",
    "license": "MIT",
    "build": "1.30.0",
    "assets": {
        "cpu-x64": {
            "url": "https://github.com/microsoft/onnxruntime/releases/download/v1.30.0/onnxruntime-win-x64-1.30.0.zip",
            "size": 82645522,
            "sha256": "c6ba983baf5681af108599675d2a89c2d145512d02de28aed0bff177cd0ba949",
        },
    },
}

OPENRAIL = {
    "name": "OpenRAIL-M",
    "url": "https://huggingface.co/Supertone/supertonic-3/blob/main/LICENSE",
    "commercial": True,
    "note": "Commercial use allowed, with use-based restrictions (no harmful uses such as impersonation or deception).",
}

SUPERTONIC_REPO = "Supertone/supertonic-3"
SUPERTONIC_REV = "3cadd1ee6394adea1bd021217a0e650ede09a323"
SUPERTONIC_FILES = [
    ("onnx/duration_predictor.onnx", 3700147, "c3eb91414d5ff8a7a239b7fe9e34e7e2bf8a8140d8375ffb14718b1c639325db"),
    ("onnx/text_encoder.onnx", 36416150, "c7befd5ea8c3119769e8a6c1486c4edc6a3bc8365c67621c881bbb774b9902ff"),
    ("onnx/vector_estimator.onnx", 256534781, "883ac868ea0275ef0e991524dc64f16b3c0376efd7c320af6b53f5b780d7c61c"),
    ("onnx/vocoder.onnx", 101424195, "085de76dd8e8d5836d6ca66826601f615939218f90e519f70ee8a36ed2a4c4ba"),
    ("onnx/tts.json", 8253, "42078d3aef1cd43ab43021f3c54f47d2d75ceb4e75f627f118890128b06a0d09"),
    ("onnx/unicode_indexer.json", 277676, "9bf7346e43883a81f8645c81224f786d43c5b57f3641f6e7671a7d6c493cb24f"),
    ("voice_styles/F1.json", 292046, "bbdec6ee00231c2c742ad05483df5334cab3b52fda3ba38e6a07059c4563dbc2"),
    ("voice_styles/F2.json", 292423, "7c722c6a72707b1a77f035d67f0d1351ba187738e06f7683e8c72b1df3477fc6"),
    ("voice_styles/F3.json", 290794, "12f6ef2573baa2defa1128069cb59f203e3ab67c92af77b42df8a0e3a2f7c6ab"),
    ("voice_styles/F4.json", 291808, "c2fa764c1225a76dfc3e2c73e8aa4f70d9ee48793860eb34c295fff01c2e032b"),
    ("voice_styles/F5.json", 291479, "45966e73316415626cf41a7d1c6f3b4c70dbc1ba2bee5c1978ef0ce33244fc8d"),
    ("voice_styles/M1.json", 291748, "e35604687f5d23694b8e91593a93eec0e4eca6c0b02bb8ed69139ab2ea6b0a5b"),
    ("voice_styles/M2.json", 292055, "b76cbf62bac707c710cf0ae5aba5e31eea1a6339a9734bfae33ab98499534a50"),
    ("voice_styles/M3.json", 290198, "ea1ac35ccb91b0d7ecad533a2fbd0eec10c91513d8951e3b25fbba99954e159b"),
    ("voice_styles/M4.json", 291522, "ca8eefad4fcd989c9379032ff3e50738adc547eeb5e221b82593a6d7b3bac303"),
    ("voice_styles/M5.json", 291469, "dd22b92740314321f8ae11c5e87f8dd60d060f15dd3a632b5adf77f471f77af2"),
]
SUPERTONIC_LANGS = ["en", "ko", "ja", "ar", "bg", "cs", "da", "de", "el", "es", "et", "fi", "fr", "hi", "hr", "hu", "id",
                    "it", "lt", "lv", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "tr", "uk", "vi"]


def voices():
    files = [{"path": f, "url": f"https://huggingface.co/{SUPERTONIC_REPO}/resolve/{SUPERTONIC_REV}/{f}", "size": n, "sha256": h}
             for f, n, h in SUPERTONIC_FILES]
    styles = [{"id": f"{g}{i}", "name": f"{'Female' if g == 'F' else 'Male'} {i}", "gender": "female" if g == "F" else "male"}
              for g in "FM" for i in range(1, 6)]
    return {
        "runtime": ONNX_RUNTIME,
        "runtime_dll": "onnxruntime.dll",
        "models": [{
            "id": "supertonic-3", "name": "Natural voices (Supertonic 3)", "publisher": "Supertone",
            "source": f"https://huggingface.co/{SUPERTONIC_REPO}",
            "description": "Ten natural-sounding voices that speak 31 languages, made on this PC in a fraction of a second.",
            "license": OPENRAIL, "languages": SUPERTONIC_LANGS, "files": files, "styles": styles,
        }],
    }


# Speaker labels in meetings: a WeSpeaker voice-print model on ONNX Runtime.
DIARIZATION = {
    "id": "wespeaker-resnet34", "name": "Speaker labels (WeSpeaker ResNet34)", "publisher": "WeNet",
    "source": "https://huggingface.co/Wespeaker/wespeaker-voxceleb-resnet34-LM",
    "description": "Tells the other people in a call apart, so the transcript says Speaker 1, Speaker 2 and so on. You can name them.",
    "license": {"name": "CC-BY-4.0", "url": "https://creativecommons.org/licenses/by/4.0/", "commercial": True,
                "note": "Commercial use allowed with attribution to the WeSpeaker project."},
    "file": "voxceleb_resnet34_LM.onnx",
    "url": "https://huggingface.co/Wespeaker/wespeaker-voxceleb-resnet34-LM/resolve/f0c48c298fd835726c27956a5d617bad7115627e/voxceleb_resnet34_LM.onnx",
    "size": 26530309,
    "sha256": "7bb2f06e9df17cdf1ef14ee8a15ab08ed28e8d0ef5054ee135741560df2ec068",
}


def main():
    out = {"schema": 1, "updated": "2026-10-06", "engine": ENGINE, "models": [], "speech": speech(), "voices": voices(), "diarization": DIARIZATION}
    for mid, name, pub, repo, lic, tags, params, (nl, kv, hd, mx), act, desc in MODELS:
        variants = [
            {"quant": q, "quality": QUALITY[q], "file": f, "url": HF.format(repo=repo, file=f), "size": s, "sha256": h}
            for q, (f, s, h) in FILES[repo].items()
        ]
        out["models"].append({
            "id": mid, "name": name, "publisher": pub, "source": f"https://huggingface.co/{repo}",
            "description": desc, "license": lic, "tags": tags, "params_b": params,
            "arch": {"n_layer": nl, "n_kv_heads": kv, "head_dim": hd, "max_ctx": mx, "active_fraction": act},
            "default_ctx": 8192, "tools": mid in TOOL_CAPABLE, "variants": variants,
        })
    path = pathlib.Path(__file__).with_name("catalog.json")
    path.write_text(json.dumps(out, indent=2) + "\n", encoding="utf-8", newline="\n")
    print(f"wrote {len(out['models'])} models and {len(out['speech']['models'])} speech models to {path}")


if __name__ == "__main__":
    main()
