// SPDX-License-Identifier: AGPL-3.0-only
// The Studio's wavetable synth (Suno Studio's): what a sound is (a patch),
// the factory sounds, the wavetables (built in, or made from any sound), and
// the synth itself on the audio thread, for a MIDI track's clips and for
// playing it live.
import { tx } from "../i18n";
import { fft } from "./fft";
import { SYNTH_PROCESSOR } from "./synthProcessor";

export type WarpMode = "none" | "bend" | "sync" | "pwm" | "mirror";
export type LfoShape = "sine" | "triangle" | "square" | "saw" | "random";
export type FilterType = "lowpass" | "highpass" | "bandpass" | "notch";
export type ModSource = "env3" | "env4" | "lfo1" | "lfo2" | "lfo3" | "velocity" | "modwheel" | "note" | "random";
export type ModTarget = "pitch" | "osc1.pitch" | "osc2.pitch" | "osc1.pos" | "osc2.pos" | "osc1.warp" | "osc2.warp" | "osc1.level" | "osc2.level" | "sub.level" | "cutoff" | "resonance" | "volume" | "pan";

export interface Osc {
  on: boolean;
  /** A built-in wavetable's name, or a made one's id. */
  table: string;
  /** Where in the wavetable (0 to 1). */
  pos: number;
  warp: WarpMode;
  warpAmt: number;
  level: number;
  octave: number;
  semi: number;
  /** Cents. */
  fine: number;
  /** Voices stacked (1 to 7), spread apart in pitch by `detune` cents and across the stereo field by `spread`. */
  unison: number;
  detune: number;
  spread: number;
  pan: number;
}

/** Attack, decay and release in seconds; sustain 0 to 1. */
export interface Env {
  a: number;
  d: number;
  s: number;
  r: number;
}

export interface Lfo {
  shape: LfoShape;
  /** Cycles a second, or (synced to the tempo) beats a cycle. */
  rate: number;
  sync: boolean;
  div: number;
  /** Starts over with each note. */
  retrig: boolean;
}

export interface Route {
  source: ModSource;
  target: ModTarget;
  /** -1 to 1. */
  amount: number;
}

export interface SynthPatch {
  name: string;
  osc1: Osc;
  osc2: Osc;
  sub: { on: boolean; shape: "sine" | "square"; level: number; octave: number };
  noise: number;
  filter: { on: boolean; type: FilterType; cutoff: number; resonance: number; /** Octaves the filter envelope opens it. */ env: number; keyTrack: number; drive: number };
  amp: Env;
  fenv: Env;
  env3: Env;
  env4: Env;
  lfos: [Lfo, Lfo, Lfo];
  matrix: Route[];
  voice: { mode: "poly" | "mono"; legato: boolean; glide: number; octave: number; semi: number; fine: number; /** How much softer quiet notes are (0 to 1). */ velocity: number; voices: number };
  level: number;
}

/** A wavetable made from a sound, kept with the project: frames of 2048 samples as 16-bit numbers in base64. */
export interface MadeTable {
  name: string;
  frames: string;
}

export const SIZE = 2048;
const LEVELS = 11;
const HARMONICS = SIZE / 2;

// ---------- the built-in wavetables ----------

/** A frame's harmonics: cosine and sine amounts of each (index 1 up). */
type Spectrum = { c: Float64Array; s: Float64Array };

function spectrum(f: (h: number) => [number, number] | number, upTo = HARMONICS - 1): Spectrum {
  const c = new Float64Array(HARMONICS);
  const s = new Float64Array(HARMONICS);
  for (let h = 1; h <= upTo; h++) {
    const v = f(h);
    if (typeof v === "number") s[h] = v;
    else [c[h], s[h]] = v;
  }
  return { c, s };
}

function fromSamples(x: Float64Array | Float32Array): Spectrum {
  const re = Float64Array.from(x);
  const im = new Float64Array(SIZE);
  fft(re, im);
  const c = new Float64Array(HARMONICS);
  const s = new Float64Array(HARMONICS);
  for (let h = 1; h < HARMONICS; h++) {
    c[h] = (2 * re[h]) / SIZE;
    s[h] = (-2 * im[h]) / SIZE;
  }
  return { c, s };
}

function samples(f: (x: number) => number): Float64Array {
  return Float64Array.from({ length: SIZE }, (_, i) => f(i / SIZE));
}

