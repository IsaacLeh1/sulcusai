// SPDX-License-Identifier: AGPL-3.0-only
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { documentText, dueAtStart, dueFor, isUpdate, neededNotice, type ConsentStatus } from "./ConsentApi";
import { ConsentNotice, NOTICES } from "./ConsentNotice";
import { ConsentReport, REPORT_PLACES } from "./ConsentReport";
import { ConsentScreen, KEY_POINTS } from "./ConsentScreen";
import rust from "../../src-tauri/src/consent.rs?raw";

// Modal reads the focused element while rendering.
(globalThis as { document?: unknown }).document ??= { activeElement: null, documentElement: { lang: "en" } };



function item(id: string, over: Partial<ConsentStatus> = {}): ConsentStatus {
  return { id, kind: "document", version: 1, accepted: null, accepted_at: null, due: true, at_start: false, file: null, with: [], ...over };
}

describe("what needs accepting", () => {
  const all = [
    item("terms", { at_start: true, file: "terms-of-use.md" }),
    item("privacy", { at_start: true, accepted: 1, due: false }),
    item("guidelines", { file: "community-guidelines.md" }),
    item("social", { kind: "notice", with: ["guidelines"] }),
    item("vpn", { kind: "notice", accepted: 1, due: false }),
  ];

  it("at start, only due start documents", () => {
    expect(dueAtStart(all).map((s) => s.id)).toEqual(["terms"]);
  });

  it("a notice brings its documents along", () => {
    expect(dueFor(all, "social").map((s) => s.id)).toEqual(["social", "guidelines"]);
    expect(dueFor(all, "vpn")).toEqual([]);
    expect(dueFor(all, "nothing")).toEqual([]);
  });

  it("tells a changed version from a first reading", () => {
    expect(isUpdate(item("terms", { version: 2, accepted: 1 }))).toBe(true);
    expect(isUpdate(item("terms"))).toBe(false);
  });

  it("reads which notice a command is waiting for", () => {
    expect(neededNotice("consent:vpn")).toBe("vpn");
    expect(neededNotice(new Error("consent:market"))).toBe("market");
    expect(neededNotice("locked")).toBeNull();
  });
});

describe("the texts match the app's records", () => {
  const ids = [...rust.matchAll(/notice\("([a-z_]+)"/g)].map((m) => m[1]);

  it("every notice in consent.rs has words here", () => {
    expect(ids.length).toBeGreaterThan(5);
    for (const id of ids) expect(NOTICES[id], id).toBeTruthy();
    for (const id of Object.keys(NOTICES)) expect(ids, id).toContain(id);
  });

  it("every document the app asks for is bundled, with a version line", async () => {
    const files = [...rust.matchAll(/"([a-z-]+\.md)"/g)].map((m) => m[1]);
    for (const n of Object.values(NOTICES)) if (n.file) files.push(n.file);
    for (const file of files) {
      const text = await documentText(file);
      expect(text, file).toMatch(/^Version \d+, \d{4}-\d{2}-\d{2}$/m);
    }
  });

  it("no em dashes in what people read", async () => {
    const all = [...KEY_POINTS, ...Object.values(NOTICES).flatMap((n) => [n.title, ...n.points]), ...REPORT_PLACES.flatMap((p) => [p.what, p.where])];
    const dash = String.fromCharCode(0x2014);
    for (const s of all) expect(s).not.toContain(dash);
    for (const file of ["terms-of-use.md", "privacy-policy.md", "community-guidelines.md", "copyright-policy.md", "likeness-consent.md", "market-data-disclaimer.md", "running-relays-and-servers.md"]) {
      expect(await documentText(file), file).not.toContain(dash);
    }
  });
});

describe("the screens", () => {
  it("the acceptance screen lists what's due and waits for the box", () => {
    const html = renderToStaticMarkup(<ConsentScreen due={[item("terms", { file: "terms-of-use.md" }), item("privacy")]} onAccepted={() => {}} />);
    expect(html).toContain("Before you start");
    expect(html).toContain("Terms of Use");
    expect(html).toContain("Privacy Policy");
    expect(html).toContain("docs/legal/terms-of-use.md");
    expect(html).toMatch(/<button class="btn primary" disabled="">I agree<\/button>/);
  });

  it("says so when the terms changed", () => {
    const html = renderToStaticMarkup(<ConsentScreen due={[item("terms", { version: 2, accepted: 1 })]} onAccepted={() => {}} onDecline={() => {}} />);
    expect(html).toContain("The terms have changed");
    expect(html).toContain("close SulcusAI");
  });

  it("a feature's notice names the documents accepted with it", () => {
    const html = renderToStaticMarkup(<ConsentNotice feature="social" due={[item("social", { kind: "notice" }), item("guidelines", { file: "community-guidelines.md" })]} onAccepted={() => {}} onCancel={() => {}} />);
    expect(html).toContain("Before you join groups");
    expect(html).toContain("Community Guidelines");
    expect(html).toContain("I understand and agree");
  });

  it("an unknown feature shows nothing", () => {
    expect(renderToStaticMarkup(<ConsentNotice feature="nothing" due={[]} onAccepted={() => {}} onCancel={() => {}} />)).toBe("");
  });

  it("reporting points to the authorities, opened outside the app", () => {
    const html = renderToStaticMarkup(<ConsentReport />);
    expect(html).toContain("report.cybertip.org");
    expect(html).toContain("emergency number");
    for (const p of REPORT_PLACES) if (p.url) expect(p.url).toMatch(/^https:\/\//);
    expect(html).not.toMatch(/href="https[^"]*"(?![^>]*target="_blank")/);
  });
});
