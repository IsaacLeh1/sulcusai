// SPDX-License-Identifier: AGPL-3.0-only
// The Studio's sound: every clip scheduled on Web Audio through its track
// (effects, volume and pan with their automation) into the mix, live from
// any point or rendered offline for export. A clip's speed, pitch and
// direction are worked out once on its samples (`audioEdit`) and kept.
import { fileUrl } from "../media";
import { formantShift, resampleSpeed, stretchSpeed, type Channels } from "../audioEdit";
import { chain, prepare, type Chain } from "./effects";
import { renderNotes } from "./midi";
import { prepareSynth, synthNode, tailOf, updateSynth, type NoteEvent } from "./synth";
import { anchors, audible, beatSecs, end, gainOf, soundAt, sourceOf, startIn, valueAt, warped, type Clip, type Project, type Track } from "./model";

const decoded = new Map<string, Promise<AudioBuffer>>();
const derived = new Map<string, Promise<AudioBuffer>>();
let decoder: AudioContext | null = null;

/** A gallery item's sound, decoded once. */
export function sourceBuffer(id: string, mime = "audio/mpeg"): Promise<AudioBuffer> {
  let p = decoded.get(id);
  if (!p) {
    decoder ??= new AudioContext();
    const ctx = decoder;
    p = fileUrl({ id, mime })
      .then((u) => fetch(u))
      .then((r) => r.arrayBuffer())
      .then((b) => ctx.decodeAudioData(b));
    p.catch(() => decoded.delete(id));
    decoded.set(id, p);
  }
  return p;
}

/** Lets go of a decoded item (deleted or replaced). */
export function forgetSource(id: string) {
  decoded.delete(id);
}

/** A stretch of a buffer's samples; silence for any of it before the start. */
function channels(buf: AudioBuffer, from: number, to: number): Channels {
  const want = Math.max(0, Math.round((to - from) * buf.sampleRate));
  const a = Math.floor(from * buf.sampleRate);
  return Array.from({ length: Math.min(2, buf.numberOfChannels) }, (_, c) => {
    const data = buf.getChannelData(c);
    if (a >= 0) return data.slice(a, Math.min(data.length, a + want));
    const out = new Float32Array(want);
    out.set(data.subarray(0, Math.max(0, Math.min(data.length, a + want))), -a);
    return out;
  });
}

/** A warped clip's sound in its own time: each stretch between anchors made
 * to last as long as they're apart (its pitch kept, unless it isn't to be),
 * the stretches cross-faded over a few milliseconds where they meet. */
function warpedChannels(buf: AudioBuffer, c: Clip, ahead: number): Channels {
  const rate = buf.sampleRate;
  const pts = [...anchors(c).filter((a) => a.t < c.length - 0.005), { t: c.length, s: soundAt(c, c.length) }];
  const total = Math.max(1, Math.round(c.length * rate));
  const out: Channels = Array.from({ length: Math.min(2, buf.numberOfChannels) }, () => new Float32Array(total));
  // Half the cross-fade at each place two stretches meet.
  const half = pts.map((p, i) => (i === 0 || i === pts.length - 1 ? 0 : Math.min(0.01, (p.t - pts[i - 1].t) / 2, (pts[i + 1].t - p.t) / 2)));
  for (let i = 0; i + 1 < pts.length; i++) {
    const [a, b] = [pts[i], pts[i + 1]];
    const factor = (b.s - a.s) / Math.max(1e-6, b.t - a.t);
    const [h0, h1] = [half[i], half[i + 1]];
    let piece = channels(buf, ahead + a.s - h0 * factor, ahead + b.s + h1 * factor);
    if (Math.abs(factor - 1) > 1e-3) piece = c.keepPitch ? stretchSpeed(piece, rate, factor) : resampleSpeed(piece, factor);
    const at = Math.round((a.t - h0) * rate);
    const [in0, in1] = [Math.round(2 * h0 * rate), Math.round(2 * h1 * rate)];
    const n = piece[0].length;
    for (let ch = 0; ch < out.length; ch++) {
      const src = piece[ch] ?? piece[0];
      const dst = out[ch];
      for (let j = 0; j < n; j++) {
        const k = at + j;
        if (k < 0 || k >= total) continue;
        let w = 1;
        if (in0 > 0 && j < in0) w = j / in0;
        if (in1 > 0 && n - j <= in1) w = Math.min(w, (n - j) / in1);
        dst[k] += src[j] * w;
      }
    }
  }
  return out;
}

