// SPDX-License-Identifier: AGPL-3.0-only
// Where the hits are in a stretch of sound (drum strikes, plucked or sung
// note starts), for placing warp markers on them: the rise in its spectrum
// from one moment to the next, peaks above the running level picked.
import { fft } from "./fft";

const FRAME = 1024;
const HOP = 512;

/** Seconds (into the buffer) of the hits between `from` and `to`, at least `gap` apart. */
export function onsets(buf: AudioBuffer, from: number, to: number, gap = 0.09): number[] {
  const rate = buf.sampleRate;
  const a = Math.max(0, Math.floor(from * rate));
  const b = Math.min(buf.length, Math.ceil(to * rate));
  const chs = Array.from({ length: Math.min(2, buf.numberOfChannels) }, (_, c) => buf.getChannelData(c));
  const window = Float64Array.from({ length: FRAME }, (_, i) => 0.5 - 0.5 * Math.cos((2 * Math.PI * i) / FRAME));
  const re = new Float64Array(FRAME);
  const im = new Float64Array(FRAME);
  let last = new Float64Array(FRAME / 2);
  const flux: number[] = [];
  for (let s = a; s + FRAME <= b; s += HOP) {
    for (let i = 0; i < FRAME; i++) {
      let v = 0;
      for (const ch of chs) v += ch[s + i];
      re[i] = (v / chs.length) * window[i];
      im[i] = 0;
    }
    fft(re, im);
    const mag = new Float64Array(FRAME / 2);
    let rise = 0;
    for (let k = 1; k < FRAME / 2; k++) {
      mag[k] = Math.log1p(10 * Math.hypot(re[k], im[k]));
      rise += Math.max(0, mag[k] - last[k]);
    }
    flux.push(rise);
    last = mag;
  }
  if (flux.length < 3) return [];
  // A peak counts when it's the highest near it and well above the level around it.
  const top = Math.max(...flux) || 1;
  const norm = flux.map((v) => v / top);
  const out: number[] = [];
  const near = 3;
  const around = 12;
  for (let i = 1; i < norm.length - 1; i++) {
    let peak = true;
    for (let j = Math.max(0, i - near); j <= Math.min(norm.length - 1, i + near); j++) {
      if (norm[j] > norm[i]) {
        peak = false;
        break;
      }
    }
    if (!peak) continue;
    let mean = 0;
    let n = 0;
    for (let j = Math.max(0, i - around); j <= Math.min(norm.length - 1, i + around); j++) {
      mean += norm[j];
      n++;
    }
    if (norm[i] < mean / n + 0.08 || norm[i] < 0.06) continue;
    const t = (a + i * HOP + HOP / 2) / rate;
    if (out.length && t - out[out.length - 1] < gap) continue;
    out.push(t);
  }
  return out;
}
