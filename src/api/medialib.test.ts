// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { cueAt, readProgress } from "./medialib";

describe("cueAt", () => {
  it("finds the cue on screen", () => {
    const cues = [{ start: 1, end: 2, text: "a" }, { start: 2, end: 3, text: "b" }];
    expect(cueAt(cues, 2)?.text).toBe("b");
    expect(cueAt(cues, 5)).toBeUndefined();
  });
});

describe("readProgress", () => {
  it("counts by text length", () => {
    const book = { title: "", author: "", sections: [{ title: "", text: "aaaa" }, { title: "", text: "bbbb" }] };
    expect(readProgress(book, 1, 0.5)).toBe(75);
    expect(readProgress({ ...book, sections: [] }, 0, 0)).toBe(0);
  });
});