function toBuffer(chs: Channels, rate: number): AudioBuffer {
  decoder ??= new AudioContext();
  const out = decoder.createBuffer(2, Math.max(1, chs[0].length), rate);
  out.copyToChannel(chs[0], 0);
  out.copyToChannel(chs[1] ?? chs[0], 1);
  return out;
}

/** What a clip plays: a buffer, where in it to start, and for how long. */
export interface ClipAudio {
  buffer: AudioBuffer;
  offset: number;
  duration: number;
}

/** A clip's sound: its source as is (offset into it), or worked out once
 * when its speed, pitch or direction differ, or rendered from its notes. */
export async function clipAudio(clip: Clip, track: Track): Promise<ClipAudio | null> {
  if (track.kind === "midi") {
    const key = `midi:${track.program}:${clip.length}:${JSON.stringify(clip.notes ?? [])}:${clip.semitones}`;
    let p = derived.get(key);
    if (!p) {
      const notes = (clip.notes ?? []).map((n) => ({ ...n, pitch: n.pitch + (track.program === 128 ? 0 : clip.semitones) }));
      p = renderNotes(notes, track.program ?? 0, clip.length);
      derived.set(key, p);
    }
    return { buffer: await p, offset: 0, duration: clip.length };
  }
  const src = sourceOf(clip);
  if (!src) return null;
  const source = await sourceBuffer(src);
  const from = startIn(clip);
  const warp = warped(clip);
  const plain = clip.speed === 1 && clip.semitones === 0 && !clip.reverse && !warp && !clip.formant && from >= 0;
  if (plain) return { buffer: source, offset: from, duration: Math.min(clip.length, source.duration - from) };
  const key = `${src}:${from}:${clip.length}:${clip.speed}:${clip.keepPitch}:${clip.semitones}:${clip.formant ?? 0}:${clip.reverse}:${warp ? JSON.stringify(anchors(clip)) : ""}`;
  let p = derived.get(key);
  if (!p) {
    p = (async () => {
      const rate = source.sampleRate;
      // A new key: played faster by its ratio (the pitch moves), then
      // stretched back to its length (the pitch stays where it moved).
      const shift = Math.pow(2, clip.semitones / 12);
      const transpose = (chs: Channels) => (shift !== 1 ? stretchSpeed(resampleSpeed(chs, shift), rate, 1 / shift) : chs);
      let chs: Channels;
      if (warp) {
        chs = warpedChannels(source, clip, from - clip.offset);
        if (clip.reverse) chs = chs.map((c) => c.slice().reverse());
        chs = transpose(chs);
      } else {
        chs = channels(source, from, from + clip.length * clip.speed);
        if (clip.reverse) chs = chs.map((c) => c.slice().reverse());
        chs = transpose(chs);
        if (clip.speed !== 1) chs = clip.keepPitch ? stretchSpeed(chs, rate, clip.speed) : resampleSpeed(chs, clip.speed);
      }
      if (clip.formant) chs = formantShift(chs, rate, clip.formant);
      return toBuffer(chs, rate);
    })();
    derived.set(key, p);
  }
  const buffer = await p;
  return { buffer, offset: 0, duration: Math.min(clip.length, buffer.duration) };
}

interface TrackNodes {
  input: GainNode;
  fx: Chain;
  volume: GainNode;
  pan: StereoPannerNode;
  gate: GainNode;
}

/** A track's nodes: clips in, effects, volume and pan, then on. */
function trackNodes(ctx: BaseAudioContext, t: Track, out: AudioNode, heard: boolean): TrackNodes {
  const input = ctx.createGain();
  const fx = chain(ctx, t.effects);
  const volume = ctx.createGain();
  const pan = ctx.createStereoPanner();
  const gate = ctx.createGain();
  gate.gain.value = heard ? 1 : 0;
  input.connect(fx.input);
  fx.output.connect(volume).connect(pan).connect(gate).connect(out);
  return { input, fx, volume, pan, gate };
}

