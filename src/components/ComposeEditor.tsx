// SPDX-License-Identifier: AGPL-3.0-only
// Compose's editor: a song written as notes, laid out the way a music
// program lays one out. Bars and beats across the top in the song's own time
// signature, its sections and chords, then a lane a part (melody, chords,
// bass, drums) played on sampled instruments. Notes snap to the grid; they
// can be selected, moved, lengthened, added, copied and deleted, and every
// edit can be undone and redone. A section can be changed in words by the
// chat model, written again, edited, copied, added or deleted: only that
// section's notes are written again, and everything after it moves with it.
// "Make it real" has the music model record a song that follows these notes
// and sings their words; the composition stays as it is.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { api, errorText, on, toBase64, type Composition, type ComposeNote, type MediaItem, type MediaModelCard, type Plan, type PlanSection, type SectionOp } from "../api";
import { clock } from "../format";
import { t, tx } from "../i18n";
import { encodeWav } from "../media";
import { ComposePlayer, INSTRUMENTS, KITS, renderDemo, secondsPerBeat } from "../compose/player";
import { MakeItReal, fitShift, type RealOptions } from "./MakeItReal";
import type { PushToast } from "./Toasts";
import { confirmed } from "../confirm";

type ENote = ComposeNote & { id: number };
type EPart = { name: string; instrument: string; notes: ENote[] };
type EComp = { plan: Plan; parts: EPart[] };

const PART_LABEL: Record<string, string> = { melody: tx("Melody"), chords: tx("Chords"), bass: tx("Bass"), drums: tx("Drums") };
const LANE_HEIGHT: Record<string, number> = { melody: 96, chords: 96, bass: 64, drums: 100 };
const OPEN_HEIGHT = 260;
const RULER = 20;
const SECTION_ROW = 26;
const CHORD_ROW = 20;
const TOP = RULER + SECTION_ROW + CHORD_ROW + 6;
/** The column of lane names, left of the timeline (as in styles.css). */
const LABELS = 150;
const HISTORY = 200;
/** Drum rows, top to bottom (General MIDI). */
const DRUM_ROWS: [number, string][] = [
  [49, tx("Crash")],
  [51, tx("Ride")],
  [46, tx("Open hi-hat")],
  [42, tx("Hi-hat")],
  [50, tx("High tom")],
  [48, tx("Tom")],
  [47, tx("Mid tom")],
  [45, tx("Low tom")],
  [38, tx("Snare")],
  [36, tx("Kick")],
];
/** Grid steps, in beats (0: a bar; -1: no snapping). */
const SNAPS: [number, string][] = [
  [0, tx("Bar")],
  [1, tx("Beat")],
  [0.5, tx("Half beat")],
  [0.25, tx("Quarter beat")],
  [-1, tx("No snapping")],
];
export const TIMES: [number, number][] = [
  [2, 4],
  [3, 4],
  [4, 4],
  [5, 4],
  [7, 4],
  [6, 8],
  [7, 8],
  [9, 8],
  [12, 8],
];
const DRUMS = ["none", "light", "steady", "busy", "half-time"] as const;
const BASS = ["none", "roots", "pulse", "walking", "syncopated"] as const;
const KEYS = ["none", "held", "arpeggio", "strummed", "stabs"] as const;
const STYLE_LABEL: Record<string, string> = {
  none: tx("None"),
  light: tx("Light"),
  steady: tx("Steady"),
  busy: tx("Busy"),
  "half-time": tx("Half-time"),
  roots: tx("Roots"),
  pulse: tx("Pulse"),
  walking: tx("Walking"),
  syncopated: tx("Syncopated"),
  held: tx("Held"),
  arpeggio: tx("Arpeggio"),
  strummed: tx("Strummed"),
  stabs: tx("Stabs"),
};
export const NOTE_NAMES = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];

let nextId = 1;
function editable(c: Composition): EComp {
  return { plan: c.plan, parts: c.parts.map((p) => ({ name: p.name, instrument: p.instrument, notes: p.notes.map((n) => ({ ...n, id: nextId++ })) })) };
}
function plain(c: EComp): Composition {
  return { plan: c.plan, parts: c.parts.map((p) => ({ name: p.name, instrument: p.instrument, notes: p.notes.map(({ id: _, ...n }) => n) })) };
}
function noteName(p: number) {
  return `${NOTE_NAMES[p % 12]}${Math.floor(p / 12) - 1}`;
}

