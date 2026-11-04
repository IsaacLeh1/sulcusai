// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { type CutWord, monoSamples, toggleWord } from "./create";

describe("create helpers", () => {
  it("toggles one word", () => {
    const w: CutWord[] = [{ text: "a", start: 0, end: 1 }, { text: "b", start: 1, end: 2 }];
    expect(toggleWord(w, 1)[1].removed).toBe(true);
    expect(toggleWord(toggleWord(w, 1), 1)[1].removed).toBe(false);
    expect(w[1].removed).toBeUndefined();
  });
  it("downmixes channels", () => {
    expect(monoSamples([new Float32Array([1, 0]), new Float32Array([0, 0])])).toEqual([0.5, 0]);
    expect(monoSamples([])).toEqual([]);
  });
});
