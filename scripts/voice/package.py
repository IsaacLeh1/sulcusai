# SPDX-License-Identifier: AGPL-3.0-only
"""Zips the qwentts.cpp builds for SulcusAI's engines release and prints the
size and SHA-256 of every file the catalog points at.
Usage: py -3.12 -I package.py <out dir>"""
import hashlib
import os
import shutil
import sys
import zipfile

OUT = sys.argv[1]
os.makedirs(OUT, exist_ok=True)
BUILD = "5916bf7"
KEEP = ("qwen-tts.exe", "qwen-codec.exe", "tts-server.exe")
LICENSE = r"C:\dev\tools\voice\qwentts.cpp\LICENSE"


def sha(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def pack(src, name):
    path = os.path.join(OUT, name)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for f in sorted(os.listdir(src)):
            if f in KEEP or (f.endswith(".dll")):
                z.write(os.path.join(src, f), f)
        z.write(LICENSE, "LICENSE-qwentts.cpp.txt")
    return path


files = [
    pack(r"C:\dev\tools\voice\qwentts.cpp\build-cpu", f"qwentts-{BUILD}-win-cpu-x64.zip"),
    pack(r"C:\dev\qb", f"qwentts-{BUILD}-win-vulkan-x64.zip"),
]
for f in ["whisper_mel", "whisper", "rmvpe", "campplus", "mel44", "regulator", "dit", "bigvgan"]:
    dst = os.path.join(OUT, f"seedvc-svc44k-{f}.onnx")
    shutil.copyfile(os.path.join(r"C:\dev\tools\voice\onnx16", f + ".onnx"), dst)
    files.append(dst)
for f in ["qwen-talker-1.7b-base-Q8_0.gguf", "qwen-talker-0.6b-base-Q8_0.gguf", "qwen-tokenizer-12hz-Q8_0.gguf"]:
    files.append(os.path.join(r"C:\dev\tools\voice\models", f))
for f in files:
    print(os.path.basename(f), os.path.getsize(f), sha(f))
