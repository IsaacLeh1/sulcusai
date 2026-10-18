// SPDX-License-Identifier: AGPL-3.0-only

/** What's inside single dollars looks like a formula, not money ("$x^2$", "$\frac{1}{2}$", "$n$"). */
function looksLikeMath(inner: string): boolean {
  if (/^\d[\d,.]*(\s|$)/.test(inner) && !/[\\^_{}=]/.test(inner)) return false;
  return (
    /[\\^_{}=<>]/.test(inner) ||
    /^[A-Za-z]$/.test(inner) ||
    /^[A-Za-z0-9]+\s*[+\-*/]\s*[A-Za-z0-9(]/.test(inner) ||
    /\b(sin|cos|tan|log|ln|lim|sqrt|pi|theta|alpha|beta|sigma|mu|lambda|delta)\b/.test(inner)
  );
}

function mathifyText(s: string): string {
  return (
    s
      // \[ … \] and a line that is only $$ … $$: a formula on its own lines.
      .replace(/\\\[([\s\S]+?)\\\]/g, (_, m: string) => `\n$$\n${m.trim()}\n$$\n`)
      .replace(/^[ \t]*\$\$([^\n$]+?)\$\$[ \t]*$/gm, (_, m: string) => `$$\n${m.trim()}\n$$`)
      // \( … \): a formula in the line.
      .replace(/\\\(([\s\S]+?)\\\)/g, (_, m: string) => `$$${m.trim()}$$`)
      // $ … $ when it's a formula (money like "$5 and $10" stays as it is).
      .replace(/(^|[^$\\\w])\$(?![\s$])([^$\n]+?)(?<![\s\\])\$(?![$\d])/g, (all, before: string, m: string) => (looksLikeMath(m) ? `${before}$$${m}$$` : all))
  );
}

/** Formulas the way models write them, the way remark-math (double dollars
 * only) reads them. Code, fenced or inline, is left alone. */
export function mathify(text: string): string {
  if (!/[$\\]/.test(text)) return text;
  return text
    .split(/(```[\s\S]*?(?:```|$)|`[^`\n]*`)/g)
    .map((part, i) => (i % 2 === 1 ? part : mathifyText(part)))
    .join("");
}