function mix(a: Spectrum, b: Spectrum, k: number): Spectrum {
  return { c: a.c.map((v, i) => v + (b.c[i] - v) * k), s: a.s.map((v, i) => v + (b.s[i] - v) * k) };
}

/** `count` frames moving through `keys` evenly. */
function morph(keys: Spectrum[], count = 16): Spectrum[] {
  return Array.from({ length: count }, (_, i) => {
    const x = (i / (count - 1)) * (keys.length - 1);
    const k = Math.min(keys.length - 2, Math.floor(x));
    return mix(keys[k], keys[k + 1], x - k);
  });
}

const sine = () => spectrum((h) => (h === 1 ? 1 : 0));
const saw = (bright = Infinity) => spectrum((h) => ((2 / Math.PI) * (h % 2 ? 1 : -1)) / h / (1 + Math.pow(h / bright, 4)));
const square = () => spectrum((h) => (h % 2 ? 4 / Math.PI / h : 0));
const triangle = () => spectrum((h) => (h % 2 ? ((8 / (Math.PI * Math.PI)) * (((h - 1) / 2) % 2 ? -1 : 1)) / (h * h) : 0));

// Vowels' formants (Hz), sung around 130 Hz.
const VOWELS: [number, number, number][] = [[800, 1150, 2900], [400, 1600, 2700], [350, 2000, 2800], [450, 800, 2830], [325, 700, 2530]];
function vowel(f: [number, number, number]) {
  const f0 = 130;
  return spectrum((h) => {
    const hz = h * f0;
    if (hz > 6000) return 0;
    const peak = f.reduce((sum, F, i) => sum + Math.exp(-Math.pow((hz - F) / (80 + 40 * i), 2)) / (1 + i), 0);
    return (0.08 + peak) / Math.sqrt(h);
  });
}

export const TABLES: Record<string, { label: string; frames: () => Spectrum[] }> = {
  basic: { label: tx("Basic shapes"), frames: () => morph([sine(), triangle(), saw(), square()]) },
  pulse: { label: tx("Pulse width"), frames: () => Array.from({ length: 16 }, (_, i) => {
    const w = 0.5 - (0.45 * i) / 15;
    return spectrum((h) => [(2 / (h * Math.PI)) * Math.sin(Math.PI * h * w), 0]);
  }) },
  analog: { label: tx("Analog sweep"), frames: () => Array.from({ length: 16 }, (_, i) => saw(1.5 * Math.pow(60, i / 15))) },
  vocal: { label: tx("Vocal"), frames: () => morph(VOWELS.map(vowel)) },
  organ: { label: tx("Organ"), frames: () => morph([[1, 1, 0, 0, 0, 0, 0, 0], [1, 1, 1, 0, 0.5, 0, 0, 0], [1, 0.8, 1, 0.8, 0.6, 0.5, 0, 0.4], [1, 1, 1, 1, 1, 1, 1, 1]].map((bars) => spectrum((h) => {
    const at = [1, 2, 3, 4, 6, 8, 10, 12].indexOf(h);
    return at >= 0 ? bars[at] * 0.5 : 0;
  }))) },
  bell: { label: tx("Bell (FM)"), frames: () => Array.from({ length: 16 }, (_, i) => fromSamples(samples((x) => Math.sin(2 * Math.PI * x + ((8 * i) / 15) * Math.sin(2 * Math.PI * 3 * x))))) },
  sync: { label: tx("Sync"), frames: () => Array.from({ length: 16 }, (_, i) => {
    const r = 1 + (7 * i) / 15;
    return fromSamples(samples((x) => {
      const y = x * r;
      return 2 * (y - Math.floor(y)) - 1;
    }));
  }) },
  bits: { label: tx("Bit crushed"), frames: () => Array.from({ length: 16 }, (_, i) => {
    const steps = Math.round(Math.pow(2, 6 - (5 * i) / 15));
    return fromSamples(samples((x) => Math.round(Math.sin(2 * Math.PI * x) * steps) / steps));
  }) },
  harmonics: { label: tx("Harmonics"), frames: () => Array.from({ length: 16 }, (_, i) => spectrum((h) => (h <= i + 1 ? 1 / Math.sqrt(h) / Math.sqrt(i + 1) : 0))) },
  phase: { label: tx("Phase distortion"), frames: () => Array.from({ length: 16 }, (_, i) => {
    const d = 0.5 - (0.48 * i) / 15;
    return fromSamples(samples((x) => -Math.cos(Math.PI * (x < d ? x / d : 1 + (x - d) / (1 - d)))));
  }) },
};

