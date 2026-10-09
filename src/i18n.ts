// SPDX-License-Identifier: AGPL-3.0-only
// The window's language. Text is written in English in the code and looked
// up by that English text in the chosen language's table
// (src/locales/<code>.json); anything not translated yet shows in English.
// Placeholders such as {name} are filled in after the lookup.
import { useSyncExternalStore } from "react";

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
  document.documentElement.lang = code;
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

/** Re-renders a component when the language changes. */
export function useLanguage(): string {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => current,
  );
}
