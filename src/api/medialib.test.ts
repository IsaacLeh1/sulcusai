// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { byMonth, clock, cueAt, dayStart, readProgress } from "./medialib";

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

describe("clock", () => {
  it("shows hours only when needed", () => {
    expect(clock(247)).toBe("4:07");
    expect(clock(3909)).toBe("1:05:09");
    expect(clock(-3)).toBe("0:00");
  });
});

describe("byMonth", () => {
  it("groups in order", () => {
    const at = (y: number, m: number) => new Date(y, m - 1, 15).getTime() / 1000;
    const g = byMonth([{ path: "a", name: "a", taken: at(2026, 11), size: 1 }, { path: "b", name: "b", taken: at(2026, 11), size: 1 }, { path: "c", name: "c", taken: at(2026, 9), size: 1 }]);
    expect(g.map(([k, l]) => [k, l.length])).toEqual([["2026-11", 2], ["2026-09", 1]]);
  });
});

describe("dayStart", () => {
  it("is local midnight", () => {
    const d = new Date(2026, 10, 4, 15, 30).getTime();
    expect(new Date(dayStart(d) * 1000).getHours()).toBe(0);
    expect(new Date(dayStart(d) * 1000).getDate()).toBe(4);
  });
});
