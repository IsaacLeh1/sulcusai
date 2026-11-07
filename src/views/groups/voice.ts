// SPDX-License-Identifier: AGPL-3.0-only
// The microphone path for calls, all on this PC: noise removal, a gate that
// keeps first words, push to talk, levelling and the voice changer. What
// leaves the PC is the processed sound only.
import { devices } from "./Devices";

export type Preset = "voice" | "music" | "lowlag";
export type Changer = "off" | "deep" | "chipmunk" | "robot" | "radio" | "pitch" | "custom";

export type VoiceSettings = {
  preset: Preset;
  /** The browser's echo cancelling and noise suppression, as calls always had. */
  echo: boolean;
  /** Automatic gain: quiet and loud people come out level. */
  agc: boolean;
  /** Spectral noise removal on top of the browser's (fans, keyboards, hum), learned from the room as it goes. */
  denoise: boolean;
  denoiseAmount: number;
  /** Voice activity: sound below the line is not sent. */
  gate: boolean;
  gateDb: number;
  pushToTalk: boolean;
  /** An accelerator the system understands, such as "F8" or "alt+KeyV". */
  pttKey: string;
  changer: Changer;
  /** Semitones for "pitch" and "custom". */
  semitones: number;
  robot: number;
  radio: boolean;
  /** Others see a "voice changed" mark while the changer is on. */
  markChanged: boolean;
  muteReminder: boolean;
  spatial: boolean;
  ducking: boolean;
  /** Bitrate follows the connection instead of staying fixed. */
  adaptive: boolean;
};

const KEY = "sulcus.voice";
export const VOICE_EVENT = "sulcus:voice";

export const DEFAULTS: VoiceSettings = {
  preset: "voice",
  echo: true,
  agc: false,
  denoise: false,
  denoiseAmount: 0.7,
  gate: false,
  gateDb: -50,
  pushToTalk: false,
  pttKey: "F8",
  changer: "off",
  semitones: 0,
  robot: 0,
  radio: false,
  markChanged: false,
  muteReminder: false,
  spatial: false,
  ducking: false,
  adaptive: false,
};

export function voiceSettings(): VoiceSettings {
  try {
    return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(KEY) ?? "null") };
  } catch {
    return { ...DEFAULTS };
  }
}

export function setVoiceSettings(v: Partial<VoiceSettings>) {
  try {
    localStorage.setItem(KEY, JSON.stringify({ ...voiceSettings(), ...v }));
  } catch {
    // Kept for this session only.
  }
  window.dispatchEvent(new Event(VOICE_EVENT));
}

/** Each person's own noise removal, for this PC only. */
const PERSON_NR = "sulcus.personNoise";
export function personNoise(): Record<string, boolean> {
  try {
    return JSON.parse(localStorage.getItem(PERSON_NR) ?? "{}") ?? {};
  } catch {
    return {};
  }
}
export function setPersonNoise(who: string, on: boolean) {
  try {
    localStorage.setItem(PERSON_NR, JSON.stringify({ ...personNoise(), [who]: on }));
  } catch {
    // Kept for this session only.
  }
  window.dispatchEvent(new Event(VOICE_EVENT));
}

/** What each preset changes; the rest stays as set. */
export const PRESETS: Record<Preset, { channels: 1 | 2; process: boolean; latency: "interactive" | "playback"; bitrate: number }> = {
  voice: { channels: 1, process: true, latency: "interactive", bitrate: 48 },
  music: { channels: 2, process: false, latency: "playback", bitrate: 128 },
  lowlag: { channels: 1, process: true, latency: "interactive", bitrate: 32 },
};

export const changerSemis = (v: VoiceSettings) => (v.changer === "deep" ? -5 : v.changer === "chipmunk" ? 7 : v.changer === "pitch" || v.changer === "custom" ? v.semitones : 0);
export const changerOn = (v: VoiceSettings) => v.changer !== "off";

