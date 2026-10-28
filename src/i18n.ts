// SPDX-License-Identifier: AGPL-3.0-only
// The window's language. Text is written in English in the code and looked
// up by that English text in the chosen language's table
// (src/locales/<code>.json); anything not translated yet shows in English.
// Placeholders such as {name} are filled in after the lookup.
import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";

export const LANGUAGES: { code: string; name: string }[] = [
  { code: "en", name: "English" },
  { code: "es", name: "Español" },
  { code: "fr", name: "Français" },
  { code: "de", name: "Deutsch" },
  { code: "pt", name: "Português" },
  { code: "zh", name: "中文" },
  { code: "ja", name: "日本語" },
];

type Table = Record<string, string>;

const loaders: Record<string, () => Promise<{ default: Table }>> = {
  es: () => import("./locales/es.json"),
  fr: () => import("./locales/fr.json"),
  de: () => import("./locales/de.json"),
  pt: () => import("./locales/pt.json"),
  zh: () => import("./locales/zh.json"),
  ja: () => import("./locales/ja.json"),
};

const STORE = "sulcus.language";
let current = "en";
let table: Table = {};
const listeners = new Set<() => void>();

function systemLanguage(): string {
  const code = (navigator.language || "en").slice(0, 2).toLowerCase();
  return LANGUAGES.some((l) => l.code === code) ? code : "en";
}

/** "system" or a code from LANGUAGES. */
export function chosenLanguage(): string {
  try {
    return localStorage.getItem(STORE) ?? "system";
  } catch {
    return "system";
  }
}

export async function setLanguage(choice: string): Promise<void> {
  try {
    if (choice === "system") localStorage.removeItem(STORE);
    else localStorage.setItem(STORE, choice);
  } catch {
    // Private storage off: the choice lasts until the window closes.
  }
  const code = choice === "system" ? systemLanguage() : choice;
  table = code === "en" || !loaders[code] ? {} : (await loaders[code]()).default;
  current = code;
  templates = null;
  document.documentElement.lang = code;
  // The core writes some text too (mail sorting reasons, notices).
  invoke("set_ui_language", { code }).catch(() => {});
  listeners.forEach((l) => l());
}

/** Loads the saved language before the first render. */
export function initLanguage(): Promise<void> {
  return setLanguage(chosenLanguage()).catch(() => undefined);
}

export function language(): string {
  return current;
}

function fill(text: string, vars?: Record<string, string | number>): string {
  if (!vars) return text;
  return text.replace(/\{(\w+)\}/g, (m, k) => (k in vars ? String(vars[k]) : m));
}

/** Marks text for translation where it's defined (lists, tables); pass
    the value through t() where it's shown. */
export function tx(english: string): string {
  return english;
}

/** The text in the window's language. */
export function t(english: string, vars?: Record<string, string | number>): string {
  return fill(table[english] || english, vars);
}

// Entries with {placeholders}, as patterns, for text the core fills in
// before it gets here ("Searched email for “google” (3 found)").
let templates: { re: RegExp; names: string[]; to: string; fixed: number }[] | null = null;
const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** Like t(), for finished text from the core: matched against templates
 * such as "Searched email for “{q}”" and translated with its values kept. */
export function tf(text: string): string {
  if (table[text]) return table[text];
  if (!templates) {
    templates = Object.keys(table)
      .filter((k) => /\{\w+\}/.test(k))
      .map((k) => {
        const names: string[] = [];
        const body = k.split(/(\{\w+\})/).map((part) => {
          const m = part.match(/^\{(\w+)\}$/);
          if (!m) return escape(part);
          names.push(m[1]);
          return "(.+?)";
        });
        return { re: new RegExp(`^${body.join("")}$`, "s"), names, to: table[k], fixed: k.replace(/\{\w+\}/g, "").length };
      })
      // The most specific first: "Checked the calendar ({range}, {1} events)"
      // before "Checked the calendar ({range})", and both before "{a} for {b}".
      .sort((a, b) => b.fixed - a.fixed);
  }
  for (const tpl of templates) {
    const m = text.match(tpl.re);
    if (m) return fill(tpl.to, Object.fromEntries(tpl.names.map((n, i) => [n, m[i + 1]])));
  }
  return text;
}

/** Re-renders a component when the language changes. */
export function useLanguage(): string {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => current,
    // Saving a chat as a page renders messages with react-dom/server, which
    // needs this or throws.
    () => current,
  );
}
