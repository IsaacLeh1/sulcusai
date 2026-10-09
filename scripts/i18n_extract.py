# SPDX-License-Identifier: AGPL-3.0-only
"""Lists every t("...")/tx("...") text in the window's code and the core's
fixed messages, and reports locale tables' missing and stale entries.
Usage: python -I i18n_extract.py <repo> [--write-keys out.json]"""
import ast
import json
import os
import re
import sys

root = sys.argv[1]
src = os.path.join(root, 'src')
BS = chr(92)
pat = re.compile(r'''\btx?\(\s*("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|`(?:[^`\\$]|\\.)*`)''')
keys = {}
for d, _, files in os.walk(src):
    if 'locales' in d:
        continue
    for f in files:
        if not f.endswith(('.ts', '.tsx')) or f.endswith('.test.ts') or f == 'mock.tsx':
            continue
        p = os.path.join(d, f)
        s = open(p, encoding='utf-8').read()
        for m in pat.finditer(s):
            lit = m.group(1)
            if lit[0] == '`':
                text = lit[1:-1].replace(BS + '`', '`')
            else:
                text = ast.literal_eval(lit)
            keys.setdefault(text, os.path.relpath(p, src))
ui = len(keys)

# The core's fixed messages (errors the window shows as they come).
rpat = re.compile(r'(?:Err\(|ok_or\(|ok_or_else\(\|\|\s*|map_err\(\|_\|\s*)"((?:[^"\\]|\\.)*)"')
rinto = re.compile(r'"((?:[^"\\]|\\.)*)"\.(?:into|to_string)\(\)')
for d, _, files in os.walk(os.path.join(root, 'src-tauri', 'src')):
    for f in files:
        if not f.endswith('.rs') or f == 'e2e.rs':
            continue
        text = open(os.path.join(d, f), encoding='utf-8').read()
        text = text.split('#[cfg(test)]\nmod tests')[0]
        for m in list(rpat.finditer(text)) + list(rinto.finditer(text)):
            lit = m.group(1)
            if '{' in lit or BS in lit or len(lit) < 12 or ' ' not in lit or not lit[0].isupper() or lit[-1] not in '.?!)':
                continue
            keys.setdefault(lit, 'core:' + f)
print(f'{ui} window texts, {len(keys) - ui} core messages')

locales = os.path.join(src, 'locales')
if os.path.isdir(locales):
    for f in sorted(os.listdir(locales)):
        if f.endswith('.json'):
            table = json.load(open(os.path.join(locales, f), encoding='utf-8'))
            missing = [k for k in keys if k not in table]
            stale = [k for k in table if k not in keys]
            print(f'{f}: {len(table)} entries, {len(missing)} missing, {len(stale)} stale')
if '--write-keys' in sys.argv:
    out = sys.argv[sys.argv.index('--write-keys') + 1]
    json.dump(sorted(keys), open(out, 'w', encoding='utf-8'), ensure_ascii=False, indent=1)
    print('wrote', out)