/** A track's volume and pan automation for the timeline from `from` to `until`, played from context time `at`. */
function automate(n: TrackNodes, t: Track, from: number, at: number, until: number) {
  const lane = (param: AudioParam, points: { t: number; v: number }[] | undefined, base: number, map: (v: number) => number) => {
    param.setValueAtTime(map(valueAt(points, from, base)), at);
    if (!points?.length) return;
    for (const pt of [...points].sort((a, b) => a.t - b.t)) {
      if (pt.t > from && pt.t < until) param.linearRampToValueAtTime(map(pt.v), at + pt.t - from);
    }
    if (Number.isFinite(until)) param.linearRampToValueAtTime(map(valueAt(points, until, base)), at + until - from);
  };
  lane(n.volume.gain, t.automation.volume, t.volume, gainOf);
  lane(n.pan.pan, t.automation.pan, t.pan, (v) => Math.max(-1, Math.min(1, v)));
}

/** A clip scheduled into its track from timeline second `from`, the timeline's `from` at context time `t0`. */
function scheduleClip(ctx: BaseAudioContext, clip: Clip, a: ClipAudio, into: AudioNode, from: number, t0: number, until: number): AudioBufferSourceNode | null {
  const clipEnd = clip.start + a.duration;
  if (clip.disabled || clipEnd <= from || clip.start >= until) return null;
  const skip = Math.max(0, from - clip.start);
  const when = t0 + Math.max(0, clip.start - from);
  const dur = Math.min(a.duration - skip, until - Math.max(from, clip.start));
  if (dur <= 0.005) return null;
  const node = ctx.createBufferSource();
  node.buffer = a.buffer;
  node.connect(clipLevel(ctx, clip, a.duration, skip, when)).connect(into);
  node.start(when, a.offset + skip, dur);
  return node;
}

/** A clip's level through its fades (`duration` long), heard from clip second `skip` at context time `when`. */
function clipLevel(ctx: BaseAudioContext, clip: Clip, duration: number, skip: number, when: number): GainNode {
  const g = ctx.createGain();
  const level = gainOf(clip.gain);
  const fadeIn = Math.min(clip.fadeIn, duration / 2);
  const fadeOut = Math.min(clip.fadeOut, duration / 2);
  const env = (s: number) => level * (fadeIn > 0 ? Math.min(1, s / fadeIn) : 1) * (fadeOut > 0 ? Math.min(1, (duration - s) / fadeOut) : 1);
  const at = (s: number) => when + s - skip;
  g.gain.setValueAtTime(env(skip), when);
  if (fadeIn > skip) g.gain.linearRampToValueAtTime(env(fadeIn), at(fadeIn));
  if (fadeOut > 0) {
    const s = Math.max(skip, duration - fadeOut);
    if (s > skip) g.gain.setValueAtTime(env(s), at(s));
    g.gain.linearRampToValueAtTime(0, at(duration));
  }
  return g;
}

/** A synth track's clip played on the wavetable synth: its notes from
 * timeline second `from` (at context time `at`) to `until`. */
function scheduleSynth(ctx: BaseAudioContext, p: Project, t: Track, clip: Clip, into: AudioNode, from: number, at: number, until: number): AudioWorkletNode | null {
  const patch = t.synth!;
  const clipEnd = Math.min(end(clip), until);
  if (clip.disabled || clipEnd <= from || clip.start >= until) return null;
  const when = (s: number) => at + s - from;
  const events: NoteEvent[] = [];
  for (const n of clip.notes ?? []) {
    const on = Math.max(from, clip.start + n.start);
    const off = Math.min(clip.start + Math.min(n.end, clip.length), until);
    if (off <= on) continue;
    const pitch = n.pitch + clip.semitones;
    events.push({ t: when(on), k: 1, n: pitch, v: n.velocity }, { t: when(off), k: 0, n: pitch, v: 0 });
  }
  if (!events.length) return null;
  // Lets go before starting again, at the same moment.
  events.sort((a, b) => a.t - b.t || a.k - b.k);
  const node = synthNode(ctx, patch, events, p.bpm, when(clipEnd) + tailOf(patch) + 0.2, p.tables);
  const skip = Math.max(0, from - clip.start);
  node.connect(clipLevel(ctx, clip, clip.length, skip, at + Math.max(0, clip.start - from))).connect(into);
  return node;
}

