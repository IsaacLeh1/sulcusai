// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { cut, fade, resampleSpeed, stretchSpeed } from "./audioEdit";

const RATE = 8000;
const tone = (hz: number, secs: number) => Float32Array.from({ length: RATE * secs }, (_, i) => 0.5 * Math.sin((2 * Math.PI * hz * i) / RATE));
// Its pitch, from how often it crosses zero in the middle second.
const pitch = (x: Float32Array) => {
  const mid = x.subarray(Math.floor(x.length / 2 - RATE / 2), Math.floor(x.length / 2 + RATE / 2));
  let n = 0;
  for (let i = 1; i < mid.length; i++) if (mid[i - 1] < 0 !== mid[i] < 0) n++;
  return n / 2;
};

describe("quick song edits", () => {
  it("fades the ends and leaves the middle", () => {
    const [x] = fade([Float32Array.from({ length: RATE * 4 }, () => 1)], RATE, 1, 2);
    expect(x[0]).toBe(0);
    expect(x[x.length - 1]).toBeLessThan(0.01);
    expect(x[RATE / 2]).toBeGreaterThan(0.6);
    expect(x[RATE * 1.5]).toBe(1);
  });

  it("takes a part out and joins the rest", () => {
    const src = Float32Array.from({ length: RATE * 10 }, (_, i) => i);
    const [x] = cut([src], RATE, 2, 5);
    expect(x.length).toBe(RATE * 7);
    expect(x[RATE]).toBe(RATE);
    expect(x[RATE * 3]).toBe(RATE * 6);
  });

  it("plays faster with the pitch up, as a record", () => {
    const [x] = resampleSpeed([tone(200, 4)], 1.25);
    expect(x.length).toBe(RATE * 4 / 1.25);
    expect(pitch(x)).toBeCloseTo(250, -1);
  });

  it("plays faster or slower with the pitch kept", () => {
    for (const factor of [0.8, 1.2]) {
      const [l, r] = stretchSpeed([tone(220, 6), tone(220, 6)], RATE, factor);
      expect(l.length).toBe(Math.floor((RATE * 6) / factor));
      expect(Math.abs(pitch(l) - 220)).toBeLessThan(6);
      expect(r.length).toBe(l.length);
      // Steady loudness, no gaps.
      const mid = l.subarray(RATE, l.length - RATE);
      const peak = mid.reduce((m, v) => Math.max(m, Math.abs(v)), 0);
      expect(peak).toBeGreaterThan(0.4);
      expect(peak).toBeLessThan(0.65);
    }
  });
});
