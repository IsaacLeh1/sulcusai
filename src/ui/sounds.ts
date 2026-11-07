// SPDX-License-Identifier: AGPL-3.0-only
import { uiPrefs } from "./prefs";

export type Chime = "done" | "error" | "note" | "join" | "leave" | "message" | "mention";

// Short tones made on the spot, so no sound files ship with the app.
const NOTES: Record<Chime, number[]> = {
  done: [660, 880],
  error: [330, 247],
  note: [587],
  join: [523, 784],
  leave: [784, 523],
  message: [698],
  mention: [880, 1047, 880],
};

let ctx: AudioContext | null = null;

export function chime(kind: Chime) {
  const p = uiPrefs();
  if (!p.sounds || p.volume <= 0) return;
  try {
    ctx ??= new AudioContext();
    let at = ctx.currentTime;
    for (const hz of NOTES[kind]) {
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();
      osc.frequency.value = hz;
      gain.gain.setValueAtTime(0.08 * p.volume, at);
      gain.gain.exponentialRampToValueAtTime(0.0001, at + 0.12);
      osc.connect(gain).connect(ctx.destination);
      osc.start(at);
      osc.stop(at + 0.13);
      at += 0.09;
    }
  } catch {
    // No audio device: sounds are a nicety.
  }
}
