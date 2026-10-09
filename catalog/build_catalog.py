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
    "unsloth/Qwen3-VL-4B-Instruct-GGUF": {
        "Q4_K_M": ("Qwen3-VL-4B-Instruct-Q4_K_M.gguf", 2497282336, "d4dcd426bfba75752a312b266b80fec8136fbaca13c62d93b7ac41fa67f0492b"),
        "Q5_K_M": ("Qwen3-VL-4B-Instruct-Q5_K_M.gguf", 2889515296, "29b491c682da5db4268e30662337ccac18846c8e491ea4ffe466b18f46b8c162"),
        "Q6_K": ("Qwen3-VL-4B-Instruct-Q6_K.gguf", 3306262816, "43d166b36806df8fd788e53d8d63e0f7d383688ba683a5ea31b5bf2df37314cb"),
        "Q8_0": ("Qwen3-VL-4B-Instruct-Q8_0.gguf", 4280406816, "e30b41d2f2cc48149dfbba1b3fb2c023c7e6bf2def75833d4d43820df75efeb3"),
    },
    "unsloth/Qwen3-VL-8B-Instruct-GGUF": {
        "Q4_K_M": ("Qwen3-VL-8B-Instruct-Q4_K_M.gguf", 5027785568, "108e7ff92b78eefd3db4741885104acba514255c11b617d3c7b197a5f46efe89"),
        "Q5_K_M": ("Qwen3-VL-8B-Instruct-Q5_K_M.gguf", 5851114336, "bb6d45711239c508c18b6a67f00dd094a41add64e8639f8554738bfeccf5a3bc"),
        "Q6_K": ("Qwen3-VL-8B-Instruct-Q6_K.gguf", 6725901152, "2f963d09b4c7485df9bfd4ac4d7ce192cc9cc004a45555b605eeb0a7a83f0057"),
        "Q8_0": ("Qwen3-VL-8B-Instruct-Q8_0.gguf", 8709520224, "cb8616bf6ed228982d9e47d7b72b42195342efa26044b0ee1873e61d9e78d3d7"),
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
TOOL_CAPABLE = {"qwen3-1.7b", "llama-3.2-3b", "qwen3-4b-2507", "qwen3-8b", "qwen2.5-coder-7b", "qwen3-14b", "gpt-oss-20b", "qwen3-vl-4b", "qwen3-vl-8b"}

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
    ("qwen3-vl-4b", "Qwen3 VL 4B", "Qwen (Alibaba)", "unsloth/Qwen3-VL-4B-Instruct-GGUF", APACHE,
     ["chat", "vision", "tool-calling"], 4.4, (36, 8, 128, 262144), 1.0,
     "Sees pictures and screenshots: describes them, reads their text and answers questions about them. Also a capable chat model with tools."),
    ("qwen3-vl-8b", "Qwen3 VL 8B", "Qwen (Alibaba)", "unsloth/Qwen3-VL-8B-Instruct-GGUF", APACHE,
     ["chat", "vision", "tool-calling"], 8.8, (36, 8, 128, 262144), 1.0,
     "The stronger picture reader: charts, documents, screenshots and photos, with good tool use."),
]

# Rough capability guides for the Models page, from public benchmarks and
# the publishers' own reports (MMLU-Pro, LiveCodeBench, IFEval, BFCL,
# multilingual evals), rounded. overall is 0-100 and orders the ranking from
# least to most capable; the areas are 0-10 and drive the tabs. agents is 0
# for models whose template can't call tools here.
#            overall coding writing research agents languages
RATINGS = {
    "qwen3-1.7b":       (22, 3, 3, 3, 3, 4),
    "llama-3.2-3b":     (28, 3, 4, 4, 3, 4),
    "gemma-3-4b":       (36, 3, 6, 4, 0, 7),
    "qwen2.5-coder-7b": (42, 7, 3, 3, 4, 3),
    "qwen3-4b-2507":    (45, 5, 5, 5, 6, 5),
    "qwen3-8b":         (55, 6, 6, 6, 7, 6),
    "gemma-3-12b":      (58, 5, 8, 6, 0, 8),
    "qwen3-14b":        (68, 8, 7, 7, 8, 7),
    "gpt-oss-20b":      (74, 8, 6, 8, 9, 5),
    "qwen3-vl-4b":      (44, 5, 5, 5, 6, 5),
    "qwen3-vl-8b":      (54, 6, 6, 6, 7, 6),
}
AREAS = ("coding", "writing", "research", "agents", "languages")

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


