# SPDX-License-Identifier: AGPL-3.0-only
"""Release helper for the macOS and Linux builds (run by
.github/workflows/release-unix.yml).

  attach <tag> <platform>  upload this machine's installers to the release;
                           if they're signed for the updater, write
                           updater-<platform>.json with the signature and URL
  merge <tag>              add every updater-*.json to the release's latest.json
"""
import glob
import json
import os
import shutil
import subprocess
import sys

REPO = "IsaacLeh1/sulcusai"
BUNDLE = "src-tauri/target/release/bundle"


def gh(*args):
    subprocess.run(["gh", *args, "--repo", REPO], check=True)


def attach(tag, platform):
    version = tag.lstrip("v")
    files, updater = [], None
    if platform.startswith("linux"):
        files += glob.glob(f"{BUNDLE}/deb/*.deb")
        images = glob.glob(f"{BUNDLE}/appimage/*.AppImage")
        files += images
        if images and os.path.exists(images[0] + ".sig"):
            updater = images[0]
    else:
        files += glob.glob(f"{BUNDLE}/dmg/*.dmg")
        archives = glob.glob(f"{BUNDLE}/macos/*.app.tar.gz")
        if archives:
            # The updater's archive gets a name that says what it is.
            named = f"{BUNDLE}/macos/SulcusAI_{version}_{platform}.app.tar.gz"
            shutil.move(archives[0], named)
            if os.path.exists(archives[0] + ".sig"):
                shutil.move(archives[0] + ".sig", named + ".sig")
                updater = named
            files.append(named)
    if not files:
        sys.exit("No installers were built.")
    gh("release", "upload", tag, *files, "--clobber")
    print("attached", files)
    if updater:
        entry = {
            "signature": open(updater + ".sig", encoding="utf-8").read().strip(),
            "url": f"https://github.com/{REPO}/releases/download/{tag}/{os.path.basename(updater)}",
        }
        json.dump({platform: entry}, open(f"updater-{platform}.json", "w", encoding="utf-8"))


def merge(tag):
    parts = glob.glob("updater-*.json")
    if not parts:
        print("No signed macOS or Linux builds; latest.json stays Windows-only.")
        return
    gh("release", "download", tag, "--pattern", "latest.json", "--clobber")
    latest = json.load(open("latest.json", encoding="utf-8-sig"))
    for p in parts:
        latest["platforms"].update(json.load(open(p, encoding="utf-8")))
    with open("latest.json", "w", encoding="utf-8", newline="\n") as f:
        json.dump(latest, f, indent=2)
    gh("release", "upload", tag, "latest.json", "--clobber")
    print("latest.json platforms:", sorted(latest["platforms"]))


if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "attach":
        attach(sys.argv[2], sys.argv[3])
    elif cmd == "merge":
        merge(sys.argv[2])
    else:
        sys.exit(__doc__)
