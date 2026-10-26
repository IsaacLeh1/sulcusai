// SPDX-License-Identifier: AGPL-3.0-only
// Quick edits to a song in the window, on its decoded samples (one array a
// channel): fades, a part cut out, a new speed with or without its pitch,
// and its formants moved.
import { fft } from "./studio/fft";

export type Channels = Float32Array<ArrayBuffer>[];

/** Fades the start in over `inS` seconds and the end out over `outS` (an
 * equal-power curve, so the fade sounds even). */
export function fade(chs: Channels, rate: number, inS: number, outS: number): Channels {
  return chs.map((src) => {
    const out = src.slice();
    const n = out.length;
    const a = Math.min(n, Math.round(inS * rate));
    const b = Math.min(n, Math.round(outS * rate));
    for (let i = 0; i < a; i++) out[i] *= Math.sin((Math.PI / 2) * (i / a));
    for (let i = 0; i < b; i++) out[n - 1 - i] *= Math.sin((Math.PI / 2) * (i / b));
    return out;
  });
}

/** The song with the part from `from` to `to` (seconds) taken out, the two
 * sides joined by a short crossfade centred on the cut, so it doesn't click
 * and the song is exactly that much shorter. */
export function cut(chs: Channels, rate: number, from: number, to: number, xfadeS = 0.06): Channels {
  return chs.map((src) => {
    const a = Math.max(0, Math.min(src.length, Math.round(from * rate)));
    const b = Math.max(a, Math.min(src.length, Math.round(to * rate)));
    const h = Math.min(Math.round((xfadeS * rate) / 2), a, src.length - b);
    const out = new Float32Array(src.length - (b - a));
    out.set(src.subarray(0, a - h), 0);
    for (let i = 0; i < 2 * h; i++) {
      const g = i / (2 * h);
      out[a - h + i] = src[a - h + i] * Math.cos((Math.PI / 2) * g) + src[b - h + i] * Math.sin((Math.PI / 2) * g);
    }
    out.set(src.subarray(b + h), a + h);
    return out;
  });
}

/** Faster or slower by `factor` (1.25 is a quarter faster), the pitch
 * moving with it, as a record played faster. */
export function resampleSpeed(chs: Channels, factor: number): Channels {
  return chs.map((src) => {
    const n = Math.floor(src.length / factor);
    const out = new Float32Array(n);
    for (let i = 0; i < n; i++) {
      const p = i * factor;
      const j = Math.floor(p);
      const f = p - j;
      out[i] = src[j] * (1 - f) + (src[j + 1] ?? src[j]) * f;
    }
    return out;
  });
}

/** The formants (a voice's character, where its vowels resonate) moved by
 * `semis` semitones with the pitch left where it is (Suno Studio's Formant):
 * each moment's spectral envelope, the cepstrum's first 1.25 ms, is
 * stretched, and the sound reshaped to the stretched one. */
export function formantShift(chs: Channels, rate: number, semis: number): Channels {
  if (!semis) return chs;
  const N = 1024;
  const H = N / 4;
  const k = Math.pow(2, semis / 12);
  const Q = Math.max(8, Math.round(rate * 0.00125));
  const win = Float64Array.from({ length: N }, (_, i) => 0.5 - 0.5 * Math.cos((2 * Math.PI * i) / N));
  const re = new Float64Array(N);
  const im = new Float64Array(N);
  const env = new Float64Array(N / 2 + 1);
  const cr = new Float64Array(N);
  const ci = new Float64Array(N);
  return chs.map((x) => {
    const len = x.length;
    const out = new Float64Array(len + 2 * N);
    const norm = new Float64Array(len + 2 * N);
    for (let start = -N; start < len; start += H) {
      for (let i = 0; i < N; i++) {
        const j = start + i;
        re[i] = (j >= 0 && j < len ? x[j] : 0) * win[i];
        im[i] = 0;
      }
      fft(re, im);
      // The envelope: the log spectrum's slow part (its low quefrencies).
      for (let i = 0; i < N; i++) {
        cr[i] = Math.log(Math.hypot(re[i], im[i]) + 1e-9);
        ci[i] = 0;
      }
      fft(cr, ci, true);
      for (let i = Q + 1; i < N - Q; i++) cr[i] = 0;
      ci.fill(0);
      fft(cr, ci);
      for (let i = 0; i <= N / 2; i++) env[i] = cr[i];
      for (let i = 0; i <= N / 2; i++) {
        // What the envelope is here once stretched, against what it was.
        const at = i / k;
        const a = Math.min(N / 2, Math.floor(at));
        const b = Math.min(N / 2, a + 1);
        const moved = env[a] + (env[b] - env[a]) * (at - a);
        const g = Math.exp(Math.max(-6, Math.min(6, moved - env[i])));
        re[i] *= g;
        im[i] *= g;
        if (i > 0 && i < N / 2) {
          re[N - i] = re[i];
          im[N - i] = -im[i];
        }
      }
      fft(re, im, true);
      for (let i = 0; i < N; i++) {
        out[start + N + i] += re[i] * win[i];
        norm[start + N + i] += win[i] * win[i];
      }
    }
    const y = new Float32Array(len);
    for (let i = 0; i < len; i++) y[i] = norm[i + N] > 1e-6 ? out[i + N] / norm[i + N] : 0;
    return y;
  });
}

/** Faster or slower by `factor` with the pitch kept (WSOLA: overlapping
 * pieces laid down at the new pace, each shifted a little to line up with
 * the last, so the sound doesn't warble). The channels move together. */
export function stretchSpeed(chs: Channels, rate: number, factor: number): Channels {
  const win = Math.round(rate * 0.06);
  const hop = Math.round(win / 2);
  const search = Math.round(rate * 0.012);
  const len = chs[0].length;
  const n = Math.floor(len / factor);
  const out = chs.map(() => new Float32Array(n + win));
  const weight = new Float32Array(n + win);
  const hann = Float32Array.from({ length: win }, (_, i) => 0.5 - 0.5 * Math.cos((2 * Math.PI * i) / win));
  const mono = chs.length > 1 ? chs[0].map((v, i) => (v + chs[1][i]) / 2) : chs[0];
  let prev = 0;
  for (let o = 0; o + win <= n + win && Math.round(o * factor) < len; o += hop) {
    // Where this piece would start, then the nearby start that best
    // continues what the last piece left off.
    const want = Math.round(o * factor);
    let best = want;
    if (o > 0) {
      const follow = prev + hop;
      let top = -Infinity;
      for (let d = -search; d <= search; d++) {
        const s = want + d;
        if (s < 0 || s + win > len) continue;
        let c = 0;
        for (let i = 0; i < hop; i += 2) c += mono[s + i] * (mono[follow + i] ?? 0);
        if (c > top) {
          top = c;
          best = s;
        }
      }
    }
    if (best + win > len) break;
    for (let c = 0; c < chs.length; c++) {
      const src = chs[c];
      const dst = out[c];
      for (let i = 0; i < win; i++) dst[o + i] += src[best + i] * hann[i];
    }
    for (let i = 0; i < win; i++) weight[o + i] += hann[i];
    prev = best;
  }
  return out.map((dst) => {
    const r = dst.subarray(0, n);
    for (let i = 0; i < n; i++) if (weight[i] > 1e-3) r[i] /= weight[i];
    return r;
  });
}
