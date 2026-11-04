// SPDX-License-Identifier: AGPL-3.0-only
// Widgets: small pages a person adds, run sealed in a frame with no network,
// no same-origin access and no navigation. The only way out is a message
// asking for one of the actions below; each is asked about once per widget
// and the answer is kept.
import { useSyncExternalStore } from "react";
import type { View } from "../App";

export type WidgetAction = "notify" | "open_page" | "copy" | "remember";
export const WIDGET_ACTIONS: WidgetAction[] = ["notify", "open_page", "copy", "remember"];

/** Pages a widget may open; Settings and anything with keys or people stay out. */
export const OPENABLE_PAGES: View[] = ["chat", "notes", "documents", "tasks", "calendar", "scheduled", "translate", "study", "studio"];

export const MAX_HTML = 64 * 1024;
export const MAX_TEXT = 200;
export const MAX_COPY = 10 * 1024;
export const MAX_MEMORY = 8 * 1024;
export const MAX_WIDGETS = 12;

export type Grant = "allowed" | "denied";

export interface Widget {
  id: string;
  name: string;
  html: string;
  /** Shown in the sidebar under the chats. */
  pinned: boolean;
  grants: Partial<Record<WidgetAction, Grant>>;
  /** What the widget kept with "remember", as one string. */
  memory: string;
}

export interface WidgetState {
  /** The Widgets switch; off until turned on. */
  on: boolean;
  widgets: Widget[];
}

const KEY = "sulcusai.widgets";
const listeners = new Set<() => void>();
let cached: WidgetState | null = null;

export function cleanWidgets(raw: unknown): WidgetState {
  const r = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
  const list = Array.isArray(r.widgets) ? r.widgets : [];
  const widgets: Widget[] = [];
  for (const w of list.slice(0, MAX_WIDGETS)) {
    if (!w || typeof w !== "object") continue;
    const o = w as Record<string, unknown>;
    if (typeof o.id !== "string" || typeof o.html !== "string" || o.html.length > MAX_HTML) continue;
    const grants: Widget["grants"] = {};
    const g = (o.grants && typeof o.grants === "object" ? o.grants : {}) as Record<string, unknown>;
    for (const a of WIDGET_ACTIONS) if (g[a] === "allowed" || g[a] === "denied") grants[a] = g[a] as Grant;
    widgets.push({
      id: o.id,
      name: typeof o.name === "string" && o.name.trim() ? o.name.slice(0, 60) : "Widget",
      html: o.html,
      pinned: o.pinned === true,
      grants,
      memory: typeof o.memory === "string" ? o.memory.slice(0, MAX_MEMORY) : "",
    });
  }
  return { on: r.on === true, widgets };
}

export function widgetState(): WidgetState {
  if (cached) return cached;
  try {
    const v = localStorage.getItem(KEY);
    cached = cleanWidgets(v ? JSON.parse(v) : null);
  } catch {
    cached = { on: false, widgets: [] };
  }
  return cached;
}

export function saveWidgets(next: WidgetState) {
  cached = cleanWidgets(next);
  try {
    localStorage.setItem(KEY, JSON.stringify(cached));
  } catch {
    // Kept for this run only.
  }
  listeners.forEach((l) => l());
}

export function updateWidget(id: string, patch: Partial<Widget>) {
  const s = widgetState();
  saveWidgets({ ...s, widgets: s.widgets.map((w) => (w.id === id ? { ...w, ...patch } : w)) });
}

export function addWidget(name: string, html: string): Widget | null {
  const s = widgetState();
  if (html.length > MAX_HTML || s.widgets.length >= MAX_WIDGETS) return null;
  const w: Widget = { id: `w${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`, name: name.trim().slice(0, 60) || "Widget", html, pinned: false, grants: {}, memory: "" };
  saveWidgets({ ...s, widgets: [...s.widgets, w] });
  return w;
}

export function removeWidget(id: string) {
  const s = widgetState();
  saveWidgets({ ...s, widgets: s.widgets.filter((w) => w.id !== id) });
}

export function useWidgets(): WidgetState {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    widgetState,
  );
}

export function resetWidgetCache() {
  cached = null;
}

export interface WidgetRequest {
  call: number;
  action: WidgetAction;
  args: Record<string, unknown>;
}

/**
 * Checks a message from a widget frame. Anything not shaped exactly like a
 * known action with arguments in bounds is dropped, so a widget can't reach
 * past the list by sending odd values.
 */
export function parseRequest(data: unknown): WidgetRequest | { call: number; error: string } | null {
  if (!data || typeof data !== "object") return null;
  const d = data as Record<string, unknown>;
  if (d.sulcusWidget !== 1 || typeof d.call !== "number" || !Number.isInteger(d.call)) return null;
  const call = d.call;
  const action = d.action;
  const args = (d.args && typeof d.args === "object" ? d.args : {}) as Record<string, unknown>;
  if (typeof action !== "string" || !(WIDGET_ACTIONS as string[]).includes(action)) return { call, error: "unknown action" };
  switch (action as WidgetAction) {
    case "notify":
      if (typeof args.text !== "string" || !args.text.trim() || args.text.length > MAX_TEXT) return { call, error: "text must be 1 to 200 characters" };
      return { call, action: "notify", args: { text: args.text } };
    case "open_page":
      if (typeof args.page !== "string" || !(OPENABLE_PAGES as string[]).includes(args.page)) return { call, error: "page not allowed" };
      return { call, action: "open_page", args: { page: args.page } };
    case "copy":
      if (typeof args.text !== "string" || args.text.length > MAX_COPY) return { call, error: "text too long" };
      return { call, action: "copy", args: { text: args.text } };
    case "remember":
      if (args.value === undefined) return { call, action: "remember", args: {} };
      if (typeof args.value !== "string" || args.value.length > MAX_MEMORY) return { call, error: "value must be a string up to 8 KB" };
      return { call, action: "remember", args: { value: args.value } };
  }
}

const CSP = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:";

/** The page given to the frame: a strict policy first, then a tiny bridge, then the widget's own HTML. */
export function widgetDoc(html: string, dark: boolean): string {
  const bridge =
    "<script>(function(){var n=0,w={};window.addEventListener('message',function(e){var d=e.data;if(e.source!==parent||!d||d.sulcusWidgetReply!==1||!w[d.call])return;var p=w[d.call];delete w[d.call];d.ok?p[0](d.result):p[1](new Error(d.error||'refused'));});" +
    "window.sulcus={call:function(a,args){return new Promise(function(r,j){var c=++n;w[c]=[r,j];parent.postMessage({sulcusWidget:1,call:c,action:a,args:args||{}},'*');});}};})();</script>";
  const base = `<style>html{color-scheme:${dark ? "dark" : "light"}}body{margin:8px;font:13px system-ui,sans-serif}</style>`;
  return `<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="${CSP}">${base}${bridge}</head><body>${html}</body></html>`;
}
