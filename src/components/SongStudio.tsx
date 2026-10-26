// SPDX-License-Identifier: AGPL-3.0-only
// The song studio (Suno Studio's): a song's tracks as clips on a timeline to
// arrange, mix, record into and add to with the music model. Clips move,
// trim, split, fade, copy and change speed or key; tracks have volume, pan,
// mute, solo, effects and automation; MIDI tracks play notes on sampled
// instruments; new parts come in takes to pick from. Kept as the window edits.
import { useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { api, errorText, fromBase64, on, toBase64, type MediaItem, type MediaRequest, type StudioAction } from "../api";
import { clock } from "../format";
import { t, tx } from "../i18n";
import { encodeWav } from "../media";
import { render, sourceBuffer, StudioEngine } from "../studio/engine";
import { DRUM_KIT, GM, programFor } from "../studio/gm";
import { expect, setOpen, waitingFor } from "../studio/jobs";
import { DRUMS, LiveSynth, midiFile, readMidiFile } from "../studio/midi";
import {
  addClip,
  addTrack,
  audioClip,
  beatSecs,
  compGroup,
  compTake,
  compWhole,
  deleteTime,
  duplicateTime,
  duplicateTrack,
  EFFECTS,
  end,
  LOG_PARAMS,
  type EffectChain,
  findClip,
  length as projectLength,
  midiClip,
  moveClips,
  moveTrack,
  newEffect,
  newId,
  newTrack,
  bindingValue,
  pasteClips,
  quantizeWarp,
  recordedClips,
  removeClips,
  removeTake,
  removeTrack,
  snap,
  soundAt,
  sourceOf,
  splitClip,
  startIn,
  stretchClip,

  tidyWarp,
  timeOfSound,
  trimClip,
  updateClip,
  updateTrack,
  warped,
  type AutoPoint,
  type Clip,
  type EffectKind,
  type Note,
  type Project,
  type Track,
  type WarpMarker,
} from "../studio/model";
import { applyBinding, LearnContext, learnable, type Learn, type LearnTarget } from "../studio/learn";
import { onsets } from "../studio/onsets";
import { encodeFrames, framesFromSound, initPatch, LiveWave, PRESETS, withSettings, type SynthPatch } from "../studio/synth";
import { SynthPanel } from "./SynthPanel";
import { NewWindowIcon } from "./Icons";
import { PianoRoll } from "./PianoRoll";
import type { PushToast } from "./Toasts";

const LANE = 68;
const HEAD = 230;
const HISTORY = 100;
const PARTS = ["vocals", "backing_vocals", "drums", "bass", "guitar", "keyboard", "percussion", "strings", "synth", "fx", "brass", "woodwinds"];
const PART_NAMES: Record<string, string> = {
  vocals: tx("Vocals"), backing_vocals: tx("Backing vocals"), drums: tx("Drums"), bass: tx("Bass"), guitar: tx("Guitar"), keyboard: tx("Keys"), percussion: tx("Percussion"), strings: tx("Strings"), synth: tx("Synth"), fx: tx("Effects"), brass: tx("Brass"), woodwinds: tx("Woodwinds"),
};
// An effect setting's name, in words.
const PARAM_NAMES: Record<string, string> = {
  low: tx("Low"), mid: tx("Middle"), high: tx("High"), midFreq: tx("Middle at (Hz)"), threshold: tx("Threshold (dB)"), ratio: tx("Ratio"), attack: tx("Attack (s)"), release: tx("Release (s)"), makeup: tx("Make-up (dB)"), size: tx("Room size (s)"), mix: tx("Mix"), time: tx("Time (s)"), feedback: tx("Repeats"), drive: tx("Drive"),
  rate: tx("Rate (Hz)"), depth: tx("Depth"), cutoff: tx("Cutoff (Hz)"), resonance: tx("Resonance"), bits: tx("Bits"), width: tx("Width"),
};
const GRIDS: [number, string][] = [[0, tx("Off")], [0.25, "1/16"], [0.5, "1/8"], [1, tx("Beat")], [4, tx("Bar")]];
// Musical typing: A to ; the white keys, W E T Y U O P the black ones (Suno's).
const TYPING: Record<string, number> = { a: 0, w: 1, s: 2, e: 3, d: 4, f: 5, t: 6, g: 7, y: 8, h: 9, u: 10, j: 11, k: 12, o: 13, l: 14, p: 15, ";": 16 };

let clipboard: { trackId: string; clip: Clip }[] = [];

type Dock = "clip" | "takes" | "effects" | "automation" | "notes" | "synth" | "make";

/** A clip's sound drawn as its peaks (following its warp markers). */
function Wave({ clip, width, height, color = "rgba(255,255,255,0.75)" }: { clip: Clip; width: number; height: number; color?: string }) {
  const ref = useRef<HTMLCanvasElement>(null);
  const src = sourceOf(clip);
  const ahead = startIn(clip) - clip.offset;
  const warpKey = warped(clip) ? JSON.stringify(clip.warp) : "";
  useEffect(() => {
    let live = true;
    if (!src || width < 2) return;
    sourceBuffer(src)
      .then((buf) => {
        const c = ref.current;
        if (!live || !c) return;
        const w = (c.width = Math.min(4000, Math.round(width)));
        const h = (c.height = height);
        const g = c.getContext("2d")!;
        const data = buf.getChannelData(0);
        const at = (x: number) => (ahead + soundAt(clip, (x / w) * clip.length)) * buf.sampleRate;
        g.clearRect(0, 0, w, h);
        g.fillStyle = color;
        for (let x = 0; x < w; x++) {
          let peak = 0;
          const a = Math.max(0, Math.floor(at(x)));
          const b = Math.min(data.length, Math.floor(at(x + 1)));
          for (let i = a; i < b; i += 8) peak = Math.max(peak, Math.abs(data[i] ?? 0));
          const bar = Math.max(1, peak * h * 0.9);
          g.fillRect(clip.reverse ? w - 1 - x : x, (h - bar) / 2, 1, bar);
        }
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [src, ahead, clip.offset, clip.length, clip.speed, clip.reverse, warpKey, width, height, color]); // eslint-disable-line react-hooks/exhaustive-deps
  return <canvas ref={ref} className="clip-wave" />;
}

/** A MIDI clip's notes, small. */
function MiniNotes({ notes, length, width, height }: { notes: Note[]; length: number; width: number; height: number }) {
  if (!notes.length) return null;
  const ps = notes.map((n) => n.pitch);
  const [lo, hi] = [Math.min(...ps) - 1, Math.max(...ps) + 1];
  const rowH = height / Math.max(6, hi - lo + 1);
  return (
    <svg width={width} height={height} className="clip-notes">
      {notes.map((n, i) => (
        <rect key={i} x={(n.start / length) * width} y={(hi - n.pitch) * rowH} width={Math.max(1.5, ((n.end - n.start) / length) * width)} height={Math.max(1.5, rowH - 0.5)} />
      ))}
    </svg>
  );
}

/** An automation lane (volume in dB or pan) over the song: click to add a point, drag to move, right-click to remove. */
function AutoLane({ points, min, max, fallback, length: len, onChange, label }: { points: AutoPoint[]; min: number; max: number; fallback: number; length: number; onChange: (p: AutoPoint[]) => void; label: string }) {
  const W = 900;
  const H = 70;
  const ref = useRef<SVGSVGElement>(null);
  const drag = useRef<number | null>(null);
  const sorted = [...points].sort((a, b) => a.t - b.t);
  const x = (s: number) => (s / len) * W;
  const y = (v: number) => H - ((v - min) / (max - min)) * H;
  const at = (e: { clientX: number; clientY: number }) => {
    const r = ref.current!.getBoundingClientRect();
    const tt = Math.max(0, Math.min(len, ((e.clientX - r.left) / r.width) * len));
    const v = Math.max(min, Math.min(max, max - ((e.clientY - r.top) / r.height) * (max - min)));
    return { t: Math.round(tt * 100) / 100, v: Math.round(v * 100) / 100 };
  };
  useEffect(() => {
    const move = (e: MouseEvent) => {
      if (drag.current === null) return;
      const p = at(e);
      onChange(sorted.map((q, i) => (i === drag.current ? p : q)));
    };
    const up = () => (drag.current = null);
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
  });
  const line = sorted.length ? [{ t: 0, v: sorted[0].v }, ...sorted, { t: len, v: sorted[sorted.length - 1].v }] : [{ t: 0, v: fallback }, { t: len, v: fallback }];
  return (
    <div className="auto-lane">
      <span className="small muted">{label}</span>
      <svg
        ref={ref}
        viewBox={`0 0 ${W} ${H}`}
        preserveAspectRatio="none"
        className="auto-svg"
        onMouseDown={(e) => {
          if (e.button !== 0 || e.target !== e.currentTarget) return;
          const p = at(e);
          onChange([...sorted, p]);
        }}
      >
        <polyline points={line.map((p) => `${x(p.t)},${y(p.v)}`).join(" ")} className="auto-line" />
        {sorted.map((p, i) => (
          <circle
            key={i}
            cx={x(p.t)}
            cy={y(p.v)}
            r={5}
            className="auto-point"
            onMouseDown={(e) => {
              e.stopPropagation();
              drag.current = i;
            }}
            onContextMenu={(e) => {
              e.preventDefault();
              onChange(sorted.filter((_, j) => j !== i));
            }}
          />
        ))}
      </svg>
    </div>
  );
}

/** `onPopOut` moves it to a window of its own (none when it's in one: `inWindow`). */
export function SongStudio({ initial, onClose, toast, onSaved, songs = [], onPopOut, inWindow = false }: { initial: Project; onClose: () => void; toast: PushToast; onSaved?: () => void; songs?: MediaItem[]; onPopOut?: () => void; inWindow?: boolean }) {
  const engine = useRef<StudioEngine | null>(null);
  // Made when first needed (and again after closing: React mounts twice while developing).
  const eng = () => (engine.current ??= new StudioEngine());
  const synth = useRef<LiveSynth | null>(null);
  const wave = useRef<LiveWave | null>(null);
  const [p, setP] = useState<Project>(initial);
  const [past, setPast] = useState<Project[]>([]);
  const [future, setFuture] = useState<Project[]>([]);
  const [sel, setSel] = useState<Set<string>>(new Set());
  const [focus, setFocus] = useState<string | null>(initial.tracks[0]?.id ?? null);
  const [range, setRange] = useState<[number, number] | null>(null);
  const [pos, setPos] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [click, setClick] = useState(false);
  const [grid, setGrid] = useState(1);
  const [pps, setPps] = useState(12);
  const [dock, setDock] = useState<Dock>("clip");
  const [saved, setSaved] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [rec, setRec] = useState<{ track: string; at: number; level: number; secs: number } | null>(null);
  const [typing, setTyping] = useState(false);
  const typed = useRef<{ start: number; notes: Note[]; held: Map<number, number> } | null>(null);
  const [secs, setSecs] = useState<Record<string, number>>({});
  const [master, setMaster] = useState(0);
  useEffect(() => {
    engine.current?.setMaster(master);
  }, [master]);
  const [waiting, setWaiting] = useState(0);
  const drag = useRef<{ kind: "move" | "start" | "end" | "stretch" | "fadeIn" | "fadeOut" | "range" | "ruler"; x: number; y: number; base: Project; clip?: string; t0: number } | null>(null);
  const scroll = useRef<HTMLDivElement>(null);

  const len = projectLength(p);
  const width = len * pps + 200;
  const x = (s: number) => s * pps;
  const focusTrack = p.tracks.find((tr) => tr.id === focus) ?? null;
  // The synth's track stays the last MIDI track picked (picking a clip to make a wavetable from moves the focus).
  const [synthTrackId, setSynthTrackId] = useState<string | null>(null);
  useEffect(() => {
    if (focusTrack?.kind === "midi") setSynthTrackId(focusTrack.id);
  }, [focusTrack?.id, focusTrack?.kind]);
  const synthTrack = focusTrack?.kind === "midi" ? focusTrack : p.tracks.find((tr) => tr.id === synthTrackId && tr.kind === "midi") ?? null;
  const selected = sel.size === 1 ? findClip(p, [...sel][0]) : null;

  // ---------- history and keeping ----------
  const commit = useCallback((next: Project | ((cur: Project) => Project)) => {
    setP((cur) => {
      const n = typeof next === "function" ? next(cur) : next;
      if (n === cur) return cur;
      setPast((h) => [...h.slice(-(HISTORY - 1)), cur]);
      setFuture([]);
      setSaved(false);
      return n;
    });
  }, []);
  const undo = () => {
    if (!past.length) return;
    setFuture((f) => [p, ...f]);
    setP(past[past.length - 1]);
    setPast((h) => h.slice(0, -1));
    setSaved(false);
  };
  const redo = () => {
    if (!future.length) return;
    setPast((h) => [...h, p]);
    setP(future[0]);
    setFuture((f) => f.slice(1));
    setSaved(false);
  };
  useEffect(() => {
    if (saved) return;
    const id = setTimeout(() => {
      api
        .studioProjectSave({ ...p, updated_at: Date.now() })
        .then(() => {
          setSaved(true);
          onSaved?.();
        })
        .catch((e) => toast(errorText(e), "error"));
    }, 1200);
    return () => clearTimeout(id);
  }, [p, saved]); // eslint-disable-line react-hooks/exhaustive-deps

  // Results of jobs started here go straight in (undoable).
  useEffect(() => {
    setOpen({ id: p.id, apply: (f) => commit(f) });
    const sub = on("media:done", () => setTimeout(() => setWaiting(waitingFor(p.id)), 50));
    return () => {
      setOpen(null);
      sub.then((u) => u());
    };
  }, [p.id, commit]);

  // Each source's length, for trimming.
  useEffect(() => {
    const ids = new Set(p.tracks.flatMap((tr) => tr.clips.flatMap((c) => c.takes.map((k) => k.source))));
    for (const id of ids) {
      if (secs[id] !== undefined) continue;
      api
        .mediaGet(id)
        .then((it) => setSecs((s) => ({ ...s, [id]: it.seconds || 600 })))
        .catch(() => setSecs((s) => ({ ...s, [id]: 600 })));
    }
  }, [p, secs]);
  const sourceSecs = (c: Clip) => (c.notes ? Infinity : secs[sourceOf(c) ?? ""] ?? 600);

  useEffect(
    () => () => {
      engine.current?.close();
      engine.current = null;
      synth.current?.close();
      synth.current = null;
      wave.current?.close();
      wave.current = null;
      if (rec) api.studioRecordCancel().catch(() => {});
    },
    [], // eslint-disable-line react-hooks/exhaustive-deps
  );

  // ---------- playing ----------
  const stop = useCallback(() => {
    engine.current?.stop();
    setPlaying(false);
  }, []);
  /** Plays from `from`: the project, or another version of it (a take auditioned), after a count-in if asked. */
  const play = async (from = pos, what: Project = p, countIn = 0) => {
    try {
      setBusy(t("Getting the sounds ready…"));
      await eng().play(what, from, click, countIn);
      eng().setMaster(master);
      setPlaying(true);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };
  // The playhead (a timer: it keeps time with the window hidden too; the engine goes round a loop).
  useEffect(() => {
    if (!playing) return;
    const id = setInterval(() => {
      const now = eng().position();
      setPos(now);
      // The timeline follows the playhead.
      const el = scroll.current;
      const px = HEAD + now * pps;
      if (el && (px > el.scrollLeft + el.clientWidth - 40 || px < el.scrollLeft + HEAD)) el.scrollLeft = Math.max(0, px - HEAD - 40);
      if (now >= len + 1 && !rec) stop();
    }, 50);
    return () => clearInterval(id);
  }, [playing, len, rec, pps]); // eslint-disable-line react-hooks/exhaustive-deps
  // Edits while playing: the mix's settings at once, anything else from where it is.
  const lastPlayed = useRef(p);
  useEffect(() => {
    const before = lastPlayed.current;
    lastPlayed.current = p;
    if (!playing) return;
    const mixOnly = before.loop === p.loop && before.bpm === p.bpm && before.tracks.length === p.tracks.length && p.tracks.every((tr, i) => tr.clips === before.tracks[i].clips && tr.program === before.tracks[i].program && !!tr.synth === !!before.tracks[i].synth && tr.effects.length === before.tracks[i].effects.length && tr.effects.every((e, j) => e.kind === before.tracks[i].effects[j].kind && e.bypass === before.tracks[i].effects[j].bypass) && tr.automation === before.tracks[i].automation);
    if (mixOnly) {
      for (const tr of p.tracks) eng().updateTrack(p, tr);
      return;
    }
    const id = setTimeout(() => play(eng().position()), 250);
    return () => clearTimeout(id);
  }, [p]); // eslint-disable-line react-hooks/exhaustive-deps

  // ---------- recording ----------
  useEffect(() => {
    const sub = on("studio:level", (l) => setRec((r) => (r ? { ...r, level: l.level, secs: l.seconds } : r)));
    return () => {
      sub.then((u) => u());
    };
  }, []);
  const [countIn, setCountIn] = useState(0);
  // When the recording began and when the music was first heard (performance.now()), to line them up.
  const recTimes = useRef<{ began: number; heard: number | null; loop: [number, number] | null }>({ began: 0, heard: null, loop: null });
  const record = async () => {
    if (rec) {
      stop();
      try {
        const item = await api.studioRecordStop();
        const { began, heard, loop } = recTimes.current;
        // The music started this far into the recording: that's where the clip's sound begins.
        const lead = heard === null ? 0 : Math.max(0, Math.min(item.seconds - 0.1, (heard - began) / 1000));
        const clips = recordedClips(item.id, t("Recording"), item.seconds, rec.at, lead, loop, (n) => t("Pass {n}", { n }));
        commit((cur) => clips.reduce((acc, c) => addClip(acc, rec.track, c), cur));
        if (clips.length && clips[clips.length - 1].takes.length > 1) {
          setSel(new Set([clips[clips.length - 1].id]));
          setDock("takes");
          toast(t("Each pass round the loop is a take: pick the best, or the best parts of each, under Takes."), "success");
        }
      } catch (e) {
        toast(errorText(e), "error");
      }
      setRec(null);
      return;
    }
    let target = focusTrack?.kind === "audio" ? focusTrack.id : null;
    if (!target) {
      const tr = newTrack(t("Recording"), "audio", p.tracks.length);
      commit((cur) => addTrack(cur, tr));
      target = tr.id;
      setFocus(tr.id);
    }
    try {
      await api.studioRecordStart();
      recTimes.current = { began: performance.now(), heard: null, loop: p.loop && pos < p.loop[1] ? p.loop : null };
      setRec({ track: target, at: pos, level: 0, secs: 0 });
      await play(pos, p, countIn * bar);
      recTimes.current.heard = eng().heardAt();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  // ---------- editing ----------
  const del = () => {
    if (!sel.size) return;
    commit((cur) => removeClips(cur, sel));
    setSel(new Set());
  };
  const split = () => {
    const ids = sel.size ? [...sel] : (focusTrack?.clips ?? []).filter((c) => c.start < pos && end(c) > pos).map((c) => c.id);
    commit((cur) => ids.reduce((acc, id) => splitClip(acc, id, pos), cur));
  };
  const copy = (cut: boolean) => {
    clipboard = [...sel].map((id) => findClip(p, id)).filter((f): f is NonNullable<typeof f> => !!f).map((f) => ({ trackId: f.track.id, clip: f.clip }));
    if (cut) del();
  };
  const paste = () => {
    if (!clipboard.length) return;
    const target = focus ?? clipboard[0].trackId;
    const single = new Set(clipboard.map((c) => c.trackId)).size === 1;
    commit((cur) => pasteClips(cur, clipboard.map((c) => ({ ...c, trackId: single ? target : c.trackId })), pos));
  };
  const duplicate = () => {
    const found = [...sel].map((id) => findClip(p, id)).filter((f): f is NonNullable<typeof f> => !!f);
    if (!found.length) return;
    const after = Math.max(...found.map((f) => end(f.clip)));
    commit((cur) => pasteClips(cur, found.map((f) => ({ trackId: f.track.id, clip: f.clip })), after));
  };
  const setTrack = (id: string, patch: Partial<Track>) => commit((cur) => updateTrack(cur, id, patch));
  const setClip = (id: string, patch: Partial<Clip>) => commit((cur) => updateClip(cur, id, patch));
  const addEmptyTrack = (kind: Track["kind"]) => {
    const tr = newTrack(kind === "midi" ? t("Keys") : t("Audio"), kind, p.tracks.length);
    commit((cur) => addTrack(cur, tr));
    setFocus(tr.id);
  };

  // ---------- the instruments played live ----------
  /** A note played now on the focused MIDI track's instrument: its synth, or its sampled instrument. */
  const liveOn = (pitch: number, velocity = 100) => {
    const tr = focusTrack?.kind === "midi" ? focusTrack : null;
    if (tr?.synth) {
      wave.current ??= new LiveWave();
      wave.current.on(tr.synth, pitch, velocity, p.tables).catch((e) => toast(errorText(e), "error"));
      return;
    }
    synth.current ??= new LiveSynth();
    synth.current.on(tr?.program ?? 0, pitch, velocity).catch(() => {});
  };
  /** Let go on both (the focus may have moved while it was held). */
  const liveOff = (pitch: number) => {
    wave.current?.off(pitch);
    synth.current?.off(focusTrack?.kind === "midi" ? focusTrack.program ?? 0 : 0, pitch);
  };
  // A synth's sound changing as a slider moves is one step to undo, from where it started.
  const synthFrom = useRef<Project | null>(null);
  const setSynth = (trackId: string, synthPatch: SynthPatch | undefined, done: boolean) => {
    if (!done) {
      synthFrom.current ??= p;
      setP((cur) => updateTrack(cur, trackId, { synth: synthPatch }));
      return;
    }
    const base = synthFrom.current;
    synthFrom.current = null;
    if (!base) return commit((cur) => updateTrack(cur, trackId, { synth: synthPatch }));
    setP((cur) => updateTrack(cur, trackId, { synth: synthPatch }));
    setPast((h) => [...h.slice(-(HISTORY - 1)), base]);
    setFuture([]);
    setSaved(false);
  };
  /** A wavetable made from a sound file, or from the selected audio clip's first seconds; kept with the project. */
  const makeTable = async (from: "file" | "clip"): Promise<string | null> => {
    try {
      let buf: AudioBuffer;
      let name: string;
      if (from === "file") {
        const { open: openFiles } = await import("@tauri-apps/plugin-dialog");
        const got = await openFiles({ multiple: false, filters: [{ name: t("Sounds"), extensions: ["wav", "mp3", "flac", "ogg", "m4a", "aac"] }] });
        const path = Array.isArray(got) ? got[0] : got;
        if (!path) return null;
        const data = fromBase64(await api.studioReadSound(path));
        const ctx = new OfflineAudioContext(1, 1, 48000);
        buf = await ctx.decodeAudioData(data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength) as ArrayBuffer);
        name = (path.split(/[\\/]/).pop() ?? "").replace(/\.[^.]+$/, "") || t("Wavetable");
      } else {
        const src = selected && selected.track.kind === "audio" ? sourceOf(selected.clip) : null;
        if (!selected || !src) return null;
        const whole = await sourceBuffer(src);
        const a = Math.max(0, Math.floor(startIn(selected.clip) * whole.sampleRate));
        const n = Math.min(whole.length - a, Math.floor(Math.min(4, selected.clip.length * selected.clip.speed) * whole.sampleRate));
        buf = new AudioBuffer({ length: Math.max(1, n), numberOfChannels: whole.numberOfChannels, sampleRate: whole.sampleRate });
        for (let c = 0; c < whole.numberOfChannels; c++) buf.copyToChannel(whole.getChannelData(c).slice(a, a + n), c);
        name = selected.clip.name;
      }
      const mono = new Float32Array(buf.length);
      for (let c = 0; c < buf.numberOfChannels; c++) {
        const d = buf.getChannelData(c);
        for (let i = 0; i < mono.length; i++) mono[i] += d[i] / buf.numberOfChannels;
      }
      const frames = framesFromSound(mono, buf.sampleRate);
      const id = newId("wt");
      commit((cur) => ({ ...cur, tables: { ...(cur.tables ?? {}), [id]: { name: name.slice(0, 40), frames: encodeFrames(frames) } }, updated_at: Date.now() }));
      toast(t("A wavetable of {n} frames made from {name}.", { n: frames.length, name }), "success");
      return id;
    } catch (e) {
      toast(errorText(e), "error");
      return null;
    }
  };

  // ---------- keys ----------
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement;
      if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement || el.closest(".piano-roll")) return;
      const mod = e.ctrlKey || e.metaKey;
      const k = e.key.toLowerCase();
      if (typing && !mod && TYPING[k] !== undefined) {
        e.preventDefault();
        if (e.repeat) return;
        const pitch = 60 + TYPING[k];
        liveOn(pitch);
        if (typed.current) typed.current.held.set(pitch, eng().position());
        return;
      }
      if (e.key === " ") {
        e.preventDefault();
        if (playing) stop();
        else play(e.altKey && range ? range[0] : pos);
      } else if (mod && k === "z" && !e.shiftKey) (e.preventDefault(), undo());
      else if (mod && (k === "y" || (k === "z" && e.shiftKey))) (e.preventDefault(), redo());
      else if (mod && e.shiftKey && (e.key === "Backspace" || e.key === "Delete") && range) (e.preventDefault(), commit((cur) => deleteTime(cur, range[0], range[1])));
      else if (mod && e.shiftKey && k === "d" && range) (e.preventDefault(), commit((cur) => duplicateTime(cur, range[0], range[1])));
      else if (e.key === "Delete" || e.key === "Backspace") (e.preventDefault(), del());
      else if (mod && k === "c") (e.preventDefault(), copy(false));
      else if (mod && k === "x") (e.preventDefault(), copy(true));
      else if (mod && k === "v") (e.preventDefault(), paste());
      else if (mod && k === "d") (e.preventDefault(), duplicate());
      else if (mod && k === "e") (e.preventDefault(), split());
      else if (mod && k === "l") (e.preventDefault(), commit((cur) => ({ ...cur, loop: cur.loop ? null : range ?? [pos, pos + beatSecs(cur) * cur.beats * 4] })));
      else if (mod && k === "k") (e.preventDefault(), setTyping((x) => !x));
      else if (mod && (e.key === "=" || e.key === "+")) (e.preventDefault(), setPps((z) => Math.min(400, z * 1.4)));
      else if (mod && e.key === "-") (e.preventDefault(), setPps((z) => Math.max(2, z / 1.4)));
      else if (mod && k === "a") (e.preventDefault(), setSel(new Set(p.tracks.flatMap((tr) => tr.clips.map((c) => c.id)))));
      else if (e.shiftKey && k === "c") setClick((c) => !c);
      else if (e.shiftKey && k === "r") record();
      else if (e.shiftKey && k === "t") addEmptyTrack("audio");
      else if (e.shiftKey && k === "m" && focusTrack) setTrack(focusTrack.id, { mute: !focusTrack.mute });
      else if (e.shiftKey && k === "s" && focusTrack) setTrack(focusTrack.id, { solo: !focusTrack.solo });
      else if (e.shiftKey && k === "f" && sel.size) commit((cur) => [...sel].reduce((acc, id) => {
        const c = findClip(acc, id)?.clip;
        return c ? updateClip(acc, id, c.fadeIn || c.fadeOut ? { fadeIn: 0, fadeOut: 0 } : { fadeIn: 0.5, fadeOut: 0.5 }) : acc;
      }, cur));
      else if (e.shiftKey && k === "q" && sel.size) commit((cur) => [...sel].reduce((acc, id) => quantizeWarp(acc, id, grid), cur));
      else if (!mod && !e.shiftKey && e.key === "3") setDock("takes");
      else if (e.key === "0" && sel.size) commit((cur) => [...sel].reduce((acc, id) => {
        const c = findClip(acc, id)?.clip;
        return c ? updateClip(acc, id, { disabled: !c.disabled }) : acc;
      }, cur));
      else if (e.key === "Escape") (setSel(new Set()), setRange(null), setLearning(null));
    };
    const up = (e: KeyboardEvent) => {
      const k = e.key.toLowerCase();
      if (!typing || TYPING[k] === undefined) return;
      const pitch = 60 + TYPING[k];
      liveOff(pitch);
      const tp = typed.current;
      const began = tp?.held.get(pitch);
      if (tp && began !== undefined) {
        tp.held.delete(pitch);
        tp.notes.push({ pitch, start: began - tp.start, end: eng().position() - tp.start, velocity: 100 });
      }
    };
    window.addEventListener("keydown", key);
    window.addEventListener("keyup", up);
    return () => {
      window.removeEventListener("keydown", key);
      window.removeEventListener("keyup", up);
    };
  });

  // ---------- MIDI Learn ----------
  const pRef = useRef(p);
  pRef.current = p;
  const [learning, setLearning] = useState<LearnTarget | null>(null);
  // Knobs move sliders without a step to undo each; a pause makes them one.
  const turnedFrom = useRef<Project | null>(null);
  const turnedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const turned = (f: (cur: Project) => Project) => {
    turnedFrom.current ??= pRef.current;
    setP((cur) => f(cur));
    setSaved(false);
    if (turnedTimer.current) clearTimeout(turnedTimer.current);
    turnedTimer.current = setTimeout(() => {
      const base = turnedFrom.current;
      turnedFrom.current = null;
      if (base) {
        setPast((h) => [...h.slice(-(HISTORY - 1)), base]);
        setFuture([]);
      }
    }, 600);
  };
  const learn: Learn = {
    learning: learning?.id ?? null,
    bound: new Set((p.midi ?? []).map((b) => b.target)),
    toggle: (target) => {
      if ((p.midi ?? []).some((b) => b.target === target.id)) {
        commit((cur) => ({ ...cur, midi: (cur.midi ?? []).filter((b) => b.target !== target.id), updated_at: Date.now() }));
        toast(t("{what} no longer follows a MIDI control.", { what: target.label }), "success");
        return;
      }
      setLearning((cur) => (cur?.id === target.id ? null : target));
    },
  };
  /** A control change: learnt, followed, or one of Suno's transport controls (CC 115 to 118: play, stop, record, loop). True when used. */
  const midiCc = useRef<(channel: number, cc: number, value: number) => boolean>(() => false);
  midiCc.current = (channel, cc, value) => {
    if (learning) {
      const bind = { cc, channel, target: learning.id, label: learning.label, min: learning.min, max: learning.max, log: learning.log, step: learning.step };
      commit((cur) => ({ ...cur, midi: [...(cur.midi ?? []).filter((b) => b.target !== bind.target && !(b.cc === cc && b.channel === channel)), bind], updated_at: Date.now() }));
      toast(t("{what} follows MIDI control {cc} now (right-click it to forget).", { what: learning.label, cc }), "success");
      setLearning(null);
      return true;
    }
    const hits = (p.midi ?? []).filter((b) => b.cc === cc && b.channel === channel);
    if (hits.length) {
      for (const b of hits) {
        const v = bindingValue(b, value);
        if (b.target === "master") setMaster(v);
        else turned((cur) => applyBinding(cur, b.target, v));
      }
      return true;
    }
    if (cc >= 115 && cc <= 118) {
      if (value > 0) {
        if (cc === 115) play();
        else if (cc === 116) stop();
        else if (cc === 117) record();
        else commit((cur) => ({ ...cur, loop: cur.loop ? null : range ?? [pos, pos + bar * 4] }));
      }
      return true;
    }
    return false;
  };
  const midiClock = useRef<(status: number) => void>(() => {});
  midiClock.current = (status) => {
    if (status === 0xfc) stop();
    else if (status === 0xfa) play(0);
    else play(pos);
  };

  // A MIDI keyboard plays like musical typing (and is kept the same way while playing).
  const midiNote = useRef<(on: boolean, pitch: number, velocity: number) => void>(() => {});
  midiNote.current = (onOff, pitch, velocity) => {
    if (onOff) {
      liveOn(pitch, velocity);
      if (typed.current) typed.current.held.set(pitch, eng().position());
      return;
    }
    liveOff(pitch);
    const tp = typed.current;
    const began = tp?.held.get(pitch);
    if (tp && began !== undefined) {
      tp.held.delete(pitch);
      tp.notes.push({ pitch, start: began - tp.start, end: eng().position() - tp.start, velocity });
    }
  };
  useEffect(() => {
    const nav = navigator as Navigator & { requestMIDIAccess?: () => Promise<MIDIAccess> };
    if (!nav.requestMIDIAccess) return;
    let access: MIDIAccess | null = null;
    const listen = (e: MIDIMessageEvent) => {
      const d = e.data;
      if (!d?.length) return;
      // The clock's start, continue and stop (one byte each).
      if (d[0] === 0xfa || d[0] === 0xfb || d[0] === 0xfc) return midiClock.current(d[0]);
      if (d.length < 3) return;
      const kind = d[0] & 0xf0;
      if (kind === 0x90 && d[2] > 0) midiNote.current(true, d[1], d[2]);
      else if (kind === 0x80 || (kind === 0x90 && d[2] === 0)) midiNote.current(false, d[1], 100);
      // The mod wheel and pitch bend reach the synth.
      else if (kind === 0xb0 && midiCc.current(d[0] & 0x0f, d[1], d[2])) return;
      else if (kind === 0xb0 && d[1] === 1) wave.current?.control("mod", d[2] / 127);
      else if (kind === 0xe0) wave.current?.control("bend", (((d[2] << 7) | d[1]) - 8192) / 8192);
    };
    nav
      .requestMIDIAccess()
      .then((a) => {
        access = a;
        a.inputs.forEach((i) => (i.onmidimessage = listen));
        a.onstatechange = () => a.inputs.forEach((i) => (i.onmidimessage = listen));
      })
      .catch(() => {});
    return () => {
      access?.inputs.forEach((i) => (i.onmidimessage = null));
    };
  }, []);

  // Musical typing into a MIDI track while playing: the notes become a clip when it stops.
  useEffect(() => {
    if (playing && typing && focusTrack?.kind === "midi") {
      typed.current = { start: eng().position(), notes: [], held: new Map() };
      return;
    }
    const tp = typed.current;
    typed.current = null;
    if (tp?.notes.length && focusTrack?.kind === "midi") {
      const last = Math.max(...tp.notes.map((n) => n.end));
      commit((cur) => addClip(cur, focusTrack.id, midiClip(t("Played"), tp.start, last + 0.5, tp.notes)));
    }
  }, [playing]); // eslint-disable-line react-hooks/exhaustive-deps

  // ---------- the mouse on the timeline ----------
  const timeAt = (clientX: number) => {
    const r = scroll.current!.getBoundingClientRect();
    return Math.max(0, (clientX - r.left + scroll.current!.scrollLeft - HEAD) / pps);
  };
  const laneAt = (clientY: number) => {
    const r = scroll.current!.getBoundingClientRect();
    return Math.floor((clientY - r.top + scroll.current!.scrollTop - 28) / LANE);
  };
  useEffect(() => {
    const move = (e: MouseEvent) => {
      const d = drag.current;
      if (!d) return;
      const tNow = timeAt(e.clientX);
      const dt = (e.clientX - d.x) / pps;
      if (d.kind === "range" || d.kind === "ruler") {
        const a = snap(p, Math.min(d.t0, tNow), grid);
        const b = snap(p, Math.max(d.t0, tNow), grid);
        setRange(b - a > 0.05 ? [a, b] : null);
        return;
      }
      const found = d.clip ? findClip(d.base, d.clip) : null;
      if (!found) return;
      const c = found.clip;
      if (d.kind === "move") {
        const target = d.base.tracks[Math.max(0, Math.min(d.base.tracks.length - 1, laneAt(e.clientY)))];
        const to = target && target.kind === found.track.kind && target.id !== found.track.id ? target.id : undefined;
        const ids = sel.has(c.id) ? sel : new Set([c.id]);
        const snapped = snap(d.base, c.start + dt, grid) - c.start;
        setP(moveClips(d.base, ids, snapped, ids.size === 1 ? to : undefined));
      } else if (d.kind === "start" || d.kind === "end") {
        setP(trimClip(d.base, c.id, d.kind, snap(d.base, (d.kind === "start" ? c.start : end(c)) + dt, grid), sourceSecs(c)));
      } else if (d.kind === "stretch") {
        setP(stretchClip(d.base, c.id, snap(d.base, end(c) + dt, grid) - c.start));
      } else if (d.kind === "fadeIn") {
        setP(updateClip(d.base, c.id, { fadeIn: Math.max(0, Math.min(c.length / 2, c.fadeIn + dt)) }));
      } else if (d.kind === "fadeOut") {
        setP(updateClip(d.base, c.id, { fadeOut: Math.max(0, Math.min(c.length / 2, c.fadeOut - dt)) }));
      }
    };
    const up = (e: MouseEvent) => {
      const d = drag.current;
      drag.current = null;
      if (!d) return;
      if (d.kind === "ruler" && Math.abs(e.clientX - d.x) < 3) {
        const at = timeAt(e.clientX);
        setPos(at);
        setRange(null);
        if (playing) play(at);
        return;
      }
      if (d.kind === "range" || d.kind === "ruler") return;
      // A drag that changed something is one step to undo.
      setP((cur) => {
        if (cur !== d.base) {
          setPast((h) => [...h.slice(-(HISTORY - 1)), d.base]);
          setFuture([]);
          setSaved(false);
        }
        return cur;
      });
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
  });

  const clipDown = (e: React.MouseEvent, tr: Track, c: Clip) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    setFocus(tr.id);
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const fromLeft = e.clientX - r.left;
    const fromRight = r.right - e.clientX;
    const top = e.clientY - r.top < 14;
    const kind = top && fromLeft < 14 + c.fadeIn * pps ? "fadeIn" : top && fromRight < 14 + c.fadeOut * pps ? "fadeOut" : fromLeft < 6 ? "start" : fromRight < 6 ? (e.shiftKey && tr.kind === "audio" ? "stretch" : "end") : "move";
    setSel((s) => (e.shiftKey ? new Set(s).add(c.id) : s.has(c.id) ? s : new Set([c.id])));
    drag.current = { kind, x: e.clientX, y: e.clientY, base: p, clip: c.id, t0: timeAt(e.clientX) };
  };

  // ---------- making ----------
  const [partKind, setPartKind] = useState("guitar");
  const [partWords, setPartWords] = useState("");
  const [partLyrics, setPartLyrics] = useState("");
  const [partTakes, setPartTakes] = useState(2);
  const [replaceWords, setReplaceWords] = useState("");
  const [realWords, setRealWords] = useState("");
  const start = async (req: MediaRequest, what: Parameters<typeof expect>[1], note: string) => {
    try {
      const id = await api.mediaStart(req);
      expect(id, what);
      setWaiting(waitingFor(p.id));
      toast(note, "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  /** The mix (or some tracks) as a WAV kept out of the gallery. */
  const renderToItem = async (from: number, to: number, only?: Set<string>, title?: string) => {
    const buf = await render(p, from, to, only);
    const wav = encodeWav(buf, 0, buf.duration);
    return api.mediaAdd({ data: toBase64(wav), mime: "audio/wav", op: "studio", prompt: title ?? p.name, hidden: !title, seconds: buf.duration, note: title ? t("Made in the song studio") : null });
  };
  /** A new part made to fit the mix, for a stretch (else the whole song), in takes. */
  const requestPart = async (kind: string, words: string, lyrics: string | null, stretch: [number, number] | null, takes: number) => {
    setBusy(t("Mixing what's there…"));
    try {
      const mix = await renderToItem(0, len);
      const [a, b] = stretch ?? [0, len];
      await start(
        { op: "studio_part", prompt: words.trim() || p.name, source: mix.id, count: takes, lyrics: kind.includes("vocals") && lyrics?.trim() ? lyrics : null, music: { track: kind } },
        { kind: "part", project: p.id, name: t(PART_NAMES[kind] ?? kind), at: a, length: b - a },
        t("Making the part: it arrives as a new track with {n} takes to pick from.", { n: takes }),
      );
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };
  const addPart = () => requestPart(partKind, partWords, partLyrics, range, partTakes);
  const [longer, setLonger] = useState(30);
  const [asWords, setAsWords] = useState("");
  /** The selected audio clip, else the last clip on the first audio track with any. */
  const songClip = () => {
    if (selected?.track.kind === "audio") return selected;
    const tr = p.tracks.find((x) => x.kind === "audio" && x.clips.length);
    const c = tr ? [...tr.clips].sort((a, b) => end(b) - end(a))[0] : null;
    return tr && c ? { track: tr, clip: c } : null;
  };
  const makeLonger = async () => {
    const f = songClip();
    const src = f && sourceOf(f.clip);
    if (!f || !src) return;
    await start(
      { op: "music", prompt: p.name, music: { task: "extend", audio: src, extend_by: longer } },
      { kind: "longer", project: p.id, clip: f.clip.id, label: t("{n} s longer", { n: longer }) },
      t("Making it longer: it arrives as a new take of the clip, {n} seconds longer.", { n: longer }),
    );
  };
  /** Remove FX (Suno Studio's): the selected clip's reverb and echo taken out, as a new take of it. */
  const removeFx = async () => {
    if (!selected || selected.track.kind !== "audio") return;
    const src = sourceOf(selected.clip);
    if (!src) return;
    await start(
      { op: "dry", prompt: "", source: src },
      { kind: "replace", project: p.id, clip: selected.clip.id, label: t("Effects removed") },
      t("Taking the reverb and echo out: it arrives as a new take of the clip."),
    );
  };
  const playAs = async () => {
    if (!selected || selected.track.kind !== "audio" || !asWords.trim()) return;
    const src = sourceOf(selected.clip);
    if (!src) return;
    await start(
      { op: "music", prompt: asWords.trim(), music: { task: "cover", audio: src, strength: 0.6 } },
      { kind: "replace", project: p.id, clip: selected.clip.id, label: t("Played as {words}", { words: asWords.trim() }) },
      t("Playing it again that way: it arrives as a new take of the clip."),
    );
  };

  // ---------- bringing things in ----------
  const bringSong = (id: string, name: string, seconds: number) => {
    const target = focusTrack?.kind === "audio" ? focusTrack.id : null;
    const clip = audioClip(id, name, pos, Math.max(1, seconds || 30));
    if (target) {
      commit((cur) => addClip(cur, target, clip));
      return;
    }
    const tr = newTrack(name, "audio", p.tracks.length);
    tr.clips = [clip];
    commit((cur) => addTrack(cur, tr));
    setFocus(tr.id);
  };
  const bringMidi = (bytes: ArrayBuffer, name: string) => {
    const found = readMidiFile(bytes);
    if (!found.length) {
      toast(t("That MIDI file has no notes."), "error");
      return;
    }
    const base = name.replace(/\.midi?$/i, "");
    commit((cur) =>
      found.reduce((acc, part) => {
        const tr = newTrack(part.program === DRUMS ? DRUM_KIT : `${base} ${part.channel + 1}`, "midi", acc.tracks.length);
        tr.program = part.program;
        const last = Math.max(...part.notes.map((n) => n.end));
        tr.clips = [midiClip(tr.name, pos, last + 0.5, part.notes)];
        return addTrack(acc, tr);
      }, cur),
    );
    toast(t("{n} MIDI part(s) brought in.", { n: found.length }), "success");
  };
  const bringFile = async () => {
    try {
      const { open: openFiles } = await import("@tauri-apps/plugin-dialog");
      const got = await openFiles({ multiple: false, filters: [{ name: t("Sounds"), extensions: ["mp3", "wav", "m4a", "aac", "flac", "ogg", "wma", "mid", "midi"] }] });
      const path = Array.isArray(got) ? got[0] : got;
      if (!path) return;
      setBusy(t("Bringing it in…"));
      if (/\.midi?$/i.test(path)) {
        const data = fromBase64(await api.studioReadMidi(path));
        bringMidi(data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength) as ArrayBuffer, path.split(/[\\/]/).pop() ?? "MIDI");
        return;
      }
      const item = await api.mediaImport(path);
      bringSong(item.id, item.title || item.prompt || t("Sound"), item.seconds);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };

  // ---------- the chat bar ----------
  const [asking, setAsking] = useState("");
  const [askBusy, setAskBusy] = useState(false);
  const [said, setSaid] = useState("");
  /** The project in words, for the chat model. */
  const summary = () => {
    const head = `Tempo ${p.bpm} bpm, ${p.beats} beats a bar. Length ${clock(len)}. Playhead ${clock(pos)}.${range ? ` Marked ${Math.round(range[0])} s to ${Math.round(range[1])} s.` : ""}${p.loop ? ` Looping ${Math.round(p.loop[0])} s to ${Math.round(p.loop[1])} s.` : ""}`;
    const tracks = p.tracks.map(
      (tr, i) =>
        `${i + 1}. "${tr.name}" (${tr.kind}${tr.kind === "midi" ? (tr.synth ? `, wavetable synth sound "${tr.synth.name}"` : `, instrument ${tr.program ?? 0}`) : ""}) volume ${tr.volume} dB, pan ${tr.pan}${tr.mute ? ", muted" : ""}${tr.solo ? ", solo" : ""}, effects: ${tr.effects.map((e) => e.kind).join(", ") || "none"}; clips: ${tr.clips.map((c) => `${Math.round(c.start)}-${Math.round(end(c))} s`).join(", ") || "none"}`,
    );
    return [head, "Tracks:", ...tracks].join("\n");
  };
  /** What the chat model asked for, done as one step to undo; new parts are made after. */
  const doActions = async (actions: StudioAction[]) => {
    const clamp = (v: unknown, lo: number, hi: number, d: number) => (v !== undefined && v !== null && Number.isFinite(Number(v)) ? Math.max(lo, Math.min(hi, Number(v))) : d);
    const named = (cur: Project, name: unknown) => {
      const n = String(name ?? "").toLowerCase().trim();
      if (!n) return undefined;
      return cur.tracks.find((x) => x.name.toLowerCase() === n) ?? cur.tracks.find((x) => x.name.toLowerCase().includes(n) || n.includes(x.name.toLowerCase()));
    };
    // A synth sound from a factory sound's name and/or settings.
    const synthFor = (a: StudioAction, current: SynthPatch | undefined) => {
      const named = PRESETS.find((x) => x.name.toLowerCase() === String(a.preset ?? "").toLowerCase().trim());
      return withSettings(named ?? current ?? initPatch(), (a.settings ?? {}) as Record<string, unknown>);
    };
    const notes: string[] = [];
    const parts: StudioAction[] = [];
    const built: EffectChain[] = [];
    let next = p;
    const allClips = (trackId: string, f: (c: Clip) => Clip) => {
      next = { ...next, tracks: next.tracks.map((x) => (x.id === trackId ? { ...x, clips: x.clips.map(f) } : x)) };
    };
    for (const a of actions) {
      const tr = named(next, a.track);
      switch (a.do) {
        case "add_track": {
          const nt = newTrack(String(a.name || (a.kind === "midi" ? t("Keys") : t("Audio"))), a.kind === "midi" ? "midi" : "audio", next.tracks.length);
          if (nt.kind === "midi") nt.program = clamp(a.instrument, 0, 128, programFor(nt.name));
          if (nt.kind === "midi" && (a.instrument === "synth" || a.preset)) nt.synth = synthFor(a, undefined);
          next = addTrack(next, nt);
          notes.push(t("added {name}", { name: nt.name }));
          break;
        }
        case "delete_track":
          if (tr) {
            next = removeTrack(next, tr.id);
            notes.push(t("deleted {name}", { name: tr.name }));
          }
          break;
        case "rename_track":
          if (tr && a.name) {
            next = updateTrack(next, tr.id, { name: String(a.name) });
            notes.push(t("renamed {name}", { name: tr.name }));
          }
          break;
        case "mute":
        case "unmute":
          if (tr) {
            next = updateTrack(next, tr.id, { mute: a.do === "mute" });
            notes.push(a.do === "mute" ? t("muted {name}", { name: tr.name }) : t("unmuted {name}", { name: tr.name }));
          }
          break;
        case "solo":
        case "unsolo":
          if (tr) {
            next = updateTrack(next, tr.id, { solo: a.do === "solo" });
            notes.push(a.do === "solo" ? t("soloed {name}", { name: tr.name }) : t("unsoloed {name}", { name: tr.name }));
          }
          break;
        case "volume":
          if (tr) {
            const db = clamp(a.db, -40, 6, tr.volume);
            next = updateTrack(next, tr.id, { volume: db });
            notes.push(t("{name} at {db} dB", { name: tr.name, db }));
          }
          break;
        case "pan":
          if (tr) {
            next = updateTrack(next, tr.id, { pan: clamp(a.value, -1, 1, tr.pan) });
            notes.push(t("panned {name}", { name: tr.name }));
          }
          break;
        case "add_effect": {
          const kind = String(a.effect) as EffectKind;
          if (!tr || !EFFECTS[kind]) break;
          const e = newEffect(kind);
          const set = (a.settings ?? {}) as Record<string, unknown>;
          for (const [k, [lo, hi]] of Object.entries(EFFECTS[kind].params)) e.params[k] = clamp(set[k], lo, hi, e.params[k]);
          next = updateTrack(next, tr.id, { effects: [...tr.effects, e] });
          notes.push(t("{effect} on {name}", { effect: t(EFFECTS[kind].label), name: tr.name }));
          break;
        }
        case "clear_effects":
          if (tr) {
            next = updateTrack(next, tr.id, { effects: [] });
            notes.push(t("no effects on {name}", { name: tr.name }));
          }
          break;
        case "synth":
          if (tr?.kind === "midi") {
            const patch = synthFor(a, tr.synth);
            next = updateTrack(next, tr.id, { synth: patch });
            notes.push(t("{name} on the synth: {sound}", { name: tr.name, sound: patch.name }));
          }
          break;
        case "instrument":
          if (tr?.kind === "midi") {
            next = updateTrack(next, tr.id, { program: clamp(a.instrument, 0, 128, 0) });
            notes.push(t("new instrument on {name}", { name: tr.name }));
          }
          break;
        case "build_effect": {
          // An effect described in words, made of the studio's effects: kept by name, and put on the track if one is named.
          const parts = Array.isArray(a.effects) ? (a.effects as Record<string, unknown>[]) : [];
          const effects = parts.flatMap((x) => {
            const kind = String(x?.effect) as EffectKind;
            if (!EFFECTS[kind]) return [];
            const e = newEffect(kind);
            const given = (x.settings ?? {}) as Record<string, unknown>;
            for (const [k, [lo, hi]] of Object.entries(EFFECTS[kind].params)) e.params[k] = clamp(given[k], lo, hi, e.params[k]);
            return [e];
          });
          if (!effects.length) break;
          const name = String(a.name || t("My effect")).trim().slice(0, 60);
          built.push({ name, effects });
          if (tr) next = updateTrack(next, tr.id, { effects: [...tr.effects, ...effects.map((e) => ({ ...e, id: newId("fx") }))] });
          notes.push(tr ? t("built {name} on {track}", { name, track: tr.name }) : t("built {name}", { name }));
          break;
        }
        case "formant":
          if (tr?.kind === "audio") {
            const n = clamp(a.semitones, -12, 12, 0);
            allClips(tr.id, (c) => ({ ...c, formant: n || undefined }));
            notes.push(t("{name}'s formants moved {n} semitones", { name: tr.name, n }));
          }
          break;
        case "transpose":
          if (tr) {
            const n = clamp(a.semitones, -12, 12, 0);
            allClips(tr.id, (c) => ({ ...c, semitones: n }));
            notes.push(t("{name} moved {n} semitones", { name: tr.name, n }));
          }
          break;
        case "fade":
          if (tr) {
            allClips(tr.id, (c) => ({ ...c, fadeIn: clamp(a.in, 0, c.length / 2, c.fadeIn), fadeOut: clamp(a.out, 0, c.length / 2, c.fadeOut) }));
            notes.push(t("fades on {name}", { name: tr.name }));
          }
          break;
        case "tempo": {
          const bpm = clamp(a.bpm, 30, 300, next.bpm);
          next = { ...next, bpm };
          notes.push(t("tempo {bpm}", { bpm }));
          break;
        }
        case "time_signature": {
          const beats = clamp(a.beats, 1, 16, next.beats);
          next = { ...next, beats };
          notes.push(t("{n} beats a bar", { n: beats }));
          break;
        }
        case "loop": {
          const from = clamp(a.from, 0, len, 0);
          const to = clamp(a.to, from + 0.5, len + 60, from + bar * 4);
          next = { ...next, loop: [from, to] };
          notes.push(t("looping {from} to {to}", { from: clock(from), to: clock(to) }));
          break;
        }
        case "loop_off":
          next = { ...next, loop: null };
          notes.push(t("no loop"));
          break;
        case "make_part":
          if (PARTS.includes(String(a.part))) parts.push(a);
          break;
        case "say":
          notes.push(String(a.text ?? ""));
          break;
      }
    }
    if (next !== p) commit({ ...next, updated_at: Date.now() });
    for (const chain of built) await api.studioLibrarySave("effects", chain).catch((e) => toast(errorText(e), "error"));
    for (const a of parts) {
      const from = a.from === undefined || a.from === null ? null : clamp(a.from, 0, len, 0);
      const to = a.to === undefined || a.to === null ? null : clamp(a.to, (from ?? 0) + 1, len, len);
      await requestPart(String(a.part), String(a.description ?? ""), a.lyrics ? String(a.lyrics) : null, from !== null || to !== null ? [from ?? 0, to ?? len] : null, clamp(a.takes, 1, 3, 2));
      notes.push(t("making a {part} part", { part: t(PART_NAMES[String(a.part)] ?? String(a.part)) }));
    }
    setSaid(notes.filter(Boolean).join(" · ") || t("Nothing to change."));
  };
  const ask = async () => {
    if (!asking.trim() || askBusy) return;
    setAskBusy(true);
    try {
      await doActions(await api.studioAsk(asking, summary()));
      setAsking("");
    } catch (e) {
      setSaid(errorText(e));
    } finally {
      setAskBusy(false);
    }
  };
  const replaceSection = async () => {
    if (!selected || selected.track.kind !== "audio" || !range) return;
    const c = selected.clip;
    const src = sourceOf(c);
    if (!src) return;
    const a = Math.max(c.start, range[0]);
    const b = Math.min(end(c), range[1]);
    if (b - a < 1) return toast(t("Mark a stretch of the selected clip first (drag on the ruler)."), "error");
    const ahead = startIn(c) - c.offset;
    const region: [number, number] = [ahead + soundAt(c, a - c.start), ahead + soundAt(c, b - c.start)];
    await start(
      { op: "music", prompt: replaceWords.trim() || p.name, music: { task: "repaint", audio: src, region } },
      { kind: "replace", project: p.id, clip: c.id, label: t("{from} to {to} made again", { from: clock(a), to: clock(b) }) },
      t("Making that stretch again: it arrives as a new take of the clip."),
    );
  };
  const makeReal = async () => {
    if (!selected || selected.track.kind !== "midi") return;
    setBusy(t("Playing the notes…"));
    try {
      const c = selected.clip;
      const only = new Set([selected.track.id]);
      const demo = await renderToItem(c.start, end(c), only);
      await start(
        { op: "music", prompt: realWords.trim() || t(GM[selected.track.program ?? 0] ?? "piano"), music: { task: "cover", audio: demo.id, strength: 0.5 } },
        { kind: "tracks", project: p.id, at: c.start, offset: 0, length: c.length },
        t("Making the notes real: they arrive as a new audio track."),
      );
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };
  const splitStems = async (tr: Track) => {
    const c = tr.clips[0];
    const src = c && sourceOf(c);
    if (!src) return;
    await start(
      { op: "stems", prompt: "", source: src, music: { track: "all" } },
      { kind: "tracks", project: p.id, at: c.start, offset: c.offset, length: c.length, mute: tr.id },
      t("Taking the track apart: its vocals, drums, bass and other instruments arrive as tracks."),
    );
  };
  const toMidi = async (tr: Track) => {
    const c = tr.clips.find((x) => sel.has(x.id)) ?? tr.clips[0];
    const src = c && sourceOf(c);
    if (!src) return;
    const drums = /drum|perc/i.test(tr.name);
    setBusy(t("Reading the notes…"));
    try {
      const found = await api.studioMidi(src, drums);
      const ahead = startIn(c) - c.offset;
      const from = startIn(c);
      const to = ahead + soundAt(c, c.length);
      const at = (s: number) => timeOfSound(c, s - ahead);
      const notes = found.filter((n) => n.end > from && n.start < to).map((n) => ({ ...n, start: at(Math.max(n.start, from)), end: at(Math.min(n.end, to)) }));
      const m = newTrack(`${tr.name} (MIDI)`, "midi", p.tracks.length);
      m.program = drums ? DRUMS : programFor(tr.name);
      m.clips = [midiClip(m.name, c.start, c.length, notes)];
      commit((cur) => addTrack(cur, m));
      toast(t("{n} notes read into a MIDI track.", { n: notes.length }), "success");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };

  // ---------- exporting ----------
  const exportMix = async (which: "mix" | "range" | "tracks") => {
    setBusy(t("Mixing…"));
    try {
      if (which === "tracks") {
        for (const tr of p.tracks.filter((x) => x.clips.length)) await renderToItem(0, len, new Set([tr.id]), `${p.name} (${tr.name})`);
        toast(t("Each track is in the gallery."), "success");
      } else {
        const [a, b] = which === "range" && range ? range : [0, len];
        await renderToItem(a, b, undefined, which === "range" ? `${p.name} (${clock(a)}–${clock(b)})` : p.name);
        toast(t("The mix is in the gallery."), "success");
      }
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };
  const exportMidi = async (tr: Track) => {
    const notes = tr.clips.flatMap((c) => (c.notes ?? []).map((n) => ({ ...n, start: n.start + c.start, end: n.end + c.start })));
    try {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const path = await save({ defaultPath: `${tr.name}.mid`, filters: [{ name: "MIDI", extensions: ["mid"] }] });
      if (!path) return;
      await api.studioSaveMidi(path, toBase64(midiFile(notes, tr.program ?? 0, tr.name)));
      toast(t("Saved to {path}", { path }), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const bar = beatSecs(p) * p.beats;
  const ticks = useMemo(() => {
    const out: { s: number; label: string | null }[] = [];
    const every = pps * bar < 40 ? Math.ceil(40 / (pps * bar)) : 1;
    for (let i = 0; i * bar < len + 200 / pps; i += every) out.push({ s: i * bar, label: String(i + 1) });
    return out;
  }, [pps, bar, len]);

  return (
    <div className={inWindow ? "song-studio-window" : "modal-backdrop"}>
      <LearnContext.Provider value={learn}>
      <div className={`${inWindow ? "" : "modal "}song-studio`} role={inWindow ? undefined : "dialog"} aria-modal={inWindow ? undefined : true} aria-label={t("Song studio")}>
        <div className="row wrap">
          <input className="input studio-name" value={p.name} onChange={(e) => setP({ ...p, name: e.target.value })} onBlur={() => setSaved(false)} aria-label={t("Project name")} />
          <span className="small muted">{saved ? t("Kept") : t("Keeping…")}</span>
          {waiting > 0 && <span className="small">{t("{n} part(s) being made", { n: waiting })}</span>}
          {busy && <span className="small muted">{busy}</span>}
          <span className="spacer" />
          <button className="btn small" onClick={undo} disabled={!past.length} title={t("Undo (Ctrl+Z)")}>{t("Undo")}</button>
          <button className="btn small" onClick={redo} disabled={!future.length} title={t("Redo (Ctrl+Y)")}>{t("Redo")}</button>
          <select
            value=""
            onChange={(e) => {
              const v = e.target.value;
              if (v) exportMix(v as "mix" | "range" | "tracks");
            }}
            aria-label={t("Export")}
          >
            <option value="">{t("Export…")}</option>
            <option value="mix">{t("The mix, to the gallery")}</option>
            {range && <option value="range">{t("The marked stretch, to the gallery")}</option>}
            <option value="tracks">{t("Each track, to the gallery")}</option>
          </select>
          {onPopOut && (
            <button
              className="icon-btn"
              aria-label={t("Open in a new window")}
              title={t("Open this project in a window of its own")}
              onClick={async () => {
                stop();
                // Kept first, so the new window opens it as it is now.
                await api.studioProjectSave({ ...p, updated_at: Date.now() }).catch(() => {});
                onPopOut();
              }}
            >
              <NewWindowIcon />
            </button>
          )}
          <button className="icon-btn" aria-label={t("Close")} onClick={() => { stop(); onClose(); }}>✕</button>
        </div>

        <div className="row studio-ask">
          <input
            className="input"
            value={asking}
            onChange={(e) => setAsking(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && ask()}
            placeholder={t("Ask the studio, e.g. add reverb to the vocals, 95 bpm, a bass part for the chorus")}
            aria-label={t("Ask the studio")}
          />
          <button className="btn small" onClick={ask} disabled={askBusy || !asking.trim()}>{askBusy ? t("Thinking…") : t("Ask")}</button>
          {said && <span className="small muted ellipsis">{said}</span>}
        </div>
        <div className="row wrap studio-transport">
          <button className="btn small primary" onClick={() => (playing ? stop() : play())} title={t("Play or stop (Space)")}>{playing ? t("Stop") : t("Play")}</button>
          <button className={`btn small ${rec ? "danger-fill" : ""}`} onClick={record} title={t("Record from the microphone into the selected audio track (Shift+R)")}>
            {rec ? t("Stop recording ({secs} s)", { secs: Math.round(rec.secs) }) : t("Record")}
          </button>
          {rec && <span className="rec-level" style={{ width: `${Math.round(rec.level * 60)}px` }} />}
          {learning && (
            <span className="learn-note small">
              {t("Move a knob or slider on your MIDI controller for {what}.", { what: learning.label })}
              <button className="btn ghost small" onClick={() => setLearning(null)}>{t("Stop")}</button>
            </span>
          )}
          <select value={countIn} onChange={(e) => setCountIn(+e.target.value)} aria-label={t("Count-in")} title={t("Clicks before recording starts")}>
            <option value={0}>{t("No count-in")}</option>
            {[1, 2, 4].map((n) => (
              <option key={n} value={n}>{n === 1 ? t("1 bar count-in") : t("{n} bars count-in", { n })}</option>
            ))}
          </select>
          <span className="mono small">{clock(pos)}</span>
          <button className={`chip ${p.loop ? "active" : ""}`} onClick={() => commit((cur) => ({ ...cur, loop: cur.loop ? null : range ?? [pos, pos + bar * 4] }))} title={t("Loop the marked stretch (Ctrl+L)")}>{t("Loop")}</button>
          <button className={`chip ${click ? "active" : ""}`} onClick={() => setClick((c) => !c)} title={t("Metronome (Shift+C)")}>{t("Click")}</button>
          <button className={`chip ${typing ? "active" : ""}`} onClick={() => setTyping((x) => !x)} title={t("Play notes on the keyboard: A to ; and W E T Y U O P (Ctrl+K). While playing, a MIDI track keeps them.")}>{t("Keys")}</button>
          <label className="row small">
            {t("Tempo")}
            <input className="input tiny" type="number" min={30} max={300} value={p.bpm} onChange={(e) => commit((cur) => ({ ...cur, bpm: Math.max(30, Math.min(300, +e.target.value || 120)) }))} />
          </label>
          <label className="row small">
            {t("Beats a bar")}
            <input className="input tiny" type="number" min={1} max={16} value={p.beats} onChange={(e) => commit((cur) => ({ ...cur, beats: Math.max(1, Math.min(16, +e.target.value || 4)) }))} />
          </label>
          <label className="row small">
            {t("Grid")}
            <select value={grid} onChange={(e) => setGrid(+e.target.value)}>
              {GRIDS.map(([g, n]) => (
                <option key={g} value={g}>{t(n)}</option>
              ))}
            </select>
          </label>
          <label className="row small" title={t("The whole mix's volume")}>
            {t("Master")}
            <input type="range" min={-30} max={6} step={0.5} value={master} onChange={(e) => setMaster(+e.target.value)} onDoubleClick={() => setMaster(0)} aria-label={t("Master volume")} {...learnable(learn, { id: "master", label: t("Master volume"), min: -30, max: 6 })} />
          </label>
          <span className="spacer" />
          <button className="btn small" onClick={() => addEmptyTrack("audio")} title={t("Add an audio track (Shift+T)")}>{t("+ Audio track")}</button>
          <button className="btn small" onClick={() => addEmptyTrack("midi")}>{t("+ MIDI track")}</button>
          <button className="btn small" onClick={() => setPps((z) => Math.max(2, z / 1.4))} aria-label={t("Zoom out")}>−</button>
          <button className="btn small" onClick={() => setPps((z) => Math.min(400, z * 1.4))} aria-label={t("Zoom in")}>+</button>
        </div>

        <div
          className="studio-timeline"
          ref={scroll}
          onMouseDown={(e) => {
            if (e.button !== 0) return;
            const lane = laneAt(e.clientY);
            const tr = p.tracks[lane];
            if (tr) setFocus(tr.id);
            setSel(new Set());
            drag.current = { kind: "range", x: e.clientX, y: e.clientY, base: p, t0: timeAt(e.clientX) };
          }}
          onDoubleClick={(e) => {
            const tr = p.tracks[laneAt(e.clientY)];
            if (tr?.kind !== "midi") return;
            const at = snap(p, timeAt(e.clientX), grid || 1);
            const c = midiClip(t("Notes"), at, bar * 2, []);
            commit((cur) => addClip(cur, tr.id, c));
            setSel(new Set([c.id]));
            setDock("notes");
          }}
        >
          <div className="studio-canvas" style={{ width: width + HEAD, height: 28 + p.tracks.length * LANE + 40 }}>
            <div
              className="studio-ruler"
              style={{ left: HEAD, width }}
              onMouseDown={(e) => {
                e.stopPropagation();
                drag.current = { kind: "ruler", x: e.clientX, y: e.clientY, base: p, t0: timeAt(e.clientX) };
              }}
            >
              {ticks.map((k) => (
                <span key={k.s} className="studio-tick" style={{ left: x(k.s) }}>{k.label}</span>
              ))}
              {p.loop && <span className="studio-loop" style={{ left: x(p.loop[0]), width: x(p.loop[1] - p.loop[0]) }} />}
            </div>
            {range && <div className="studio-range" style={{ left: HEAD + x(range[0]), width: x(range[1] - range[0]) }} />}
            {p.tracks.map((tr, i) => (
              <div key={tr.id} className={`studio-lane ${focus === tr.id ? "focused" : ""} ${tr.kind}`} style={{ top: 28 + i * LANE, height: LANE, width: width + HEAD }}>
                <div className="studio-head" style={{ width: HEAD, borderLeftColor: tr.color }} onMouseDown={(e) => { e.stopPropagation(); setFocus(tr.id); }}>
                  <div className="row">
                    <input className="input tiny-name" value={tr.name} onChange={(e) => setP(updateTrack(p, tr.id, { name: e.target.value }))} onBlur={() => setSaved(false)} aria-label={t("Track name")} />
                    <button className={`chip mini ${tr.mute ? "active" : ""}`} onClick={() => setTrack(tr.id, { mute: !tr.mute })} title={t("Mute (Shift+M)")}>M</button>
                    <button className={`chip mini ${tr.solo ? "active" : ""}`} onClick={() => setTrack(tr.id, { solo: !tr.solo })} title={t("Solo (Shift+S)")}>S</button>
                    <select
                      className="mini-select"
                      value=""
                      aria-label={t("Track actions")}
                      onChange={(e) => {
                        const v = e.target.value;
                        if (v === "up") commit((cur) => moveTrack(cur, tr.id, -1));
                        if (v === "down") commit((cur) => moveTrack(cur, tr.id, 1));
                        if (v === "dup") commit((cur) => duplicateTrack(cur, tr.id));
                        if (v === "del") commit((cur) => removeTrack(cur, tr.id));
                        if (v === "stems") splitStems(tr);
                        if (v === "midi") toMidi(tr);
                        if (v === "midifile") exportMidi(tr);
                      }}
                    >
                      <option value="">⋯</option>
                      <option value="up">{t("Move up")}</option>
                      <option value="down">{t("Move down")}</option>
                      <option value="dup">{t("Duplicate")}</option>
                      {tr.kind === "audio" && tr.clips.length > 0 && <option value="stems">{t("Split into stems")}</option>}
                      {tr.kind === "audio" && tr.clips.length > 0 && <option value="midi">{t("Notes from this track (MIDI)")}</option>}
                      {tr.kind === "midi" && <option value="midifile">{t("Save as MIDI…")}</option>}
                      <option value="del">{t("Delete track")}</option>
                    </select>
                  </div>
                  <div className="row small">
                    <input type="range" min={-40} max={6} step={0.5} value={tr.volume} onChange={(e) => setP(updateTrack(p, tr.id, { volume: +e.target.value }))} onMouseUp={() => setSaved(false)} aria-label={t("Volume")} title={t("Volume {db} dB", { db: tr.volume })} {...learnable(learn, { id: `track:${tr.id}:volume`, label: t("{track}: volume", { track: tr.name }), min: -40, max: 6 })} />
                    <input type="range" min={-1} max={1} step={0.05} value={tr.pan} onChange={(e) => setP(updateTrack(p, tr.id, { pan: +e.target.value }))} onDoubleClick={() => setTrack(tr.id, { pan: 0 })} aria-label={t("Pan")} title={t("Pan (double-click: middle)")} className="pan" {...learnable(learn, { id: `track:${tr.id}:pan`, label: t("{track}: pan", { track: tr.name }), min: -1, max: 1 })} />
                    <button className="link small" onClick={() => { setFocus(tr.id); setDock("effects"); }}>{t("FX {n}", { n: tr.effects.length })}</button>
                  </div>
                  {tr.kind === "midi" && (
                    <select
                      className="mini-select wide"
                      value={tr.synth ? -1 : tr.program ?? 0}
                      onChange={(e) => {
                        const v = +e.target.value;
                        if (v === -1) {
                          setTrack(tr.id, { synth: tr.synth ?? initPatch() });
                          setFocus(tr.id);
                          setDock("synth");
                        } else setTrack(tr.id, { program: v, synth: undefined });
                      }}
                      aria-label={t("Instrument")}
                    >
                      <option value={-1}>{t("Wavetable synth")}{tr.synth ? `: ${tr.synth.name}` : ""}</option>
                      {GM.map((n, j) => (
                        <option key={j} value={j}>{n}</option>
                      ))}
                      <option value={DRUMS}>{DRUM_KIT}</option>
                    </select>
                  )}
                </div>
                {tr.clips.map((c) => (
                  <div
                    key={c.id}
                    className={`studio-clip ${sel.has(c.id) ? "selected" : ""} ${c.disabled ? "disabled" : ""}`}
                    style={{ left: HEAD + x(c.start), width: Math.max(4, x(c.length)), background: tr.color }}
                    onMouseDown={(e) => clipDown(e, tr, c)}
                    onDoubleClick={(e) => {
                      e.stopPropagation();
                      setSel(new Set([c.id]));
                      setDock(tr.kind === "midi" ? "notes" : "clip");
                    }}
                    title={c.name}
                  >
                    {tr.kind === "audio" ? <Wave clip={c} width={x(c.length)} height={LANE - 22} /> : <MiniNotes notes={c.notes ?? []} length={c.length} width={x(c.length)} height={LANE - 22} />}
                    <span className="clip-name">{c.name}{c.takes.length > 1 ? ` · ${t("{n} takes", { n: c.takes.length })}` : ""}</span>
                    {c.fadeIn > 0 && <span className="clip-fade in" style={{ width: x(c.fadeIn) }} />}
                    {c.fadeOut > 0 && <span className="clip-fade out" style={{ width: x(c.fadeOut) }} />}
                  </div>
                ))}
              </div>
            ))}
            <div className="studio-playhead" style={{ left: HEAD + x(pos), height: 28 + p.tracks.length * LANE }} />
          </div>
        </div>

        <div className="studio-dock">
          <div className="filters tight">
            {(["clip", "takes", "effects", "automation", "notes", "synth", "make"] as Dock[]).map((d) => (
              <button key={d} className={`chip ${dock === d ? "active" : ""}`} onClick={() => setDock(d)}>
                {{ clip: t("Clip"), takes: t("Takes"), effects: t("Effects"), automation: t("Automation"), notes: t("Notes"), synth: t("Synth"), make: t("Make with the music model") }[d]}
              </button>
            ))}
            <span className="spacer" />
            <span className="small muted">{t("Ctrl+E split · Ctrl+D duplicate · Ctrl+C / V copy / paste · 0 turns a clip off · Shift+F fades · drag the ruler to mark a stretch")}</span>
          </div>

          {dock === "clip" && (selected ? (
            <ClipPanel clip={selected.clip} track={selected.track} p={p} grid={grid} onChange={(patch) => setClip(selected.clip.id, patch)} onCommitTake={(take) => setClip(selected.clip.id, { take })} onRemoveFx={removeFx} />
          ) : (
            <p className="small muted">{t("Select a clip to change its level, fades, speed, key, stretch or takes.")}</p>
          ))}

          {dock === "takes" && (selected?.track.kind === "audio" ? (
            <TakesPanel
              p={p}
              clipId={selected.clip.id}
              grid={grid}
              onChange={(next, at) => {
                commit(next);
                // Comping cuts and joins pieces: keep a piece of it selected.
                const c = next.tracks.find((x) => x.id === selected.track.id)?.clips.find((x) => x.start <= at + 0.001 && end(x) > at);
                if (c) setSel(new Set([c.id]));
              }}
              onHear={(preview, from) => play(from, preview)}
            />
          ) : (
            <p className="small muted">{t("Select an audio clip to see its takes.")}</p>
          ))}

          {dock === "effects" && (focusTrack ? (
            <EffectsPanel track={focusTrack} onChange={(effects) => setTrack(focusTrack.id, { effects })} toast={toast} />
          ) : (
            <p className="small muted">{t("Pick a track.")}</p>
          ))}

          {dock === "automation" && focusTrack && (
            <div className="form">
              <AutoLane label={t("{track}: volume (dB)", { track: focusTrack.name })} points={focusTrack.automation.volume ?? []} min={-40} max={6} fallback={focusTrack.volume} length={len} onChange={(pts) => setTrack(focusTrack.id, { automation: { ...focusTrack.automation, volume: pts } })} />
              <AutoLane label={t("{track}: pan (left to right)", { track: focusTrack.name })} points={focusTrack.automation.pan ?? []} min={-1} max={1} fallback={focusTrack.pan} length={len} onChange={(pts) => setTrack(focusTrack.id, { automation: { ...focusTrack.automation, pan: pts } })} />
              <p className="small muted">{t("Click to add a point, drag to move it, right-click to take it away. With points, they decide the level instead of the track's slider.")}</p>
            </div>
          )}

          {dock === "notes" && (selected?.track.kind === "midi" ? (
            <PianoRoll
              notes={selected.clip.notes ?? []}
              length={selected.clip.length}
              grid={(grid || 0.25) * beatSecs(p)}
              onChange={(notes) => setClip(selected.clip.id, { notes })}
              onTap={(pitch) => {
                if (selected.track.synth) {
                  wave.current ??= new LiveWave();
                  wave.current.tap(selected.track.synth, pitch, p.tables).catch(() => {});
                  return;
                }
                synth.current ??= new LiveSynth();
                synth.current.tap(selected.track.program ?? 0, pitch).catch(() => {});
              }}
            />
          ) : (
            <p className="small muted">{t("Select a MIDI clip to edit its notes (double-click an empty spot on a MIDI track to make one).")}</p>
          ))}

          {dock === "synth" && (synthTrack ? (
            synthTrack.synth ? (
              <SynthPanel
                trackId={synthTrack.id}
                track={synthTrack.name}
                patch={synthTrack.synth}
                made={p.tables ?? {}}
                onPatch={(patch, done) => setSynth(synthTrack.id, patch, done)}
                onMakeTable={makeTable}
                canCapture={selected?.track.kind === "audio"}
                toast={toast}
              />
            ) : (
              <div className="row wrap">
                <span className="small muted">{t("{track} plays a sampled instrument.", { track: synthTrack.name })}</span>
                <button className="btn small" onClick={() => setTrack(synthTrack.id, { synth: initPatch() })}>{t("Play it on the wavetable synth")}</button>
              </div>
            )
          ) : (
            <p className="small muted">{t("Pick a MIDI track to play it on the wavetable synth.")}</p>
          ))}

          {dock === "make" && (
            <div className="studio-make">
              <div className="form">
                <strong>{t("Add a part")}</strong>
                <div className="row wrap">
                  <select value={partKind} onChange={(e) => setPartKind(e.target.value)} aria-label={t("The part")}>
                    {PARTS.map((k) => (
                      <option key={k} value={k}>{t(PART_NAMES[k])}</option>
                    ))}
                  </select>
                  <input className="input" value={partWords} onChange={(e) => setPartWords(e.target.value)} placeholder={t("How it sounds (optional), e.g. bright funky rhythm guitar")} aria-label={t("How it sounds")} />
                  <select value={partTakes} onChange={(e) => setPartTakes(+e.target.value)} aria-label={t("Takes")}>
                    {[1, 2, 3].map((n) => (
                      <option key={n} value={n}>{n === 1 ? t("1 take") : t("{n} takes", { n })}</option>
                    ))}
                  </select>
                </div>
                {partKind.includes("vocals") && <textarea className="input" rows={3} value={partLyrics} onChange={(e) => setPartLyrics(e.target.value)} placeholder={t("Words to sing (optional)")} aria-label={t("Words to sing")} />}
                <button className="btn small primary" onClick={addPart} disabled={!!busy}>{range ? t("Make it for the marked stretch") : t("Make it for the whole song")}</button>
                <span className="small muted">{t("Made to fit what's playing, with ACE-Step 1.5 Base; the takes come in a new track to pick from.")}</span>
              </div>
              <div className="form">
                <strong>{t("Make a stretch again")}</strong>
                <input className="input" value={replaceWords} onChange={(e) => setReplaceWords(e.target.value)} placeholder={t("How it should go (optional)")} aria-label={t("How it should go")} />
                <button className="btn small" onClick={replaceSection} disabled={!selected || selected.track.kind !== "audio" || !range}>{t("Make the marked stretch of the selected clip again")}</button>
              </div>
              <div className="form">
                <strong>{t("Make the song longer")}</strong>
                <div className="row wrap">
                  <select value={longer} onChange={(e) => setLonger(+e.target.value)} aria-label={t("How much longer")}>
                    {[15, 30, 60, 90].map((n) => (
                      <option key={n} value={n}>{t("{n} s more", { n })}</option>
                    ))}
                  </select>
                  <button className="btn small" onClick={makeLonger} disabled={!songClip()}>{t("Make it longer")}</button>
                </div>
                <span className="small muted">{t("The selected clip (else the last one on the song's track) goes on past its end, as a new take.")}</span>
              </div>
              <div className="form">
                <strong>{t("Play a clip another way")}</strong>
                <input className="input" value={asWords} onChange={(e) => setAsWords(e.target.value)} placeholder={t("e.g. on a violin, as jazz piano, sung by a choir")} aria-label={t("How to play it")} />
                <button className="btn small" onClick={playAs} disabled={!selected || selected.track.kind !== "audio" || !asWords.trim()}>{t("Play the selected clip that way")}</button>
              </div>
              <div className="form">
                <strong>{t("Bring in")}</strong>
                <button className="btn small" onClick={bringFile} disabled={!!busy}>{t("A sound or MIDI file…")}</button>
                {songs.length > 0 && (
                  <select
                    value=""
                    onChange={(e) => {
                      const s = songs.find((x) => x.id === e.target.value);
                      if (s) bringSong(s.id, s.title || s.prompt || t("Song"), s.seconds);
                    }}
                    aria-label={t("A song from the gallery")}
                  >
                    <option value="">{t("A song from the gallery…")}</option>
                    {songs.map((s) => (
                      <option key={s.id} value={s.id}>{s.title || s.prompt || t("A song")}</option>
                    ))}
                  </select>
                )}
                <span className="small muted">{t("At the playhead, on the selected audio track (else a new one).")}</span>
              </div>
              <div className="form">
                <strong>{t("Notes to sound")}</strong>
                <input className="input" value={realWords} onChange={(e) => setRealWords(e.target.value)} placeholder={t("What it should sound like, e.g. warm cello")} aria-label={t("What it should sound like")} />
                <button className="btn small" onClick={makeReal} disabled={!selected || selected.track.kind !== "midi" || !!busy}>{t("Make the selected MIDI clip real")}</button>
              </div>
            </div>
          )}
        </div>
      </div>
      </LearnContext.Provider>
    </div>
  );
}

function ClipPanel({ clip, track, p, grid, onChange, onCommitTake, onRemoveFx }: { clip: Clip; track: Track; p: Project; grid: number; onChange: (patch: Partial<Clip>) => void; onCommitTake: (take: string) => void; onRemoveFx: () => void }) {
  const setSpeed = (v: number) => {
    const speed = Math.max(0.25, Math.min(4, v || 1));
    onChange({ speed, length: (clip.length * clip.speed) / speed, warp: clip.warp?.map((m) => ({ ...m, t: (m.t * clip.speed) / speed })) });
  };
  return (
    <div className="studio-clip-panel row wrap">
      {track.kind === "audio" && <WarpEditor clip={clip} p={p} grid={grid} onChange={onChange} />}
      <input className="input" value={clip.name} onChange={(e) => onChange({ name: e.target.value })} aria-label={t("Clip name")} />
      <label className="row small">
        {t("Level")}
        <input type="range" min={-24} max={12} step={0.5} value={clip.gain} onChange={(e) => onChange({ gain: +e.target.value })} />
        {clip.gain} dB
      </label>
      <label className="row small">
        {t("Fade in")}
        <input type="range" min={0} max={Math.min(10, clip.length / 2)} step={0.1} value={clip.fadeIn} onChange={(e) => onChange({ fadeIn: +e.target.value })} />
      </label>
      <label className="row small">
        {t("Fade out")}
        <input type="range" min={0} max={Math.min(10, clip.length / 2)} step={0.1} value={clip.fadeOut} onChange={(e) => onChange({ fadeOut: +e.target.value })} />
      </label>
      {track.kind === "audio" && (
        <>
          <label className="row small" title={t("Shift-drag a clip's right edge to stretch it to fit")}>
            {t("Speed")}
            <input className="input tiny" type="number" min={0.25} max={4} step={0.01} value={Math.round(clip.speed * 1000) / 1000} onChange={(e) => setSpeed(+e.target.value)} />×
            <button className="btn ghost small" onClick={() => setSpeed(clip.speed / 2)} title={t("Half as fast")}>½×</button>
            <button className="btn ghost small" onClick={() => setSpeed(clip.speed * 2)} title={t("Twice as fast")}>2×</button>
          </label>
          <label className="row small">
            <input type="checkbox" checked={clip.keepPitch} onChange={(e) => onChange({ keepPitch: e.target.checked })} /> {t("Keep the pitch")}
          </label>
          <label className="row small">
            <input type="checkbox" checked={clip.reverse} onChange={(e) => onChange({ reverse: e.target.checked })} /> {t("Reverse")}
          </label>
        </>
      )}
      <label className="row small">
        {t("Key")}
        <input className="input tiny" type="number" min={-12} max={12} value={clip.semitones} onChange={(e) => onChange({ semitones: Math.max(-12, Math.min(12, Math.round(+e.target.value))) })} />
        {t("semitones")}
      </label>
      {track.kind === "audio" && (
        <label className="row small" title={t("Moves a voice's character up or down with its pitch left alone. To keep a voice natural when changing its key, turn this the other way by as much.")}>
          {t("Formant")}
          <input type="range" min={-12} max={12} step={0.5} value={clip.formant ?? 0} onChange={(e) => onChange({ formant: +e.target.value || undefined })} onDoubleClick={() => onChange({ formant: undefined })} aria-label={t("Formant")} />
          <span className="mono">{(clip.formant ?? 0) > 0 ? "+" : ""}{clip.formant ?? 0}</span>
        </label>
      )}
      <label className="row small">
        <input type="checkbox" checked={clip.disabled} onChange={(e) => onChange({ disabled: e.target.checked })} /> {t("Off")}
      </label>
      {track.kind === "audio" && (
        <button className="btn small" onClick={onRemoveFx} title={t("Takes the reverb and echo out with a model trained on single voices: best on a vocal (split a song into stems first). Arrives as a new take.")}>
          {t("Remove FX")}
        </button>
      )}
      {clip.takes.length > 1 && (
        <div className="row wrap small studio-takes">
          <span className="muted">{t("Takes:")}</span>
          {clip.takes.map((k, i) => (
            <button key={k.id} className={`chip ${clip.take === k.id ? "active" : ""}`} onClick={() => onCommitTake(k.id)} title={k.label}>
              {t("Take {n}", { n: i + 1 })}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** A clip's sound with its warp markers (Suno's Stretch): double-click to put
 * one down, drag one to stretch the sound either side of it, right-click one
 * to take it away; or put them on the hits or the beats, then line them up
 * with the grid. */
function WarpEditor({ clip, p, grid, onChange }: { clip: Clip; p: Project; grid: number; onChange: (patch: Partial<Clip>) => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [moving, setMoving] = useState<WarpMarker[] | null>(null);
  const drag = useRef<number | null>(null);
  const [note, setNote] = useState("");
  const on = { ...clip, warpOff: false };
  const shown: Clip = moving ? { ...on, warp: moving } : clip;
  const marks = [...(shown.warp ?? [])].sort((a, b) => a.t - b.t);
  const step = grid || 1;
  const tAt = (clientX: number) => {
    const r = ref.current!.getBoundingClientRect();
    return Math.max(0, Math.min(clip.length, ((clientX - r.left) / r.width) * clip.length));
  };
  const pct = (s: number) => `${(s / clip.length) * 100}%`;
  useEffect(() => {
    const move = (e: MouseEvent) => {
      const i = drag.current;
      if (i === null || !moving) return;
      const lo = i > 0 ? moving[i - 1].t + 0.02 : 0.02;
      const hi = i < moving.length - 1 ? moving[i + 1].t - 0.02 : clip.length - 0.02;
      let tt = tAt(e.clientX);
      if (grid > 0 && !e.altKey) tt = snap(p, clip.start + tt, grid) - clip.start;
      setMoving(moving.map((m, j) => (j === i ? { ...m, t: Math.max(lo, Math.min(hi, tt)) } : m)));
    };
    const up = () => {
      if (drag.current === null) return;
      drag.current = null;
      if (moving) onChange({ warp: tidyWarp(moving, on), warpOff: false });
      setMoving(null);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
  });
  const setMarks = (list: WarpMarker[]) => onChange({ warp: list.length ? tidyWarp(list, on) : undefined, warpOff: false });
  const onHits = async () => {
    const src = sourceOf(clip);
    if (!src) return;
    setNote(t("Listening for the hits…"));
    try {
      const buf = await sourceBuffer(src);
      const ahead = startIn(clip) - clip.offset;
      const hits = onsets(buf, startIn(clip), ahead + soundAt(on, clip.length));
      const list = hits.map((o) => ({ s: o - ahead, t: timeOfSound(on, o - ahead) })).filter((m) => m.t > 0.02 && m.t < clip.length - 0.02);
      setMarks(list);
      setNote(t("{n} hits marked. Line them up with the grid to put them on the beat.", { n: list.length }));
    } catch (e) {
      setNote(errorText(e));
    }
  };
  const lines: { t: number; bar: boolean }[] = [];
  const beat = beatSecs(p) * step;
  for (let k = Math.ceil(clip.start / beat); k * beat < end(clip) && lines.length < 400; k++) {
    const tt = k * beat - clip.start;
    if (tt > 0.001) lines.push({ t: tt, bar: Math.abs((k * step) % p.beats) < 1e-6 });
  }
  return (
    <div className="warp-panel">
      <div className="row wrap small">
        <strong>{t("Stretch")}</strong>
        <label className="row small">
          <input type="checkbox" checked={!clip.warpOff} onChange={(e) => onChange({ warpOff: !e.target.checked })} /> {t("On")}
        </label>
        <button className="btn ghost small" onClick={onHits}>{t("Markers on the hits")}</button>
        <button className="btn ghost small" onClick={() => setMarks(lines.map((l) => ({ t: l.t, s: soundAt(on, l.t) })))}>{t("Markers on the beats")}</button>
        <button className="btn ghost small" disabled={!marks.length} onClick={() => setMarks(marks.map((m) => ({ ...m, t: snap(p, clip.start + m.t, step) - clip.start })))} title={t("Each marker to the nearest grid line (Shift+Q)")}>{t("Line up with the grid")}</button>
        <button className="btn ghost small" disabled={!marks.length} onClick={() => setMarks([])}>{t("Clear")}</button>
        <span className="muted">{note || t("Double-click to add a marker, drag it to stretch, right-click to remove.")}</span>
      </div>
      <div
        ref={ref}
        className="warp-editor"
        onDoubleClick={(e) => {
          const tt = tAt(e.clientX);
          setMarks([...marks, { t: tt, s: soundAt(on, tt) }]);
        }}
      >
        <Wave clip={shown} width={1200} height={84} color="rgba(128,128,128,0.85)" />
        {lines.map((l) => (
          <span key={l.t} className={`warp-beat ${l.bar ? "bar" : ""}`} style={{ left: pct(l.t) }} />
        ))}
        {marks.map((m, i) => (
          <span
            key={`${i}-${m.s}`}
            className="warp-marker"
            style={{ left: pct(m.t) }}
            title={t("{at}: drag to stretch, right-click to remove", { at: clock(clip.start + m.t) })}
            onMouseDown={(e) => {
              if (e.button !== 0) return;
              e.stopPropagation();
              setMoving(marks);
              drag.current = i;
            }}
            onDoubleClick={(e) => e.stopPropagation()}
            onContextMenu={(e) => {
              e.preventDefault();
              setMarks(marks.filter((_, j) => j !== i));
            }}
          />
        ))}
      </div>
    </div>
  );
}

/** A clip's takes, a lane each across its whole comp (Suno's take lanes):
 * click a lane to hear that take from there in the mix, drag across a lane to
 * play that stretch from it (comping), or use one for all of it. */
function TakesPanel({ p, clipId, grid, onChange, onHear }: { p: Project; clipId: string; grid: number; onChange: (next: Project, at: number) => void; onHear: (preview: Project, from: number) => void }) {
  const g = compGroup(p, clipId);
  const [pick, setPick] = useState<{ take: string; a: number; b: number } | null>(null);
  const picked = useRef<typeof pick>(null);
  const drag = useRef<{ take: string; t0: number; el: HTMLElement } | null>(null);
  const lo = g?.clips[0].start ?? 0;
  const hi = g ? end(g.clips[g.clips.length - 1]) : 1;
  const span = Math.max(0.01, hi - lo);
  const tAt = (el: HTMLElement, clientX: number) => {
    const r = el.getBoundingClientRect();
    return lo + Math.max(0, Math.min(1, (clientX - r.left) / r.width)) * span;
  };
  useEffect(() => {
    const move = (e: MouseEvent) => {
      const d = drag.current;
      if (!d) return;
      const now = tAt(d.el, e.clientX);
      const [a, b] = [Math.min(d.t0, now), Math.max(d.t0, now)].map((x) => Math.max(lo, Math.min(hi, grid > 0 && !e.altKey ? snap(p, x, grid) : x)));
      picked.current = { take: d.take, a, b };
      setPick(picked.current);
    };
    const up = () => {
      const d = drag.current;
      drag.current = null;
      if (!d) return;
      const cur = picked.current;
      picked.current = null;
      setPick(null);
      if (cur && cur.b - cur.a > 0.05) onChange(compTake(p, clipId, cur.take, cur.a, cur.b), cur.a);
      else onHear(compTake(p, clipId, d.take, lo, hi), d.t0);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
  });
  if (!g || g.clips[0].takes.length < 2) {
    return <p className="small muted">{t("This clip has one take. Takes come from recording round a loop, new parts, and making a stretch again, longer or dry.")}</p>;
  }
  const whole = compWhole(g.clips);
  const pct = (s: number) => `${(s / span) * 100}%`;
  return (
    <div className="take-lanes">
      <span className="small muted">{t("Click a take to hear it from there in the mix. Drag across a take to use that stretch of it; the highlighted parts are what plays.")}</span>
      {g.clips[0].takes.map((k, i) => (
        <div className="take-lane" key={k.id}>
          <div className="take-lane-head">
            <strong>{t("Take {n}", { n: i + 1 })}</strong>
            <span className="muted ellipsis" title={k.label}>{k.label}</span>
            <div className="row">
              <button className="btn ghost small" onClick={() => onChange(compTake(p, clipId, k.id, lo, hi), lo)}>{t("Use for all")}</button>
              <button className="btn ghost small" onClick={() => onChange(removeTake(p, clipId, k.id), lo)}>{t("Remove")}</button>
            </div>
          </div>
          <div
            className="take-wave"
            onMouseDown={(e) => {
              if (e.button !== 0) return;
              const el = e.currentTarget as HTMLElement;
              drag.current = { take: k.id, t0: tAt(el, e.clientX), el };
            }}
          >
            <Wave clip={{ ...whole, take: k.id }} width={1200} height={48} color="rgba(128,128,128,0.85)" />
            {g.clips.filter((c) => (c.take ?? c.takes[0]?.id) === k.id).map((c) => (
              <span key={c.id} className="take-used" style={{ left: pct(c.start - lo), width: pct(c.length) }} />
            ))}
            {pick?.take === k.id && <span className="take-pick" style={{ left: pct(pick.a - lo), width: pct(pick.b - pick.a) }} />}
          </div>
        </div>
      ))}
    </div>
  );
}

/** A track's effects in order, each with its settings; effects added one
 * by one or as a whole set kept by name (built from words in the chat bar,
 * or saved from a track), and this track's set saved by name. */
function EffectsPanel({ track, onChange, toast }: { track: Track; onChange: (e: Track["effects"]) => void; toast: PushToast }) {
  const learn = useContext(LearnContext);
  const [kept, setKept] = useState<EffectChain[]>([]);
  const [name, setName] = useState("");
  useEffect(() => {
    api.studioLibrary("effects").then((l) => setKept(l as EffectChain[])).catch(() => {});
  }, []);
  const set = (id: string, patch: Partial<Track["effects"][number]>) => onChange(track.effects.map((e) => (e.id === id ? { ...e, ...patch } : e)));
  const add = (v: string) => {
    if (v.startsWith("#")) {
      const chain = kept.find((c) => c.name === v.slice(1));
      if (chain) onChange([...track.effects, ...chain.effects.filter((e) => EFFECTS[e.kind]).map((e) => ({ ...e, id: newId("fx") }))]);
      return;
    }
    if (v) onChange([...track.effects, newEffect(v as EffectKind)]);
  };
  const keep = async () => {
    const chain = { name: name.trim(), effects: track.effects };
    try {
      await api.studioLibrarySave("effects", chain);
      setKept((k) => [...k.filter((c) => c.name !== chain.name), chain]);
      setName("");
      toast(t("Saved as {name}.", { name: chain.name }), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <div className="studio-effects">
      <div className="row wrap">
        <strong>{track.name}</strong>
        <select value="" onChange={(e) => add(e.target.value)} aria-label={t("Add an effect")}>
          <option value="">{t("Add an effect…")}</option>
          <optgroup label={t("Effects")}>
            {(Object.keys(EFFECTS) as EffectKind[]).map((k) => (
              <option key={k} value={k}>{t(EFFECTS[k].label)}</option>
            ))}
          </optgroup>
          {kept.length > 0 && (
            <optgroup label={t("Your effects")}>
              {kept.map((c) => (
                <option key={c.name} value={`#${c.name}`}>{c.name}</option>
              ))}
            </optgroup>
          )}
        </select>
        {track.effects.length > 0 && (
          <>
            <input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("Name to save these as")} aria-label={t("Name to save these as")} />
            <button className="btn small" disabled={!name.trim()} onClick={keep}>{t("Save these effects")}</button>
          </>
        )}
      </div>
      {kept.length > 0 && (
        <div className="row wrap small">
          <span className="muted">{t("Your effects:")}</span>
          {kept.map((c) => (
            <span key={c.name} className="chip">
              {c.name}
              <button
                className="link small"
                aria-label={t("Delete {name}", { name: c.name })}
                onClick={() => api.studioLibraryDelete("effects", c.name).then(() => setKept((k) => k.filter((x) => x.name !== c.name))).catch((e) => toast(errorText(e), "error"))}
              >
                ✕
              </button>
            </span>
          ))}
        </div>
      )}
      {!track.effects.length && <p className="small muted">{t("No effects on this track. Describe one in the chat bar (\"a lo-fi telephone voice\") and it's built and kept here.")}</p>}
      {track.effects.map((e, i) => (
        <div key={e.id} className="studio-effect">
          <div className="row">
            <strong className="small">{t(EFFECTS[e.kind].label)}</strong>
            <label className="row small">
              <input type="checkbox" checked={!e.bypass} onChange={(x) => set(e.id, { bypass: !x.target.checked })} /> {t("On")}
            </label>
            <span className="spacer" />
            <button className="btn ghost small" disabled={i === 0} onClick={() => onChange(track.effects.map((x, j) => (j === i - 1 ? e : j === i ? track.effects[i - 1] : x)))} aria-label={t("Move up")}>↑</button>
            <button className="btn ghost small" onClick={() => onChange(track.effects.filter((x) => x.id !== e.id))}>{t("Remove")}</button>
          </div>
          <div className="row wrap">
            {Object.entries(EFFECTS[e.kind].params).map(([k, [min, max]]) => {
              // Frequencies on a logarithmic slider: as much room for each octave.
              const log = LOG_PARAMS.has(k);
              const toU = (v: number) => (log ? Math.log(v / min) / Math.log(max / min) : (v - min) / (max - min));
              const fromU = (u: number) => (log ? min * Math.pow(max / min, u) : min + (max - min) * u);
              const value = e.params[k] ?? EFFECTS[e.kind].params[k][2];
              return (
                <label key={k} className="row small" title={`${Math.round(value * 100) / 100}`}>
                  {t(PARAM_NAMES[k] ?? k)}
                  <input type="range" min={0} max={1} step={0.005} value={toU(value)} onChange={(x) => set(e.id, { params: { ...e.params, [k]: fromU(+x.target.value) } })} {...learnable(learn, { id: `fx:${track.id}:${e.id}:${k}`, label: `${track.name}: ${t(EFFECTS[e.kind].label)} ${t(PARAM_NAMES[k] ?? k)}`, min, max, log })} />
                </label>
              );
            })}
          </div>
        </div>
      ))}
    </div>
  );
}
