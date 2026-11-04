// SPDX-License-Identifier: AGPL-3.0-only
// The window's layout, chat view and look, kept on this PC and applied as
// attributes on the root element so the stylesheet does the rest. The Layout
// assistant turns plain words into changes here; it never runs anything else.
import { useSyncExternalStore } from "react";

export type SidebarSide = "left" | "right";
export type Density = "comfortable" | "compact";
export type ChatWidth = "narrow" | "normal" | "wide";
export type ChatStyle = "bubbles" | "plain";
export type Accent = "violet" | "blue" | "green" | "orange" | "pink" | "gray";

export interface LayoutPrefs {
  /** The Layout assistant switch; off until turned on. */
  on: boolean;
  sidebar: SidebarSide;
  density: Density;
  chatWidth: ChatWidth;
  chatStyle: ChatStyle;
  /** Text size in percent of the normal size. */
  textSize: number;
  accent: Accent;
}

export const ACCENTS: Accent[] = ["violet", "blue", "green", "orange", "pink", "gray"];
export const TEXT_MIN = 85;
export const TEXT_MAX = 140;
const TEXT_STEP = 10;

export const DEFAULT_LAYOUT: LayoutPrefs = {
  on: false,
  sidebar: "left",
  density: "comfortable",
  chatWidth: "normal",
  chatStyle: "bubbles",
  textSize: 100,
  accent: "violet",
};

const KEY = "sulcusai.layout";
const listeners = new Set<() => void>();
let cached: LayoutPrefs | null = null;
let undoStack: LayoutPrefs[] = [];

function oneOf<T extends string>(v: unknown, list: readonly T[], fallback: T): T {
  return typeof v === "string" && (list as readonly string[]).includes(v) ? (v as T) : fallback;
}

export function clampText(n: number): number {
  if (!Number.isFinite(n)) return 100;
  return Math.min(TEXT_MAX, Math.max(TEXT_MIN, Math.round(n)));
}

/** Reads stored prefs, dropping anything unknown so a bad value can't break the window. */
export function cleanLayout(raw: unknown): LayoutPrefs {
  const r = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
  const d = DEFAULT_LAYOUT;
  return {
    on: r.on === true,
    sidebar: oneOf(r.sidebar, ["left", "right"] as const, d.sidebar),
    density: oneOf(r.density, ["comfortable", "compact"] as const, d.density),
    chatWidth: oneOf(r.chatWidth, ["narrow", "normal", "wide"] as const, d.chatWidth),
    chatStyle: oneOf(r.chatStyle, ["bubbles", "plain"] as const, d.chatStyle),
    textSize: typeof r.textSize === "number" ? clampText(r.textSize) : d.textSize,
    accent: oneOf(r.accent, ACCENTS, d.accent),
  };
}

export function layoutPrefs(): LayoutPrefs {
  if (cached) return cached;
  try {
    const v = localStorage.getItem(KEY);
    cached = cleanLayout(v ? JSON.parse(v) : null);
  } catch {
    cached = { ...DEFAULT_LAYOUT };
  }
  return cached;
}

/** The attributes the stylesheet reads; empty while the assistant is off so the window looks as shipped. */
export function layoutAttributes(p: LayoutPrefs): Record<string, string> {
  if (!p.on) return {};
  return {
    sidebar: p.sidebar,
    density: p.density,
    chatWidth: p.chatWidth,
    chatStyle: p.chatStyle,
    accent: p.accent,
  };
}

const ATTRS = ["sidebar", "density", "chatWidth", "chatStyle", "accent"] as const;

function apply() {
  const p = layoutPrefs();
  const root = document.documentElement;
  const attrs = layoutAttributes(p);
  for (const a of ATTRS) {
    if (attrs[a]) root.dataset[a] = attrs[a];
    else delete root.dataset[a];
  }
  // Every size in the stylesheet is in px, so zoom is the one switch that scales all text alike.
  root.style.setProperty("zoom", p.on && p.textSize !== 100 ? String(p.textSize / 100) : "");
  listeners.forEach((l) => l());
}

export function setLayout(next: LayoutPrefs, remember = true) {
  const now = layoutPrefs();
  if (remember) undoStack = [...undoStack.slice(-19), now];
  cached = cleanLayout(next);
  try {
    localStorage.setItem(KEY, JSON.stringify(cached));
  } catch {
    // Not saved, but still applied for now.
  }
  apply();
}

export function canUndoLayout(): boolean {
  return undoStack.length > 0;
}

export function undoLayout() {
  const prev = undoStack.pop();
  if (prev) setLayout(prev, false);
}

export function initLayout() {
  apply();
  window.addEventListener("storage", (e) => {
    if (e.key === KEY) {
      cached = null;
      apply();
    }
  });
}