export function micConstraints(v: VoiceSettings = voiceSettings()): MediaTrackConstraints {
  const p = PRESETS[v.preset];
  const raw = !p.process;
  return {
    echoCancellation: !raw && v.echo,
    noiseSuppression: !raw && v.echo,
    autoGainControl: !raw && v.agc,
    channelCount: p.channels,
    ...(devices().mic ? { deviceId: { ideal: devices().mic } } : {}),
  };
}

// Runs in the audio thread. A short look-ahead delay lets the gate open
// before the sound it opened for, so the first word is not clipped.
const WORKLET = `
function fft(re, im, inverse) {
  const n = re.length;
  for (let i = 1, j = 0; i < n; i++) {
    let bit = n >> 1;
    for (; j & bit; bit >>= 1) j ^= bit;
    j ^= bit;
    if (i < j) { let t = re[i]; re[i] = re[j]; re[j] = t; t = im[i]; im[i] = im[j]; im[j] = t; }
  }
  for (let len = 2; len <= n; len <<= 1) {
    const ang = (2 * Math.PI / len) * (inverse ? 1 : -1);
    const wr = Math.cos(ang), wi = Math.sin(ang);
    for (let i = 0; i < n; i += len) {
      let cr = 1, ci = 0;
      for (let k = 0; k < len / 2; k++) {
        const a = i + k, b = a + len / 2;
        const tr = re[b] * cr - im[b] * ci, ti = re[b] * ci + im[b] * cr;
        re[b] = re[a] - tr; im[b] = im[a] - ti; re[a] += tr; im[a] += ti;
        const nr = cr * wr - ci * wi; ci = cr * wi + ci * wr; cr = nr;
      }
    }
  }
  if (inverse) for (let i = 0; i < n; i++) { re[i] /= n; im[i] /= n; }
}
class SulcusVoice extends AudioWorkletProcessor {
  constructor() {
    super();
    this.o = { gate: false, gateDb: -50, open: true, nr: 0, pitch: 1, robot: 0 };
    this.port.onmessage = (e) => Object.assign(this.o, e.data);
    this.look = new Float32Array(2048); this.li = 0; this.lookN = Math.round(sampleRate * 0.015);
    this.env = 0; this.gain = 0; this.hold = 0; this.floor = 0.0005;
    this.buf = new Float32Array(4096); this.wi = 0; this.d = 0; this.ph = 0;
    this.peak = 0; this.n = 0; this.on = false;
    // Spectral noise removal: 512-point frames, half overlapped, a noise level
    // per frequency that follows the quietest moments, and a gain per frequency.
    this.win = new Float32Array(512);
    for (let i = 0; i < 512; i++) this.win[i] = Math.sqrt(0.5 - 0.5 * Math.cos(2 * Math.PI * i / 512));
    this.ring = new Float32Array(512); this.rp = 0;
    this.ola = new Float32Array(512); this.outq = new Float32Array(256); this.oi = 0;
    this.re = new Float32Array(512); this.im = new Float32Array(512);
    this.noise = new Float32Array(257).fill(0.0001); this.pg = new Float32Array(257).fill(1);
  }
  den(x) {
    this.ring[this.rp] = x; this.rp = (this.rp + 1) & 511;
    const y = this.outq[this.oi++];
    if (this.oi === 256) { this.frame(); this.oi = 0; }
    return y;
  }
  frame() {
    const re = this.re, im = this.im, amt = this.o.nr;
    for (let i = 0; i < 512; i++) { re[i] = this.ring[(this.rp + i) & 511] * this.win[i]; im[i] = 0; }
    fft(re, im, false);
    const low = 1 - 0.9 * amt;
    for (let k = 0; k <= 256; k++) {
      const mag = Math.hypot(re[k], im[k]);
      const n = this.noise[k];
      this.noise[k] = mag < n ? n * 0.9 + mag * 0.1 : Math.min(n * 1.003, 1);
      let g = Math.max(low, 1 - amt * 1.5 * this.noise[k] / (mag + 1e-9));
      g = this.pg[k] * 0.5 + g * 0.5;
      this.pg[k] = g;
      re[k] *= g; im[k] *= g;
      if (k > 0 && k < 256) { re[512 - k] *= g; im[512 - k] *= g; }
    }
    fft(re, im, true);
    for (let i = 0; i < 512; i++) this.ola[i] += re[i] * this.win[i];
    this.outq.set(this.ola.subarray(0, 256));
    this.ola.copyWithin(0, 256); this.ola.fill(0, 256);
  }
  process(inputs, outputs) {
    const inp = inputs[0] && inputs[0][0];
    const out = outputs[0];
    if (!inp) return true;
    const o = this.o;
    const thr = Math.pow(10, o.gateDb / 20);
    const att = 1 - Math.exp(-1 / (sampleRate * 0.004));
    const rel = 1 - Math.exp(-1 / (sampleRate * 0.15));
    const holdN = sampleRate * 0.3;
    const G = 2048, M = 4096;
    for (let i = 0; i < inp.length; i++) {
      const x = o.nr > 0 ? this.den(inp[i]) : inp[i];
      const ax = Math.abs(x);
      this.env += (ax - this.env) * (ax > this.env ? 0.2 : 0.002);
      this.floor += (this.env - this.floor) * (this.env < this.floor ? 0.01 : 0.00003);
      if (!o.gate || this.env > thr) this.hold = holdN; else if (this.hold > 0) this.hold--;
      const target = o.open && this.hold > 0 ? 1 : 0;
      this.gain += (target - this.gain) * (target > this.gain ? att : rel);
      this.look[this.li] = x;
      let y = this.look[(this.li - this.lookN + 2048) % 2048] * this.gain;
      this.li = (this.li + 1) % 2048;
      if (o.pitch !== 1) {
        this.buf[this.wi] = y;
        this.d = (this.d + 1 - o.pitch + G) % G;
        const d2 = (this.d + G / 2) % G;
        const a = this.buf[(this.wi - Math.floor(this.d) + M) % M];
        const b = this.buf[(this.wi - Math.floor(d2) + M) % M];
        const wa = Math.sin(Math.PI * this.d / G) ** 2;
        y = a * wa + b * (1 - wa);
        this.wi = (this.wi + 1) % M;
      }
      if (o.robot > 0) { this.ph += 55 / sampleRate; if (this.ph > 1) this.ph -= 1; y *= 1 - o.robot + o.robot * Math.sin(2 * Math.PI * this.ph) * 1.4; }
      for (let c = 0; c < out.length; c++) out[c][i] = y;
      if (ax > this.peak) this.peak = ax;
    }
    this.on = this.gain > 0.5;
    if ((this.n += inp.length) >= sampleRate / 20) {
      this.port.postMessage({ level: this.peak, floor: this.floor, open: this.on, raw: this.env > thr });
      this.peak = 0; this.n = 0;
    }
    return true;
  }
}
registerProcessor("sulcus-voice", SulcusVoice);
`;
let workletUrl = "";

