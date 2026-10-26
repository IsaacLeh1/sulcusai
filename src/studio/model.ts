// SPDX-License-Identifier: AGPL-3.0-only
// The Studio's project (Suno Studio's): tracks of clips on a timeline, each
// clip a stretch of a gallery item (audio) or notes (MIDI), with its takes,
// fades, gain, speed and pitch; tracks with volume, pan, mute, solo,
// effects and automation. Pure functions on plain data, so every edit is a
// new project (undo keeps the old one).
import { tx } from "../i18n";
import type { MadeTable, SynthPatch } from "./synth";

export type EffectKind = "eq" | "compressor" | "reverb" | "delay" | "distortion" | "gate" | "chorus" | "phaser" | "tremolo" | "autopan" | "lowpass" | "highpass" | "bitcrusher" | "width" | "limiter";

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
  /** Seconds into its source the clip's own place starts later (a loop
   * recording's passes are takes of one recording, each a loop further on). */
  shift?: number;
}

/** A point of a clip's sound pinned to a moment: `s` seconds into the clip's
 * sound (as `offset` counts) plays `t` seconds after the clip starts; between
 * markers the sound is stretched to fit, its pitch kept. */
export interface WarpMarker {
  s: number;
  t: number;
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
  /** Audio: its formants (its character) moved, in semitones (Suno's Formant). */
  formant?: number;
  reverse: boolean;
  /** Silent and greyed (Suno's disable). */
  disabled: boolean;
  /** Audio: warp markers (Suno's Stretch), and whether they're off for now. */
  warp?: WarpMarker[];
  warpOff?: boolean;
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
  /** MIDI: the wavetable synth's sound, when it plays that instead. */
  synth?: SynthPatch;
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
  /** Wavetables made from sounds, by id, for the synth. */
  tables?: Record<string, MadeTable>;
  /** Sliders following a MIDI controller's knobs (MIDI Learn). */
  midi?: MidiBinding[];
  created_at: number;
  updated_at: number;
}

/** A MIDI control (`cc` on `channel`, 0 to 15) moving a slider (`target`, as `LearnTarget` names it) over its range. */
export interface MidiBinding {
  cc: number;
  channel: number;
  target: string;
  label: string;
  min: number;
  max: number;
  log?: boolean;
  /** Whole steps only (an octave, a voice count). */
  step?: number;
}

/** A MIDI control's value (0 to 127) on its slider's range. */
export function bindingValue(b: MidiBinding, value: number) {
  const u = Math.max(0, Math.min(1, value / 127));
  const v = b.log && b.min > 0 ? b.min * Math.pow(b.max / b.min, u) : b.min + (b.max - b.min) * u;
  return b.step ? Math.round(v / b.step) * b.step : v;
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
  chorus: { label: tx("Chorus"), params: { rate: [0.05, 5, 0.8], depth: [0, 1, 0.5], mix: [0, 1, 0.5] } },
  phaser: { label: tx("Phaser"), params: { rate: [0.05, 5, 0.5], depth: [0, 1, 0.7], mix: [0, 1, 0.5] } },
  tremolo: { label: tx("Tremolo"), params: { rate: [0.5, 20, 5], depth: [0, 1, 0.5] } },
  autopan: { label: tx("Auto-pan"), params: { rate: [0.05, 10, 0.5], depth: [0, 1, 0.8] } },
  lowpass: { label: tx("Low-pass filter"), params: { cutoff: [40, 20000, 2000], resonance: [0, 1, 0.2], rate: [0, 10, 0], depth: [0, 1, 0] } },
  highpass: { label: tx("High-pass filter"), params: { cutoff: [20, 10000, 200], resonance: [0, 1, 0.2], rate: [0, 10, 0], depth: [0, 1, 0] } },
  bitcrusher: { label: tx("Bit crusher"), params: { bits: [1, 16, 6], mix: [0, 1, 1] } },
  width: { label: tx("Stereo width"), params: { width: [0, 2, 1.4] } },
  limiter: { label: tx("Limiter"), params: { threshold: [-24, 0, -3] } },
};

/** Settings shown on a logarithmic slider (frequencies). */
export const LOG_PARAMS = new Set(["cutoff", "midFreq"]);

/** Effects kept by name to put on any track (Suno's custom plugins, built from words or saved from a track). */
export interface EffectChain {
  name: string;
  effects: Effect[];
}

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

/** The take a clip plays now. */
export function takeOf(c: Clip): Take | null {
  return c.takes.find((t) => t.id === c.take) ?? c.takes[0] ?? null;
}

/** The source a clip plays now (its chosen take's), if audio. */
export function sourceOf(c: Clip): string | null {
  return takeOf(c)?.source ?? null;
}