export function useLayout(): LayoutPrefs {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    layoutPrefs,
  );
}

/** For tests: forget the cached prefs and the undo history. */
export function resetLayoutCache() {
  cached = null;
  undoStack = [];
}

export interface Change {
  en: string;
  vars?: Record<string, string | number>;
}

export interface Interpretation {
  next: LayoutPrefs;
  /** What changed, as English source strings translated where shown. */
  changes: Change[];
}

const ACCENT_WORDS: Record<string, Accent> = {
  violet: "violet", purple: "violet", lilac: "violet",
  blue: "blue", navy: "blue", teal: "blue",
  green: "green", mint: "green",
  orange: "orange", amber: "orange", yellow: "orange",
  pink: "pink", red: "pink", rose: "pink",
  gray: "gray", grey: "gray", neutral: "gray",
};

/**
 * Turns a plain request ("sidebar on the right, smaller text, compact") into
 * new prefs. Only the listed words act; anything else changes nothing, so a
 * sentence can never do more than move, size or colour the window.
 */
export function interpretLayout(text: string, current: LayoutPrefs): Interpretation {
  const s = ` ${text.toLowerCase().replace(/[^a-z0-9%\s-]/g, " ").replace(/\s+/g, " ")} `;
  const next = { ...current, on: true };
  const changes: Change[] = [];

  if (/\b(reset|default|defaults|as shipped|start over)\b/.test(s)) {
    return { next: { ...DEFAULT_LAYOUT, on: true }, changes: [{ en: "Back to the usual layout" }] };
  }

  const side = /\b(sidebar|side bar|menu|list|panel|chats)\b[^.]*\bright\b|\bright\b[^.]*\b(sidebar|side bar|panel)\b/.test(s)
    ? "right"
    : /\b(sidebar|side bar|menu|list|panel|chats)\b[^.]*\bleft\b|\bleft\b[^.]*\b(sidebar|side bar|panel)\b/.test(s)
      ? "left"
      : null;
  if (side && side !== current.sidebar) {
    next.sidebar = side;
    changes.push({ en: side === "right" ? "Sidebar on the right" : "Sidebar on the left" });
  }

  if (/\b(compact|dense|tighter|tight|less space|less padding)\b/.test(s)) next.density = "compact";
  else if (/\b(comfortable|roomy|spacious|more space|more padding|airy)\b/.test(s)) next.density = "comfortable";
  if (next.density !== current.density) changes.push({ en: next.density === "compact" ? "Compact spacing" : "Roomy spacing" });

  const widthCtx = /\b(chat|chats|messages|conversation|column|reading)\b/.test(s);
  if (/\b(full width|wide|wider|widest)\b/.test(s) && (widthCtx || !/\b(text|font|letters)\b/.test(s))) next.chatWidth = "wide";
  else if (/\b(narrow|narrower|thin)\b/.test(s)) next.chatWidth = "narrow";
  else if (/\bnormal width\b/.test(s)) next.chatWidth = "normal";
  if (next.chatWidth !== current.chatWidth) changes.push({ en: next.chatWidth === "wide" ? "Wide chat" : next.chatWidth === "narrow" ? "Narrow chat" : "Normal chat width" });

  if (/\b(no bubbles|plain|without bubbles|flat|document)\b/.test(s)) next.chatStyle = "plain";
  else if (/\b(bubbles|bubble)\b/.test(s)) next.chatStyle = "bubbles";
  if (next.chatStyle !== current.chatStyle) changes.push({ en: next.chatStyle === "plain" ? "Messages without bubbles" : "Messages in bubbles" });

  const pct = s.match(/\b(\d{2,3})\s?%/);
  const textCtx = /\b(text|font|letters|words|type|everything)\b/.test(s);
  if (pct && textCtx) next.textSize = clampText(Number(pct[1]));
  else if (textCtx && /\b(bigger|larger|big|large|increase)\b/.test(s)) next.textSize = clampText(current.textSize + (/\bmuch\b/.test(s) ? 2 : 1) * TEXT_STEP);
  else if (textCtx && /\b(smaller|small|tiny|decrease)\b/.test(s)) next.textSize = clampText(current.textSize - (/\bmuch\b/.test(s) ? 2 : 1) * TEXT_STEP);
  else if (textCtx && /\bnormal\b/.test(s)) next.textSize = 100;
  if (next.textSize !== current.textSize) changes.push({ en: "Text size {n}%", vars: { n: next.textSize } });

  for (const [word, accent] of Object.entries(ACCENT_WORDS)) {
    if (new RegExp(`\\b${word}\\b`).test(s)) {
      next.accent = accent;
      break;
    }
  }
  if (next.accent !== current.accent) changes.push({ en: "Accent colour changed" });

  return { next, changes };
}
