// SPDX-License-Identifier: AGPL-3.0-only
// The Studio's project (Suno Studio's): tracks of clips on a timeline, each
// clip a stretch of a gallery item (audio) or notes (MIDI), with its takes,
// fades, gain, speed and pitch; tracks with volume, pan, mute, solo,
// effects and automation. Pure functions on plain data, so every edit is a
// new project (undo keeps the old one).
import { tx } from "../i18n";

export type EffectKind = "eq" | "compressor" | "reverb" | "delay" | "distortion" | "gate";

export interface Effect {
  id: string;
  kind: EffectKind;
  bypass: boolean;
  /** The effect's settings by name (see `EFFECTS`). */
  params: Record<string, number>;
}

/** A point of an automation lane: seconds, and the value (volume in dB, pan -1 to 1). */
export interface AutoPoint {
  t: number;
  v: number;
}

export interface Note {
  pitch: number;
  /** Seconds from the clip's start. */
  start: number;
  end: number;
  velocity: number;
}

export interface Take {
  id: string;
  /** The gallery item it plays (audio). */
  source: string;
  /** What made it ("Take 2", "Replaced 0:42 to 0:58"). */
  label: string;
}

export interface Clip {
  id: string;
  name: string;
  /** Where it starts on the timeline, in seconds. */
  start: number;
  /** How far into its source it starts playing, in seconds (of the source). */
  offset: number;
  /** How long it plays on the timeline, in seconds. */
  length: number;
  /** Audio: the take playing (one of `takes`). MIDI: none. */
  take?: string;
  takes: Take[];
  /** MIDI: its notes. */
  notes?: Note[];
  gain: number;
  fadeIn: number;
  fadeOut: number;
  /** Plays faster or slower (1 as made); `keepPitch` keeps its key. */
  speed: number;
  keepPitch: boolean;
  /** Up or down, in semitones. */
  semitones: number;
  reverse: boolean;
  /** Silent and greyed (Suno's disable). */
  disabled: boolean;
}

export interface Track {
  id: string;
  name: string;
  kind: "audio" | "midi";
  color: string;
  /** dB, 0 as made. */
  volume: number;
  pan: number;
  mute: boolean;
  solo: boolean;
  /** MIDI: the General MIDI program it plays (drums: 128). */
  program?: number;
  effects: Effect[];
  clips: Clip[];
  automation: { volume?: AutoPoint[]; pan?: AutoPoint[] };
}

export interface Project {
  id: string;
  name: string;
  /** Quarter notes a minute; and the time signature's beats a bar. */
  bpm: number;
  beats: number;
  tracks: Track[];
  /** A loop region, in seconds. */
  loop: [number, number] | null;
  /** The song it was opened from (its settings help new parts fit). */
  song?: string;
  created_at: number;
  updated_at: number;
}

export const COLORS = ["#6d5dfc", "#e0565b", "#2fa36b", "#e19b2d", "#2f8fd8", "#b357c9", "#4fb3a9", "#c7792f"];

export const EFFECTS: Record<EffectKind, { label: string; params: Record<string, [number, number, number]> }> = {
  // [min, max, default]
  eq: { label: tx("EQ"), params: { low: [-15, 15, 0], mid: [-15, 15, 0], high: [-15, 15, 0], midFreq: [200, 5000, 1000] } },
  compressor: { label: tx("Compressor"), params: { threshold: [-60, 0, -18], ratio: [1, 20, 4], attack: [0, 0.5, 0.01], release: [0.02, 1, 0.25], makeup: [0, 24, 3] } },
  reverb: { label: tx("Reverb"), params: { size: [0.3, 6, 2], mix: [0, 1, 0.25] } },
  delay: { label: tx("Delay"), params: { time: [0.02, 1.5, 0.375], feedback: [0, 0.9, 0.35], mix: [0, 1, 0.25] } },
  distortion: { label: tx("Distortion"), params: { drive: [0, 1, 0.3], mix: [0, 1, 1] } },
  gate: { label: tx("Gate"), params: { threshold: [-80, 0, -45] } },
};