// ---------- band-limited copies ----------

/** A wavetable ready for the synth: each frame at 11 brightnesses (up to
 * 1024 harmonics, then half as many each), normalized to a peak of 1. */
export interface Mipmapped {
  frames: number;
  data: Float32Array;
}

function mipmap(frames: Spectrum[]): Mipmapped {
  const data = new Float32Array(frames.length * LEVELS * SIZE);
  let peak = 0;
  const re = new Float64Array(SIZE);
  const im = new Float64Array(SIZE);
  frames.forEach((f, fi) => {
    for (let level = 0; level < LEVELS; level++) {
      const top = Math.max(1, (HARMONICS >> level) - (level === 0 ? 1 : 0));
      re.fill(0);
      im.fill(0);
      for (let h = 1; h <= top && h < HARMONICS; h++) {
        // x(t) = sum c cos + s sin, as a spectrum to transform back.
        re[h] = (f.c[h] * SIZE) / 2;
        im[h] = (-f.s[h] * SIZE) / 2;
        re[SIZE - h] = re[h];
        im[SIZE - h] = -im[h];
      }
      fft(re, im, true);
      const at = (fi * LEVELS + level) * SIZE;
      for (let i = 0; i < SIZE; i++) {
        data[at + i] = re[i];
        if (level === 0) peak = Math.max(peak, Math.abs(re[i]));
      }
    }
  });
  if (peak > 0) for (let i = 0; i < data.length; i++) data[i] /= peak;
  return { frames: frames.length, data };
}

const ready = new Map<string, Mipmapped>();

/** The table `id` ready for the synth: built in, or made (from `made`). */
export function tableFor(id: string, made?: Record<string, MadeTable>): Mipmapped | null {
  let t = ready.get(id);
  if (t) return t;
  if (TABLES[id]) t = mipmap(TABLES[id].frames());
  else if (made?.[id]) t = mipmap(decodeFrames(made[id].frames).map(fromSamples));
  else return null;
  ready.set(id, t);
  return t;
}

/** One frame of a table at `pos`, for drawing (its fullest copy). */
export function frameOf(id: string, pos: number, made?: Record<string, MadeTable>): Float32Array | null {
  const t = tableFor(id, made);
  if (!t) return null;
  const f = Math.round(pos * (t.frames - 1));
  return t.data.subarray(f * LEVELS * SIZE, f * LEVELS * SIZE + SIZE);
}

// ---------- tables made from a sound ----------