/** Clicks on every beat (louder on the bar's first), from `from` to `until`. */
function metronome(ctx: BaseAudioContext, p: Project, out: AudioNode, from: number, t0: number, until: number) {
  const beat = beatSecs(p);
  for (let n = Math.ceil(from / beat); n * beat < until; n++) {
    const when = t0 + n * beat - from;
    const osc = ctx.createOscillator();
    const g = ctx.createGain();
    osc.frequency.value = n % Math.max(1, p.beats) === 0 ? 1600 : 1000;
    g.gain.setValueAtTime(0.0001, when);
    g.gain.exponentialRampToValueAtTime(0.5, when + 0.002);
    g.gain.exponentialRampToValueAtTime(0.0001, when + 0.06);
    osc.connect(g).connect(out);
    osc.start(when);
    osc.stop(when + 0.08);
  }
}

/** Every clip's sound, ready (decoded, worked out, rendered). */
async function allAudio(p: Project, only?: Set<string>): Promise<Map<string, ClipAudio>> {
  const out = new Map<string, ClipAudio>();
  await Promise.all(
    p.tracks
      .filter((t) => (!only || only.has(t.id)) && !t.synth)
      .flatMap((t) => t.clips.map(async (c) => {
        const a = await clipAudio(c, t).catch(() => null);
        if (a) out.set(c.id, a);
      })),
  );
  return out;
}

/** A stretch of the timeline being played: from `from` to `until`, starting at context time `at`. */
interface Pass {
  at: number;
  from: number;
  until: number;
}

export class StudioEngine {
  ctx = new AudioContext();
  private master = this.ctx.createGain();
  private nodes = new Map<string, TrackNodes>();
  private sources: AudioScheduledSourceNode[] = [];
  private bus: GainNode | null = null;
  private passes: Pass[] = [];
  private looper: ReturnType<typeof setInterval> | null = null;
  /** Each synth track's synths playing now (they hear its sound changed at once). */
  private synths = new Map<string, AudioWorkletNode[]>();

  constructor() {
    this.master.connect(this.ctx.destination);
  }

  get playing() {
    return this.passes.length > 0;
  }

  /** Where it's playing, in timeline seconds (round a loop, within it). */
  position(): number {
    const now = this.ctx.currentTime;
    let pass = this.passes[0];
    if (!pass) return 0;
    for (const x of this.passes) if (x.at <= now) pass = x;
    return pass.from + Math.max(0, now - pass.at);
  }

  /** The `performance.now()` moment the first pass is heard (for lining a recording up with what was played). */
  heardAt(): number | null {
    const first = this.passes[0];
    if (!first) return null;
    const ts = this.ctx.getOutputTimestamp();
    if (ts.performanceTime && ts.contextTime) return ts.performanceTime + (first.at - ts.contextTime) * 1000;
    return performance.now() + (first.at - this.ctx.currentTime + this.ctx.outputLatency) * 1000;
  }

