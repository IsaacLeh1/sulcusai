# SPDX-License-Identifier: AGPL-3.0-only
"""A module-definition file from `dumpbin /exports` output, for making an
import library: python -I def_from_dumpbin.py exports.txt out.def"""
import re
import sys

names = []
started = False
for line in open(sys.argv[1], encoding="utf-8", errors="replace"):
    if re.match(r"\s+ordinal\s+hint\s+RVA\s+name", line):
        started = True
        continue
    if started:
        m = re.match(r"\s+\d+\s+[0-9A-Fa-f]+\s+[0-9A-Fa-f]+\s+(\S+)", line)
        if m:
            names.append(m.group(1))
        elif line.strip().startswith("Summary"):
            break
with open(sys.argv[2], "w", encoding="ascii") as f:
    f.write("LIBRARY vulkan-1\nEXPORTS\n")
    for n in names:
        f.write(f"    {n}\n")
print(len(names), "exports")
