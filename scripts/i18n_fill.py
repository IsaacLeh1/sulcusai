# SPDX-License-Identifier: AGPL-3.0-only
"""Fills a locale table's missing texts with a local model.

Usage: python -I i18n_fill.py <keys.json> <table.json> <code>
           [--server http://127.0.0.1:18080] [--jobs 4]

The keys come from `i18n_extract.py --write-keys`. Any OpenAI-shaped chat
server works (llama-server, or SulcusAI's own local API). The table is saved
after every batch, so a stopped run carries on where it was."""
import concurrent.futures
import json
import os
import re
import sys
import threading
import urllib.request

LANGS = {
    "es": ("Spanish", "Address the user as tú."),
    "fr": ("French", "Address the user as vous."),
    "de": ("German", "Address the user as du."),
    "pt": ("Brazilian Portuguese", "Address the user as você."),
    "zh": ("Simplified Chinese", ""),
    "ja": ("Japanese", "Use polite です/ます forms."),
    "ko": ("Korean", "Use the polite 해요체 style."),
    "it": ("Italian", "Address the user as tu."),
    "ru": ("Russian", "Address the user as вы (lowercase)."),
    "pl": ("Polish", "Address the user informally (ty), as modern apps do."),
    "tr": ("Turkish", "Use the polite siz forms."),
    "nl": ("Dutch", "Address the user as je."),
    "id": ("Indonesian", "Address the user as Anda."),
    "vi": ("Vietnamese", "Address the user as bạn."),
    "uk": ("Ukrainian", "Address the user as ви (lowercase)."),
    "hi": ("Hindi", "Address the user as आप; keep technical words Hindi apps leave in English (GPU, AI, PDF) in Latin letters."),
    "th": ("Thai", "Use a neutral polite tone without gendered particles."),
    "zh-TW": ("Traditional Chinese as used in Taiwan", "Use Taiwan's terms (檔案, 資料夾, 設定, 顯示卡, 影片)."),
    "sv": ("Swedish", "Address the user as du."),
    "ar": ("Modern Standard Arabic", "Address the user directly and politely, as Arabic apps do."),
    "he": ("Hebrew", "Address the user directly, as Hebrew apps do, in the masculine-neutral form apps use."),
}

PLACEHOLDER = re.compile(r"\{\w+\}")
QUOTED = re.compile(r"[“\"]([^”\"]{2,40})[”\"]")

# Hindi came back in Latin letters ("Puchh raha hoon") for some sentences.
SCRIPT = {
    "ru": [(0x0400, 0x04FF)],
    "uk": [(0x0400, 0x04FF)],
    "hi": [(0x0900, 0x097F)],
    "th": [(0x0E00, 0x0E7F)],
    "ar": [(0x0600, 0x06FF)],
    "he": [(0x0590, 0x05FF)],
    "zh": [(0x4E00, 0x9FFF)],
    "zh-TW": [(0x4E00, 0x9FFF)],
    "ja": [(0x3040, 0x30FF), (0x4E00, 0x9FFF)],
    "ko": [(0xAC00, 0xD7AF)],
}


def args():
    a = sys.argv[1:]
    opt = {"--server": "http://127.0.0.1:18080", "--jobs": "4"}
    for k in list(opt):
        if k in a:
            i = a.index(k)
            opt[k] = a[i + 1]
            del a[i : i + 2]
    return a[0], a[1], a[2], opt["--server"], int(opt["--jobs"])


def glossary_terms(keys):
    """Names other texts point at ("Studio › Your voice", "“Best of 3”"),
    translated first so every batch names them the same way."""
    have = set(keys)
    count = {}

    def seen(term):
        count[term] = count.get(term, 0) + 1

    for k in keys:
        for q in QUOTED.findall(k):
            if q in have:
                seen(q)
        parts = k.split(" › ")
        for i, part in enumerate(parts):
            words = re.sub(r"[.,:;!?)]+$", "", part).split()
            # The words next to each › are the menu's names.
            ends = []
            if i > 0:
                ends += [" ".join(words[:n]) for n in range(4, 0, -1)]
            if i < len(parts) - 1:
                ends += [" ".join(words[-n:]) for n in range(4, 0, -1)]
            for e in ends:
                if e in have:
                    seen(e)
                    break
    return [t for t, n in sorted(count.items(), key=lambda x: -x[1])]


def batches(items, size=25, chars=2500):
    out, cur, n = [], [], 0
    for k in items:
        if cur and (len(cur) >= size or n + len(k) > chars):
            out.append(cur)
            cur, n = [], 0
        cur.append(k)
        n += len(k)
    if cur:
        out.append(cur)
    return out


def ok(en, tr, lang=""):
    if not isinstance(tr, str) or not tr.strip():
        return False
    if lang in SCRIPT and len(en) > 30 and not any(lo <= ord(c) <= hi for c in tr for lo, hi in SCRIPT[lang]):
        return False
    if sorted(PLACEHOLDER.findall(en)) != sorted(PLACEHOLDER.findall(tr)):
        return False
    if (en[:1] == " ") != (tr[:1] == " ") or (en[-1:] == " ") != (tr[-1:] == " "):
        return False
    if en.count("\n") != tr.count("\n"):
        return False
    # The repository's texts never use the long dash.
    return chr(0x2014) not in tr


# Korean: the model spaces particles and counters off Latin letters and
# digits ("PC 에서", "10 분"), which Korean writes joined.
KO_PARTICLE = "(?:은|는|이|가|을|를|에서|에게|에|의|와|과|으로|로|도|만|까지|부터|보다|처럼|이나|나|라는|이라는|입니다|이에요|예요)"
KO_JOIN = re.compile(r"(?<=[A-Za-z0-9)}\]%]) (?=" + KO_PARTICLE + KO_PARTICLE + r"?(?![가-힣]))")
KO_COUNTER = re.compile(r"(?<=[0-9}]) (?=(?:분|초|시간|개|번|명|장|곡|줄|배|년)|(?:일|주|달|자)(?![가-힣]))")