export type Level = { level: number; floor: number; open: boolean; raw: boolean };

/** One microphone run through the processing. `stream` is what to send. */
export class MicChain {
  ctx: AudioContext;
  stream: MediaStream = new MediaStream();
  private node: AudioWorkletNode | null = null;
  private radio: BiquadFilterNode[] = [];
  private shaper: WaveShaperNode | null = null;
  private comp: DynamicsCompressorNode;
  private pass: boolean;
  private open = true;
  onLevel: (l: Level) => void = () => {};

  private constructor(
    readonly mic: MediaStream,
    private v: VoiceSettings,
  ) {
    const p = PRESETS[v.preset];
    this.ctx = new AudioContext({ latencyHint: p.latency });
    this.pass = !p.process;
    this.comp = this.ctx.createDynamicsCompressor();
  }

  static async open(v: VoiceSettings = voiceSettings(), mic?: MediaStream): Promise<MicChain> {
    const stream = mic ?? (await navigator.mediaDevices.getUserMedia({ audio: micConstraints(v) }));
    const c = new MicChain(stream, v);
    try {
      await c.build();
    } catch (e) {
      c.close();
      throw e;
    }
    return c;
  }

  private async build() {
    const src = this.ctx.createMediaStreamSource(this.mic);
    const out = this.ctx.createMediaStreamDestination();
    out.channelCount = PRESETS[this.v.preset].channels;
    this.stream = out.stream;
    if (this.pass) {
      // Music keeps every sound as it is, in stereo, but mute still works.
      const g = this.ctx.createGain();
      src.connect(g).connect(out);
      this.gainNode = g;
      return;
    }
    if (!workletUrl) workletUrl = URL.createObjectURL(new Blob([WORKLET], { type: "text/javascript" }));
    await this.ctx.audioWorklet.addModule(workletUrl);
    const hp = this.ctx.createBiquadFilter();
    hp.type = "highpass";
    hp.frequency.value = 80;
    const node = new AudioWorkletNode(this.ctx, "sulcus-voice", { numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [1] });
    node.port.onmessage = (e) => this.onLevel(e.data as Level);
    this.node = node;
    const bp = this.ctx.createBiquadFilter();
    bp.type = "bandpass";
    bp.frequency.value = 1700;
    bp.Q.value = 0.9;
    const shaper = this.ctx.createWaveShaper();
    const curve = new Float32Array(256);
    for (let i = 0; i < 256; i++) {
      const x = (i / 255) * 2 - 1;
      curve[i] = Math.tanh(3 * x);
    }
    shaper.curve = curve;
    this.radio = [bp];
    this.shaper = shaper;
    this.comp.threshold.value = -24;
    this.comp.ratio.value = 4;
    this.comp.knee.value = 12;
    this.comp.attack.value = 0.005;
    this.comp.release.value = 0.2;
    src.connect(hp).connect(node);
    this.wire(out);
    this.apply(this.v);
  }

