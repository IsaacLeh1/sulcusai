// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_LAYOUT, cleanLayout, interpretLayout, layoutAttributes, layoutPrefs, resetLayoutCache, setLayout, undoLayout } from "./layout";

const on = { ...DEFAULT_LAYOUT, on: true };

describe("interpretLayout", () => {
  it("moves the sidebar and tightens spacing", () => {
    const r = interpretLayout("Put the sidebar on the right and make it compact", on);
    expect(r.next.sidebar).toBe("right");
    expect(r.next.density).toBe("compact");
    expect(r.changes.length).toBe(2);
  });
  it("sizes text by steps and by percent, within limits", () => {
    expect(interpretLayout("bigger text", on).next.textSize).toBe(110);
    expect(interpretLayout("much smaller font", on).next.textSize).toBe(85);
    expect(interpretLayout("text at 120%", on).next.textSize).toBe(120);
    expect(interpretLayout("text at 300%", on).next.textSize).toBe(140);
  });
  it("chat width, plain messages and colour", () => {
    const r = interpretLayout("wide chat, no bubbles, green please", on);
    expect(r.next.chatWidth).toBe("wide");
    expect(r.next.chatStyle).toBe("plain");
    expect(r.next.accent).toBe("green");
  });
  it("unknown words change nothing", () => {
    const r = interpretLayout("delete all my files", on);
    expect(r.changes).toEqual([]);
    expect(r.next).toEqual(on);
  });
  it("reset goes back to the usual layout", () => {
    const r = interpretLayout("reset", { ...on, sidebar: "right", accent: "pink" });
    expect(r.next).toEqual(on);
  });
});

describe("prefs", () => {
  const store = new Map<string, string>();
  const root = { dataset: {} as Record<string, string>, style: { setProperty: () => {} } };
  beforeEach(() => {
    store.clear();
    root.dataset = {};
    // The tests run without a page, so the two browser objects layout.ts touches are stand-ins.
    vi.stubGlobal("localStorage", { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => void store.set(k, v) });
    vi.stubGlobal("document", { documentElement: root });
    resetLayoutCache();
  });
  it("cleans bad stored values", () => {
    expect(cleanLayout({ sidebar: "top", textSize: 9999, accent: "x", on: "yes" })).toEqual({ ...DEFAULT_LAYOUT, textSize: 140 });
  });
  it("no attributes while off", () => {
    expect(layoutAttributes({ ...DEFAULT_LAYOUT, sidebar: "right" })).toEqual({});
    expect(layoutAttributes({ ...on, sidebar: "right" }).sidebar).toBe("right");
  });
  it("saves, applies and undoes", () => {
    setLayout({ ...on, sidebar: "right" });
    expect(root.dataset.sidebar).toBe("right");
    resetLayoutCache();
    expect(layoutPrefs().sidebar).toBe("right");
    setLayout({ ...layoutPrefs(), accent: "blue" });
    undoLayout();
    expect(layoutPrefs().accent).toBe("violet");
    setLayout({ ...layoutPrefs(), on: false });
    expect(root.dataset.sidebar).toBeUndefined();
  });
});
