# SPDX-License-Identifier: AGPL-3.0-only
"""Zips the yue2.cpp builds (scripts/music/build-yue2-*.cmd) for SulcusAI's
engines release and prints each zip's size and SHA-256 for the catalog.
Usage: py -3.12 -I package_yue.py <out dir> <build> <cpu build dir> <vulkan build dir> <yue2.cpp checkout>"""
import hashlib
import os
import sys
import zipfile

out, build, cpu, vulkan, src = sys.argv[1:6]
os.makedirs(out, exist_ok=True)
KEEP = ("yue-synth.exe", "yue-plan.exe", "yue-server.exe", "yue-tokenize.exe")


def pack(src_dir, name):
    path = os.path.join(out, name)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for f in sorted(os.listdir(src_dir)):
            if f in KEEP or f.endswith(".dll"):
                z.write(os.path.join(src_dir, f), f)
        z.write(os.path.join(src, "LICENSE"), "LICENSE-yue2.cpp.txt")
    h = hashlib.sha256(open(path, "rb").read()).hexdigest()
    print(name, os.path.getsize(path), h)


pack(cpu, f"yue2-{build}-win-cpu-x64.zip")
pack(vulkan, f"yue2-{build}-win-vulkan-x64.zip")
