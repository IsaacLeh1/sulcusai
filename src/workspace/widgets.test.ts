// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { MAX_HTML, MAX_WIDGETS, addWidget, cleanWidgets, parseRequest, removeWidget, resetWidgetCache, updateWidget, widgetDoc, widgetState } from "./widgets";

const req = (action: string, args: object = {}) => ({ sulcusWidget: 1, call: 1, action, args });

describe("parseRequest", () => {
  it("ignores messages that aren't from the bridge", () => {
    expect(parseRequest(null)).toBeNull();
    expect(parseRequest({ call: 1, action: "notify" })).toBeNull();
    expect(parseRequest({ sulcusWidget: 1, call: "1", action: "notify" })).toBeNull();
  });
  it("refuses actions off the list", () => {
    expect(parseRequest(req("run_command", { cmd: "del" }))).toEqual({ call: 1, error: "unknown action" });
    expect(parseRequest(req("fetch", { url: "https://x" }))).toMatchObject({ error: "unknown action" });
  });
  it("checks arguments", () => {
    expect(parseRequest(req("notify", { text: "hi" }))).toEqual({ call: 1, action: "notify", args: { text: "hi" } });
    expect(parseRequest(req("notify", { text: "x".repeat(201) }))).toHaveProperty("error");
    expect(parseRequest(req("notify", { text: 5 }))).toHaveProperty("error");
    expect(parseRequest(req("open_page", { page: "notes" }))).toMatchObject({ action: "open_page" });
    expect(parseRequest(req("open_page", { page: "settings" }))).toHaveProperty("error");
    expect(parseRequest(req("open_page", { page: "connectors" }))).toHaveProperty("error");
    expect(parseRequest(req("remember"))).toEqual({ call: 1, action: "remember", args: {} });
    expect(parseRequest(req("remember", { value: "x".repeat(9000) }))).toHaveProperty("error");
  });
  it("drops extra arguments", () => {
    expect(parseRequest(req("copy", { text: "a", extra: "b" }))).toEqual({ call: 1, action: "copy", args: { text: "a" } });
  });
});

describe("widgetDoc", () => {
  it("puts the no-network policy before the widget's HTML", () => {
    const doc = widgetDoc("<script>fetch('https://x')</script>", false);
    const csp = doc.indexOf("Content-Security-Policy");
    expect(csp).toBeGreaterThan(-1);
    expect(csp).toBeLessThan(doc.indexOf("fetch("));
    expect(doc).toContain("default-src 'none'");
    expect(doc).not.toContain("connect-src");
  });
});

describe("store", () => {
  const store = new Map<string, string>();
  beforeEach(() => {
    store.clear();
    // No page in the tests, so storage is a stand-in.
    vi.stubGlobal("localStorage", { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => void store.set(k, v) });
    resetWidgetCache();
  });
  it("starts off and empty", () => {
    expect(widgetState()).toEqual({ on: false, widgets: [] });
  });
  it("adds, grants, keeps and removes", () => {
    const w = addWidget("Clock", "<p>hi</p>")!;
    updateWidget(w.id, { grants: { notify: "allowed" } });
    resetWidgetCache();
    expect(widgetState().widgets[0].grants).toEqual({ notify: "allowed" });
    removeWidget(w.id);
    expect(widgetState().widgets).toEqual([]);
  });
  it("limits size and count", () => {
    expect(addWidget("big", "x".repeat(MAX_HTML + 1))).toBeNull();
    for (let i = 0; i < MAX_WIDGETS; i++) addWidget(`w${i}`, "x");
    expect(addWidget("one more", "x")).toBeNull();
  });
  it("cleans stored values", () => {
    const s = cleanWidgets({ on: true, widgets: [{ id: "a", html: "x", grants: { notify: "yes", copy: "denied", shell: "allowed" } }, { html: "no id" }] });
    expect(s.widgets).toHaveLength(1);
    expect(s.widgets[0].grants).toEqual({ copy: "denied" });
  });
});