# ---------- media: images, video, music ----------

# stable-diffusion.cpp (MIT) runs image, editing, upscaling and video models.
SD_ENGINE = {
    "name": "stable-diffusion.cpp",
    "license": "MIT",
    "build": "master-948-228c707",
    "assets": {
        "vulkan-x64": {
            "url": "https://github.com/leejet/stable-diffusion.cpp/releases/download/master-948-228c707/sd-master-228c707-bin-win-vulkan-x64.zip",
            "size": 30098372,
            "sha256": "6de279c833a47ca5ede5fe16415dcf8a2a718ca3ececfbe024fc2f86c7728e89",
        },
        "cpu-x64": {
            "url": "https://github.com/leejet/stable-diffusion.cpp/releases/download/master-948-228c707/sd-master-228c707-bin-win-cpu-x64.zip",
            "size": 17517910,
            "sha256": "e6da0b2c0ca1774ae3d2f36807102ffd859af8108265117882f84da422fc6655",
        },
    },
}

# acestep.cpp (MIT) runs ACE-Step music models. Upstream publishes no
# versioned Windows builds, so this is our build of commit d881ad2 (MSVC
# 2022, static runtime, CPU backends): HANDOFF.md says how it's made and
# where it has to be published.
MUSIC_ENGINE = {
    "name": "acestep.cpp",
    "license": "MIT",
    "build": "d881ad2",
    "assets": {
        "cpu-x64": {
            "url": "https://github.com/IsaacLeh1/sulcusai/releases/download/engines/acestep-d881ad2-win-cpu-x64.zip",
            "size": 5136624,
            "sha256": "d30427419b5211df6218877091b6f64ee4d68767252eaeb18629310a51ff59a4",
        },
    },
}

BSD3 = {"name": "BSD-3-Clause", "url": "https://github.com/xinntao/Real-ESRGAN/blob/master/LICENSE", "commercial": True}


def mf(role, repo, path, size, sha):
    return {"role": role, "file": path.rsplit("/", 1)[-1], "url": f"https://huggingface.co/{repo}/resolve/main/{path}", "size": size, "sha256": sha}


KLEIN_TE = mf("llm", "unsloth/Qwen3-4B-GGUF", "Qwen3-4B-Q4_K_M.gguf", 2497281312, "f6f851777709861056efcdad3af01da38b31223a3ba26e61a4f8bf3a2195813a")
UMT5 = mf("t5xxl", "city96/umt5-xxl-encoder-gguf", "umt5-xxl-encoder-Q4_K_M.gguf", 3655145312, "17cf97a5bbbc60a646d6105b832b6f657ce904a8a1ad970e4b59df0c67584a40")
ACE = "Serveurperso/ACE-Step-1.5-GGUF"
ACE_EMBED = mf("embedding", ACE, "Qwen3-Embedding-0.6B-Q8_0.gguf", 784144960, "972f23255e46adfe744a0eb9a0039f3c63988f65753b0968d776e8b27168c321")
ACE_VAE = mf("vae", ACE, "vae-BF16.gguf", 337420928, "0599862ac5d15cd308e1d2e368373aea6c02e25ebd1737ad4a4562a0901b0ef8")
NEGATIVE_VIDEO = "blurry, distorted, static, low quality, watermark, subtitles, extra limbs, deformed"