  /** Plays the project from `from` seconds (with clicks when `click`) to its
   * end, or round its loop without a gap (each pass scheduled before the last ends). */
  async play(p: Project, from: number, click = false, countIn = 0): Promise<void> {
    this.stop();
    await prepare(this.ctx);
    if (p.tracks.some((t) => t.synth)) await prepareSynth(this.ctx);
    const audio = await allAudio(p);
    await this.ctx.resume();
    // A count-in: clicks for that long before it starts.
    const t0 = this.ctx.currentTime + 0.08 + countIn;
    const songEnd = Math.max(...p.tracks.flatMap((t) => t.clips.map(end)), from + 1);
    const loop = p.loop && p.loop[1] - p.loop[0] >= 0.25 && from < p.loop[1] ? p.loop : null;
    const heard = audible(p);
    const bus = this.ctx.createGain();
    bus.connect(this.master);
    if (countIn > 0) metronome(this.ctx, p, bus, 0, t0 - countIn, countIn);
    for (const t of p.tracks) this.nodes.set(t.id, trackNodes(this.ctx, t, bus, heard.has(t.id)));
    this.bus = bus;
    const schedule = (pass: Pass) => {
      for (const t of p.tracks) {
        const n = this.nodes.get(t.id)!;
        automate(n, t, pass.from, pass.at, pass.until);
        for (const c of t.clips) {
          if (t.synth) {
            const s = scheduleSynth(this.ctx, p, t, c, n.input, pass.from, pass.at, pass.until);
            if (s) this.synths.set(t.id, [...(this.synths.get(t.id) ?? []).slice(-15), s]);
            continue;
          }
          const a = audio.get(c.id);
          const node = a && scheduleClip(this.ctx, c, a, n.input, pass.from, pass.at, pass.until);
          if (node) {
            this.sources.push(node);
            node.addEventListener("ended", () => (this.sources = this.sources.filter((s) => s !== node)));
          }
        }
      }
      if (click) metronome(this.ctx, p, bus, pass.from, pass.at, pass.until);
      this.passes.push(pass);
    };
    schedule({ at: t0, from, until: loop ? loop[1] : songEnd });
    if (loop) {
      this.looper = setInterval(() => {
        const last = this.passes[this.passes.length - 1];
        if (!last) return;
        const next = last.at + (last.until - last.from);
        if (next - this.ctx.currentTime < 0.75) schedule({ at: next, from: loop[0], until: loop[1] });
        if (this.passes.length > 64) this.passes = this.passes.slice(-8);
      }, 100);
    }
  }

  stop() {
    if (this.looper) clearInterval(this.looper);
    this.looper = null;
    for (const s of this.sources) {
      try {
        s.stop();
      } catch {
        // already ended
      }
    }
    this.sources = [];
    // Synths stop making sound and let themselves go.
    for (const list of this.synths.values()) for (const s of list) s.port.postMessage({ type: "silence" });
    this.synths.clear();
    this.bus?.disconnect();
    this.bus = null;
    this.nodes.clear();
    this.passes = [];
  }

  /** A track's mix settings changed while playing: heard, volume, pan, effects' settings. */
  updateTrack(p: Project, t: Track) {
    const n = this.nodes.get(t.id);
    if (!n) return;
    const heard = audible(p);
    for (const [id, other] of this.nodes) other.gate.gain.value = heard.has(id) ? 1 : 0;
    if (!t.automation.volume?.length) n.volume.gain.value = gainOf(t.volume);
    if (!t.automation.pan?.length) n.pan.pan.value = t.pan;
    for (const e of t.effects) n.fx.update(e);
    if (t.synth) for (const s of this.synths.get(t.id) ?? []) updateSynth(s, t.synth, p.tables);
  }

  setMaster(db: number) {
    this.master.gain.value = gainOf(db);
  }

  close() {
    this.stop();
    this.ctx.close();
  }
}

/** The mix (or only some tracks') from `from` to `to`, rendered at 48 kHz. */
export async function render(p: Project, from: number, to: number, only?: Set<string>): Promise<AudioBuffer> {
  const rate = 48000;
  const ctx = new OfflineAudioContext({ numberOfChannels: 2, length: Math.max(1, Math.ceil((to - from) * rate)), sampleRate: rate });
  await prepare(ctx);
  if (p.tracks.some((t) => t.synth)) await prepareSynth(ctx);
  const audio = await allAudio(p, only);
  const heard = audible(p);
  for (const t of p.tracks) {
    if (only && !only.has(t.id)) continue;
    const n = trackNodes(ctx, t, ctx.destination, only ? true : heard.has(t.id));
    automate(n, t, from, 0, Infinity);
    for (const c of t.clips) {
      if (t.synth) {
        scheduleSynth(ctx, p, t, c, n.input, from, 0, to);
        continue;
      }
      const a = audio.get(c.id);
      if (a) scheduleClip(ctx, c, a, n.input, from, 0, to);
    }
  }
  return ctx.startRendering();
}
