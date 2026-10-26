// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { cut, fade, formantShift, resampleSpeed, stretchSpeed } from "./audioEdit";
import { fft } from "./studio/fft";

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

describe("formants", () => {
  // A voice-like sound: pulses at 150 Hz through one resonance at 1000 Hz.
  const rate = 44100;
  const vowel = () => {
    const x = new Float32Array(rate * 2);
    const r = 0.995;
    const w = (2 * Math.PI * 1000) / rate;
    let [y1, y2] = [0, 0];
    for (let i = 0; i < x.length; i++) {
      const pulse = i % Math.round(rate / 150) === 0 ? 1 : 0;
      const y = pulse + 2 * r * Math.cos(w) * y1 - r * r * y2;
      [y2, y1] = [y1, y];
      x[i] = y * 0.01;
    }
    return x;
  };
  // The loudest frequency between 300 Hz and 5 kHz in the middle of it.
  const loudest = (x: Float32Array) => {
    const n = 16384;
    const re = new Float64Array(n);
    const im = new Float64Array(n);
    const a = Math.floor(x.length / 2 - n / 2);
    for (let i = 0; i < n; i++) re[i] = x[a + i] * (0.5 - 0.5 * Math.cos((2 * Math.PI * i) / n));
    fft(re, im);
    let best = 0;
    let top = 0;
    for (let k = Math.floor((300 * n) / rate); k < (5000 * n) / rate; k++) {
      const m = Math.hypot(re[k], im[k]);
      if (m > top) [top, best] = [m, k];
    }
    return (best * rate) / n;
  };

  it("moves the resonance and leaves the pitch", () => {
    const x = vowel();
    const before = loudest(x);
    const up = loudest(formantShift([x], rate, 12)[0]);
    const down = loudest(formantShift([x], rate, -7)[0]);
    expect(before).toBeGreaterThan(900);
    expect(before).toBeLessThan(1100);
    expect(up).toBeGreaterThan(1750);
    expect(up).toBeLessThan(2250);
    expect(down).toBeGreaterThan(550);
    expect(down).toBeLessThan(800);
    // Still the 150 Hz voice: its loudest is one of its harmonics.
    for (const f of [before, up, down]) expect(Math.abs(f / 150 - Math.round(f / 150))).toBeLessThan(0.08);
    expect(formantShift([x], rate, 0)[0]).toBe(x);
  });
});
