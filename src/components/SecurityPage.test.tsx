// SPDX-License-Identifier: AGPL-3.0-only
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { Finding, SecurityReport } from "../api/security";
import { SecurityPageView } from "./SecurityPage";
import { fixAfterAsking, groupFindings, keyAge, lastUsed } from "./securityPageLogic";

// The text takes its language from the window's document.
(globalThis as { document?: unknown }).document ??= { documentElement: { lang: "en" } };

const NOW = 1_800_000_000_000;
const DAY = 24 * 60 * 60 * 1000;

function finding(over: Partial<Finding> & { id: string }): Finding {
  return { severity: "notice", title: "Title of {name}", detail: "Detail for {name}", args: { name: "Pixel" }, fix: null, open: null, ...over };
}

function report(findings: Finding[]): SecurityReport {
  return {
    findings,
    overview: {
      version: "1.0.0",
      now: NOW,
      protection: { encryption: true, lock_enabled: true, hello_enabled: false, auto_lock_minutes: 0, keep_working: false },
      phones: {
        page_on: true,
        anywhere: true,
        while_locked: false,
        open_sessions: 1,
        devices: [
          { id: "a", name: "Pixel", added: NOW - 40 * DAY, last_seen: NOW - 3 * DAY, has_password: true },
          { id: "b", name: "Tablet", added: NOW - 40 * DAY, last_seen: null, has_password: true },
        ],
      },
      listeners: [
        { id: "api", name: "Local API server", port: 7340, enabled: true, running: true, local_only: true, problem: null },
        { id: "ollama", name: "Server for programs made for Ollama", port: 11434, enabled: false, running: false, local_only: true, problem: null },
        { id: "phone", name: "Phone page", port: 47818, enabled: true, running: true, local_only: false, problem: null },
      ],
      addons: [{ id: "n", name: "Notes", enabled: true, host: "mcp.example.com", tools: 5, auto_run: ["search"], ask: 3, off: 1, signed_in: true, has_secrets: false }],
      keys: [
        { kind: "api_server", label: "Local API server", host: null, age_days: 12 },
        { kind: "site", label: "Finnhub", host: "finnhub.io", age_days: 400 },
      ],
    },
  };
}

const fixable = finding({ id: "phone_stale:a", fix: { label: "Remove {name}", confirm: "Remove {name}? You can pair it again afterwards.", call: { kind: "remove_phone", id: "a" } } });

describe("the Security page", () => {
  it("shows the overview: version, lock, phones, servers, add-ons and keys", () => {
    const html = renderToStaticMarkup(<SecurityPageView report={report([])} busy={null} onFix={() => {}} onOpen={() => {}} />);
    expect(html).toContain("1.0.0");
    expect(html).toContain("Never"); // locks after
    expect(html).toContain("Pixel");
    expect(html).toContain("Used 3 days ago");
    expect(html).toContain("Never used");
    expect(html).toContain("Signed-in phones: 1");
    expect(html).toContain("port 7340");
    expect(html).toContain("this PC only");
    expect(html).toContain("local network");
    expect(html).toContain("Talks to mcp.example.com");
    expect(html).toContain("1 run unasked");
    expect(html).toContain("In use for 400 days");
    expect(html).toContain("finnhub.io");
    expect(html).toContain("No problems found.");
  });

  it("shows each finding with its values filled in, worst group first, and a button only where there is a fix", () => {
    const list = [
      finding({ id: "n", severity: "notice", open: "security" }),
      fixable,
      finding({ id: "c", severity: "critical", title: "{name} is open", detail: "Close it." }),
    ];
    const html = renderToStaticMarkup(<SecurityPageView report={report(list)} busy={null} onFix={() => {}} onOpen={() => {}} />);
    expect(html).toContain("Pixel is open");
    expect(html).toContain("Detail for Pixel");
    expect(html.indexOf("Pixel is open")).toBeLessThan(html.indexOf("Title of Pixel"));
    expect(html).toContain("Remove Pixel");
    expect(html).toContain("Show the setting");
    expect(html.match(/<button/g)).toHaveLength(2);
  });

  it("disables every fix button while one is working", () => {
    const html = renderToStaticMarkup(<SecurityPageView report={report([fixable, finding({ id: "x", fix: fixable.fix })])} busy="phone_stale:a" onFix={() => {}} onOpen={() => {}} />);
    expect(html.match(/disabled=""/g)).toHaveLength(2);
    expect(html).toContain("Working…");
  });

  it("groups findings by severity, worst first, leaving empty groups out", () => {
    const groups = groupFindings([finding({ id: "1", severity: "info" }), finding({ id: "2", severity: "critical" }), finding({ id: "3", severity: "info" })]);
    expect(groups.map((g) => [g.severity, g.items.length])).toEqual([["critical", 1], ["info", 2]]);
  });

  it("says how long ago a phone was used and how old a key is", () => {
    expect(lastUsed(null, NOW)).toBe("Never used");
    expect(lastUsed(NOW - 1000, NOW)).toBe("Used today");
    expect(lastUsed(NOW - DAY, NOW)).toBe("Used yesterday");
    expect(lastUsed(NOW - 5 * DAY, NOW)).toBe("Used 5 days ago");
    expect(keyAge(0)).toBe("First seen today");
    expect(keyAge(1)).toBe("In use for 1 day");
    expect(keyAge(30)).toBe("In use for 30 days");
  });
});

describe("applying a fix", () => {
  it("changes nothing when the person says no", async () => {
    const apply = vi.fn();
    const ask = vi.fn().mockResolvedValue(false);
    expect(await fixAfterAsking(fixable, ask, apply)).toBeNull();
    expect(ask).toHaveBeenCalledWith("Remove Pixel? You can pair it again afterwards.");
    expect(apply).not.toHaveBeenCalled();
  });

  it("applies exactly that finding, confirmed, after a yes", async () => {
    const next = report([]);
    const apply = vi.fn().mockResolvedValue(next);
    expect(await fixAfterAsking(fixable, async () => true, apply)).toBe(next);
    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply).toHaveBeenCalledWith("phone_stale:a", true);
  });

  it("never asks or applies for a finding with no fix", async () => {
    const apply = vi.fn();
    const ask = vi.fn().mockResolvedValue(true);
    expect(await fixAfterAsking(finding({ id: "x", open: "security" }), ask, apply)).toBeNull();
    expect(ask).not.toHaveBeenCalled();
    expect(apply).not.toHaveBeenCalled();
  });

  it("waits for the answer before applying", async () => {
    const order: string[] = [];
    const ask = () => new Promise<boolean>((done) => setTimeout(() => (order.push("answered"), done(true)), 5));
    const apply = async () => (order.push("applied"), report([]));
    await fixAfterAsking(fixable, ask, apply);
    expect(order).toEqual(["answered", "applied"]);
  });

  it("does not apply anything just by showing the page", () => {
    const apply = vi.fn();
    renderToStaticMarkup(<SecurityPageView report={report([fixable])} busy={null} onFix={apply} onOpen={apply} />);
    expect(apply).not.toHaveBeenCalled();
  });
});