let counter = 0;
export function newId(prefix = "x"): string {
  counter += 1;
  return `${prefix}${Date.now().toString(36)}${counter.toString(36)}${Math.random().toString(36).slice(2, 6)}`;
}

export function newEffect(kind: EffectKind): Effect {
  const params = Object.fromEntries(Object.entries(EFFECTS[kind].params).map(([k, [, , d]]) => [k, d]));
  return { id: newId("fx"), kind, bypass: false, params };
}

export function newTrack(name: string, kind: Track["kind"], index: number): Track {
  return { id: newId("tr"), name, kind, color: COLORS[index % COLORS.length], volume: 0, pan: 0, mute: false, solo: false, program: kind === "midi" ? 0 : undefined, effects: [], clips: [], automation: {} };
}

export function audioClip(source: string, name: string, start: number, seconds: number): Clip {
  const take = newId("tk");
  return { id: newId("cl"), name, start, offset: 0, length: seconds, take, takes: [{ id: take, source, label: name }], gain: 0, fadeIn: 0, fadeOut: 0, speed: 1, keepPitch: true, semitones: 0, reverse: false, disabled: false };
}

export function midiClip(name: string, start: number, length: number, notes: Note[]): Clip {
  return { id: newId("cl"), name, start, offset: 0, length, takes: [], notes, gain: 0, fadeIn: 0, fadeOut: 0, speed: 1, keepPitch: true, semitones: 0, reverse: false, disabled: false };
}

/** A new project around a song: one track with the whole song. */
export function projectFor(song: { id: string; title?: string | null; prompt: string; seconds: number }, bpm: number, beats: number): Project {
  const name = song.title || song.prompt.slice(0, 60) || "Song";
  const track = newTrack(name, "audio", 0);
  track.clips = [audioClip(song.id, name, 0, song.seconds)];
  const now = Date.now();
  return { id: newId("pj"), name, bpm, beats, tracks: [track], loop: null, song: song.id, created_at: now, updated_at: now };
}

/** The source a clip plays now (its chosen take's), if audio. */
export function sourceOf(c: Clip): string | null {
  return c.takes.find((t) => t.id === c.take)?.source ?? c.takes[0]?.source ?? null;
}

export function end(c: Clip) {
  return c.start + c.length;
}

/** The project's length: its last clip's end (a minute at least). */
export function length(p: Project) {
  return Math.max(60, ...p.tracks.flatMap((t) => t.clips.map(end)));
}

/** Seconds a beat lasts. */
export function beatSecs(p: Project) {
  return 60 / Math.max(20, p.bpm);
}

/** `t` moved to the nearest grid line (`grid` beats apart). */
export function snap(p: Project, t: number, grid: number) {
  if (grid <= 0) return t;
  const step = beatSecs(p) * grid;
  return Math.max(0, Math.round(t / step) * step);
}

function mapTrack(p: Project, trackId: string, f: (t: Track) => Track): Project {
  return { ...p, tracks: p.tracks.map((t) => (t.id === trackId ? f(t) : t)), updated_at: Date.now() };
}

export function updateTrack(p: Project, trackId: string, patch: Partial<Track>): Project {
  return mapTrack(p, trackId, (t) => ({ ...t, ...patch }));
}

export function updateClip(p: Project, clipId: string, patch: Partial<Clip>): Project {
  return { ...p, tracks: p.tracks.map((t) => (t.clips.some((c) => c.id === clipId) ? { ...t, clips: t.clips.map((c) => (c.id === clipId ? { ...c, ...patch } : c)) } : t)), updated_at: Date.now() };
}

export function findClip(p: Project, clipId: string): { track: Track; clip: Clip } | null {
  for (const track of p.tracks) {
    const clip = track.clips.find((c) => c.id === clipId);
    if (clip) return { track, clip };
  }
  return null;
}

export function addTrack(p: Project, t: Track): Project {
  return { ...p, tracks: [...p.tracks, t], updated_at: Date.now() };
}

export function removeTrack(p: Project, trackId: string): Project {
  return { ...p, tracks: p.tracks.filter((t) => t.id !== trackId), updated_at: Date.now() };
}