/** Where in its source a clip's sound starts, for the take it plays. */
export function startIn(c: Clip): number {
  return c.offset + (takeOf(c)?.shift ?? 0);
}

/** Whether a clip's warp markers stretch its sound. */
export function warped(c: Clip): boolean {
  return !!c.warp?.length && !c.warpOff && !c.notes;
}

/** The fixed points of a clip's timing: its start, then its warp markers. */
export function anchors(c: Clip): WarpMarker[] {
  const out = [{ s: c.offset, t: 0 }];
  if (!warped(c)) return out;
  // In time order, none on top of another.
  for (const m of [...c.warp!].sort((a, b) => a.t - b.t)) if (m.t > out[out.length - 1].t + 0.0005) out.push(m);
  return out;
}

/** Where in its sound (as `offset` counts) a clip is `t` seconds after its
 * start: straight between anchors, at its speed before the first and after the last. */
export function soundAt(c: Clip, t: number): number {
  const a = anchors(c);
  if (t <= 0 || a.length === 1) return c.offset + t * c.speed;
  for (let i = 1; i < a.length; i++) {
    if (t <= a[i].t) return a[i - 1].s + ((a[i].s - a[i - 1].s) * (t - a[i - 1].t)) / Math.max(1e-6, a[i].t - a[i - 1].t);
  }
  const last = a[a.length - 1];
  return last.s + (t - last.t) * c.speed;
}

/** The other way: seconds after the clip's start that sound second `s` plays. */
export function timeOfSound(c: Clip, s: number): number {
  const a = anchors(c);
  if (s <= c.offset || a.length === 1) return (s - c.offset) / c.speed;
  for (let i = 1; i < a.length; i++) {
    if (s <= a[i].s) return a[i - 1].t + ((a[i].t - a[i - 1].t) * (s - a[i - 1].s)) / Math.max(1e-6, a[i].s - a[i - 1].s);
  }
  const last = a[a.length - 1];
  return last.t + (s - last.s) / c.speed;
}