export function encodeFrames(frames: Float32Array[]): string {
  const out = new Int16Array(frames.length * SIZE);
  frames.forEach((f, i) => {
    for (let j = 0; j < SIZE; j++) out[i * SIZE + j] = Math.max(-32767, Math.min(32767, Math.round(f[j] * 32767)));
  });
  const bytes = new Uint8Array(out.buffer);
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

export function decodeFrames(b64: string): Float32Array[] {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  const ints = new Int16Array(bytes.buffer, 0, Math.floor(bytes.length / 2));
  const count = Math.floor(ints.length / SIZE);
  return Array.from({ length: count }, (_, i) => Float32Array.from(ints.subarray(i * SIZE, (i + 1) * SIZE), (v) => v / 32767));
}

/** The length of one cycle near sample `at` (in samples, a fraction allowed),
 * or null when the sound there doesn't repeat. */
function periodAt(x: Float32Array, at: number, rate: number): number | null {
  const win = 4096;
  if (x.length < win + 2) return null;
  const a = Math.max(0, Math.min(x.length - win - 1, at));
  const lo = Math.floor(rate / 1500);
  const hi = Math.min(win / 2, Math.ceil(rate / 45));
  let e0 = 0;
  for (let i = 0; i < win / 2; i++) e0 += x[a + i] * x[a + i];
  if (e0 < 1e-6) return null;
  const r = new Float64Array(hi + 2);
  for (let lag = lo; lag <= hi + 1; lag++) {
    let s = 0;
    let e1 = 0;
    for (let i = 0; i < win / 2; i++) {
      s += x[a + i] * x[a + i + lag];
      e1 += x[a + i + lag] * x[a + i + lag];
    }
    r[lag] = s / Math.sqrt(e0 * e1 + 1e-12);
  }
  let best = -1;
  for (let lag = lo + 1; lag <= hi; lag++) if (r[lag] > r[lag - 1] && r[lag] >= r[lag + 1] && (best < 0 || r[lag] > r[best])) best = lag;
  if (best < 0 || r[best] < 0.5) return null;
  // The shortest lag nearly as good (not a multiple of the cycle).
  for (let lag = lo + 1; lag < best; lag++) {
    if (r[lag] > r[lag - 1] && r[lag] >= r[lag + 1] && r[lag] > 0.9 * r[best]) {
      best = lag;
      break;
    }
  }
  const [y0, y1, y2] = [r[best - 1], r[best], r[best + 1]];
  const d = y0 - 2 * y1 + y2;
  return best + (d !== 0 ? (0.5 * (y0 - y2)) / d : 0);
}

/** A wavetable from a sound (Suno's capture or upload): a file that already
 * is one (a whole number of 2048-sample frames) as it is; anything else, a
 * single cycle taken at `count` places through it, each stretched to a frame. */
export function framesFromSound(mono: Float32Array, rate: number, count = 16): Float32Array[] {
  if (mono.length >= SIZE && mono.length % SIZE === 0 && mono.length / SIZE <= 256) {
    const n = mono.length / SIZE;
    const pick = Math.min(n, 64);
    return Array.from({ length: pick }, (_, i) => {
      const f = Math.round((i * (n - 1)) / Math.max(1, pick - 1));
      return mono.slice(f * SIZE, (f + 1) * SIZE);
    });
  }
  // Frames from where it's heard (a tenth of its loudest or more), spread evenly.
  const loud: number[] = [];
  const level = (w: number) => {
    let s = 0;
    for (let i = w * SIZE; i < Math.min(mono.length, (w + 1) * SIZE); i++) s += mono[i] * mono[i];
    return Math.sqrt(s / SIZE);
  };
  const levels = Array.from({ length: Math.max(1, Math.floor(mono.length / SIZE)) }, (_, w) => level(w));
  const top = Math.max(...levels);
  levels.forEach((l, w) => l >= top * 0.1 && loud.push(w * SIZE));
  if (!loud.length) loud.push(0);
  const frames: Float32Array[] = [];
  for (let k = 0; k < count; k++) {
    const at = loud[Math.round((k * (loud.length - 1)) / Math.max(1, count - 1))];
    const period = periodAt(mono, at, rate) ?? Math.min(SIZE, mono.length - 1);
    // Start on a rising zero crossing, so each frame lines up with the last.
    let z = at;
    for (let i = at; i < Math.min(mono.length - period - 2, at + period); i++) {
      if (mono[i] <= 0 && mono[i + 1] > 0) {
        z = i;
        break;
      }
    }
    if (z + period + 2 >= mono.length) z = Math.max(0, mono.length - period - 2);
    // Exactly where it crosses, between two samples.
    const rise = mono[z + 1] - mono[z];
    const start = z + (mono[z] <= 0 && rise > 0 ? Math.min(1, -mono[z] / rise) : 0);
    const f = new Float32Array(SIZE);
    let mean = 0;
    for (let j = 0; j < SIZE; j++) {
      const p = start + (j * period) / SIZE;
      const i = Math.floor(p);
      f[j] = (mono[i] ?? 0) + ((mono[i + 1] ?? 0) - (mono[i] ?? 0)) * (p - i);
      mean += f[j];
    }
    mean /= SIZE;
    let peak = 0;
    for (let j = 0; j < SIZE; j++) peak = Math.max(peak, Math.abs((f[j] -= mean)));
    if (peak > 1e-4) for (let j = 0; j < SIZE; j++) f[j] /= peak;
    frames.push(f);
  }
  return frames;
}

// ---------- sounds ----------

const env = (a: number, d: number, s: number, r: number): Env => ({ a, d, s, r });
const osc = (o: Partial<Osc> = {}): Osc => ({ on: true, table: "basic", pos: 0.66, warp: "none", warpAmt: 0, level: 0.8, octave: 0, semi: 0, fine: 0, unison: 1, detune: 0, spread: 0, pan: 0, ...o });
const lfo = (o: Partial<Lfo> = {}): Lfo => ({ shape: "sine", rate: 2, sync: false, div: 1, retrig: false, ...o });

/** A plain saw through an open filter (Suno's Init). */
export function initPatch(): SynthPatch {
  return {
    name: "Init",
    osc1: osc(),
    osc2: osc({ on: false, table: "pulse", pos: 0, level: 0.5 }),
    sub: { on: false, shape: "sine", level: 0.5, octave: -1 },
    noise: 0,
    filter: { on: true, type: "lowpass", cutoff: 8000, resonance: 0.1, env: 0, keyTrack: 0, drive: 0 },
    amp: env(0.005, 0.3, 0.8, 0.25),
    fenv: env(0.005, 0.4, 0.3, 0.3),
    env3: env(0, 0.15, 0, 0.1),
    env4: env(0.5, 1, 0.5, 0.5),
    lfos: [lfo(), lfo({ rate: 0.3 }), lfo({ shape: "triangle", rate: 5 })],
    matrix: [],
    voice: { mode: "poly", legato: false, glide: 0, octave: 0, semi: 0, fine: 0, velocity: 0.5, voices: 8 },
    level: 0.5,
  };
}

type Change = (p: SynthPatch) => void;
const preset = (name: string, change: Change): SynthPatch => {
  const p = initPatch();
  change(p);
  p.name = name;
  return p;
};

/** The factory sounds (Suno's: 808, Reese, acid, pads, plucks, retro game, and more). */
export const PRESETS: SynthPatch[] = [
  initPatch(),
  preset("808", (p) => {
    Object.assign(p.osc1, { table: "basic", pos: 0, level: 1 });
    p.filter = { ...p.filter, cutoff: 900, drive: 0.35 };
    p.amp = env(0.002, 1.6, 0, 0.4);
    p.env3 = env(0, 0.06, 0, 0.05);
    p.matrix = [{ source: "env3", target: "pitch", amount: 0.5 }];
    p.voice = { ...p.voice, mode: "mono", glide: 0.06, octave: -1 };
    p.level = 0.8;
  }),
  preset("Reese", (p) => {
    Object.assign(p.osc1, { table: "basic", pos: 0.66, unison: 4, detune: 22, spread: 0.6 });
    Object.assign(p.osc2, { on: true, table: "basic", pos: 0.66, level: 0.7, fine: 9, unison: 2, detune: 15 });
    p.filter = { ...p.filter, cutoff: 700, resonance: 0.2, drive: 0.25 };
    p.amp = env(0.005, 0.5, 0.9, 0.2);
    p.lfos[0] = lfo({ rate: 0.2 });
    p.matrix = [{ source: "lfo1", target: "cutoff", amount: 0.12 }];
    p.voice = { ...p.voice, mode: "mono", legato: true, octave: -1 };
  }),
  preset("Acid", (p) => {
    Object.assign(p.osc1, { table: "basic", pos: 0.66 });
    p.filter = { ...p.filter, cutoff: 280, resonance: 0.8, env: 4.5, drive: 0.45 };
    p.fenv = env(0.002, 0.22, 0, 0.12);
    p.amp = env(0.002, 0.3, 0.7, 0.08);
    p.voice = { ...p.voice, mode: "mono", legato: true, glide: 0.08, velocity: 0.8 };
  }),
  preset("Warm pad", (p) => {
    Object.assign(p.osc1, { table: "basic", pos: 0.4, unison: 5, detune: 18, spread: 0.8 });
    Object.assign(p.osc2, { on: true, table: "vocal", pos: 0.3, level: 0.35, octave: 1, unison: 3, detune: 12, spread: 0.6 });
    p.filter = { ...p.filter, cutoff: 2200, resonance: 0.15 };
    p.amp = env(0.9, 1.5, 0.85, 1.8);
    p.lfos[0] = lfo({ rate: 0.15 });
    p.matrix = [{ source: "lfo1", target: "osc1.pos", amount: 0.25 }, { source: "lfo1", target: "osc2.pos", amount: 0.4 }];
    p.level = 0.4;
  }),
  preset("Glass pad", (p) => {
    Object.assign(p.osc1, { table: "bell", pos: 0.25, unison: 3, detune: 10, spread: 0.7 });
    Object.assign(p.osc2, { on: true, table: "harmonics", pos: 0.5, level: 0.4, octave: 1 });
    p.filter = { ...p.filter, cutoff: 5000 };
    p.amp = env(0.6, 2, 0.7, 2.5);
    p.lfos[1] = lfo({ rate: 0.1, shape: "triangle" });
    p.matrix = [{ source: "lfo2", target: "osc1.pos", amount: 0.3 }];
    p.level = 0.4;
  }),
  preset("Pluck", (p) => {
    Object.assign(p.osc1, { table: "basic", pos: 0.66, unison: 2, detune: 8, spread: 0.4 });
    p.filter = { ...p.filter, cutoff: 300, resonance: 0.3, env: 5 };
    p.fenv = env(0.001, 0.25, 0, 0.2);
    p.amp = env(0.001, 0.45, 0, 0.3);
  }),
  preset("Bell pluck", (p) => {
    Object.assign(p.osc1, { table: "bell", pos: 0.6 });
    p.filter = { ...p.filter, cutoff: 9000 };
    p.amp = env(0.001, 1.2, 0, 1);
    p.env3 = env(0, 0.6, 0, 0.5);
    p.matrix = [{ source: "env3", target: "osc1.pos", amount: 0.4 }];
  }),
  preset("Retro game", (p) => {
    Object.assign(p.osc1, { table: "pulse", pos: 0.3 });
    p.filter = { ...p.filter, on: false };
    p.amp = env(0.001, 0.1, 0.6, 0.05);
    p.lfos[2] = lfo({ shape: "triangle", rate: 6, retrig: true });
    p.matrix = [{ source: "lfo3", target: "pitch", amount: 0.008 }];
    p.voice = { ...p.voice, velocity: 0 };
  }),
  preset("Supersaw lead", (p) => {
    Object.assign(p.osc1, { table: "basic", pos: 0.66, unison: 7, detune: 30, spread: 1 });
    p.filter = { ...p.filter, cutoff: 6000, resonance: 0.1 };
    p.amp = env(0.01, 0.4, 0.85, 0.3);
    p.voice = { ...p.voice, glide: 0.03 };
  }),
  preset("Sub bass", (p) => {
    Object.assign(p.osc1, { table: "basic", pos: 0.2, level: 0.6 });
    p.sub = { on: true, shape: "sine", level: 0.9, octave: -1 };
    p.filter = { ...p.filter, cutoff: 400 };
    p.amp = env(0.003, 0.3, 0.9, 0.12);
    p.voice = { ...p.voice, mode: "mono", legato: true };
  }),
  preset("Wobble bass", (p) => {
    Object.assign(p.osc1, { table: "basic", pos: 0.8, unison: 3, detune: 12 });
    p.sub = { on: true, shape: "sine", level: 0.6, octave: -1 };
    p.filter = { ...p.filter, cutoff: 250, resonance: 0.55, drive: 0.4 };
    p.lfos[0] = lfo({ sync: true, div: 0.5, shape: "sine", retrig: true });
    p.matrix = [{ source: "lfo1", target: "cutoff", amount: 0.5 }];
    p.voice = { ...p.voice, mode: "mono", octave: -1 };
  }),
  preset("Brass", (p) => {
    Object.assign(p.osc1, { table: "basic", pos: 0.66, unison: 3, detune: 7, spread: 0.3 });
    p.filter = { ...p.filter, cutoff: 600, env: 3, resonance: 0.1 };
    p.fenv = env(0.06, 0.4, 0.5, 0.2);
    p.amp = env(0.04, 0.3, 0.85, 0.2);
  }),
  preset("Strings", (p) => {
    Object.assign(p.osc1, { table: "basic", pos: 0.66, unison: 5, detune: 14, spread: 0.9 });
    p.filter = { ...p.filter, cutoff: 3500 };
    p.amp = env(0.35, 0.5, 0.9, 0.8);
    p.lfos[2] = lfo({ shape: "sine", rate: 5.5 });
    p.matrix = [{ source: "lfo3", target: "pitch", amount: 0.004 }];
    p.level = 0.4;
  }),
  preset("Vocal pad", (p) => {
    Object.assign(p.osc1, { table: "vocal", pos: 0, unison: 4, detune: 12, spread: 0.8 });
    p.filter = { ...p.filter, cutoff: 5000 };
    p.amp = env(0.5, 1, 0.8, 1.5);
    p.lfos[0] = lfo({ rate: 0.2, shape: "triangle" });
    p.matrix = [{ source: "lfo1", target: "osc1.pos", amount: 0.5 }];
    p.level = 0.45;
  }),
  preset("Organ", (p) => {
    Object.assign(p.osc1, { table: "organ", pos: 0.5 });
    p.filter = { ...p.filter, on: false };
    p.amp = env(0.005, 0.1, 1, 0.08);
    p.lfos[2] = lfo({ rate: 6.5 });
    p.matrix = [{ source: "lfo3", target: "volume", amount: 0.12 }];
    p.voice = { ...p.voice, velocity: 0 };
  }),
  preset("Keys", (p) => {
    Object.assign(p.osc1, { table: "bell", pos: 0.15 });
    Object.assign(p.osc2, { on: true, table: "basic", pos: 0.2, level: 0.5 });
    p.filter = { ...p.filter, cutoff: 3000, env: 1.5 };
    p.fenv = env(0.002, 0.8, 0.2, 0.4);
    p.amp = env(0.002, 1.5, 0.3, 0.5);
    p.voice = { ...p.voice, velocity: 0.8 };
  }),
];

/** A sound with settings the chat model gave by name (the studio's chat bar builds sounds from words this way). */
export function withSettings(base: SynthPatch, s: Record<string, unknown>): SynthPatch {
  const p = structuredClone(base);
  const num = (k: string, lo: number, hi: number): number | null => {
    const v = s[k];
    return v === undefined || v === null || !Number.isFinite(Number(v)) ? null : Math.max(lo, Math.min(hi, Number(v)));
  };
  const set = (k: string, lo: number, hi: number, f: (v: number) => void) => {
    const v = num(k, lo, hi);
    if (v !== null) f(v);
  };
  const table = String(s.wavetable ?? s.table ?? "");
  if (TABLES[table]) p.osc1.table = table;
  set("position", 0, 1, (v) => (p.osc1.pos = v));
  set("unison", 1, 7, (v) => (p.osc1.unison = Math.round(v)));
  set("detune", 0, 100, (v) => (p.osc1.detune = v));
  set("spread", 0, 1, (v) => (p.osc1.spread = v));
  const osc2 = String(s.osc2 ?? "");
  if (TABLES[osc2]) Object.assign(p.osc2, { on: true, table: osc2 });
  else if (s.osc2 === true) p.osc2.on = true;
  else if (s.osc2 === false) p.osc2.on = false;
  set("osc2_octave", -3, 3, (v) => Object.assign(p.osc2, { on: true, octave: Math.round(v) }));
  set("osc2_semitones", -12, 12, (v) => Object.assign(p.osc2, { on: true, semi: Math.round(v) }));
  set("sub", 0, 1, (v) => (p.sub = { ...p.sub, on: v > 0, level: v }));
  set("noise", 0, 1, (v) => (p.noise = v));
  const ft = String(s.filter_type ?? "");
  if (["lowpass", "highpass", "bandpass", "notch"].includes(ft)) p.filter.type = ft as FilterType;
  set("cutoff", 20, 20000, (v) => Object.assign(p.filter, { on: true, cutoff: v }));
  set("resonance", 0, 1, (v) => (p.filter.resonance = v));
  set("filter_env", -6, 6, (v) => (p.filter.env = v));
  set("drive", 0, 1, (v) => (p.filter.drive = v));
  set("attack", 0, 10, (v) => (p.amp.a = v));
  set("decay", 0, 10, (v) => (p.amp.d = v));
  set("sustain", 0, 1, (v) => (p.amp.s = v));
  set("release", 0, 10, (v) => (p.amp.r = v));
  set("glide", 0, 2, (v) => (p.voice.glide = v));
  set("octave", -3, 3, (v) => (p.voice.octave = Math.round(v)));
  if (typeof s.mono === "boolean") p.voice.mode = s.mono ? "mono" : "poly";
  if (typeof s.legato === "boolean") p.voice.legato = s.legato;
  const to: Record<string, ModTarget> = { position: "osc1.pos", cutoff: "cutoff", pitch: "pitch", volume: "volume", pan: "pan", warp: "osc1.warp" };
  const target = to[String(s.lfo_to ?? "")];
  if (target) {
    set("lfo_rate", 0.02, 40, (v) => Object.assign(p.lfos[0], { rate: v, sync: false }));
    const beats = num("lfo_beats", 0.125, 16);
    if (beats !== null) Object.assign(p.lfos[0], { sync: true, div: beats });
    p.matrix = [...p.matrix.filter((m) => m.source !== "lfo1"), { source: "lfo1", target, amount: num("lfo_amount", -1, 1) ?? 0.3 }];
  }
  if (typeof s.name === "string" && s.name.trim()) p.name = s.name.trim().slice(0, 40);
  return p;
}

/** How long a patch rings on after its last note ends. */
export function tailOf(p: SynthPatch) {
  return Math.min(8, p.amp.r * 1.2 + 0.05);
}

// ---------- the synth on the audio thread ----------

const workletUrl = URL.createObjectURL(new Blob([SYNTH_PROCESSOR], { type: "text/javascript" }));
const loaded = new WeakMap<BaseAudioContext, Promise<void>>();
const sent = new WeakMap<BaseAudioContext, Set<string>>();

/** Readies the synth on a context (once). */
export function prepareSynth(ctx: BaseAudioContext): Promise<void> {
  let p = loaded.get(ctx);
  if (!p) {
    p = ctx.audioWorklet.addModule(workletUrl);
    loaded.set(ctx, p);
  }
  return p;
}

/** The wavetables a patch plays that `ctx`'s synths don't have yet (they share them once given). */
function newTables(ctx: BaseAudioContext, patch: SynthPatch, made?: Record<string, MadeTable>) {
  let have = sent.get(ctx);
  if (!have) sent.set(ctx, (have = new Set()));
  const out: Record<string, Mipmapped> = {};
  for (const id of [patch.osc1.table, patch.osc2.table]) {
    if (have.has(id)) continue;
    const t = tableFor(id, made);
    if (t) {
      out[id] = t;
      have.add(id);
    }
  }
  return out;
}

/** A note to play: `t` the context time, `k` 1 to start it and 0 to let it go. */
export interface NoteEvent {
  t: number;
  k: 0 | 1;
  n: number;
  v: number;
}

/** A synth playing `events`, then letting itself go once they're over (`endAt`, a context time). */
export function synthNode(ctx: BaseAudioContext, patch: SynthPatch, events: NoteEvent[], bpm: number, endAt: number, made?: Record<string, MadeTable>): AudioWorkletNode {
  return new AudioWorkletNode(ctx, "sulcusai-wavesynth", {
    numberOfInputs: 0,
    numberOfOutputs: 1,
    outputChannelCount: [2],
    processorOptions: { patch, tables: newTables(ctx, patch, made), events, bpm, endAt },
  });
}

/** A patch changed while it plays: the synths playing it hear it at once. */
export function updateSynth(node: AudioWorkletNode, patch: SynthPatch, made?: Record<string, MadeTable>) {
  const tables = newTables(node.context, patch, made);
  if (Object.keys(tables).length) node.port.postMessage({ type: "tables", tables });
  node.port.postMessage({ type: "patch", patch });
}

/** The synth played live (musical typing, a MIDI keyboard, notes tapped in the piano roll). */
export class LiveWave {
  private ctx = new AudioContext();
  private node: AudioWorkletNode | null = null;
  private starting: Promise<AudioWorkletNode> | null = null;

  private ready(patch: SynthPatch, made?: Record<string, MadeTable>): Promise<AudioWorkletNode> {
    this.starting ??= (async () => {
      await prepareSynth(this.ctx);
      const node = synthNode(this.ctx, patch, [], 120, Infinity, made);
      node.connect(this.ctx.destination);
      this.node = node;
      return node;
    })().catch((e) => {
      this.starting = null;
      throw e;
    });
    return this.starting;
  }

  async on(patch: SynthPatch, pitch: number, velocity = 100, made?: Record<string, MadeTable>) {
    const node = await this.ready(patch, made);
    await this.ctx.resume();
    updateSynth(node, patch, made);
    node.port.postMessage({ type: "now", k: 1, n: pitch, v: velocity });
  }

  off(pitch: number) {
    this.node?.port.postMessage({ type: "now", k: 0, n: pitch, v: 0 });
  }

  /** A MIDI keyboard's mod wheel (0 to 1) or pitch bend (-1 to 1). */
  control(kind: "mod" | "bend", value: number) {
    this.node?.port.postMessage({ type: "now", k: kind === "mod" ? 2 : 3, n: 0, v: value });
  }

  async tap(patch: SynthPatch, pitch: number, made?: Record<string, MadeTable>) {
    await this.on(patch, pitch, 100, made);
    setTimeout(() => this.off(pitch), 300);
  }

  close() {
    this.node?.disconnect();
    this.ctx.close();
  }
}
