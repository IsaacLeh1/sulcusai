# SPDX-License-Identifier: AGPL-3.0-only
"""Adds a version's release notes to src/updateNotes.json, which the app
shows under Settings > Update notes (kept in the app, so they show offline).
Run it before building a release, then commit; release.ps1 refuses to build
a version that isn't in the file, and publishes these notes.
Usage: py -3.12 -I scripts/add_update_notes.py <notes.md> [version] [YYYY-MM-DD]
The version defaults to the one in src-tauri/tauri.conf.json, the date to today."""
import datetime
import json
import os
import sys

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
path = os.path.join(root, 'src', 'updateNotes.json')
if len(sys.argv) < 2:
    sys.exit(__doc__)
notes = open(sys.argv[1], encoding='utf-8').read().replace('\r\n', '\n').strip()
if not notes:
    sys.exit('The notes file is empty.')
version = sys.argv[2] if len(sys.argv) > 2 else json.load(open(os.path.join(root, 'src-tauri', 'tauri.conf.json'), encoding='utf-8'))['version']
day = sys.argv[3] if len(sys.argv) > 3 else datetime.date.today().isoformat()
entries = json.load(open(path, encoding='utf-8'))
entries = [e for e in entries if e['version'] != version]
entries.append({'version': version, 'date': day, 'notes': notes})
entries.sort(key=lambda e: tuple(int(x) for x in e['version'].split('.')), reverse=True)
with open(path, 'w', encoding='utf-8', newline='\n') as f:
    f.write(json.dumps(entries, ensure_ascii=False, indent=1))
print(f'{version} ({day}): {len(notes)} characters of notes; {len(entries)} versions in {os.path.relpath(path, root)}')
