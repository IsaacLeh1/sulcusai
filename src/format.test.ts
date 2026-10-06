// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { bytes, clock, duration, insertDictation, percent, placementLabel, speedLabel, tokens } from "./format";

describe("format", () => {
  it("formats sizes the way people read them", () => {
    expect(bytes(2_497_281_120)).toBe("2.3 GB");
    expect(bytes(12_510_212_576)).toBe("12 GB");
    expect(bytes(33_355_526)).toBe("32 MB");
  });

  it("formats token counts with separators", () => {
    expect(tokens(8192)).toBe("8,192");
  });

  it("clamps percentages", () => {
    expect(percent(5, 0)).toBe(0);
    expect(percent(50, 100)).toBe(50);
    expect(percent(150, 100)).toBe(100);
  });

  it("explains placement and speed in plain words", () => {
    expect(placementLabel("gpu")).toMatch(/graphics card/);
    expect(placementLabel(null)).toMatch(/Can't run/);
    expect(speedLabel(50)).toBe("Very fast");
    expect(speedLabel(5)).toBe("Slow");
  });
});

describe("meeting and dictation helpers", () => {
  it("formats clock times", () => {
    expect(clock(5)).toBe("0:05");
    expect(clock(725)).toBe("12:05");
    expect(clock(3729)).toBe("1:02:09");
  });

  it("inserts dictation with natural spacing", () => {
    expect(insertDictation("", 0, 0, "Hello there.")).toEqual({ value: "Hello there.", cursor: 12 });
    const r = insertDictation("Dear Sam,", 9, 9, "thanks for the notes.");
    expect(r.value).toBe("Dear Sam, thanks for the notes.");
    const mid = insertDictation("one three", 3, 3, "two");
    expect(mid.value).toBe("one two three");
    expect(mid.value.slice(0, mid.cursor)).toBe("one two");
  });

  it("describes durations", () => {
    expect(duration(30_000)).toBe("1 min");
    expect(duration(45 * 60_000)).toBe("45 min");
    expect(duration(65 * 60_000)).toBe("1 h 5 min");
  });
});
