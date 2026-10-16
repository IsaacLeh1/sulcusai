import { describe, expect, it } from "vitest";
import { distance, rank, words } from "./fuzzy";

const SETTINGS = ["Appearance", "Theme", "Language", "Lock with Windows Hello", "Auto-lock after", "Notifications", "Graphics card limit", "Sync with my other PCs", "Phone", "Cloud models", "API key", "Check for updates", "Crash reports", "Web search"];
const find = (q: string, syn: Record<string, string[]> = {}) => rank(SETTINGS, q, (s) => s, syn).map((m) => m.item);

describe("settings search", () => {
  it("counts typos, swaps and missing letters", () => {
    expect(distance("pasword", "password")).toBe(1);
    expect(distance("langauge", "language")).toBe(1);
    expect(distance("notifcations", "notifications")).toBe(1);
    expect(distance("abc", "xyzw")).toBeGreaterThan(2);
  });

  it("finds settings from part of a word, any case, and with typos", () => {
    expect(find("lang")[0]).toBe("Language");
    expect(find("LANGAUGE")[0]).toBe("Language");
    expect(find("notifcations")[0]).toBe("Notifications");
    expect(find("updtes")[0]).toBe("Check for updates");
    expect(find("crash")[0]).toBe("Crash reports");
  });

  it("finds settings by other words for them, and says what it corrected", () => {
    const syn = { gpu: ["graphics card"], password: ["lock", "hello"], key: ["api key"], dark: ["theme", "appearance"] };
    expect(find("gpu", syn)[0]).toBe("Graphics card limit");
    expect(find("password", syn)).toContain("Lock with Windows Hello");
    expect(find("dark mode", syn)[0]).toMatch(/Theme|Appearance/);
    const m = rank(SETTINGS, "langauge", (s) => s)[0];
    expect(m.corrected).toEqual([["langauge", "language"]]);
  });

  it("finds nothing for nonsense", () => {
    expect(find("zzzz")).toEqual([]);
    expect(words("  ")).toEqual([]);
  });
});