/** Markers kept in order: each later in time and in sound than the one before. */
export function tidyWarp(markers: WarpMarker[], c: Clip): WarpMarker[] {
  const out: WarpMarker[] = [];
  let prev = { s: c.offset, t: 0 };
  for (const m of [...markers].sort((a, b) => a.t - b.t)) {
    if (m.t <= prev.t + 0.01 || m.s <= prev.s + 0.005 || m.t > c.length + 0.005) continue;
    out.push(m);
    prev = m;
  }
  return out;
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
  const marks = (keep: (m: WarpMarker) => boolean, by: number) => {
    const out = c.warp?.filter(keep).map((m) => ({ ...m, t: m.t - by }));
    return out?.length ? out : undefined;
  };
  // The left piece ends on a marker of its own, so its last stretch keeps its pace.
  const tail = c.warp?.length ? [{ t: left, s: soundAt({ ...c, warpOff: false }, left) }] : [];
  const kept = marks((m) => m.t < left - 0.005, 0);
  const a: Clip = { ...c, length: left, fadeOut: Math.min(c.fadeOut, 0), notes: c.notes?.filter((n) => n.start < left).map((n) => ({ ...n, end: Math.min(n.end, left) })), warp: kept || tail.length ? [...(kept ?? []), ...tail] : undefined };
  const b: Clip = {
    ...c,
    id: newId("cl"),
    start: t,
    offset: soundAt(c, left),
    length: c.length - left,
    fadeIn: 0,
    notes: c.notes?.filter((n) => n.end > left).map((n) => ({ ...n, start: Math.max(0, n.start - left), end: n.end - left })),
    warp: marks((m) => m.t > left + 0.005, left),
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
  const ahead = takeOf(c)?.shift ?? 0;
  if (edge === "end") {
    const maxLen = c.notes ? Infinity : timeOfSound(c, sourceSecs - ahead);
    return updateClip(p, clipId, { length: Math.max(0.1, Math.min(t - c.start, maxLen)) });
  }
  // The start moves; the source under it moves with it, not past its beginning.
  const earliest = c.start - startIn(c) / r;
  const start = Math.min(Math.max(t, earliest, 0), end(c) - 0.1);
  const shift = start - c.start;
  const warp = c.warp?.map((m) => ({ ...m, t: m.t - shift })).filter((m) => m.t > 0.01);
  return updateClip(p, clipId, {
    start,
    offset: soundAt(c, shift),
    length: c.length - shift,
    notes: c.notes?.map((n) => ({ ...n, start: n.start - shift, end: n.end - shift })).filter((n) => n.end > 0),
    warp: warp?.length ? warp : undefined,
  });
}

/** A clip's warp markers each moved to the nearest grid line (`grid` beats apart), so what's under them lands on the beat (Suno's Quantize). */
export function quantizeWarp(p: Project, clipId: string, grid: number): Project {
  const c = findClip(p, clipId)?.clip;
  if (!c?.warp?.length) return p;
  return updateClip(p, clipId, { warp: tidyWarp(c.warp.map((m) => ({ ...m, t: snap(p, c.start + m.t, grid || 1) - c.start })), { ...c, warpOff: false }), warpOff: false });
}

/** An audio clip stretched (or squeezed) to `length` seconds, its pitch kept
 * (Suno's Shift-drag on a clip's edge): its speed and warp markers follow. */
export function stretchClip(p: Project, clipId: string, length: number): Project {
  const found = findClip(p, clipId);
  if (!found || found.clip.notes) return p;
  const c = found.clip;
  const to = Math.max(0.1, length);
  const k = to / c.length;
  const speed = Math.max(0.25, Math.min(4, c.speed / k));
  return updateClip(p, clipId, { length: (c.length * c.speed) / speed, speed, keepPitch: true, warp: c.warp?.map((m) => ({ ...m, t: m.t * (c.speed / speed) })) });
}

function sameTakes(a: Clip, b: Clip) {
  return a.takes.length === b.takes.length && a.takes.every((k, i) => k.id === b.takes[i].id);
}

/** The pieces of a comp a clip is one of: the clips beside it on its track,
 * end to end, with the same takes (cutting a clip keeps its takes). */
export function compGroup(p: Project, clipId: string): { track: Track; clips: Clip[] } | null {
  const found = findClip(p, clipId);
  if (!found || found.clip.notes) return null;
  const sorted = [...found.track.clips].sort((a, b) => a.start - b.start);
  const i = sorted.findIndex((c) => c.id === clipId);
  let lo = i;
  let hi = i;
  while (lo > 0 && sameTakes(sorted[lo - 1], sorted[i]) && Math.abs(end(sorted[lo - 1]) - sorted[lo].start) < 0.005) lo--;
  while (hi < sorted.length - 1 && sameTakes(sorted[hi + 1], sorted[i]) && Math.abs(end(sorted[hi]) - sorted[hi + 1].start) < 0.005) hi++;
  return { track: found.track, clips: sorted.slice(lo, hi + 1) };
}

/** A comp's pieces as one clip over them all (to draw a take across the whole comp). */
export function compWhole(clips: Clip[]): Clip {
  const first = clips[0];
  const lo = first.start;
  const warp: WarpMarker[] = [];
  for (const c of clips) {
    if (c !== first) warp.push({ s: c.offset, t: c.start - lo });
    if (!c.warpOff) for (const m of c.warp ?? []) warp.push({ ...m, t: m.t + c.start - lo });
  }
  return { ...first, length: end(clips[clips.length - 1]) - lo, warp: warp.length ? warp : undefined, warpOff: false };
}

/** Whether `b` plays straight on from `a`: the same take and settings, nothing
 * cut out or faded between. */
function joinable(a: Clip, b: Clip) {
  return (
    !a.notes &&
    !b.notes &&
    Math.abs(b.start - end(a)) < 0.005 &&
    sameTakes(a, b) &&
    a.take === b.take &&
    a.speed === b.speed &&
    a.keepPitch === b.keepPitch &&
    a.semitones === b.semitones &&
    (a.formant ?? 0) === (b.formant ?? 0) &&
    a.reverse === b.reverse &&
    a.disabled === b.disabled &&
    a.gain === b.gain &&
    !a.fadeOut &&
    !b.fadeIn &&
    !!a.warpOff === !!b.warpOff &&
    Math.abs(soundAt(a, a.length) - b.offset) < 0.002
  );
}

/** Clips that play straight on from each other joined back into one (Suno's
 * Heal edits), on a track, between `from` and `to`. */
export function heal(p: Project, trackId: string, from = -Infinity, to = Infinity): Project {
  return mapTrack(p, trackId, (tr) => {
    const out: Clip[] = [];
    for (const c of [...tr.clips].sort((a, b) => a.start - b.start)) {
      const prev = out[out.length - 1];
      if (prev && prev.start >= from - 0.001 && end(c) <= to + 0.001 && joinable(prev, c)) {
        const joint = prev.warp?.some((m) => Math.abs(m.t - prev.length) < 0.001) ? [] : [{ s: c.offset, t: prev.length }];
        const warp = [...(prev.warp ?? []), ...(c.warp?.length ? [...joint, ...c.warp.map((m) => ({ ...m, t: m.t + prev.length }))] : [])];
        out[out.length - 1] = { ...prev, length: prev.length + c.length, fadeOut: c.fadeOut, warp: warp.length ? warp : undefined };
      } else out.push(c);
    }
    return { ...tr, clips: out };
  });
}

/** The stretch from `a` to `b` (timeline seconds) of a comp played from
 * `take` (Suno's comping): its pieces cut at `a` and `b`, those between on
 * that take, and pieces that now play on from each other joined. */
export function compTake(p: Project, clipId: string, take: string, a: number, b: number): Project {
  const g = compGroup(p, clipId);
  if (!g || !g.clips[0].takes.some((k) => k.id === take)) return p;
  const lo = g.clips[0].start;
  const hi = end(g.clips[g.clips.length - 1]);
  const [from, to] = [Math.max(lo, a), Math.min(hi, b)];
  if (to - from < 0.02) return p;
  const ours = (c: Clip) => sameTakes(c, g.clips[0]) && c.start >= lo - 0.001 && end(c) <= hi + 0.001;
  let out = p;
  for (const edge of [from, to]) {
    const tr = out.tracks.find((x) => x.id === g.track.id)!;
    for (const c of tr.clips) if (ours(c) && c.start < edge - 0.005 && end(c) > edge + 0.005) out = splitClip(out, c.id, edge);
  }
  out = mapTrack(out, g.track.id, (tr) => ({ ...tr, clips: tr.clips.map((c) => (ours(c) && c.start >= from - 0.001 && end(c) <= to + 0.001 ? { ...c, take } : c)) }));
  return heal(out, g.track.id, lo, hi);
}

/** A take taken off every piece of a comp (one is always left). */
export function removeTake(p: Project, clipId: string, take: string): Project {
  const g = compGroup(p, clipId);
  if (!g || g.clips[0].takes.length < 2) return p;
  const ids = new Set(g.clips.map((c) => c.id));
  const takes = g.clips[0].takes.filter((k) => k.id !== take);
  const out = mapTrack(p, g.track.id, (tr) => ({ ...tr, clips: tr.clips.map((c) => (ids.has(c.id) ? { ...c, takes, take: c.take === take ? takes[0].id : c.take } : c)) }));
  return heal(out, g.track.id, g.clips[0].start, end(g.clips[g.clips.length - 1]));
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

/** What a recording becomes. `lead` is how far into the recording the music
 * started (at timeline second `at`). Played straight through: one clip from
 * `at`. Round a loop (Suno's loop recording): one clip over the loop with each
 * pass as a take of it (a pass is the same recording, a loop further on; the
 * last full pass chosen), and what came before the loop as a clip of its own. */
export function recordedClips(source: string, name: string, seconds: number, at: number, lead: number, loop: [number, number] | null, passName: (n: number) => string): Clip[] {
  const plain = (to = Infinity) => [{ ...audioClip(source, name, at, Math.max(0.1, Math.min(seconds - lead, to - at))), offset: lead }];
  if (!loop || loop[1] - loop[0] < 0.5 || at >= loop[1]) return plain();
  const [l0, l1] = loop;
  const span = l1 - l0;
  if (seconds - lead < l1 - at + 0.5) return plain(l1);
  // The recording's second where each pass reaches the loop's start (the
  // first can be before it began, when it began inside the loop).
  const first = lead + (l0 - at);
  const passes: number[] = [];
  for (let r = first; r < seconds - 1; r += span) {
    if (Math.min(seconds, r + span) - Math.max(0, r) >= 1) passes.push(r);
  }
  const clip = audioClip(source, name, l0, span);
  const takes = passes.map((r, i) => ({ id: `${clip.take}-${i}`, source, label: passName(i + 1), shift: r }));
  const full = passes.filter((r) => r >= 0 && r + span <= seconds + 0.05);
  const chosen = full.length ? passes.indexOf(full[full.length - 1]) : passes.length - 1;
  const out: Clip[] = [{ ...clip, takes, take: takes[chosen].id }];
  if (at < l0 - 0.05) out.unshift({ ...audioClip(source, name, at, l0 - at), offset: lead });
  return out;
}

/** A take added to a clip and chosen. */
export function addTake(p: Project, clipId: string, source: string, label: string): Project {
  const found = findClip(p, clipId);
  if (!found) return p;
  // Made from the take playing (made again, longer, played another way, made
  // dry), so it keeps that take's timing.
  const shift = takeOf(found.clip)?.shift;
  const take = { id: newId("tk"), source, label, ...(shift ? { shift } : {}) };
  // Every piece of its comp gets it (so they stay one comp); this one plays it.
  const pieces = new Set((compGroup(p, clipId)?.clips ?? [found.clip]).map((c) => c.id));
  return mapTrack(p, found.track.id, (tr) => ({ ...tr, clips: tr.clips.map((c) => (pieces.has(c.id) ? { ...c, takes: [...c.takes, take], take: c.id === clipId ? take.id : c.take } : c)) }));
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