# kind: image, video, music, upscale or background; can: what it does.
# min_vram_gb: the smallest graphics card it's offered on; cpu: whether it
# also runs without one. secs: one default job on the reference PC (RTX 5070
# Ti Laptop, 12 GB, Vulkan), a rough guide replaced by measurements.
# quality: 0-10, for ordering and the suggestion.
MEDIA_MODELS = [
    {
        "id": "flux2-klein-4b", "name": "FLUX.2 klein 4B", "publisher": "Black Forest Labs",
        "source": "https://huggingface.co/black-forest-labs/FLUX.2-klein-4B",
        "description": "Fast, photo-quality pictures from a description, and edits from plain instructions (\"make it night\", \"add a hat\"). Also fills in painted areas, extends the edges and restyles.",
        "license": APACHE, "kind": "image", "can": ["generate", "edit", "fill", "extend", "restyle"],
        "files": [
            mf("diffusion", "unsloth/FLUX.2-klein-4B-GGUF", "flux-2-klein-4b-Q4_K_M.gguf", 2604311104, "0b25d143c8469b342bc5af3bce92b783bf6b0636d285f7b2f75e38af63af9a15"),
            KLEIN_TE,
            mf("vae", "unsloth/FLUX.2-VAE", "split_files/vae/flux2-vae.safetensors", 336213556, "d64f3a68e1cc4f9f4e29b6e0da38a0204fe9a49f2d4053f0ec1fa1ca02f9c4b5"),
        ],
        "min_vram_gb": 4, "cpu": True, "secs": 14, "quality": 8,
        "defaults": {"steps": 4, "cfg": 1.0, "sampler": "euler", "width": 1024, "height": 1024},
    },
    {
        "id": "z-image-turbo", "name": "Z-Image Turbo", "publisher": "Tongyi-MAI (Alibaba)",
        "source": "https://huggingface.co/Tongyi-MAI/Z-Image-Turbo",
        "description": "Very realistic photos and clean text in pictures (signs, posters, labels), in English and Chinese. Creates and fills in; for edits from instructions, use FLUX.2 klein.",
        "license": APACHE, "kind": "image", "can": ["generate", "fill", "extend"],
        "files": [
            mf("diffusion", "leejet/Z-Image-Turbo-GGUF", "z_image_turbo-Q4_K.gguf", 3864250304, "14b375ab4f226bc5378f68f37e899ef3c2242b8541e61e2bc1aff40976086fbd"),
            mf("llm", "unsloth/Qwen3-4B-Instruct-2507-GGUF", "Qwen3-4B-Instruct-2507-Q4_K_M.gguf", 2497281120, "3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597"),
            mf("vae", "Comfy-Org/z_image_turbo", "split_files/vae/ae.safetensors", 335304388, "afc8e28272cd15db3919bacdb6918ce9c1ed22e96cb12c4d5ed0fba823529e38"),
        ],
        "min_vram_gb": 6, "cpu": True, "secs": 25, "quality": 9,
        "defaults": {"steps": 8, "cfg": 1.0, "sampler": "euler", "width": 1024, "height": 1024},
    },
    {
        "id": "wan2.2-ti2v-5b", "name": "Wan 2.2 5B", "publisher": "Wan (Alibaba)",
        "source": "https://huggingface.co/Wan-AI/Wan2.2-TI2V-5B",
        "description": "Short video clips from a description, or brings a picture to life. The best video model that fits an 8-12 GB graphics card.",
        "license": APACHE, "kind": "video", "can": ["text", "image"],
        "files": [
            mf("diffusion", "QuantStack/Wan2.2-TI2V-5B-GGUF", "Wan2.2-TI2V-5B-Q4_K_M.gguf", 3433116000, "95b19697b7f98e65b0a543640e9ca7b4dfec32e2a6e3731e8e10708be52655e2"),
            mf("vae", "Comfy-Org/Wan_2.2_ComfyUI_Repackaged", "split_files/vae/wan2.2_vae.safetensors", 1409400960, "e40321bd36b9709991dae2530eb4ac303dd168276980d3e9bc4b6e2b75fed156"),
            UMT5,
        ],
        "min_vram_gb": 8, "cpu": False, "secs": 300, "quality": 8,
        "defaults": {"steps": 20, "cfg": 5.0, "sampler": "euler", "width": 832, "height": 480, "fps": 24, "seconds": 3, "flow_shift": 5.0, "negative": NEGATIVE_VIDEO},
    },
    {
        "id": "wan2.1-t2v-1.3b", "name": "Wan 2.1 1.3B", "publisher": "Wan (Alibaba)",
        "source": "https://huggingface.co/Wan-AI/Wan2.1-T2V-1.3B",
        "description": "A small, quicker video model for short clips from a description. Less detail than Wan 2.2, but it fits a 6 GB graphics card.",
        "license": APACHE, "kind": "video", "can": ["text"],
        "files": [
            mf("diffusion", "Comfy-Org/Wan_2.1_ComfyUI_repackaged", "split_files/diffusion_models/wan2.1_t2v_1.3B_fp16.safetensors", 2838303560, "be531024cd9018cb5b48c40cfbb6a6191645b1c792eb8bf4f8c1c6e10f924dc5"),
            mf("vae", "Comfy-Org/Wan_2.2_ComfyUI_Repackaged", "split_files/vae/wan_2.1_vae.safetensors", 253815318, "2fc39d31359a4b0a64f55876d8ff7fa8d780956ae2cb13463b0223e15148976b"),
            UMT5,
        ],
        "min_vram_gb": 6, "cpu": False, "secs": 150, "quality": 5,
        "defaults": {"steps": 20, "cfg": 6.0, "sampler": "euler", "width": 832, "height": 480, "fps": 16, "seconds": 3, "flow_shift": 3.0, "negative": NEGATIVE_VIDEO},
    },
    {
        "id": "realesrgan-x4plus", "name": "Real-ESRGAN x4plus", "publisher": "Xintao Wang",
        "source": "https://github.com/xinntao/Real-ESRGAN",
        "description": "Makes pictures four times larger and sharper. Small and quick.",
        "license": BSD3, "kind": "upscale", "can": ["upscale"],
        "files": [{"role": "upscale", "file": "RealESRGAN_x4plus.pth",
                   "url": "https://github.com/xinntao/Real-ESRGAN/releases/download/v0.1.0/RealESRGAN_x4plus.pth",
                   "size": 67040989, "sha256": "4fa0d38905f75ac06eb49a7951b426670021be3018265fd191d2125df9d682f1"}],
        "min_vram_gb": 0, "cpu": True, "secs": 18, "quality": 7,
    },
    {
        "id": "birefnet-lite", "name": "BiRefNet lite", "publisher": "Peng Zheng et al.",
        "source": "https://huggingface.co/ZhengPeng7/BiRefNet_lite",
        "description": "Cuts the subject out of a picture and removes the background, keeping hair and fine edges.",
        "license": MIT, "kind": "background", "can": ["remove_background"],
        "files": [mf("onnx", "onnx-community/BiRefNet_lite-ONNX", "onnx/model.onnx", 224005088, "5600024376f572a557870a5eb0afb1e5961636bef4e1e22132025467d0f03333")],
        "min_vram_gb": 0, "cpu": True, "secs": 6, "quality": 7,
    },
    {
        "id": "ace-step-1.5", "name": "ACE-Step 1.5", "publisher": "ACE Studio and StepFun",
        "source": "https://huggingface.co/ACE-Step/Ace-Step1.5",
        "description": "Songs from a description, with lyrics and vocals or instrumental, in many styles and languages. Also rough sound effects.",
        "license": MIT, "kind": "music", "can": ["music", "sound"],
        "files": [
            mf("dit", ACE, "acestep-v15-turbo-Q8_0.gguf", 2549528000, "288f708a61cfc241013a98a62f98ba331f83fe34d0d3559acdd9b0f6a2f7cd6b"),
            mf("lm", ACE, "acestep-5Hz-lm-1.7B-Q8_0.gguf", 1975837568, "726f99a82f050b32ebc5c5e36acaa9d7acd06bfe5e579a62b00e0d0d6d0b7ec6"),
            ACE_EMBED, ACE_VAE,
        ],
        "min_vram_gb": 0, "cpu": True, "secs": 70, "quality": 8,
        "defaults": {"seconds": 30},
    },
    {
        "id": "ace-step-1.5-small", "name": "ACE-Step 1.5 (small)", "publisher": "ACE Studio and StepFun",
        "source": "https://huggingface.co/ACE-Step/Ace-Step1.5",
        "description": "A lighter ACE-Step for PCs with less memory: quicker, with simpler songs and lyrics.",
        "license": MIT, "kind": "music", "can": ["music", "sound"],
        "files": [
            mf("dit", ACE, "acestep-v15-turbo-Q4_K_M.gguf", 1445710272, "55b4d8514850f3d0f82536f37e99673aaf48df802b5ae5b153eea32a2e2daa5e"),
            mf("lm", ACE, "acestep-5Hz-lm-0.6B-Q8_0.gguf", 709846656, "bdaf9e292d4470f31c19cafeaca1b74936a114667e3a85e5d33b65247e9908ec"),
            ACE_EMBED, ACE_VAE,
        ],
        "min_vram_gb": 0, "cpu": True, "secs": 45, "quality": 5,
        "defaults": {"seconds": 30},
    },
]


