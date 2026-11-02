# SPDX-License-Identifier: AGPL-3.0-only
"""Copies what the Telegram bot says (telegram.rs `say`) from the window's
locale tables into src-tauri/src/telegram_texts.json, which the core builds
in, so the bot speaks the app's language. Run it after the tables are
filled. Usage: python -I telegram_texts.py"""
import json
import os
import re

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
src = open(os.path.join(root, 'src-tauri', 'src', 'telegram.rs'), encoding='utf-8').read()
block = src[src.index('pub mod say {'):]
block = block[:block.index('\n}\n')]
texts = [json.loads('"' + m + '"') for m in re.findall(r'pub const [A-Z_]+: &str = "((?:[^"\\]|\\.)*)";', block)]
locales = os.path.join(root, 'src', 'locales')
out = {}
missing = []
for f in sorted(os.listdir(locales)):
    if not f.endswith('.json'):
        continue
    lang = f[:-5]
    table = json.load(open(os.path.join(locales, f), encoding='utf-8'))
    out[lang] = {t: table[t] for t in texts if t in table}
    missing += [f'{lang}: {t}' for t in texts if t not in table]
path = os.path.join(root, 'src-tauri', 'src', 'telegram_texts.json')
with open(path, 'w', encoding='utf-8', newline='\n') as fh:
    fh.write(json.dumps(out, ensure_ascii=False, indent=1, sort_keys=True))
print(f'{len(texts)} texts in {len(out)} languages; {len(missing)} missing')
for m in missing[:20]:
    print('MISSING', m)