/** A chord's name moved by `steps` semitones ("Am7" up 2 is "Bm7"). */
function transposeChord(name: string, steps: number): string {
  const one = (s: string) => {
    const m = /^([A-G])([#b]?)(.*)$/.exec(s.trim());
    if (!m) return s;
    const base = { C: 0, D: 2, E: 4, F: 5, G: 7, A: 9, B: 11 }[m[1] as "C"] + (m[2] === "#" ? 1 : m[2] === "b" ? -1 : 0);
    return NOTE_NAMES[(((base + steps) % 12) + 12) % 12] + m[3];
  };
  return name.split("/").map(one).join("/");
}

/** A key's tonic as a pitch class ("A minor" is 9). */
function keyOf(key: string): number {
  const m = /^\s*([A-G])([#b]?)/.exec(key);
  return m ? ({ C: 0, D: 2, E: 4, F: 5, G: 7, A: 9, B: 11 }[m[1] as "C"] + (m[2] === "#" ? 1 : m[2] === "b" ? -1 : 0) + 12) % 12 : 0;
}

export function ComposeEditor({ item, composition, onClose, onSaved, onStarted, musicModels, defaultMusic, toast }: {
  /** The gallery item it was opened from (null: not kept yet). */
  item: MediaItem | null;
  composition: Composition;
  onClose: () => void;
  onSaved: (i: MediaItem) => void;
  /** "Make it real" started. */
  onStarted: () => void;
  /** A music model is installed. */
  musicModels: MediaModelCard[];
  defaultMusic: string | null;
  toast: PushToast;
}) {
  const box = useRef<HTMLDivElement>(null);
  const player = useRef<ComposePlayer | null>(null);
  const [kept, setKept] = useState<MediaItem | null>(item);
  // The composition and its history, changed together (a held Ctrl+Z steps back one at a time).
  const [hist, setHist] = useState<{ past: EComp[]; now: EComp; future: EComp[] }>(() => ({ past: [], now: editable(composition), future: [] }));
  const { past, now: comp, future } = hist;
  const setComp = (now: EComp) => setHist((h) => ({ ...h, now }));
  const [saved, setSaved] = useState<EComp | null>(() => (item ? hist.now : null));
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [fitWidth, setFitWidth] = useState(900);
  const [zoom, setZoom] = useState(1);
  const [snap, setSnap] = useState(1);
  const [open, setOpen] = useState<string | null>("melody");
  const [muted, setMuted] = useState<Set<string>>(new Set());
  const [playing, setPlaying] = useState(false);
  const [loading, setLoading] = useState<number | null>(null);
  const [pos, setPos] = useState(0);
  const [menu, setMenu] = useState<{ i: number; x: number; y: number } | null>(null);
  const [panel, setPanel] = useState<{ kind: "ai" | "edit" | "add"; i: number; s: PlanSection } | null>(null);
  const [aiText, setAiText] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [real, setReal] = useState(false);
  // As `media::REAL_STRENGTH_SUNG` and `_PLAYED`: a sung song follows less closely, so it sings.
  const clip = useRef<{ part: string; notes: ComposeNote[] }[] | null>(null);
  // A loop region (in beats, whole bars) that playing goes round, and the metronome.
  const [loop, setLoop] = useState<{ a: number; b: number } | null>(null);
  const [looping, setLooping] = useState(false);
  const [click, setClick] = useState(false);
  const loopDrag = useRef<{ x: number; left: number; anchor: number; moved: boolean } | null>(null);
  // What's playing (an edit while playing is heard from where it is).
  const lastPlayed = useRef<EComp | null>(null);
  // A drag in progress: the notes it moves, the composition before it and its latest result.
  const drag = useRef<{ kind: "move" | "resize"; x: number; y: number; ids: Set<number>; base: EComp; last: EComp; moved: boolean; perPitch: number; drums: boolean } | null>(null);
  const dirty = saved !== comp;
  const plan = comp.plan;

  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const fit = () => setFitWidth(Math.max(400, el.clientWidth - LABELS));
    fit();
    const ro = new ResizeObserver(fit);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  useEffect(() => () => player.current?.close(), []);
  useEffect(() => {
    const sub = on("compose:getting", (d) => setLoading(d.fraction));
    return () => {
      sub.then((un) => un());
    };
  }, []);

  // ---------- the grid ----------
  const barBeats = Math.max(1, plan.beats);
  const starts = useMemo(() => {
    let at = 0;
    return plan.sections.map((s) => {
      const a = at;
      at += s.bars * barBeats;
      return a;
    });
  }, [plan.sections, barBeats]);
  const total = Math.max(barBeats, plan.sections.reduce((n, s) => n + s.bars, 0) * barBeats);
  const spb = secondsPerBeat(plan);
  const ppb = (fitWidth / total) * zoom;
  const width = Math.max(fitWidth, total * ppb);
  const x = (beat: number) => beat * ppb;
  const step = snap === 0 ? barBeats : snap;
  const floorSnap = (b: number) => (snap < 0 ? Math.round(b * 64) / 64 : Math.floor(b / step + 1e-6) * step);
  const bars = Math.round(total / barBeats);
  const barEvery = Math.max(1, Math.ceil(28 / (barBeats * ppb)));
  const chords = useMemo(() => {
    const out: { beat: number; name: string }[] = [];
    plan.sections.forEach((s, i) => {
      for (let b = 0; b < s.bars; b++) {
        const name = s.chords.length ? s.chords[b % s.chords.length] : "";
        if (name) out.push({ beat: starts[i] + b * barBeats, name });
      }
    });
    return out;
  }, [plan.sections, starts, barBeats]);

  // Each lane's pitch range: what it had when opened, widened by edits.
  const firstRange = useMemo(
    () =>
      Object.fromEntries(
        composition.parts.map((p) => {
          const ps = p.notes.map((n) => n.pitch);
          return [p.name, ps.length ? [Math.min(...ps) - 3, Math.max(...ps) + 3] : [48, 72]];
        }),
      ) as Record<string, [number, number]>,
    [composition],
  );
  const lanes = useMemo(() => {
    let y = TOP;
    return comp.parts.map((p, i) => {
      const drums = p.name === "drums";
      const h = open === p.name ? OPEN_HEIGHT : (LANE_HEIGHT[p.name] ?? 80);
      const [lo0, hi0] = firstRange[p.name] ?? [48, 72];
      const ps = p.notes.map((n) => n.pitch);
      const lo = Math.min(lo0, ...ps);
      const hi = Math.max(hi0, ...ps, lo + 12);
      const lane = { part: p, index: i, y, h, lo, hi, drums };
      y += h + 6;
      return lane;
    });
  }, [comp.parts, open, firstRange]);
  type Lane = (typeof lanes)[number];
  const height = lanes.length ? lanes[lanes.length - 1].y + lanes[lanes.length - 1].h + 4 : TOP + 10;
  const rowH = (l: Lane) => (l.drums ? (l.h - 4) / DRUM_ROWS.length : (l.h - 6) / Math.max(12, l.hi - l.lo + 1));
  const noteY = (l: Lane, pitch: number) => {
    if (l.drums) {
      const r = DRUM_ROWS.findIndex(([p]) => p === pitch);
      return l.y + 2 + (r < 0 ? DRUM_ROWS.length - 1 : r) * rowH(l);
    }
    return l.y + 3 + (l.hi - pitch) * rowH(l);
  };
  const pitchAt = (l: Lane, py: number) => {
    if (l.drums) return DRUM_ROWS[Math.max(0, Math.min(DRUM_ROWS.length - 1, Math.floor((py - l.y - 2) / rowH(l))))][0];
    return Math.max(0, Math.min(127, l.hi - Math.floor((py - l.y - 3) / rowH(l))));
  };

  // ---------- history ----------
  const commit = useCallback((before: EComp, after: EComp) => {
    if (before === after) return;
    setHist((h) => ({ past: [...h.past.slice(-(HISTORY - 1)), before], now: after, future: [] }));
  }, []);
  const change = (after: EComp) => commit(comp, after);
  const undo = () => {
    setHist((h) => (h.past.length ? { past: h.past.slice(0, -1), now: h.past[h.past.length - 1], future: [h.now, ...h.future] } : h));
    setSelected(new Set());
  };
  const redo = () => {
    setHist((h) => (h.future.length ? { past: [...h.past, h.now], now: h.future[0], future: h.future.slice(1) } : h));
    setSelected(new Set());
  };
  const mapSelected = (f: (n: ENote, part: string) => ENote | null) =>
    change({
      ...comp,
      parts: comp.parts.map((p) => ({
        ...p,
        notes: p.notes
          .map((n) => (selected.has(n.id) ? f(n, p.name) : n))
          .filter((n): n is ENote => n !== null)
          .sort((a, b) => a.beat - b.beat),
      })),
    });
  const removeSelected = () => {
    if (!selected.size) return;
    mapSelected(() => null);
    setSelected(new Set());
  };
  const copySelected = () => {
    const picked = comp.parts.map((p) => ({ part: p.name, notes: p.notes.filter((n) => selected.has(n.id)) })).filter((p) => p.notes.length);
    if (!picked.length) return;
    const first = Math.min(...picked.flatMap((p) => p.notes.map((n) => n.beat)));
    clip.current = picked.map((p) => ({ part: p.part, notes: p.notes.map(({ id: _, ...n }) => ({ ...n, beat: n.beat - first })) }));
  };
  const paste = () => {
    if (!clip.current) return;
    const at = floorSnap(pos / spb);
    const added = new Set<number>();
    change({
      ...comp,
      parts: comp.parts.map((p) => {
        const c = clip.current!.find((x) => x.part === p.name);
        if (!c) return p;
        const fresh = c.notes.map((n) => {
          const id = nextId++;
          added.add(id);
          return { ...n, beat: n.beat + at, id };
        });
        return { ...p, notes: [...p.notes, ...fresh].sort((a, b) => a.beat - b.beat) };
      }),
    });
    setSelected(added);
  };

  // ---------- playing ----------
  const getPlayer = () => {
    if (!player.current) {
      player.current = new ComposePlayer();
      player.current.onEnd = () => setPlaying(false);
    }
    return player.current;
  };
  const stop = useCallback(() => {
    player.current?.stop();
    setPlaying(false);
  }, []);
  /** The composition as heard: with a click on every beat when the metronome is on (louder on each bar's first). */
  const heard = (c: EComp): Composition => {
    const p = plain(c);
    if (!click) return p;
    const notes = Array.from({ length: Math.round(total) }, (_, i) => ({ beat: i, len: 0.1, pitch: i % barBeats === 0 ? 84 : 77, vel: i % barBeats === 0 ? 115 : 80 }));
    return { ...p, parts: [...p.parts, { name: "click", instrument: "woodblock", notes }] };
  };
  const play = async (from = pos) => {
    const p = getPlayer();
    // Looping: from the loop's start when the playhead is outside it.
    const start = looping && loop && (from < loop.a * spb || from >= loop.b * spb) ? loop.a * spb : from;
    try {
      if (!p.loaded) setLoading((l) => l ?? 0);
      lastPlayed.current = comp;
      await p.play(heard(comp), start, muted);
      setPlaying(true);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setLoading(null);
    }
  };
  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    const tick = () => {
      const now = player.current?.time ?? 0;
      if (looping && loop && now >= loop.b * spb) {
        player.current?.seek(loop.a * spb);
        setPos(loop.a * spb);
        raf = requestAnimationFrame(tick);
        return;
      }
      setPos(now);
      if (now >= total * spb + 0.5) stop();
      else raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing, total, spb, stop, looping, loop]);
  // The metronome turned on or off while playing: heard from where it is.
  useEffect(() => {
    if (playing) player.current?.play(heard(comp), player.current.time, muted).catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [click]);
  // An edit while playing: heard from where it is.
  useEffect(() => {
    if (!playing || lastPlayed.current === comp || drag.current) return;
    lastPlayed.current = comp;
    const timer = setTimeout(() => player.current?.play(heard(comp), player.current.time, muted).catch(() => {}), 150);
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [comp, playing]);
  const hear = (part: string, pitch: number) => {
    if (!muted.has(part)) getPlayer().preview(plain(comp), part, pitch).catch(() => {});
  };

  // ---------- keys ----------
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      const typing = e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement || e.target instanceof HTMLSelectElement;
      if (typing || busy) return;
      const mod = e.ctrlKey || e.metaKey;
      const k = e.key.toLowerCase();
      if (mod && k === "z" && !e.shiftKey) (e.preventDefault(), undo());
      else if (mod && (k === "y" || (k === "z" && e.shiftKey))) (e.preventDefault(), redo());
      else if (mod && k === "a") {
        e.preventDefault();
        setSelected(new Set(comp.parts.filter((p) => !open || p.name === open).flatMap((p) => p.notes.map((n) => n.id))));
      } else if (mod && k === "c") copySelected();
      else if (mod && k === "v") (e.preventDefault(), paste());
      else if (e.key === "Delete" || e.key === "Backspace") (e.preventDefault(), removeSelected());
      else if (e.key === " ") (e.preventDefault(), playing ? stop() : play());
      else if (e.key === "Escape" && selected.size) setSelected(new Set());
      else if (selected.size && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
        e.preventDefault();
        const d = (e.key === "ArrowUp" ? 1 : -1) * (e.shiftKey ? 12 : 1);
        mapSelected((n, part) => {
          if (part !== "drums") return { ...n, pitch: Math.max(0, Math.min(127, n.pitch + d)) };
          const r = DRUM_ROWS.findIndex(([p]) => p === n.pitch);
          return { ...n, pitch: DRUM_ROWS[Math.max(0, Math.min(DRUM_ROWS.length - 1, r - Math.sign(d)))][0] };
        });
      } else if (selected.size && (e.key === "ArrowLeft" || e.key === "ArrowRight")) {
        e.preventDefault();
        const d = (e.key === "ArrowRight" ? 1 : -1) * (snap < 0 ? 0.25 : step);
        mapSelected((n) => ({ ...n, beat: Math.max(0, Math.min(total - 0.01, n.beat + d)) }));
      }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  });

  // ---------- editing notes with the mouse ----------
  const svgPoint = (e: React.MouseEvent) => {
    const r = (e.currentTarget as Element).closest("svg")!.getBoundingClientRect();
    return { px: e.clientX - r.left, py: e.clientY - r.top };
  };
  const onNoteDown = (e: React.MouseEvent, l: Lane, n: ENote) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    const { px } = svgPoint(e);
    const right = x(n.beat + n.len);
    const kind = !l.drums && right - px < 6 && right - x(n.beat) > 10 ? "resize" : "move";
    const sel = e.shiftKey ? new Set(selected).add(n.id) : selected.has(n.id) ? selected : new Set([n.id]);
    setSelected(sel);
    hear(l.part.name, n.pitch);
    drag.current = { kind, x: e.clientX, y: e.clientY, ids: sel, base: comp, last: comp, moved: false, perPitch: rowH(l), drums: l.drums };
  };
  useEffect(() => {
    const move = (e: MouseEvent) => {
      const d = drag.current;
      if (!d) return;
      if (!d.moved && Math.abs(e.clientX - d.x) < 3 && Math.abs(e.clientY - d.y) < 3) return;
      d.moved = true;
      const raw = (e.clientX - d.x) / ppb;
      const dt = snap < 0 ? raw : Math.round(raw / step) * step;
      const steps = -Math.round((e.clientY - d.y) / d.perPitch);
      d.last = {
        ...d.base,
        parts: d.base.parts.map((p) => ({
          ...p,
          notes: p.notes.map((n) => {
            if (!d.ids.has(n.id)) return n;
            if (d.kind === "resize") {
              const end = snap < 0 ? n.beat + n.len + raw : Math.round((n.beat + n.len + raw) / step) * step;
              return { ...n, len: Math.max(snap < 0 ? 0.0625 : Math.min(step, 0.25), end - n.beat) };
            }
            const beat = Math.max(0, Math.min(total - 0.01, n.beat + dt));
            if (p.name === "drums") {
              const r = DRUM_ROWS.findIndex(([q]) => q === n.pitch);
              return { ...n, beat, pitch: DRUM_ROWS[Math.max(0, Math.min(DRUM_ROWS.length - 1, (r < 0 ? DRUM_ROWS.length - 1 : r) - steps))][0] };
            }
            return { ...n, beat, pitch: Math.max(0, Math.min(127, n.pitch + steps)) };
          }),
        })),
      };
      setComp(d.last);
    };
    const up = () => {
      const d = drag.current;
      drag.current = null;
      if (!d?.moved) return;
      const sorted = { ...d.last, parts: d.last.parts.map((p) => ({ ...p, notes: [...p.notes].sort((a, b) => a.beat - b.beat) })) };
      commit(d.base, sorted);
      // A moved note is heard at its new pitch.
      const first = sorted.parts.flatMap((p) => p.notes.filter((n) => d.ids.has(n.id)).map((n) => [p.name, n.pitch] as const))[0];
      if (first && d.kind === "move") hear(first[0], first[1]);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ppb, snap, step, total, commit]);
  const addAt = (e: React.MouseEvent, l: Lane) => {
    const { px, py } = svgPoint(e);
    const beat = Math.max(0, Math.min(total - 0.01, floorSnap(px / ppb)));
    const pitch = pitchAt(l, py);
    const len = l.drums ? 0.25 : snap < 0 ? 1 : step;
    const n: ENote = { id: nextId++, beat, len, pitch, vel: 90 };
    change({ ...comp, parts: comp.parts.map((p, i) => (i === l.index ? { ...p, notes: [...p.notes, n].sort((a, b) => a.beat - b.beat) } : p)) });
    setSelected(new Set([n.id]));
    hear(l.part.name, pitch);
  };
  const seek = (e: React.MouseEvent) => {
    const { px, py } = svgPoint(e);
    // On the bar numbers: dragging sets a loop region (a click still moves the playhead).
    if (py < RULER) {
      const left = (e.currentTarget as Element).closest("svg")!.getBoundingClientRect().left;
      loopDrag.current = { x: e.clientX, left, anchor: px / ppb, moved: false };
    }
    const to = Math.max(0, Math.min(total, px / ppb)) * spb;
    setPos(to);
    setSelected(new Set());
    if (playing) play(to);
  };
  const melodyPitches = useMemo(() => comp.parts.find((p) => p.name === "melody")?.notes.map((n) => n.pitch) ?? [], [comp.parts]);
  useEffect(() => {
    const move = (e: MouseEvent) => {
      const d = loopDrag.current;
      if (!d || (!d.moved && Math.abs(e.clientX - d.x) < 4)) return;
      d.moved = true;
      const at = Math.max(0, Math.min(total, (e.clientX - d.left) / ppb));
      const a = Math.floor(Math.min(d.anchor, at) / barBeats) * barBeats;
      const b = Math.max(a + barBeats, Math.ceil(Math.max(d.anchor, at) / barBeats) * barBeats);
      setLoop({ a, b: Math.min(b, total) });
      setLooping(true);
    };
    const up = () => {
      loopDrag.current = null;
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
  }, [ppb, total, barBeats]);
  /** Loop on or off; with no region yet, the section the playhead is in. */
  const toggleLoop = () => {
    if (!loop) {
      const at = pos / spb;
      const i = Math.max(0, starts.findIndex((s0, k) => at >= s0 && at < s0 + plan.sections[k].bars * barBeats));
      setLoop({ a: starts[i] ?? 0, b: (starts[i] ?? 0) + (plan.sections[i]?.bars ?? 4) * barBeats });
    }
    setLooping((l) => !l);
  };
  const selectedNotes = comp.parts.flatMap((p) => p.notes.filter((n) => selected.has(n.id)));
  const loudness = selectedNotes.length ? Math.round(selectedNotes.reduce((s, n) => s + n.vel, 0) / selectedNotes.length) : 0;

  // ---------- the song's settings ----------
  const setPlan = (p: Partial<Plan>) => change({ ...comp, plan: { ...plan, ...p } });
  const setTime = async (beats: number, unit: number) => {
    if (beats === plan.beats && unit === plan.unit) return;
    stop();
    setBusy(t("Writing the notes again in {time}", { time: `${beats}/${unit}` }));
    try {
      const next = await api.composeArrange({ ...plan, beats, unit });
      // The parts keep the instruments picked for them.
      const kept = editable({ ...next, parts: next.parts.map((p) => ({ ...p, instrument: comp.parts.find((q) => q.name === p.name)?.instrument ?? p.instrument })) });
      change(kept);
      toast(t("Every note is written again in {time}. Undo brings back the notes as they were.", { time: `${beats}/${unit}` }), "info");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };
  /** A new key: every chord and note moved there, the nearest way (up to a tritone). */
  const setKey = (key: string) => {
    let d = (keyOf(key) - keyOf(plan.key) + 12) % 12;
    if (d > 6) d -= 12;
    change({
      plan: { ...plan, key, sections: plan.sections.map((s) => ({ ...s, chords: s.chords.map((c) => transposeChord(c, d)) })) },
      parts: comp.parts.map((p) => (p.name === "drums" ? p : { ...p, notes: p.notes.map((n) => ({ ...n, pitch: Math.max(0, Math.min(127, n.pitch + d)) })) })),
    });
  };
  const setInstrument = (part: string, instrument: string) =>
    change({
      plan: { ...plan, instruments: { ...plan.instruments, [part]: instrument } },
      parts: comp.parts.map((p) => (p.name === part ? { ...p, instrument } : p)),
    });

  // ---------- sections ----------
  const section = async (i: number, op: SectionOp, words?: string) => {
    stop();
    setBusy(words ? t("The chat model is changing the {name}", { name: t(plan.sections[i]?.name ?? "") }) : t("Writing the notes"));
    try {
      const next = await api.composeSection(plain(comp), i, op, words);
      change(editable(next));
      setSelected(new Set());
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };
  useEffect(() => {
    if (!menu) return;
    const shut = () => setMenu(null);
    window.addEventListener("click", shut);
    return () => window.removeEventListener("click", shut);
  }, [menu]);

  // ---------- keeping it ----------
  /** The demo rendered and kept with the composition; the gallery item. */
  const keep = async (now: EComp = comp): Promise<MediaItem | null> => {
    stop();
    setBusy(t("Recording the demo on the instruments"));
    try {
      const c = plain(now);
      const seconds = total * spb;
      const buf = await renderDemo(c, seconds);
      const wav = encodeWav(buf, 0, Math.min(buf.duration, seconds + 1.5));
      const made = await api.composeSave(kept?.id ?? null, c, toBase64(wav), seconds);
      setKept(made);
      setSaved(now);
      onSaved(made);
      return made;
    } catch (e) {
      toast(errorText(e), "error");
      return null;
    } finally {
      setBusy(null);
    }
  };
  const saveMidi = async () => {
    const base = (plan.title || "song").replace(/[^\w\- ]+/g, "").trim().slice(0, 40).replace(/\s+/g, "-") || "song";
    const path = await save({ defaultPath: `${base}.mid`, filters: [{ name: "MIDI", extensions: ["mid"] }] });
    if (!path) return;
    try {
      await api.composeMidi(plain(comp), path);
      toast(t("The song is saved as a MIDI file."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const makeReal = async (o: RealOptions) => {
    // Another voice: the tune moves by octaves into its range (made real, the
    // music model follows the demo's pitch), and the demo is recorded again.
    const voice = o.song.voice ?? "";
    const shift = voice ? fitShift(melodyPitches, voice) : 0;
    let now = comp;
    if (voice && (shift !== 0 || voice !== plan.voice)) {
      now = {
        plan: { ...plan, voice },
        parts: comp.parts.map((p) => (p.name === "melody" && shift ? { ...p, notes: p.notes.map((n) => ({ ...n, pitch: Math.max(0, Math.min(127, n.pitch + shift)) })) } : p)),
      };
      change(now);
    }
    const it = now !== comp || dirty || !kept ? await keep(now) : kept;
    if (!it) return;
    try {
      await api.composeMakeReal(it.id, o);
      setReal(false);
      toast(t("Started. The real version appears in the gallery when it's done; your composition stays as it is."), "success");
      onStarted();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const close = async () => {
    if (dirty && !await confirmed(kept ? t("Close without keeping your changes?") : t("Close without keeping this song? It isn't in the gallery yet."))) return;
    stop();
    onClose();
  };

  const posBeat = pos / spb;
  const where = `${Math.floor(posBeat / barBeats) + 1}.${Math.floor(posBeat % barBeats) + 1}`;
  const keyNames = NOTE_NAMES.flatMap((n) => [`${n} major`, `${n} minor`]);

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="modal song-view compose-view" role="dialog" aria-modal="true" aria-label={t("Compose music")}>
        <div className="row wrap">
          <input className="input compose-title" value={plan.title} placeholder={t("Untitled song")} aria-label={t("Title")} onChange={(e) => setComp({ ...comp, plan: { ...plan, title: e.target.value } })} />
          <span className="spacer" />
          <button className="btn small" onClick={undo} disabled={!past.length || !!busy} title={t("Undo (Ctrl+Z)")}>{t("Undo")}</button>
          <button className="btn small" onClick={redo} disabled={!future.length || !!busy} title={t("Redo (Ctrl+Y)")}>{t("Redo")}</button>
          <button className="btn small" onClick={() => keep()} disabled={(!dirty && !!kept) || !!busy}>{kept ? t("Keep changes") : t("Keep in the gallery")}</button>
          <button className="btn small" onClick={saveMidi} disabled={!!busy}>{t("Save as MIDI")}</button>
          <button className="btn small primary" onClick={() => setReal(!real)} disabled={!!busy} aria-expanded={real}>{t("Make it real")}</button>
          <button className="icon-btn" aria-label={t("Close")} onClick={close}>✕</button>
        </div>
        {real && <MakeItReal plan={plan} melody={melodyPitches} models={musicModels} defaultModel={defaultMusic} busy={!!busy} onCancel={() => setReal(false)} onRecord={makeReal} />}
        <div className="row wrap song-transport">
          <button className="btn small" onClick={() => (playing ? stop() : play())} disabled={!!busy}>{playing ? t("Stop") : t("Play")}</button>
          <button className={`btn small ${looping ? "active" : ""}`} aria-pressed={looping} onClick={toggleLoop} title={t("Play a part of the song over and over: drag across the bar numbers to set it")}>{t("Loop")}</button>
          <button className={`btn small ${click ? "active" : ""}`} aria-pressed={click} onClick={() => setClick((c) => !c)} title={t("A click on every beat, louder on the first of each bar")}>{t("Metronome")}</button>
          <span className="small mono" title={t("Bar and beat")}>{where}</span>
          <span className="small mono muted">{clock(pos)} / {clock(total * spb)}</span>
          {loading !== null && <span className="small muted">{loading > 0 && loading < 1 ? t("Getting the instruments (once): {pct}%", { pct: Math.round(loading * 100) }) : t("Loading the instruments")}</span>}
          <span className="spacer" />
          <label className="small row">
            {t("Tempo")}
            <input type="number" className="input narrow" min={40} max={220} value={plan.bpm} onChange={(e) => setPlan({ bpm: Math.max(40, Math.min(220, +e.target.value || plan.bpm)) })} />
          </label>
          <label className="small row">
            {t("Time signature")}
            <select className="input" value={`${plan.beats}/${plan.unit}`} disabled={!!busy} onChange={(e) => { const [b, u] = e.target.value.split("/").map(Number); setTime(b, u); }}>
              {[...TIMES, ...(TIMES.some(([b, u]) => b === plan.beats && u === plan.unit) ? [] : [[plan.beats, plan.unit] as [number, number]])].map(([b, u]) => (
                <option key={`${b}/${u}`} value={`${b}/${u}`}>{`${b}/${u}`}</option>
              ))}
            </select>
          </label>
          <label className="small row">
            {t("Key")}
            <select className="input" value={keyNames.find((k) => k.toLowerCase() === plan.key.toLowerCase()) ?? plan.key} onChange={(e) => setKey(e.target.value)}>
              {!keyNames.some((k) => k.toLowerCase() === plan.key.toLowerCase()) && <option value={plan.key}>{plan.key}</option>}
              {keyNames.map((k) => (
                <option key={k} value={k}>{k.replace("major", t("major")).replace("minor", t("minor"))}</option>
              ))}
            </select>
          </label>
          <label className="small row">
            {t("Snap")}
            <select className="input" value={snap} onChange={(e) => setSnap(+e.target.value)}>
              {SNAPS.map(([v, label]) => (
                <option key={v} value={v}>{t(label)}</option>
              ))}
            </select>
          </label>
          <button className="btn small" onClick={() => setZoom((z) => Math.max(1, z / 1.5))} disabled={zoom <= 1} aria-label={t("Zoom out")}>−</button>
          <button className="btn small" onClick={() => setZoom((z) => Math.min(32, z * 1.5))} aria-label={t("Zoom in")}>+</button>
        </div>
        {busy && <p className="small muted" role="status">{busy}…</p>}
        <div className="song-view-stage compose-stage" ref={box}>
          <div className="song-view-labels compose-labels" style={{ height }}>
            <span style={{ top: RULER + 4 }}>{t("Sections")}</span>
            <span style={{ top: RULER + SECTION_ROW + 2 }}>{t("Chords")}</span>
            {lanes.map((l) => (
              <span key={l.part.name} className="song-lane-label" style={{ top: l.y + 2 }}>
                <button className="link small" title={t("Taller, to edit its notes")} onClick={() => setOpen(open === l.part.name ? null : l.part.name)}>
                  {t(PART_LABEL[l.part.name] ?? l.part.name)}
                </button>
                <select className="input compose-instrument" aria-label={t("Instrument")} value={l.part.instrument} onChange={(e) => setInstrument(l.part.name, e.target.value)}>
                  {(l.drums ? KITS : INSTRUMENTS.map(([n]) => n)).filter((n, i, all) => all.indexOf(n) === i).map((n) => (
                    <option key={n} value={n}>{t(n)}</option>
                  ))}
                  {!(l.drums ? KITS : INSTRUMENTS.map(([n]) => n)).includes(l.part.instrument) && <option value={l.part.instrument}>{l.part.instrument}</option>}
                </select>
                <label className="small muted" title={t("Hear this part")}>
                  <input
                    type="checkbox"
                    checked={!muted.has(l.part.name)}
                    onChange={(e) => {
                      const next = new Set(muted);
                      if (e.target.checked) next.delete(l.part.name);
                      else next.add(l.part.name);
                      setMuted(next);
                      player.current?.mute(plain(comp), l.part.name, !e.target.checked);
                    }}
                  />{" "}
                  {t("heard")}
                </label>
              </span>
            ))}
          </div>
          <div className="song-view-scroll">
            <svg width={width} height={height} className="song-view-svg compose-svg" role="img" aria-label={t("The song's bars, sections, chords and notes")} onMouseDown={(e) => e.button === 0 && seek(e)}>
              {/* Bars and beats */}
              {Array.from({ length: bars + 1 }, (_, b) => (
                <g key={`bar${b}`}>
                  <line className="cmp-bar" x1={x(b * barBeats)} x2={x(b * barBeats)} y1={RULER - 4} y2={height} />
                  {b < bars && b % barEvery === 0 && <text className="cmp-bar-num" x={x(b * barBeats) + 3} y={RULER - 7}>{b + 1}</text>}
                  {b < bars && barBeats * ppb > 40 &&
                    Array.from({ length: barBeats - 1 }, (_, k) => <line key={k} className="cmp-beat" x1={x(b * barBeats + k + 1)} x2={x(b * barBeats + k + 1)} y1={TOP} y2={height} />)}
                </g>
              ))}
              {plan.sections.map((s, i) => (
                <g
                  key={`${i}-${s.name}`}
                  className={`song-section ${i % 2 ? "odd" : ""}`}
                  onMouseDown={(e) => e.stopPropagation()}
                  onClick={(e) => {
                    e.stopPropagation();
                    setMenu({ i, x: e.clientX, y: e.clientY });
                  }}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    setMenu({ i, x: e.clientX, y: e.clientY });
                  }}
                >
                  <rect x={x(starts[i])} y={RULER} width={Math.max(2, x(s.bars * barBeats) - 2)} height={SECTION_ROW - 4} rx={5} />
                  <rect className="song-section-shade" x={x(starts[i])} y={TOP} width={Math.max(2, x(s.bars * barBeats) - 2)} height={height - TOP} />
                  <text x={x(starts[i]) + 6} y={RULER + SECTION_ROW / 2 + 2}>{t(s.name)}</text>
                  <title>{`${t(s.name)} · ${t("{n} bars", { n: s.bars })}${s.lines.length ? `\n${s.lines.join("\n")}` : ""}`}</title>
                </g>
              ))}
              {chords.map((c, i) => (
                <text key={i} className="cmp-chord" x={x(c.beat) + 3} y={RULER + SECTION_ROW + CHORD_ROW - 6}>
                  {barBeats * ppb > 26 || i % 2 === 0 ? c.name : ""}
                </text>
              ))}
              {lanes.map((l) => (
                <g key={l.part.name} className={`song-lane cmp-lane-${l.part.name} ${muted.has(l.part.name) ? "cmp-muted" : ""}`}>
                  <rect className="song-lane-bg" x={0} y={l.y} width={width} height={l.h} onDoubleClick={(e) => addAt(e, l)} />
                  {open === l.part.name &&
                    (l.drums
                      ? DRUM_ROWS.map(([p, name], r) => (
                          <text key={p} className="cmp-row-name" x={4} y={l.y + 2 + (r + 0.75) * rowH(l)}>{t(name)}</text>
                        ))
                      : Array.from({ length: l.hi - l.lo + 1 }, (_, k) => l.lo + k)
                          .filter((p) => p % 12 === 0)
                          .map((p) => (
                            <g key={p}>
                              <line className="cmp-octave" x1={0} x2={width} y1={noteY(l, p) + rowH(l)} y2={noteY(l, p) + rowH(l)} />
                              <text className="cmp-row-name" x={4} y={noteY(l, p) + rowH(l) - 2}>{noteName(p)}</text>
                            </g>
                          )))}
                  {l.part.notes.map((n) => (
                    <rect
                      key={n.id}
                      className={`song-note ${selected.has(n.id) ? "selected" : ""}`}
                      x={x(n.beat)}
                      y={noteY(l, n.pitch)}
                      width={l.drums ? Math.max(3, Math.min(8, x(0.25))) : Math.max(2, x(n.len) - 1)}
                      height={Math.max(2, rowH(l) - 1)}
                      rx={1}
                      opacity={0.45 + n.vel / 230}
                      onMouseDown={(e) => onNoteDown(e, l, n)}
                    >
                      <title>{l.drums ? t(DRUM_ROWS.find(([p]) => p === n.pitch)?.[1] ?? "Drum") : `${noteName(n.pitch)} · ${t("bar {bar}", { bar: Math.floor(n.beat / barBeats) + 1 })}`}</title>
                    </rect>
                  ))}
                </g>
              ))}
              {loop && <rect className={`cmp-loop ${looping ? "on" : ""}`} x={x(loop.a)} y={1} width={Math.max(2, x(loop.b - loop.a))} height={RULER - 5} rx={3} />}
              {loop && looping && <rect className="cmp-loop-shade" x={x(loop.a)} y={TOP} width={Math.max(2, x(loop.b - loop.a))} height={height - TOP} />}
              <line className="song-playhead" x1={x(posBeat)} x2={x(posBeat)} y1={0} y2={height} />
            </svg>
          </div>
        </div>
        <div className="row wrap small">
          {selected.size > 0 ? (
            <>
              <span className="muted">{t("{n} notes selected", { n: selected.size })}</span>
              <label className="row">
                {t("Loudness")}
                <input
                  type="range"
                  min={1}
                  max={127}
                  value={loudness}
                  onChange={(e) => {
                    const v = +e.target.value;
                    setComp({ ...comp, parts: comp.parts.map((p) => ({ ...p, notes: p.notes.map((n) => (selected.has(n.id) ? { ...n, vel: v } : n)) })) });
                  }}
                  // One undo step for the whole slide.
                  onPointerDown={() => setHist((h) => ({ past: [...h.past.slice(-(HISTORY - 1)), h.now], now: h.now, future: [] }))}
                />
              </label>
              <button className="link small" onClick={copySelected}>{t("Copy")}</button>
              <button className="link small" onClick={removeSelected}>{t("Delete")}</button>
            </>
          ) : (
            <span className="muted">
              {t("Click a section for its menu. Click notes to select them (Shift for more), drag to move them, drag a note's end to lengthen it, double-click to add one. Arrows move them, Ctrl+C and Ctrl+V copy them to the playhead, Delete removes them. Drag across the bar numbers to set a loop.")}
            </span>
          )}
        </div>
        {menu && plan.sections[menu.i] && (
          <div className="context-menu" style={{ left: menu.x, top: menu.y }} role="menu" onClick={(e) => e.stopPropagation()}>
            <div className="small muted pad-s">{t(plan.sections[menu.i].name)} · {t("{n} bars", { n: plan.sections[menu.i].bars })}</div>
            <button role="menuitem" onClick={() => { setPanel({ kind: "ai", i: menu.i, s: plan.sections[menu.i] }); setAiText(""); setMenu(null); }}>{t("Change it with the chat model…")}</button>
            <button role="menuitem" onClick={() => { section(menu.i, { do: "again" }); setMenu(null); }}>{t("Write its tune again")}</button>
            <button role="menuitem" onClick={() => { setPanel({ kind: "edit", i: menu.i, s: plan.sections[menu.i] }); setMenu(null); }}>{t("Edit the section…")}</button>
            <hr />
            <button role="menuitem" onClick={() => { setPanel({ kind: "add", i: menu.i, s: { ...plan.sections[menu.i], name: "Verse", lines: [], take: 0 } }); setMenu(null); }}>{t("Add a section after it…")}</button>
            <button role="menuitem" onClick={() => { section(menu.i, { do: "duplicate" }); setMenu(null); }}>{t("Copy it after itself")}</button>
            <button role="menuitem" className="danger" disabled={plan.sections.length < 2} onClick={() => { const i = menu.i; setMenu(null); section(i, { do: "delete" }); }}>{t("Delete this section")}</button>
          </div>
        )}
        {panel && (
          <SectionPanel
            panel={panel}
            aiText={aiText}
            setAiText={setAiText}
            busy={!!busy}
            onCancel={() => setPanel(null)}
            onDone={(s) => {
              const p = panel;
              setPanel(null);
              if (p.kind === "ai") section(p.i, { do: "again" }, aiText);
              else if (p.kind === "edit") section(p.i, { do: "set", section: s });
              else section(p.i, { do: "add", section: s });
            }}
          />
        )}
      </div>
    </div>
  );
}

/** A section changed in words, edited, or a new one added. */
function SectionPanel({ panel, aiText, setAiText, busy, onCancel, onDone }: {
  panel: { kind: "ai" | "edit" | "add"; i: number; s: PlanSection };
  aiText: string;
  setAiText: (s: string) => void;
  busy: boolean;
  onCancel: () => void;
  onDone: (s: PlanSection) => void;
}) {
  const [s, setS] = useState<PlanSection>(panel.s);
  const [chords, setChords] = useState(panel.s.chords.join(" "));
  const [lines, setLines] = useState(panel.s.lines.join("\n"));
  if (panel.kind === "ai") {
    return (
      <div className="song-view-ask form">
        <strong className="small">{t("Change the {name}", { name: t(panel.s.name) })}</strong>
        <textarea className="input" rows={2} autoFocus value={aiText} onChange={(e) => setAiText(e.target.value)} placeholder={t("E.g. a busier bass and bigger drums, or end on a held chord")} />
        <p className="small muted">{t("Only this section's notes are written again; the rest stay as they are.")}</p>
        <div className="row">
          <button className="btn small" onClick={onCancel}>{t("Cancel")}</button>
          <button className="btn primary small" disabled={!aiText.trim() || busy} onClick={() => onDone(s)}>{t("Change it")}</button>
        </div>
      </div>
    );
  }
  const done = () =>
    onDone({
      ...s,
      chords: chords.split(/[\s,|]+/).map((c) => c.trim()).filter(Boolean),
      lines: lines.split("\n").map((l) => l.trim()).filter(Boolean),
    });
  const pick = <K extends "drums" | "bass" | "keys">(k: K, list: readonly string[], label: string) => (
    <label className="small row">
      {t(label)}
      <select className="input" value={s[k]} onChange={(e) => setS({ ...s, [k]: e.target.value })}>
        {list.map((v) => (
          <option key={v} value={v}>{t(STYLE_LABEL[v] ?? v)}</option>
        ))}
      </select>
    </label>
  );
  return (
    <div className="song-view-ask form compose-section-form">
      <strong className="small">{panel.kind === "edit" ? t("Edit the {name}", { name: t(panel.s.name) }) : t("A new section after the {name}", { name: t(panel.s.name) })}</strong>
      <div className="row wrap">
        <label className="small row">
          {t("Name")}
          <input className="input" value={s.name} onChange={(e) => setS({ ...s, name: e.target.value })} />
        </label>
        <label className="small row">
          {t("Bars")}
          <input type="number" className="input narrow" min={1} max={64} value={s.bars} onChange={(e) => setS({ ...s, bars: Math.max(1, Math.min(64, +e.target.value || 1)) })} />
        </label>
        <label className="small row">
          {t("Energy")}
          <input type="range" min={1} max={5} value={s.energy} onChange={(e) => setS({ ...s, energy: +e.target.value })} />
        </label>
        <label className="small row">
          <input type="checkbox" checked={s.melody} onChange={(e) => setS({ ...s, melody: e.target.checked })} /> {t("Melody")}
        </label>
      </div>
      <label className="small">
        {t("Chords, a bar each (they repeat)")}
        <input className="input" value={chords} onChange={(e) => setChords(e.target.value)} placeholder="Am F C G" />
      </label>
      <div className="row wrap">
        {pick("drums", DRUMS, tx("Drums"))}
        {pick("bass", BASS, tx("Bass"))}
        {pick("keys", KEYS, tx("Chords"))}
      </div>
      <label className="small">
        {t("Words sung here, a line each")}
        <textarea className="input" rows={3} value={lines} onChange={(e) => setLines(e.target.value)} />
      </label>
      <div className="row">
        <button className="btn small" onClick={onCancel}>{t("Cancel")}</button>
        <button className="btn primary small" disabled={busy} onClick={done}>{panel.kind === "edit" ? t("Write it") : t("Add it")}</button>
      </div>
    </div>
  );
}
