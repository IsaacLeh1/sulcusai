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


def main():
    out = {"schema": 1, "updated": "2026-10-06", "engine": ENGINE, "models": []}
    for mid, name, pub, repo, lic, tags, params, (nl, kv, hd, mx), act, desc in MODELS:
        variants = [
            {"quant": q, "quality": QUALITY[q], "file": f, "url": HF.format(repo=repo, file=f), "size": s, "sha256": h}
            for q, (f, s, h) in FILES[repo].items()
        ]
        out["models"].append({
            "id": mid, "name": name, "publisher": pub, "source": f"https://huggingface.co/{repo}",
            "description": desc, "license": lic, "tags": tags, "params_b": params,
            "arch": {"n_layer": nl, "n_kv_heads": kv, "head_dim": hd, "max_ctx": mx, "active_fraction": act},
            "default_ctx": 8192, "variants": variants,
        })
    path = pathlib.Path(__file__).with_name("catalog.json")
    path.write_text(json.dumps(out, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {len(out['models'])} models to {path}")


if __name__ == "__main__":
    main()
