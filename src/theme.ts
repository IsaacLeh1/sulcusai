// SPDX-License-Identifier: AGPL-3.0-only
// Light, dark and OLED (true black) looks. The choice is kept on this PC and
// applied before the first paint; "system" follows Windows' setting.
import { useSyncExternalStore } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

export type ThemeChoice = "system" | "light" | "dark" | "oled";
export const THEMES: ThemeChoice[] = ["system", "light", "dark", "oled"];

const KEY = "sulcusai.theme";
const media = () => window.matchMedia("(prefers-color-scheme: dark)");
const listeners = new Set<() => void>();

export function themeChoice(): ThemeChoice {
  try {
    const v = localStorage.getItem(KEY);
    if (v && (THEMES as string[]).includes(v)) return v as ThemeChoice;
  } catch {
    // Storage blocked: follow Windows.
  }
  return "system";
}

/** The look in use: light, dark or oled. */
export function resolvedTheme(choice = themeChoice()): "light" | "dark" | "oled" {
  return choice === "system" ? (media().matches ? "dark" : "light") : choice;
}

function apply() {
  const look = resolvedTheme();
  const root = document.documentElement;
  root.dataset.theme = look;
  root.style.colorScheme = look === "light" ? "light" : "dark";
  // The window's own title bar follows (null: Windows decides, for "system").
  const choice = themeChoice();
  try {
    getCurrentWindow()
      .setTheme(choice === "system" ? null : look === "light" ? "light" : "dark")
      .catch(() => {});
  } catch {
    // Not in an app window (a preview): the page alone changes.
  }
  listeners.forEach((l) => l());
}

export function setTheme(choice: ThemeChoice) {
  try {
    localStorage.setItem(KEY, choice);
  } catch {
    // Not saved, but still applied for now.
  }
  apply();
}

const ACCENT_KEY = "sulcus.accent";

/** The app's own accent colour (Social Extras), or "" for the theme's. */
export function accentChoice(): string {
  try {
    return localStorage.getItem(ACCENT_KEY) ?? "";
  } catch {
    return "";
  }
}

function applyAccent() {
  const c = accentChoice();
  const root = document.documentElement.style;
  if (/^#[0-9a-f]{6}$/i.test(c)) {
    root.setProperty("--accent", c);
    root.setProperty("--accent-soft", `color-mix(in srgb, ${c} 18%, transparent)`);
  } else {
    root.removeProperty("--accent");
    root.removeProperty("--accent-soft");
  }
}

export function setAccent(c: string) {
  try {
    if (c) localStorage.setItem(ACCENT_KEY, c);
    else localStorage.removeItem(ACCENT_KEY);
  } catch {
    // Applied for now only.
  }
  applyAccent();
}

/** Applies the saved look and follows Windows when the choice is "system". */
export function initTheme() {
  apply();
  applyAccent();
  window.addEventListener("storage", (e) => e.key === ACCENT_KEY && applyAccent());
  media().addEventListener("change", () => themeChoice() === "system" && apply());
  // Another window (quick ask) changed it.
  window.addEventListener("storage", (e) => e.key === KEY && apply());
}

export function useTheme(): ThemeChoice {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    themeChoice,
  );
}
