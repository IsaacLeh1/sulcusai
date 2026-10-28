# SPDX-License-Identifier: AGPL-3.0-only
"""Lists the titles the agent's tools give their results (Outcome::ok's
title, Outcome::error's and denied's), as translation templates: {} becomes
{1}, {2}, … and {name} stays, so the window can match a finished title to its
template and fill in the translation.
Usage: python -I i18n_titles.py <repo>   (prints one template per line)"""
import os
import re
import sys

BS = chr(92)
QUOTE = chr(34)


def scan_string(s, k):
    """Index just past the string literal starting at s[k]."""
    e = k + 1
    while s[e] != QUOTE:
        e += 2 if s[e] == BS else 1
    return e + 1


def call_args(s, i):
    """Top-level arguments of the call whose '(' ends just before i."""
    parts, cur, depth, k = [], '', 1, i
    while k < len(s):
        c = s[k]
        if c == QUOTE:
            e = scan_string(s, k)
            cur += s[k:e]
            k = e
            continue
        if c in '([{':
            depth += 1
        elif c in ')]}':
            depth -= 1
            if depth == 0:
                break
        if c == ',' and depth == 1:
            parts.append(cur.strip())
            cur = ''
        else:
            cur += c
        k += 1
    parts.append(cur.strip())
    return parts


def template(lit):
    n = [0]

    def num(m):
        n[0] += 1
        return '{' + str(n[0]) + '}'
    lit = lit.replace(BS + QUOTE, QUOTE)
    lit = re.sub(r'\{(\w+)(:[^}]*)?\}', lambda m: '{' + m.group(1) + '}', lit)
    return re.sub(r'\{(:[^}]*)?\}', num, lit)


def titles(root):
    out = set()
    base = os.path.join(root, 'src-tauri', 'src')
    for d, _, files in os.walk(base):
        for f in files:
            # Tests in files of their own, and the end-to-end tests' folder.
            if not f.endswith('.rs') or f in ('e2e.rs', 'tests.rs') or os.path.basename(d) == 'e2e':
                continue
            # Up to an inline test module (`mod tests;` only names one in its own file).
            s = open(os.path.join(d, f), encoding='utf-8').read().replace('\r\n', '\n').split('#[cfg(test)]\nmod tests {')[0]
            for m in re.finditer(r'Outcome::(ok|error|denied)\(', s):
                parts = call_args(s, m.end())
                idx = 1 if m.group(1) == 'ok' else 0
                if len(parts) <= idx:
                    continue
                for lit in re.findall(r'"((?:[^"\\]|\\.)*)"', parts[idx]):
                    if len(lit) > 3 and lit[0].isupper():
                        out.add(template(lit))
    return sorted(out)


if __name__ == '__main__':
    for t in titles(sys.argv[1]):
        print(t)
