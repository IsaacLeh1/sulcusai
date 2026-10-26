// SPDX-License-Identifier: AGPL-3.0-only
// The Studio's sound: every clip scheduled on Web Audio through its track
// (effects, volume and pan with their automation) into the mix, live from
// any point or rendered offline for export. A clip's speed, pitch and
// direction are worked out once on its samples (`audioEdit`) and kept.
import { fileUrl } from "../media";
import { resampleSpeed, stretchSpeed, type Channels } from "../audioEdit";
import { chain, prepare, type Chain } from "./effects";
import { renderNotes } from "./midi";
import { audible, beatSecs, end, gainOf, sourceOf, valueAt, type Clip, type Project, type Track } from "./model";

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

function channels(buf: AudioBuffer, from: number, to: number): Channels {
  const a = Math.max(0, Math.floor(from * buf.sampleRate));
  const b = Math.min(buf.length, Math.ceil(to * buf.sampleRate));
  return Array.from({ length: Math.min(2, buf.numberOfChannels) }, (_, c) => buf.getChannelData(c).slice(a, Math.max(a, b)));
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
  const plain = clip.speed === 1 && clip.semitones === 0 && !clip.reverse;
  if (plain) return { buffer: source, offset: clip.offset, duration: Math.min(clip.length, source.duration - clip.offset) };
  const key = `${src}:${clip.offset}:${clip.length}:${clip.speed}:${clip.keepPitch}:${clip.semitones}:${clip.reverse}`;
  let p = derived.get(key);
  if (!p) {
    p = (async () => {
      const rate = source.sampleRate;
      let chs = channels(source, clip.offset, clip.offset + clip.length * clip.speed);
      if (clip.reverse) chs = chs.map((c) => c.slice().reverse());
      // A new key: played faster by its ratio (the pitch moves), then
      // stretched back to its length (the pitch stays where it moved).
      const shift = Math.pow(2, clip.semitones / 12);
      if (shift !== 1) chs = stretchSpeed(resampleSpeed(chs, shift), rate, 1 / shift);
      if (clip.speed !== 1) chs = clip.keepPitch ? stretchSpeed(chs, rate, clip.speed) : resampleSpeed(chs, clip.speed);
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

/** A track's nodes: clips in, effects, volume and pan (with their automation from `from`, started at `t0`), then on. */
function trackNodes(ctx: BaseAudioContext, t: Track, out: AudioNode, from: number, t0: number, heard: boolean): TrackNodes {
  const input = ctx.createGain();
  const fx = chain(ctx, t.effects);
  const volume = ctx.createGain();
  const pan = ctx.createStereoPanner();
  const gate = ctx.createGain();
  gate.gain.value = heard ? 1 : 0;
  input.connect(fx.input);
  fx.output.connect(volume).connect(pan).connect(gate).connect(out);
  const lane = (param: AudioParam, points: { t: number; v: number }[] | undefined, base: number, map: (v: number) => number) => {
    param.setValueAtTime(map(valueAt(points, from, base)), t0);
    for (const pt of [...(points ?? [])].sort((a, b) => a.t - b.t)) {
      if (pt.t > from) param.linearRampToValueAtTime(map(pt.v), t0 + pt.t - from);
    }
  };
  lane(volume.gain, t.automation.volume, t.volume, gainOf);
  lane(pan.pan, t.automation.pan, t.pan, (v) => Math.max(-1, Math.min(1, v)));
  return { input, fx, volume, pan, gate };
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
  const g = ctx.createGain();
  // Its level through the fades, at clip second `s`, scheduled from where playing starts.
  const level = gainOf(clip.gain);
  const fadeIn = Math.min(clip.fadeIn, a.duration / 2);
  const fadeOut = Math.min(clip.fadeOut, a.duration / 2);
  const env = (s: number) => level * (fadeIn > 0 ? Math.min(1, s / fadeIn) : 1) * (fadeOut > 0 ? Math.min(1, (a.duration - s) / fadeOut) : 1);
  const at = (s: number) => when + s - skip;
  g.gain.setValueAtTime(env(skip), when);
  if (fadeIn > skip) g.gain.linearRampToValueAtTime(env(fadeIn), at(fadeIn));
  if (fadeOut > 0) {
    const s = Math.max(skip, a.duration - fadeOut);
    if (s > skip) g.gain.setValueAtTime(env(s), at(s));
    g.gain.linearRampToValueAtTime(0, at(a.duration));
  }
  node.connect(g).connect(into);
  node.start(when, a.offset + skip, dur);
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
      .filter((t) => !only || only.has(t.id))
      .flatMap((t) => t.clips.map(async (c) => {
        const a = await clipAudio(c, t).catch(() => null);
        if (a) out.set(c.id, a);
      })),
  );
  return out;
}

export class StudioEngine {
  ctx = new AudioContext();
  private master = this.ctx.createGain();
  private nodes = new Map<string, TrackNodes>();
  private sources: AudioScheduledSourceNode[] = [];
  private bus: GainNode | null = null;
  private started: { at: number; from: number } | null = null;

  constructor() {
    this.master.connect(this.ctx.destination);
  }

  get playing() {
    return this.started !== null;
  }

  /** Where it's playing, in timeline seconds. */
  position(): number {
    return this.started ? this.started.from + (this.ctx.currentTime - this.started.at) : 0;
  }

  /** Plays the project from `from` seconds (with clicks when `click`), until its end. */
  async play(p: Project, from: number, click = false): Promise<void> {
    this.stop();
    await prepare(this.ctx);
    const audio = await allAudio(p);
    await this.ctx.resume();
    const t0 = this.ctx.currentTime + 0.08;
    const until = Math.max(...p.tracks.flatMap((t) => t.clips.map(end)), from + 1);
    const heard = audible(p);
    const bus = this.ctx.createGain();
    bus.connect(this.master);
    for (const t of p.tracks) {
      const n = trackNodes(this.ctx, t, bus, from, t0, heard.has(t.id));
      this.nodes.set(t.id, n);
      for (const c of t.clips) {
        const a = audio.get(c.id);
        const node = a && scheduleClip(this.ctx, c, a, n.input, from, t0, until);
        if (node) this.sources.push(node);
      }
    }
    if (click) metronome(this.ctx, p, bus, from, t0, until);
    this.bus = bus;
    this.started = { at: t0, from };
  }

  stop() {
    for (const s of this.sources) {
      try {
        s.stop();
      } catch {
        // already ended
      }
    }
    this.sources = [];
    this.bus?.disconnect();
    this.bus = null;
    this.nodes.clear();
    this.started = null;
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
  const audio = await allAudio(p, only);
  const heard = audible(p);
  for (const t of p.tracks) {
    if (only && !only.has(t.id)) continue;
    const n = trackNodes(ctx, t, ctx.destination, from, 0, only ? true : heard.has(t.id));
    for (const c of t.clips) {
      const a = audio.get(c.id);
      if (a) scheduleClip(ctx, c, a, n.input, from, 0, to);
    }
  }
  return ctx.startRendering();
}
