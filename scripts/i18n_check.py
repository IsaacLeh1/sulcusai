# SPDX-License-Identifier: AGPL-3.0-only
"""Checks a translation table against the source strings.
Usage: python -I i18n_check.py <source.json> <table.json>"""
import json
import re
import sys

src = json.load(open(sys.argv[1], encoding='utf-8'))
table = json.load(open(sys.argv[2], encoding='utf-8'))
ph = re.compile(r'\{\w+\}')
missing = [s['en'] for s in src if s['en'] not in table]
extra = [k for k in table if k not in {s['en'] for s in src}]
bad = []
for s in src:
    en = s['en']
    tr = table.get(en)
    if tr is None:
        continue
    if not isinstance(tr, str) or not tr.strip():
        bad.append((en, 'empty'))
        continue
    if sorted(ph.findall(en)) != sorted(ph.findall(tr)):
        bad.append((en, 'placeholders differ: ' + tr))
    if en[:1] == ' ' and tr[:1] != ' ' or en[-1:] == ' ' and tr[-1:] != ' ':
        bad.append((en, 'leading/trailing space lost: ' + repr(tr)))
print(f'{len(table)} entries; {len(missing)} missing; {len(extra)} not in source; {len(bad)} problems')
for m in missing[:20]:
    print('MISSING', repr(m))
for e in extra[:10]:
    print('EXTRA', repr(e))
for en, why in bad[:30]:
    print('PROBLEM', repr(en), '->', why)
sys.exit(1 if missing or bad else 0)
