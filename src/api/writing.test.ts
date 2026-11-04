import { describe, expect, it } from "vitest";
import { charsLeft, pieces } from "./writing";

describe("writing helpers", () => {
  it("counts characters, not UTF-16 units", () => {
    expect(charsLeft("héllo 👋", 10)).toBe(3);
    expect(charsLeft("x".repeat(12), 10)).toBe(-2);
  });

  it("splits text around issues using byte offsets", () => {
    // "café is is" : "is is" starts at byte 6 (é is two bytes).
    const p = pieces("café is is", [{ start: 6, end: 11, kind: "repeat", message: "", fix: "is" }]);
    expect(p.map((x) => x.text)).toEqual(["café ", "is is"]);
    expect(p[1].issue?.kind).toBe("repeat");
  });

  it("skips overlapping issues", () => {
    const p = pieces("a b c", [
      { start: 0, end: 3, kind: "x", message: "", fix: null },
      { start: 2, end: 5, kind: "y", message: "", fix: null },
    ]);
    expect(p.map((x) => x.text)).toEqual(["a b", " c"]);
  });
});
