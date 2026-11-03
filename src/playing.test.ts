// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { playing, trackAudioContexts } from "./playing";

describe("playing", () => {
  it("sees a playing media element and ignores paused or ended ones", () => {
    expect(playing([{ paused: true, ended: false }])).toBe(false);
    expect(playing([{ paused: false, ended: true }])).toBe(false);
    expect(playing([{ paused: true, ended: false }, { paused: false, ended: false }])).toBe(true);
  });

  it("sees a running audio context made after tracking started", () => {
    class FakeContext {
      state = "running";
    }
    const win = { AudioContext: FakeContext as unknown as typeof AudioContext };
    trackAudioContexts(win);
    const ctx = new win.AudioContext() as unknown as FakeContext;
    expect(playing([])).toBe(true);
    ctx.state = "suspended";
    expect(playing([])).toBe(false);
  });
});
