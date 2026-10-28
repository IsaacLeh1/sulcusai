# SPDX-License-Identifier: AGPL-3.0-only
"""Packs Ditto for SulcusAI's quick talking pictures (catalog media.ditto).

Ditto's source at one commit (Apache-2.0, from GitHub), with SulcusAI's
files added at its top: the runner (talk.py), and the Python packages it
needs, locked: pyproject.toml and uv.lock (PyTorch 2.7.1 for CUDA 12.8, the
same build the music trainer uses, so uv's cache shares it), and the same in
cpu/ with PyTorch's processor build, for PCs without an NVIDIA card. The app
unpacks the zip beside its engines and runs `uv sync --frozen` once in the
folder for this PC.

    py -3.12 -I scripts/ditto/package.py <ditto-talkinghead-<commit>.zip> <out.zip>

The upstream zip is https://github.com/antgroup/ditto-talkinghead/archive/<COMMIT>.zip.
Prints the size and SHA-256 for the catalog; upload the zip to the
`engines` release.
"""
import hashlib
import os
import sys
import zipfile

COMMIT = "c3e47eee2e626500017a0556b470d6d4182f85e8"
HERE = os.path.dirname(os.path.abspath(__file__))
ADDED = ["talk.py", "pyproject.toml", "uv.lock", "cpu/pyproject.toml", "cpu/uv.lock"]


def entry(name):
    # A fixed date, so the same files always make the same zip (and hash).
    e = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
    e.compress_type = zipfile.ZIP_DEFLATED
    return e


def main(upstream, out):
    top = f"ditto-talkinghead-{COMMIT}/"
    with zipfile.ZipFile(upstream) as src, zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as dst:
        names = src.namelist()
        if not names or not names[0].startswith(top):
            sys.exit(f"{upstream} isn't Ditto at {COMMIT[:8]}")
        for n in names:
            rel = n[len(top):]
            if not rel or n.endswith("/") or rel in ADDED:
                continue
            # Only the inference code: no examples or TensorRT scripts.
            if rel.startswith(("example/", "scripts/")):
                continue
            dst.writestr(entry(rel), src.read(n))
        for f in ADDED:
            dst.writestr(entry(f), open(os.path.join(HERE, f), "rb").read())
    data = open(out, "rb").read()
    print(f"{out}: size {len(data)} sha256 {hashlib.sha256(data).hexdigest()}")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
