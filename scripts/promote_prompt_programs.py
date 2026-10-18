# SPDX-License-Identifier: AGPL-3.0-only
"""Copies prompt-writer programs tuned on this PC into the bundled catalog.

In the app, Studio > Pictures > Prompt writer > "Teach it this model" makes
practice pictures, scores them with the picture reader and saves a better
program for that model family under the app's data folder
(prompt_programs/<family>.json). Run this to ship those to everyone:

    py -3.12 -I scripts/promote_prompt_programs.py            # show what would change
    py -3.12 -I scripts/promote_prompt_programs.py --write    # update catalog/prompt_programs.json

Only programs whose tuned score beat the bundled one are copied. The
"tuned" stats are dropped (they describe this PC's run); each demo keeps its
score.
"""
import json
import os
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
BUNDLED = ROOT / "catalog" / "prompt_programs.json"


def data_dir() -> pathlib.Path:
    base = os.environ.get("LOCALAPPDATA") or str(pathlib.Path.home() / ".local" / "share")
    return pathlib.Path(base) / "app.sulcusai.desktop" / "prompt_programs"


def main() -> int:
    write = "--write" in sys.argv
    src = data_dir()
    bundled = json.loads(BUNDLED.read_text(encoding="utf-8"))
    if not src.is_dir():
        print(f"Nothing tuned yet ({src} doesn't exist).")
        return 0
    changed = []
    for f in sorted(src.glob("*.json")):
        p = json.loads(f.read_text(encoding="utf-8"))
        fam = p.get("family") or f.stem
        tuned = p.get("tuned") or {}
        before, after = tuned.get("baseline", 0), tuned.get("score", 0)
        if after <= before:
            print(f"{fam}: skipped (tuned {after:.0%} didn't beat the bundled {before:.0%})")
            continue
        p.pop("tuned", None)
        p["family"] = fam
        if bundled.get(fam) == p:
            print(f"{fam}: already bundled")
            continue
        bundled[fam] = p
        changed.append(fam)
        print(f"{fam}: {before:.0%} -> {after:.0%} over {tuned.get('pictures', 0)} pictures, {len(p.get('demos', []))} demos")
    if changed and write:
        BUNDLED.write_text(json.dumps(bundled, indent=2, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")
        print(f"Updated {BUNDLED.relative_to(ROOT)}: {', '.join(changed)}. Rebuild the app to ship them.")
    elif changed:
        print("Run again with --write to update the catalog.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
