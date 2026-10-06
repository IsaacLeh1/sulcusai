// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { bytes, percent, placementLabel, speedLabel, tokens } from "./format";

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
