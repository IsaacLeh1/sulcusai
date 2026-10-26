// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { decodeFrames, encodeFrames, framesFromSound, initPatch, PRESETS, SIZE, tableFor, TABLES, withSettings } from "./synth";

describe("wavetable synth", () => {
  it("keeps a wavetable's higher copies free of harmonics they can't hold", () => {
    const t = tableFor("basic")!;
    expect(t.frames).toBe(16);
    // The last frame is a square; its last copy (one harmonic) is a plain sine.
    const last = t.data.subarray((15 * 11 + 10) * SIZE, (15 * 11 + 11) * SIZE);
    const peak = Math.max(...Array.from(last, Math.abs));
    for (let i = 0; i < SIZE; i += 128) expect(last[i] / peak).toBeCloseTo(Math.sin((2 * Math.PI * i) / SIZE), 2);
    for (const id of Object.keys(TABLES)) expect(tableFor(id)!.data.every(Number.isFinite)).toBe(true);
  });

  it("makes a wavetable of single cycles from a held note", () => {
    const rate = 48000;
    const tone = Float32Array.from({ length: rate }, (_, i) => Math.sin((2 * Math.PI * 220 * i) / rate) + 0.3 * Math.sin((2 * Math.PI * 660 * i) / rate));
    const frames = framesFromSound(tone, rate);
    expect(frames.length).toBe(16);
    for (const f of frames) {
      let ups = 0;
      for (let i = 2; i < SIZE; i++) if (f[i - 1] <= 0 && f[i] > 0) ups++;
      expect(ups).toBe(0);
      expect(Math.max(...Array.from(f, Math.abs))).toBeCloseTo(1, 2);
    }
    // A file that already is a wavetable is taken as it is.
    const table = new Float32Array(SIZE * 4).map((_, i) => Math.sin((2 * Math.PI * i) / SIZE) * (1 + Math.floor(i / SIZE)) / 4);
    expect(framesFromSound(table, rate).length).toBe(4);
    const back = decodeFrames(encodeFrames(frames));
    expect(back.length).toBe(16);
    expect(back[3][100]).toBeCloseTo(frames[3][100], 3);
  });

  it("builds a sound from settings given by name, within their ranges", () => {
    const p = withSettings(PRESETS.find((x) => x.name === "Reese")!, { wavetable: "vocal", cutoff: 99999, unison: 9.4, mono: false, lfo_to: "cutoff", lfo_beats: 0.5, lfo_amount: 0.6, name: "Growl" });
    expect([p.osc1.table, p.filter.cutoff, p.osc1.unison, p.voice.mode, p.name]).toEqual(["vocal", 20000, 7, "poly", "Growl"]);
    expect(p.lfos[0]).toMatchObject({ sync: true, div: 0.5 });
    expect(p.matrix.filter((m) => m.source === "lfo1")).toEqual([{ source: "lfo1", target: "cutoff", amount: 0.6 }]);
    // Unknown names and values are left alone.
    const q = withSettings(initPatch(), { wavetable: "nope", attack: "slow" });
    expect(q).toEqual(initPatch());
  });
});
