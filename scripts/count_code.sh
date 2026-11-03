#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Counts tracked files, lines by kind, and tests, so two versions can be
# compared. Run from the repo root: sh scripts/count_code.sh
echo "commit: $(git log --oneline -1)"
echo "tracked files: $(git ls-files | wc -l)"
for ext in rs ts tsx css js mjs py ps1 json md html toml; do
  f=$(git ls-files "*.$ext" | grep -v '^src/locales/')
  n=$(echo "$f" | grep -c .)
  l=$(echo "$f" | grep . | tr '\n' '\0' | xargs -0 cat 2>/dev/null | wc -l)
  printf "%-5s %5s files %8s lines\n" "$ext" "$n" "$l"
done
echo "locales: $(git ls-files 'src/locales/*.json' | wc -l) files, $(git ls-files 'src/locales/*.json' | tr '\n' '\0' | xargs -0 cat | wc -l) lines"
code=$(git ls-files '*.rs' '*.ts' '*.tsx' '*.css' '*.js' '*.mjs' '*.py' '*.ps1' '*.sh' | grep -v '^src/locales/')
echo "code: $(echo "$code" | wc -l) files, $(echo "$code" | tr '\n' '\0' | xargs -0 cat | wc -l) lines"
echo "Rust tests: $(git grep -cE '^\s*#\[(tokio::)?test' -- '*.rs' | awk -F: '{s+=$2} END {print s}') (ignored: $(git grep -cE '^\s*#\[ignore' -- '*.rs' | awk -F: '{s+=$2} END {print s}'))"
echo "window tests: $(git grep -cE '^\s*(it|test)\(' -- '*.test.ts' '*.test.tsx' | awk -F: '{s+=$2} END {print s}')"
echo "source files over 1,500 lines: $(git ls-files '*.rs' '*.ts' '*.tsx' | xargs wc -l | awk '$1>1500 && $2!="total"' | wc -l)"
