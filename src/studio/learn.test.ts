// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { applyBinding } from "./learn";
import { bindingValue, newEffect, newTrack, type Project } from "./model";
import { initPatch } from "./synth";

function project(): Project {
  const a = newTrack("Song", "audio", 0);
  a.effects = [newEffect("lowpass")];
  const b = newTrack("Keys", "midi", 1);
  b.synth = initPatch();
  return { id: "p", name: "P", bpm: 120, beats: 4, tracks: [a, b], loop: null, created_at: 0, updated_at: 0 };
}

describe("MIDI Learn", () => {
  it("puts a control's value on its slider's range", () => {
    expect(bindingValue({ cc: 1, channel: 0, target: "master", label: "", min: -30, max: 6 }, 127)).toBe(6);
    expect(bindingValue({ cc: 1, channel: 0, target: "master", label: "", min: -30, max: 6 }, 0)).toBe(-30);
    expect(bindingValue({ cc: 1, channel: 0, target: "x", label: "", min: 20, max: 20000, log: true }, 63.5)).toBeCloseTo(632.5, 0);
    expect(bindingValue({ cc: 1, channel: 0, target: "x", label: "", min: -3, max: 3, step: 1 }, 80)).toBe(1);
  });

  it("moves a track's volume, an effect's setting or a synth's", () => {
    const p = project();
    const [song, keys] = p.tracks;
    expect(applyBinding(p, `track:${song.id}:volume`, -12).tracks[0].volume).toBe(-12);
    const fx = applyBinding(p, `fx:${song.id}:${song.effects[0].id}:cutoff`, 500);
    expect(fx.tracks[0].effects[0].params.cutoff).toBe(500);
    const synth = applyBinding(p, `synth:${keys.id}:filter.cutoff`, 1200);
    expect(synth.tracks[1].synth?.filter.cutoff).toBe(1200);
    expect(applyBinding(p, `synth:${keys.id}:lfos.2.rate`, 7).tracks[1].synth?.lfos[2].rate).toBe(7);
    // The original isn't touched, and something gone is left alone.
    expect(p.tracks[1].synth?.filter.cutoff).toBe(initPatch().filter.cutoff);
    expect(applyBinding(p, "track:gone:volume", 3)).toBe(p);
  });
});
