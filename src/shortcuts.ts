// SPDX-License-Identifier: AGPL-3.0-only
/* The "/" menu in the message box: saved prompts, built-in commands and
 * the active skills, and which of them what's typed after "/" picks. */
import { wordScore } from "./fuzzy";
import { tx } from "./i18n";

export type Action = "summarize" | "new" | "model" | "plan" | "auto" | "bypass" | "go-back" | "compact" | "prompts";

export interface Shortcut {
  name: string;
  /** Shown beside the name (built-ins' are translated where shown). */
  hint: string;
  action?: Action;
  /** A skill: the next message starts by asking for it. */
  skill?: string;
  /** A saved prompt: its text fills the box. */
  prompt?: string;
}

export const BUILT_IN: { name: Action; hint: string }[] = [
  { name: "summarize", hint: tx("Ask for a summary of the chat so far") },
  { name: "new", hint: tx("Start a new chat") },
  { name: "model", hint: tx("Choose the model for this chat") },
  { name: "plan", hint: tx("Plan mode: it looks around and proposes, and changes nothing") },
  { name: "auto", hint: tx("Auto mode: it changes files and runs commands, and asks before anything risky") },
  { name: "bypass", hint: tx("Bypass mode: it changes files and runs commands without asking") },
  { name: "go-back", hint: tx("Go back to your last message, to change it") },
  { name: "compact", hint: tx("Sum up the chat so far, so it has room to go on") },
  { name: "prompts", hint: tx("Add or change saved prompts") },
];

/** A skill's name as typed after "/": lower case, spaces as dashes. */
export function slug(name: string): string {
  return name
    .trim()
    .toLowerCase()
    .replace(/\s+/g, "-")
    .replace(/[^\p{L}\p{N}_-]/gu, "");
}

/** Everything the menu offers, saved prompts first (a prompt keeps its
 * name even when a command or skill has the same one). */
export function shortcuts(prompts: { name: string; text: string }[], skills: { name: string; description: string }[]): Shortcut[] {
  const taken = new Set<string>();
  const out: Shortcut[] = [];
  const add = (s: Shortcut) => {
    const key = s.name.toLowerCase();
    if (!key || taken.has(key)) return;
    taken.add(key);
    out.push(s);
  };
  for (const p of prompts) add({ name: p.name, hint: p.text.replace(/\s+/g, " ").slice(0, 80), prompt: p.text });
  for (const b of BUILT_IN) add({ name: b.name, hint: b.hint, action: b.name });
  for (const k of skills) add({ name: slug(k.name), hint: k.description.replace(/\s+/g, " ").slice(0, 80), skill: k.name });
  return out;
}

/** In order of how well they match what's typed: names starting with it,
 * then containing it, then with its letters in order or a small typo. */
export function matching<T extends { name: string }>(all: T[], typed: string): T[] {
  const q = typed.toLowerCase();
  if (!q) return all;
  const inOrder = (name: string) => {
    let at = 0;
    for (const ch of name) if (ch === q[at]) at++;
    return at === q.length;
  };
  const score = (name: string) => {
    const n = name.toLowerCase();
    if (n.startsWith(q)) return 4;
    if (n.includes(q)) return 3;
    if (q.length >= 2 && inOrder(n)) return 2;
    return n.split(/[-_]/).some((w) => wordScore(q, w) >= 0.6) ? 1 : 0;
  };
  return all
    .map((item, i) => ({ item, i, s: score(item.name) }))
    .filter((x) => x.s > 0)
    .sort((a, b) => b.s - a.s || a.i - b.i)
    .map((x) => x.item);
}

/** What the box starts with after picking a skill. */
export function skillAsk(skill: string): string {
  return `Use the ${skill} skill for this: `;
}

/** The message /summarize sends. */
export const SUMMARY_ASK = "Summarize this chat so far: what was asked, what was done or decided, and what's still open.";