def tidy(lang, text):
    # Many languages write a long dash where English has a comma; the
    # repository's texts never use one.
    text = text.replace(" " + chr(0x2014) + " ", ", ").replace(chr(0x2014), "-")
    if lang == "ko":
        text = KO_COUNTER.sub("", KO_JOIN.sub("", text))
    return text


def where_used(repo, keys):
    """The source file each text comes from, so a batch is one screen's
    texts and the model knows which screen ("Attention out" is the Lens)."""
    have = set(keys)
    out = {}
    literal = re.compile(r'"((?:[^"\\\n]|\\.)*)"')
    for base in ("src", "src-tauri/src"):
        for root, dirs, files in os.walk(os.path.join(repo, base)):
            dirs[:] = [d for d in dirs if d not in ("locales", "e2e")]
            for f in files:
                if not f.endswith((".ts", ".tsx", ".rs")) or f in ("tests.rs",) or ".test." in f:
                    continue
                path = os.path.join(root, f)
                rel = os.path.relpath(path, repo).replace(os.sep, "/")
                for raw in literal.findall(open(path, encoding="utf-8").read()):
                    try:
                        s = json.loads('"' + raw + '"')
                    except ValueError:
                        continue
                    if s in have and s not in out:
                        out[s] = rel
    return out


def ask(server, lang, items, glossary, screen):
    name, style = LANGS[lang]
    terms = {e: t for e, t in glossary.items() if any(e in k for k in items)}
    rules = [
        f"You translate the interface of SulcusAI, a desktop app that runs AI models on the person's own PC (chat, agents, voice, pictures, video and songs), from English into {name}. {style}",
        f"These texts all come from one part of the app, the source file {screen}: let it tell you what each short label means." if screen else "",
        "Translate every text, about as short as the English, as a native app in that language would say it.",
        "Keep placeholders such as {n} or {name} exactly as they are.",
        "Keep SulcusAI, product and company names (Windows, NVIDIA, Hugging Face, Ollama, Claude), file types, keyboard keys (Ctrl+Alt+D), units and symbols (›, ←, …, %) as they are.",
        "Keep any space at the start or end, and the same line breaks.",
        "Never use the long dash; use a comma, colon or parentheses instead.",
    ]
    if terms:
        rules.append("Use these translations for these names: " + json.dumps(terms, ensure_ascii=False))
    rules.append("Answer with one JSON object mapping each number to its translation, and nothing else.")
    body = {
        "model": "translator",
        "messages": [
            {"role": "system", "content": "\n".join(r for r in rules if r)},
            {"role": "user", "content": json.dumps({str(i + 1): k for i, k in enumerate(items)}, ensure_ascii=False)},
        ],
        "temperature": 0.2,
        "max_tokens": 6000,
        "response_format": {"type": "json_object"},
        "chat_template_kwargs": {"enable_thinking": False},
    }
    req = urllib.request.Request(f"{server}/v1/chat/completions", json.dumps(body).encode(), {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=600) as r:
        text = json.load(r)["choices"][0]["message"]["content"]
    try:
        got = json.loads(text)
    except ValueError:
        return {}
    out = {}
    for i, k in enumerate(items):
        tr = got.get(str(i + 1))
        if isinstance(tr, str):
            # Spaces at the ends join texts together; models drop them.
            tr = k[: len(k) - len(k.lstrip(" "))] + tidy(lang, tr).strip(" ") + k[len(k.rstrip(" ")) :]
        if ok(k, tr, lang):
            out[k] = tr
    return out


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    keys_path, table_path, lang, server, jobs = args()
    keys = json.load(open(keys_path, encoding="utf-8"))
    table = json.load(open(table_path, encoding="utf-8")) if os.path.exists(table_path) else {}
    lock = threading.Lock()
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    screen = where_used(repo, keys)

    def save():
        ordered = {k: table[k] for k in keys if k in table}
        tmp = table_path + ".tmp"
        with open(tmp, "w", encoding="utf-8", newline="\n") as f:
            f.write(json.dumps(ordered, ensure_ascii=False, indent=1) + "\n")
        os.replace(tmp, table_path)

    def by_screen(items, size=None):
        out = []
        for s in sorted({screen.get(k, "") for k in items}):
            same = [k for k in items if screen.get(k, "") == s]
            out += [(s, b) for b in (batches(same, size=size) if size else batches(same))]
        return out

    def run(items, glossary):
        failed = []
        for size in (None, 5, 1):
            groups = by_screen(failed, size) if size else by_screen(items)
            failed = []
            with concurrent.futures.ThreadPoolExecutor(jobs) as pool:
                for (_, group), got in zip(groups, pool.map(lambda g: ask(server, lang, g[1], glossary, g[0]), groups)):
                    with lock:
                        table.update(got)
                        save()
                        failed += [k for k in group if k not in got]
                        print(f"{lang}: {sum(1 for k in keys if k in table)} of {len(keys)}", flush=True)
            if not failed:
                break
        return failed

    terms = [t for t in glossary_terms(keys) if t not in table]
    run(terms, {})
    glossary = {t: table[t] for t in glossary_terms(keys) if t in table}
    left = run([k for k in keys if k not in table], glossary)
    print(f"{lang}: done, {len(left)} left in English")
    for k in left[:20]:
        print("  LEFT", repr(k))


if __name__ == "__main__":
    main()