  private gainNode: GainNode | null = null;
  private out: AudioNode | null = null;
  private wire(out: AudioNode) {
    this.out = out;
    this.relink();
  }
  private relink() {
    if (!this.node || !this.out || !this.shaper) return;
    this.node.disconnect();
    this.radio[0].disconnect();
    this.shaper.disconnect();
    this.comp.disconnect();
    let at: AudioNode = this.node;
    if (this.v.changer === "radio" || (this.v.changer === "custom" && this.v.radio)) {
      at.connect(this.radio[0]).connect(this.shaper);
      at = this.shaper;
    }
    if (this.v.agc) {
      at.connect(this.comp);
      at = this.comp;
    }
    at.connect(this.out);
  }

  /** New settings without reopening the microphone. */
  apply(v: VoiceSettings) {
    this.v = v;
    const semis = changerSemis(v);
    const robot = v.changer === "robot" ? 0.8 : v.changer === "custom" ? v.robot : 0;
    this.node?.port.postMessage({ gate: v.gate, gateDb: v.gateDb, nr: v.denoise ? v.denoiseAmount : 0, pitch: Math.pow(2, semis / 12), robot, open: this.open });
    this.relink();
  }

  /** Push to talk and mute: the gate shuts, the meter keeps working. */
  setOpen(open: boolean) {
    this.open = open;
    this.node?.port.postMessage({ open });
    if (this.gainNode) this.gainNode.gain.value = open ? 1 : 0;
  }

  close() {
    this.mic.getTracks().forEach((t) => t.stop());
    this.stream.getTracks().forEach((t) => t.stop());
    this.ctx.close().catch(() => {});
  }
}

/** A key for push to talk: global through the app, with the window's own keys as a fallback. */
export function keyMatches(e: KeyboardEvent, accel: string): boolean {
  const parts = accel.toLowerCase().split("+");
  const key = parts.pop() ?? "";
  const want = (m: string) => parts.includes(m);
  if (want("ctrl") !== e.ctrlKey || want("alt") !== e.altKey || want("shift") !== e.shiftKey) return false;
  return e.code.toLowerCase() === key || e.key.toLowerCase() === key;
}

/** Turns a key press into the accelerator text the system takes. */
export function accelFrom(e: KeyboardEvent): string | null {
  if (["Control", "Alt", "Shift", "Meta"].includes(e.key)) return null;
  const mods = [e.ctrlKey && "ctrl", e.altKey && "alt", e.shiftKey && "shift"].filter(Boolean);
  return [...mods, e.code].join("+");
}
