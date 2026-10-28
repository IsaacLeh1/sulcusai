// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { BUILT_IN, matching, shortcuts, skillAsk, slug } from "./shortcuts";

const names = (xs: { name: string }[]) => xs.map((x) => x.name);

describe("the / menu", () => {
  it("offers saved prompts, then the commands, then the skills that are on", () => {
    const all = shortcuts([{ name: "standup", text: "Write my standup" }], [{ name: "Haiku Writer", description: "Answers as a haiku" }]);
    expect(names(all)).toEqual(["standup", ...BUILT_IN.map((b) => b.name), "haiku-writer"]);
    expect(all.at(-1)).toMatchObject({ skill: "Haiku Writer", hint: "Answers as a haiku" });
    // The skill goes when its plugin is turned off.
    expect(names(shortcuts([], []))).not.toContain("haiku-writer");
  });

  it("keeps a saved prompt's name over a command or skill of the same name", () => {
    const all = shortcuts([{ name: "plan", text: "Plan my week" }], [{ name: "plan", description: "x" }]);
    expect(all.filter((s) => s.name === "plan")).toEqual([{ name: "plan", hint: "Plan my week", prompt: "Plan my week" }]);
  });

  it("matches starts first, then insides, then letters in order or a typo", () => {
    const all = shortcuts([], [{ name: "go-fast", description: "" }]);
    expect(names(matching(all, "go")).slice(0, 2)).toEqual(["go-back", "go-fast"]);
    expect(names(matching(all, "back"))).toEqual(["go-back"]);
    expect(names(matching(all, "gb"))).toEqual(["go-back"]);
    expect(names(matching(all, "sumarize"))).toEqual(["summarize"]);
    expect(matching(all, "zzz")).toEqual([]);
    expect(matching(all, "")).toBe(all);
  });

  it("turns skill names into what's typed, and starts the message for one", () => {
    expect(slug("  Release Notes (v2) ")).toBe("release-notes-v2");
    expect(skillAsk("Haiku Writer")).toBe("Use the Haiku Writer skill for this: ");
  });
});