def media():
    return {"engine": SD_ENGINE, "music_engine": MUSIC_ENGINE, "models": MEDIA_MODELS}


# Vision: llama.cpp image encoders (mmproj) for models that can see.
VISION = {
    "gemma-3-4b": mf("mmproj", "unsloth/gemma-3-4b-it-GGUF", "mmproj-F16.gguf", 851251328, "731199e016ec5f227b8293fef839899472e0ee4c51adf5f9e5cb66f6558fa142"),
    "gemma-3-12b": mf("mmproj", "unsloth/gemma-3-12b-it-GGUF", "mmproj-F16.gguf", 854200448, "5de4ccfc379faa4cbf608dd365c028aa41f9774cdb1191d148d88910d1014f71"),
    "qwen3-vl-4b": mf("mmproj", "unsloth/Qwen3-VL-4B-Instruct-GGUF", "mmproj-F16.gguf", 836180640, "1b9f4e92f0fbda14d7d7b58baed86039b8a980fe503d9d6a9393f25c0028f1fc"),
    "qwen3-vl-8b": mf("mmproj", "unsloth/Qwen3-VL-8B-Instruct-GGUF", "mmproj-F16.gguf", 1159030336, "d406d03ebabefdef86a2c86bf0c1b65f9e046f7a81c218f25de4931b46a07fc4"),
}


def main():
    out = {"schema": 1, "updated": "2026-10-09", "engine": ENGINE, "models": [], "speech": speech(), "voices": voices(), "diarization": DIARIZATION, "media": media()}
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
            "ratings": {"overall": RATINGS[mid][0], **dict(zip(AREAS, RATINGS[mid][1:]))},
        })
        if mid in VISION:
            v = dict(VISION[mid])
            v.pop("role")
            out["models"][-1]["vision"] = v
    path = pathlib.Path(__file__).with_name("catalog.json")
    path.write_text(json.dumps(out, indent=2) + "\n", encoding="utf-8", newline="\n")
    print(f"wrote {len(out['models'])} models and {len(out['speech']['models'])} speech models to {path}")


if __name__ == "__main__":
    main()
