// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";

/** How the app looks and feels on this PC. Kept in the browser store: each PC has its own screen. */
export interface UiPrefs {
  density: "comfortable" | "compact";
  contrast: boolean;
  colorSafe: boolean;
  reducedMotion: boolean;
  dyslexiaFont: boolean;
  calm: boolean;
  sounds: boolean;
  volume: number;
  tabs: string[];
  hiddenTabs: string[];
  sidebarIcons: boolean;
  panelSizes: Record<string, number>;
  shortcuts: Record<string, string>;
}

export const TOP_TABS = ["home", "chat", "create", "social", "networks", "library", "settings"] as const;
export type TopTab = (typeof TOP_TABS)[number];

export const DEFAULT_SHORTCUTS: Record<string, string> = {
  commandBar: "Ctrl+K",
  newChat: "Ctrl+N",
  closeTab: "Ctrl+W",
  nextTab: "Ctrl+Tab",
  prevTab: "Ctrl+Shift+Tab",
  settings: "Ctrl+,",
  home: "Alt+Home",
};

const DEFAULTS: UiPrefs = {
  density: "comfortable",
  contrast: false,
  colorSafe: false,
  reducedMotion: false,
  dyslexiaFont: false,
  calm: false,
  sounds: false,
  volume: 0.5,
  tabs: [...TOP_TABS],
  hiddenTabs: [],
  sidebarIcons: false,
  panelSizes: {},
  shortcuts: { ...DEFAULT_SHORTCUTS },
};

const KEY = "sulcus.ui";
let current: UiPrefs = read();

function read(): UiPrefs {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) ?? "{}");
    return { ...DEFAULTS, ...saved, shortcuts: { ...DEFAULT_SHORTCUTS, ...(saved.shortcuts ?? {}) } };
  } catch {
    return { ...DEFAULTS };
  }
}

export function uiPrefs(): UiPrefs {
  return current;
}

export function setUiPrefs(change: Partial<UiPrefs>) {
  current = { ...current, ...change };
  try {
    localStorage.setItem(KEY, JSON.stringify(current));
  } catch {
    // A full or blocked store only loses the remembering, not the change.
  }
  applyUiPrefs();
  window.dispatchEvent(new Event("sulcus:ui-prefs"));
}

/** Data attributes on <html> drive the CSS in styles-ui.css. */
export function applyUiPrefs() {
  const el = document.documentElement;
  el.dataset.density = current.density;
  el.toggleAttribute("data-contrast", current.contrast);
  el.toggleAttribute("data-colorsafe", current.colorSafe);
  const osReduced = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
  el.toggleAttribute("data-reduced-motion", current.reducedMotion || !!osReduced);
  el.toggleAttribute("data-dyslexia", current.dyslexiaFont);
  el.toggleAttribute("data-calm", current.calm);
}

export function useUiPrefs(): UiPrefs {
  const [p, setP] = useState(current);
  useEffect(() => {
    const on = () => setP(current);
    window.addEventListener("sulcus:ui-prefs", on);
    return () => window.removeEventListener("sulcus:ui-prefs", on);
  }, []);
  return p;
}

/** True when a key event matches a shortcut written like "Ctrl+Shift+K". */
export function matches(e: KeyboardEvent, combo: string | undefined): boolean {
  if (!combo) return false;
  const parts = combo.toLowerCase().split("+");
  const key = parts[parts.length - 1];
  const want = (m: string) => parts.includes(m);
  if (want("ctrl") !== (e.ctrlKey || e.metaKey)) return false;
  if (want("shift") !== e.shiftKey) return false;
  if (want("alt") !== e.altKey) return false;
  return e.key.toLowerCase() === key || (key === "," && e.key === ",");
}