export function moveTrack(p: Project, trackId: string, by: number): Project {
  const i = p.tracks.findIndex((t) => t.id === trackId);
  const j = i + by;
  if (i < 0 || j < 0 || j >= p.tracks.length) return p;
  const tracks = [...p.tracks];
  [tracks[i], tracks[j]] = [tracks[j], tracks[i]];
  return { ...p, tracks, updated_at: Date.now() };
}

export function duplicateTrack(p: Project, trackId: string): Project {
  const i = p.tracks.findIndex((t) => t.id === trackId);
  if (i < 0) return p;
  const t = p.tracks[i];
  const copy: Track = { ...t, id: newId("tr"), name: `${t.name} 2`, clips: t.clips.map((c) => ({ ...c, id: newId("cl") })), effects: t.effects.map((e) => ({ ...e, id: newId("fx") })) };
  const tracks = [...p.tracks];
  tracks.splice(i + 1, 0, copy);
  return { ...p, tracks, updated_at: Date.now() };
}

export function addClip(p: Project, trackId: string, c: Clip): Project {
  return mapTrack(p, trackId, (t) => ({ ...t, clips: [...t.clips, c] }));
}

export function removeClips(p: Project, ids: Set<string>): Project {
  return { ...p, tracks: p.tracks.map((t) => ({ ...t, clips: t.clips.filter((c) => !ids.has(c.id)) })), updated_at: Date.now() };
}

/** Seconds of source a clip plays per second of timeline. */
export function rate(c: Clip) {
  return c.speed;
}

/** A clip cut in two at `t` (timeline seconds); unchanged when `t` isn't inside it. */
export function splitClip(p: Project, clipId: string, t: number): Project {
  const found = findClip(p, clipId);
  if (!found) return p;
  const c = found.clip;
  if (t <= c.start + 0.01 || t >= end(c) - 0.01) return p;
  const left = t - c.start;
  const a: Clip = { ...c, length: left, fadeOut: Math.min(c.fadeOut, 0), notes: c.notes?.filter((n) => n.start < left).map((n) => ({ ...n, end: Math.min(n.end, left) })) };
  const b: Clip = {
    ...c,
    id: newId("cl"),
    start: t,
    offset: c.offset + left * rate(c),
    length: c.length - left,
    fadeIn: 0,
    notes: c.notes?.filter((n) => n.end > left).map((n) => ({ ...n, start: Math.max(0, n.start - left), end: n.end - left })),
  };
  return mapTrack(p, found.track.id, (tr) => ({ ...tr, clips: tr.clips.flatMap((x) => (x.id === clipId ? [a, b] : [x])) }));
}

/** Clips copied to start `at` seconds (keeping their spacing), on their own tracks. */
export function pasteClips(p: Project, clips: { trackId: string; clip: Clip }[], at: number): Project {
  if (!clips.length) return p;
  const first = Math.min(...clips.map((c) => c.clip.start));
  let out = p;
  for (const { trackId, clip } of clips) {
    const track = out.tracks.some((t) => t.id === trackId) ? trackId : out.tracks[0]?.id;
    if (!track) continue;
    out = addClip(out, track, { ...clip, id: newId("cl"), start: at + clip.start - first });
  }
  return out;
}

/** Clips moved by `dt` seconds (not before 0), and optionally to another track. */
export function moveClips(p: Project, ids: Set<string>, dt: number, toTrack?: string): Project {
  const leaving: Clip[] = [];
  const tracks = p.tracks.map((t) => ({
    ...t,
    clips: t.clips.flatMap((c) => {
      if (!ids.has(c.id)) return [c];
      const moved = { ...c, start: Math.max(0, c.start + dt) };
      if (toTrack && toTrack !== t.id) {
        leaving.push(moved);
        return [];
      }
      return [moved];
    }),
  }));
  const out = { ...p, tracks, updated_at: Date.now() };
  return toTrack && leaving.length ? mapTrack(out, toTrack, (t) => ({ ...t, clips: [...t.clips, ...leaving] })) : out;
}

