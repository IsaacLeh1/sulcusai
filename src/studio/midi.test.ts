// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { BasicMIDI } from "spessasynth_core";
import { DRUMS, midiFile } from "./midi";

describe("studio MIDI", () => {
  it("writes notes a MIDI reader reads back at the same times", () => {
    const bytes = midiFile([{ pitch: 60, start: 0.5, end: 1.5, velocity: 90 }, { pitch: 64, start: 1, end: 2.25, velocity: 70 }], 25, "Keys");
    const midi = BasicMIDI.fromArrayBuffer(bytes.buffer as ArrayBuffer);
    expect(midi.duration).toBeCloseTo(2.25, 1);
    const ons = midi.tracks[0].events.filter((e) => (e.statusByte & 0xf0) === 0x90);
    expect(ons.map((e) => e.data[0])).toEqual([60, 64]);
  });

  it("puts drums on channel 10 without a program", () => {
    const bytes = midiFile([{ pitch: 36, start: 0, end: 0.1, velocity: 100 }], DRUMS);
    const midi = BasicMIDI.fromArrayBuffer(bytes.buffer as ArrayBuffer);
    const on = midi.tracks[0].events.find((e) => (e.statusByte & 0xf0) === 0x90)!;
    expect(on.statusByte & 0x0f).toBe(9);
    expect(midi.tracks[0].events.some((e) => (e.statusByte & 0xf0) === 0xc0)).toBe(false);
  });
});
