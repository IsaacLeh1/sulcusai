# SPDX-License-Identifier: AGPL-3.0-only
"""Packs Heretic for SulcusAI's uncensored copies (catalog `heretic`).

Heretic itself (AGPL-3.0) comes from PyPI as a locked package; the zip holds
SulcusAI's runner (uncensor.py), the locked Python packages (pyproject.toml
and uv.lock: PyTorch 2.7.1 for CUDA 12.8, the same build the music trainer
uses, so uv's cache shares it), and llama.cpp's GGUF converter (MIT) at the
engine's release, in llama-convert/, to turn the uncensored model into a
file the engine runs. The app unpacks the zip beside its engines and runs
`uv sync --frozen` there once.

    py -3.12 -I scripts/heretic/package.py <llama.cpp-<BUILD>.zip> <out.zip>

The llama.cpp zip is https://github.com/ggml-org/llama.cpp/archive/refs/tags/<BUILD>.zip.
Prints the size and SHA-256 for the catalog; upload the zip to the
`engines` release.
"""
import hashlib
import os
import sys
import zipfile

# The engine's llama.cpp release (catalog engine build).
BUILD = "b11450"
HERE = os.path.dirname(os.path.abspath(__file__))
ADDED = ["uncensor.py", "pyproject.toml", "uv.lock"]


def entry(name):
    # A fixed date, so the same files always make the same zip (and hash).
    e = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
    e.compress_type = zipfile.ZIP_DEFLATED
    return e


def converter(rel):
    """Whether a file of llama.cpp's source is part of the converter."""
    if rel in ("convert_hf_to_gguf.py", "LICENSE"):
        return True
    if rel.startswith("conversion/") and rel.endswith(".py"):
        return True
    return rel.startswith("gguf-py/gguf/") and rel.endswith(".py") and not rel.startswith("gguf-py/gguf/scripts/")


def main(upstream, out):
    top = f"llama.cpp-{BUILD}/"
    with zipfile.ZipFile(upstream) as src, zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as dst:
        names = src.namelist()
        if not names or not names[0].startswith(top):
            sys.exit(f"{upstream} isn't llama.cpp at {BUILD}")
        for n in sorted(names):
            rel = n[len(top):]
            if n.endswith("/") or not converter(rel):
                continue
            dst.writestr(entry(f"llama-convert/{rel}"), src.read(n))
        for f in ADDED:
            dst.writestr(entry(f), open(os.path.join(HERE, f), "rb").read())
    data = open(out, "rb").read()
    print(f"{out}: size {len(data)} sha256 {hashlib.sha256(data).hexdigest()}")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