/** A clip's left or right edge moved to `t` (trimming or uncovering its source). */
export function trimClip(p: Project, clipId: string, edge: "start" | "end", t: number, sourceSecs: number): Project {
  const found = findClip(p, clipId);
  if (!found) return p;
  const c = found.clip;
  const r = rate(c);
  if (edge === "end") {
    const maxLen = c.notes ? Infinity : (sourceSecs - c.offset) / r;
    return updateClip(p, clipId, { length: Math.max(0.1, Math.min(t - c.start, maxLen)) });
  }
  // The start moves; the source under it moves with it, not past its beginning.
  const earliest = c.start - c.offset / r;
  const start = Math.min(Math.max(t, earliest, 0), end(c) - 0.1);
  const shift = start - c.start;
  return updateClip(p, clipId, {
    start,
    offset: c.offset + shift * r,
    length: c.length - shift,
    notes: c.notes?.map((n) => ({ ...n, start: n.start - shift, end: n.end - shift })).filter((n) => n.end > 0),
  });
}

/** Time from `a` to `b` taken out of every track (Suno's Delete time): what's after moves earlier. */
export function deleteTime(p: Project, a: number, b: number): Project {
  if (b <= a) return p;
  const gap = b - a;
  let out = p;
  for (const t of p.tracks) {
    for (const c of t.clips) {
      if (c.start < b && end(c) > a) {
        if (c.start < a) out = splitClip(out, c.id, a);
      }
    }
  }
  for (const t of out.tracks) {
    for (const c of t.clips) {
      if (c.start < b && end(c) > b) out = splitClip(out, c.id, b);
    }
  }
  return {
    ...out,
    tracks: out.tracks.map((t) => ({ ...t, clips: t.clips.filter((c) => !(c.start >= a - 0.001 && end(c) <= b + 0.001)).map((c) => (c.start >= b - 0.001 ? { ...c, start: c.start - gap } : c)) })),
    updated_at: Date.now(),
  };
}

/** Time from `a` to `b` copied right after itself on every track (Suno's Duplicate time). */
export function duplicateTime(p: Project, a: number, b: number): Project {
  if (b <= a) return p;
  const gap = b - a;
  let out = p;
  for (const t of p.tracks) for (const c of t.clips) if (c.start < a && end(c) > a) out = splitClip(out, c.id, a);
  for (const t of out.tracks) for (const c of t.clips) if (c.start < b && end(c) > b) out = splitClip(out, c.id, b);
  return {
    ...out,
    tracks: out.tracks.map((t) => {
      const inside = t.clips.filter((c) => c.start >= a - 0.001 && end(c) <= b + 0.001);
      const shifted = t.clips.map((c) => (c.start >= b - 0.001 ? { ...c, start: c.start + gap } : c));
      return { ...t, clips: [...shifted, ...inside.map((c) => ({ ...c, id: newId("cl"), start: c.start + gap }))] };
    }),
    updated_at: Date.now(),
  };
}

/** A take added to a clip and chosen. */
export function addTake(p: Project, clipId: string, source: string, label: string): Project {
  const found = findClip(p, clipId);
  if (!found) return p;
  const take = { id: newId("tk"), source, label };
  return updateClip(p, clipId, { takes: [...found.clip.takes, take], take: take.id });
}

/** A value on an automation lane at `t` (straight lines between points; before and after, the nearest). */
export function valueAt(points: AutoPoint[] | undefined, t: number, fallback: number) {
  if (!points?.length) return fallback;
  const sorted = [...points].sort((a, b) => a.t - b.t);
  if (t <= sorted[0].t) return sorted[0].v;
  for (let i = 1; i < sorted.length; i++) {
    const [a, b] = [sorted[i - 1], sorted[i]];
    if (t <= b.t) return a.v + ((b.v - a.v) * (t - a.t)) / Math.max(1e-6, b.t - a.t);
  }
  return sorted[sorted.length - 1].v;
}

/** Which tracks are heard: solo wins over mute. */
export function audible(p: Project): Set<string> {
  const soloed = p.tracks.some((t) => t.solo);
  return new Set(p.tracks.filter((t) => (soloed ? t.solo : !t.mute)).map((t) => t.id));
}

/** dB as a gain. */
export function gainOf(db: number) {
  return Math.pow(10, db / 20);
}
