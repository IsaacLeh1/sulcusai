// SPDX-License-Identifier: AGPL-3.0-only
// How a song is built, as an editor: its sections on top (where their words
// are sung) and its parts below (the voice, other instruments, bass and
// drums) with their notes. The notes are read from the song, so editing them
// doesn't change its sound; they can be heard on a simple built-in
// instrument, beside the song or alone, saved with the song and saved as a
// MIDI file. Every edit can be undone and redone. Right-click a section to
// have the music model change it, make it again, delete it or add one after
// it: a new version, whose notes are read again so everything lines up.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { api, errorText, toBase64, type MediaItem, type MediaRequest, type SongMap, type SongNote } from "../api";
import { clock } from "../format";
import { t } from "../i18n";
import { useMediaUrl } from "../media";
import type { PushToast } from "./Toasts";
import { confirmed } from "../confirm";

type Section = SongMap["sections"][number];
type ENote = SongNote & { id: number };
type EPart = { name: string; notes: ENote[] };
type EMap = { length: number; sections: Section[]; parts: EPart[] };
type Mode = "song" | "notes" | "both";

const PART_LABEL: Record<string, string> = { vocals: "Voice", other: "Instruments", bass: "Bass", drums: "Drums" };
const LANE_HEIGHT: Record<string, number> = { vocals: 84, other: 84, bass: 56, drums: 48 };
const OPEN_HEIGHT = 220;
const SECTION_ROW = 30;
/** The column of lane names, left of the timeline (as in styles.css). */
const LABELS = 112;
/** Drum rows, top to bottom: hi-hat, snare, kick (General MIDI). */
const DRUMS = [42, 38, 36];
const HISTORY = 200;

let nextId = 1;
function editable(m: SongMap): EMap {
  return { length: m.length, sections: m.sections, parts: m.parts.map((p) => ({ name: p.name, notes: p.notes.map((n) => ({ ...n, id: nextId++ })) })) };
}
function plain(m: EMap): SongMap {
  return { length: m.length, sections: m.sections, parts: m.parts.map((p) => ({ name: p.name, notes: p.notes.map(({ id: _, ...n }) => n) })) };
}

/** The song's words rebuilt from its sections (a section without words is its tag alone). */
function lyricsOf(sections: Section[]): string {
  return sections.map((s) => (s.lines.length ? `[${s.name}]\n${s.lines.join("\n")}` : `[${s.name}]`)).join("\n\n");
}

/** The map after [a, b) is cut out (b = a: nothing) and `insert` seconds of silence put at a. */
function shifted(m: EMap, a: number, b: number, insert: number): EMap {
  const d = insert - (b - a);
  const at = (x: number) => (x >= b ? x + d : x);
  return {
    length: m.length + d,
    sections: m.sections.filter((s) => !(s.start >= a && s.end <= b && b > a)).map((s) => ({ ...s, start: at(s.start), end: at(s.end) })),
    parts: m.parts.map((p) => ({ name: p.name, notes: p.notes.filter((n) => !(n.start >= a && n.start < b)).map((n) => ({ ...n, start: at(n.start), end: at(n.end) })) })),
  };
}

/** Stereo 16-bit WAV of the song with [cut0, cut1) taken out and `insert` seconds of silence put at cut0, joined with short fades. */
function spliced(buf: AudioBuffer, cut0: number, cut1: number, insert: number): Uint8Array {
  const rate = buf.sampleRate;
  const a = Math.max(0, Math.round(cut0 * rate));
  const b = Math.min(buf.length, Math.round(cut1 * rate));
  const gap = Math.round(insert * rate);
  const frames = a + gap + (buf.length - b);
  const ch = Math.min(2, buf.numberOfChannels);
  const fade = Math.round(0.02 * rate);
  const data = new DataView(new ArrayBuffer(44 + frames * 4));
  const str = (o: number, s: string) => [...s].forEach((c, i) => data.setUint8(o + i, c.charCodeAt(0)));
  str(0, "RIFF");
  data.setUint32(4, 36 + frames * 4, true);
  str(8, "WAVEfmt ");
  data.setUint32(16, 16, true);
  data.setUint16(20, 1, true);
  data.setUint16(22, 2, true);
  data.setUint32(24, rate, true);
  data.setUint32(28, rate * 4, true);
  data.setUint16(32, 4, true);
  data.setUint16(34, 16, true);
  str(36, "data");
  data.setUint32(40, frames * 4, true);
  const chans = [buf.getChannelData(0), buf.getChannelData(ch > 1 ? 1 : 0)];
  let o = 44;
  for (let i = 0; i < frames; i++) {
    for (let c = 0; c < 2; c++) {
      let v = 0;
      if (i < a) v = chans[c][i] * Math.min(1, (a - i) / fade);
      else if (i >= a + gap) v = chans[c][b + (i - a - gap)] * Math.min(1, (i - a - gap) / fade);
      data.setInt16(o, Math.max(-32768, Math.min(32767, Math.round(v * 32767))), true);
      o += 2;
    }
  }
  return new Uint8Array(data.buffer);
}

/** Plays notes on simple built-in instruments, to hear an edit: not the song's sound. */
class NotePlayer {
  private ctx = new AudioContext();
  private out: GainNode | null = null;
  private noise: AudioBuffer;
  constructor() {
    const n = this.ctx.sampleRate;
    this.noise = this.ctx.createBuffer(1, n, n);
    const d = this.noise.getChannelData(0);
    for (let i = 0; i < n; i++) d[i] = Math.random() * 2 - 1;
  }
  start(m: EMap, from: number, heard: Set<string>) {
    this.stop();
    const ctx = this.ctx;
    ctx.resume();
    const out = ctx.createGain();
    out.gain.value = 0.8;
    out.connect(ctx.destination);
    this.out = out;
    const t0 = ctx.currentTime + 0.05;
    for (const p of m.parts) {
      if (!heard.has(p.name)) continue;
      for (const n of p.notes) {
        if (n.end <= from) continue;
        const at = t0 + Math.max(0, n.start - from);
        const len = Math.max(0.05, n.end - Math.max(n.start, from));
        const vol = 0.05 + (n.velocity / 127) * 0.2;
        if (p.name === "drums") this.drum(n.pitch, at, vol, out);
        else this.tone(p.name, n.pitch, at, len, vol, out);
      }
    }
  }
  private tone(part: string, pitch: number, at: number, len: number, vol: number, out: AudioNode) {
    const ctx = this.ctx;
    const o = ctx.createOscillator();
    o.type = part === "bass" ? "sawtooth" : part === "vocals" ? "triangle" : "sine";
    o.frequency.value = 440 * Math.pow(2, (pitch - 69) / 12);
    const g = ctx.createGain();
    g.gain.setValueAtTime(0, at);
    g.gain.linearRampToValueAtTime(vol * (part === "other" ? 0.7 : 1), at + 0.01);
    g.gain.setValueAtTime(vol * (part === "other" ? 0.7 : 1), at + len);
    g.gain.linearRampToValueAtTime(0, at + len + 0.08);
    let node: AudioNode = o;
    if (part === "bass") {
      const f = ctx.createBiquadFilter();
      f.type = "lowpass";
      f.frequency.value = 700;
      o.connect(f);
      node = f;
    }
    node.connect(g).connect(out);
    o.start(at);
    o.stop(at + len + 0.1);
  }
  private drum(pitch: number, at: number, vol: number, out: AudioNode) {
    const ctx = this.ctx;
    const g = ctx.createGain();
    g.connect(out);
    if (pitch === 36) {
      const o = ctx.createOscillator();
      o.frequency.setValueAtTime(130, at);
      o.frequency.exponentialRampToValueAtTime(45, at + 0.12);
      g.gain.setValueAtTime(vol * 2.5, at);
      g.gain.exponentialRampToValueAtTime(0.001, at + 0.2);
      o.connect(g);
      o.start(at);
      o.stop(at + 0.22);
      return;
    }
    const s = ctx.createBufferSource();
    s.buffer = this.noise;
    const f = ctx.createBiquadFilter();
    f.type = pitch === 42 ? "highpass" : "bandpass";
    f.frequency.value = pitch === 42 ? 7000 : 1800;
    const len = pitch === 42 ? 0.05 : 0.14;
    g.gain.setValueAtTime(vol * (pitch === 42 ? 0.8 : 1.6), at);
    g.gain.exponentialRampToValueAtTime(0.001, at + len);
    s.connect(f).connect(g);
    s.start(at);
    s.stop(at + len + 0.02);
  }
  stop() {
    if (this.out) {
      this.out.disconnect();
      this.out = null;
    }
  }
  close() {
    this.stop();
    this.ctx.close();
  }
}

export function SongView({ item, onClose, onStart, onSaved, onMapSaved, toast }: {
  item: MediaItem & { map: SongMap };
  onClose: () => void;
  onStart: (req: MediaRequest) => void;
  /** A new version made here (a section cut out). */
  onSaved: (i: MediaItem) => void;
  /** The song, its edited notes kept. */
  onMapSaved: (i: MediaItem) => void;
  toast: PushToast;
}) {
  const src = useMediaUrl(item);
  const audio = useRef<HTMLAudioElement>(null);
  const box = useRef<HTMLDivElement>(null);
  const player = useRef<NotePlayer | null>(null);
  const [map, setMap] = useState<EMap>(() => editable(item.map));
  const [saved, setSaved] = useState<EMap>(map);
  const [past, setPast] = useState<EMap[]>([]);
  const [future, setFuture] = useState<EMap[]>([]);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [fitWidth, setFitWidth] = useState(900);
  const [zoom, setZoom] = useState(1);
  const [open, setOpen] = useState<string | null>(null);
  const [heard, setHeard] = useState<Set<string>>(() => new Set(item.map.parts.map((p) => p.name)));
  const [mode, setMode] = useState<Mode>("song");
  const [playing, setPlaying] = useState(false);
  const [pos, setPos] = useState(0);
  const started = useRef<{ at: number; from: number } | null>(null);
  // A drag in progress: the notes it moves, the map before it and its latest result.
  const drag = useRef<{ kind: "move" | "resize"; x: number; y: number; ids: Set<number>; base: EMap; last: EMap; moved: boolean; perPitch: number } | null>(null);
  const [menu, setMenu] = useState<{ s: Section; x: number; y: number } | null>(null);
  const [asking, setAsking] = useState<{ s: Section; kind: "edit" | "add" } | null>(null);
  const [text, setText] = useState("");
  const [words, setWords] = useState("");
  const [addSecs, setAddSecs] = useState(10);
  const [busy, setBusy] = useState(false);
  const dirty = map !== saved;

  useEffect(() => {
    const el = box.current;
    if (!el) return;
    // The stage less its column of lane names: now, and whenever it changes.
    const fit = () => setFitWidth(Math.max(400, el.clientWidth - LABELS));
    fit();
    const ro = new ResizeObserver(fit);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  useEffect(() => () => player.current?.close(), []);

  const length = Math.max(1, map.length);
  const pps = (fitWidth / length) * zoom;
  const width = Math.max(fitWidth, length * pps);
  const x = (s: number) => s * pps;

  // Each lane's pitch range: what it had when opened, widened by edits.
  const firstRange = useMemo(
    () =>
      Object.fromEntries(
        item.map.parts.map((p) => {
          const ps = p.notes.map((n) => n.pitch);
          return [p.name, ps.length ? [Math.min(...ps) - 2, Math.max(...ps) + 2] : [48, 72]];
        }),
      ) as Record<string, [number, number]>,
    [item.map],
  );
  const lanes = useMemo(() => {
    let y = SECTION_ROW + 6;
    return map.parts.map((p, i) => {
      const h = open === p.name ? OPEN_HEIGHT : (LANE_HEIGHT[p.name] ?? 70);
      const [lo0, hi0] = firstRange[p.name] ?? [48, 72];
      const ps = p.notes.map((n) => n.pitch);
      const lo = Math.min(lo0, ...ps);
      const hi = Math.max(hi0, ...ps, lo + 12);
      const lane = { part: p, index: i, y, h, lo, hi, drums: p.name === "drums" };
      y += h + 6;
      return lane;
    });
  }, [map, open, firstRange]);
  type Lane = (typeof lanes)[number];
  const height = lanes.length ? lanes[lanes.length - 1].y + lanes[lanes.length - 1].h + 4 : SECTION_ROW + 10;
  const noteY = (l: Lane, pitch: number) => (l.drums ? l.y + 2 + Math.max(0, DRUMS.indexOf(pitch)) * ((l.h - 4) / 3) : l.y + l.h - 3 - ((pitch - l.lo) / Math.max(1, l.hi - l.lo)) * (l.h - 6));
  const noteH = (l: Lane) => (l.drums ? (l.h - 4) / 3 - 2 : Math.max(3, (l.h - 6) / Math.max(12, l.hi - l.lo)));

  // ---------- history ----------
  const commit = useCallback(
    (before: EMap, after: EMap) => {
      if (before === after) return;
      setPast((p) => [...p.slice(-(HISTORY - 1)), before]);
      setFuture([]);
      setMap(after);
    },
    [],
  );
  const undo = () => {
    if (!past.length) return;
    setFuture((f) => [map, ...f]);
    setMap(past[past.length - 1]);
    setPast((p) => p.slice(0, -1));
  };
  const redo = () => {
    if (!future.length) return;
    setPast((p) => [...p, map]);
    setMap(future[0]);
    setFuture((f) => f.slice(1));
  };
  const removeSelected = () => {
    if (!selected.size) return;
    commit(map, { ...map, parts: map.parts.map((p) => ({ ...p, notes: p.notes.filter((n) => !selected.has(n.id)) })) });
    setSelected(new Set());
  };

  // ---------- playing ----------
  const stop = useCallback(() => {
    audio.current?.pause();
    player.current?.stop();
    started.current = null;
    setPlaying(false);
  }, []);
  const play = (from = pos) => {
    stop();
    if (mode !== "notes" && audio.current) {
      audio.current.currentTime = from;
      audio.current.play().catch(() => {});
    }
    if (mode !== "song") {
      player.current ??= new NotePlayer();
      player.current.start(map, from, heard);
    }
    started.current = { at: performance.now(), from };
    setPlaying(true);
  };
  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    const tick = () => {
      const s = started.current;
      const now = mode !== "notes" && audio.current ? audio.current.currentTime : s ? s.from + (performance.now() - s.at) / 1000 : pos;
      setPos(now);
      if (now >= length) stop();
      else raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
    // (`pos` is only where a stopped play began: not a reason to restart.)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [playing, mode, length, stop]);

  // ---------- keys ----------
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      const typing = e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement;
      if (typing) return;
      const mod = e.ctrlKey || e.metaKey;
      if (mod && e.key.toLowerCase() === "z" && !e.shiftKey) (e.preventDefault(), undo());
      else if (mod && (e.key.toLowerCase() === "y" || (e.key.toLowerCase() === "z" && e.shiftKey))) (e.preventDefault(), redo());
      else if (e.key === "Delete" || e.key === "Backspace") (e.preventDefault(), removeSelected());
      else if (e.key === " ") (e.preventDefault(), playing ? stop() : play());
      else if (e.key === "Escape" && selected.size) setSelected(new Set());
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
    const right = x(n.end);
    const kind = !l.drums && right - px < 6 && right - x(n.start) > 10 ? "resize" : "move";
    const sel = e.shiftKey ? new Set(selected).add(n.id) : selected.has(n.id) ? selected : new Set([n.id]);
    setSelected(sel);
    drag.current = { kind, x: e.clientX, y: e.clientY, ids: sel, base: map, last: map, moved: false, perPitch: l.drums ? (l.h - 4) / 3 : (l.h - 6) / Math.max(1, l.hi - l.lo) };
  };
  useEffect(() => {
    const move = (e: MouseEvent) => {
      const d = drag.current;
      if (!d) return;
      const dt = (e.clientX - d.x) / pps;
      const steps = -Math.round((e.clientY - d.y) / d.perPitch);
      if (!d.moved && Math.abs(e.clientX - d.x) < 3 && Math.abs(e.clientY - d.y) < 3) return;
      d.moved = true;
      d.last = {
        ...d.base,
        parts: d.base.parts.map((p) => ({
          ...p,
          notes: p.notes.map((n) => {
            if (!d.ids.has(n.id)) return n;
            if (d.kind === "resize") return { ...n, end: Math.max(n.start + 0.05, n.end + dt) };
            const start = Math.max(0, n.start + dt);
            const pitch = p.name === "drums" ? DRUMS[Math.max(0, Math.min(2, DRUMS.indexOf(n.pitch) - steps))] : Math.max(0, Math.min(127, n.pitch + steps));
            return { ...n, start, end: start + (n.end - n.start), pitch };
          }),
        })),
      };
      setMap(d.last);
    };
    const up = () => {
      const d = drag.current;
      drag.current = null;
      if (d?.moved) commit(d.base, d.last);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
  }, [pps, commit]);
  const addAt = (e: React.MouseEvent, l: Lane) => {
    const { px, py } = svgPoint(e);
    const start = Math.max(0, Math.round((px / pps) * 20) / 20);
    const pitch = l.drums
      ? DRUMS[Math.max(0, Math.min(2, Math.floor((py - l.y - 2) / ((l.h - 4) / 3))))]
      : Math.round(l.lo + ((l.y + l.h - 3 - py) / (l.h - 6)) * (l.hi - l.lo));
    const n: ENote = { id: nextId++, start, end: start + (l.drums ? 0.1 : 0.25), pitch, velocity: 90 };
    commit(map, { ...map, parts: map.parts.map((p, i) => (i === l.index ? { ...p, notes: [...p.notes, n].sort((a, b) => a.start - b.start) } : p)) });
    setSelected(new Set([n.id]));
  };
  const seek = (e: React.MouseEvent) => {
    const { px } = svgPoint(e);
    const to = Math.max(0, Math.min(length, px / pps));
    setPos(to);
    setSelected(new Set());
    if (playing) play(to);
  };

  // ---------- saving ----------
  const saveNotes = async () => {
    try {
      const updated = await api.mediaSongMapSave(item.id, plain(map));
      setSaved(map);
      onMapSaved(updated);
      toast(t("Your note changes are kept with the song."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const saveMidi = async () => {
    const base = (item.prompt || "song").replace(/[^\w\- ]+/g, "").trim().slice(0, 40).replace(/\s+/g, "-") || "song";
    const path = await save({ defaultPath: `${base}.mid`, filters: [{ name: "MIDI", extensions: ["mid"] }] });
    if (!path) return;
    try {
      await api.mediaSongMidi(item.id, path, plain(map));
      toast(t("The song's notes are saved as a MIDI file."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const close = async () => {
    if (dirty && !await confirmed(t("Close without keeping your note changes?"))) return;
    stop();
    onClose();
  };

  // ---------- sections, with the music model ----------
  const remake = (s: Section, change: string) => {
    stop();
    onStart({
      op: "music",
      model_id: item.model_id,
      prompt: change.trim() ? `${item.prompt.trim().replace(/\.$/, "")}, ${change.trim()}` : item.prompt,
      lyrics: item.lyrics,
      music: { task: "repaint", audio: item.id, region: [s.start, Math.max(s.end, s.start + 1)] },
      then_map: true,
    });
  };
  const decoded = async () => {
    if (!src) throw new Error(t("The song isn't loaded yet."));
    const bytes = await (await fetch(src)).arrayBuffer();
    const ctx = new AudioContext();
    try {
      return await ctx.decodeAudioData(bytes);
    } finally {
      ctx.close();
    }
  };
  const remove = async (s: Section) => {
    if (!await confirmed(t("Delete the {name} ({from} to {to}) from this song? The song as it is now stays in the gallery.", { name: t(s.name), from: clock(s.start), to: clock(s.end) }))) return;
    setBusy(true);
    stop();
    try {
      const buf = await decoded();
      const wav = spliced(buf, s.start, s.end, 0);
      // The notes and sections after it move up with the music.
      const next = shifted(map, s.start, s.end, 0);
      const made = await api.mediaAdd({ data: toBase64(wav), mime: "audio/wav", op: "trim", prompt: item.prompt, parent: item.id, seconds: buf.duration - (s.end - s.start), lyrics: lyricsOf(next.sections) });
      const mapped = await api.mediaSongMapSave(made.id, plain({ ...next, length: buf.duration - (s.end - s.start) }));
      toast(t("The {name} is cut out: the shorter song is in the gallery.", { name: t(s.name) }), "success");
      onSaved(mapped);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const add = async (s: Section, what: string, sung: string, secs: number) => {
    setBusy(true);
    stop();
    try {
      const buf = await decoded();
      const wav = spliced(buf, s.end, s.end, secs);
      const lines = sung.split("\n").map((l) => l.trim()).filter(Boolean);
      const at = map.sections.indexOf(s) + 1;
      // The tags go to the music model in English, as the lyrics' others do.
      const added: Section = { name: lines.length ? "Verse" : "Instrumental", start: s.end, end: s.end + secs, lines };
      const all = [...map.sections.slice(0, at), added, ...map.sections.slice(at)];
      const gap = await api.mediaAdd({ data: toBase64(wav), mime: "audio/wav", op: "trim", prompt: item.prompt, parent: item.id, hidden: true, seconds: buf.duration + secs, lyrics: lyricsOf(all) });
      onStart({
        op: "music",
        model_id: item.model_id,
        prompt: what.trim() ? `${item.prompt.trim().replace(/\.$/, "")}, ${what.trim()}` : item.prompt,
        lyrics: lyricsOf(all),
        music: { task: "repaint", audio: gap.id, region: [s.end, s.end + secs] },
        then_map: true,
      });
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  useEffect(() => {
    if (!menu) return;
    const shut = () => setMenu(null);
    window.addEventListener("click", shut);
    return () => window.removeEventListener("click", shut);
  }, [menu]);

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="modal song-view" role="dialog" aria-modal="true" aria-label={t("How the song is built")}>
        <div className="row wrap">
          <strong>{t("How the song is built")}</strong>
          <span className="spacer" />
          <button className="btn small" onClick={undo} disabled={!past.length} title={t("Undo (Ctrl+Z)")}>{t("Undo")}</button>
          <button className="btn small" onClick={redo} disabled={!future.length} title={t("Redo (Ctrl+Y)")}>{t("Redo")}</button>
          <button className="btn small primary" onClick={saveNotes} disabled={!dirty}>{t("Keep note changes")}</button>
          <button className="btn small" onClick={saveMidi}>{t("Save as MIDI")}</button>
          <button className="icon-btn" aria-label={t("Close")} onClick={close}>✕</button>
        </div>
        <div className="row wrap song-transport">
          <button className="btn small" onClick={() => (playing ? stop() : play())}>{playing ? t("Stop") : t("Play")}</button>
          <span className="small mono">{clock(pos)} / {clock(length)}</span>
          <span className="filters tight" role="radiogroup" aria-label={t("What plays")}>
            {(["song", "notes", "both"] as Mode[]).map((m) => (
              <button
                key={m}
                role="radio"
                aria-checked={mode === m}
                className={`chip ${mode === m ? "active" : ""}`}
                onClick={() => {
                  setMode(m);
                  if (playing) stop();
                }}
              >
                {m === "song" ? t("The song") : m === "notes" ? t("The notes") : t("Both")}
              </button>
            ))}
          </span>
          <span className="spacer" />
          <span className="small muted">{t("Zoom")}</span>
          <button className="btn small" onClick={() => setZoom((z) => Math.max(1, z / 1.5))} disabled={zoom <= 1} aria-label={t("Zoom out")}>−</button>
          <button className="btn small" onClick={() => setZoom((z) => Math.min(24, z * 1.5))} aria-label={t("Zoom in")}>+</button>
        </div>
        {src && <audio ref={audio} src={src} preload="auto" />}
        <div className="song-view-stage" ref={box}>
          <div className="song-view-labels" style={{ height }}>
            <span style={{ top: 6 }}>{t("Sections")}</span>
            {lanes.map((l) => (
              <span key={l.part.name} className="song-lane-label" style={{ top: l.y + 2 }}>
                <button className="link small" title={t("Taller, to edit its notes")} onClick={() => setOpen(open === l.part.name ? null : l.part.name)}>
                  {t(PART_LABEL[l.part.name] ?? l.part.name)}
                </button>
                <label className="small muted" title={t("Hear this part's notes")}>
                  <input
                    type="checkbox"
                    checked={heard.has(l.part.name)}
                    onChange={(e) => {
                      const next = new Set(heard);
                      if (e.target.checked) next.add(l.part.name);
                      else next.delete(l.part.name);
                      setHeard(next);
                    }}
                  />{" "}
                  {t("heard")}
                </label>
              </span>
            ))}
          </div>
          <div className="song-view-scroll">
            <svg width={width} height={height} className="song-view-svg" role="img" aria-label={t("The song's sections and parts")} onMouseDown={(e) => e.button === 0 && seek(e)}>
              {map.sections.map((s, i) => (
                <g
                  key={`${s.name}-${s.start}`}
                  className={`song-section ${i % 2 ? "odd" : ""}`}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    setMenu({ s, x: e.clientX, y: e.clientY });
                  }}
                >
                  <rect x={x(s.start)} y={2} width={Math.max(2, x(s.end) - x(s.start) - 2)} height={SECTION_ROW - 4} rx={5} />
                  <rect className="song-section-shade" x={x(s.start)} y={SECTION_ROW} width={Math.max(2, x(s.end) - x(s.start) - 2)} height={height - SECTION_ROW} />
                  <text x={x(s.start) + 6} y={SECTION_ROW / 2 + 4}>{t(s.name)}</text>
                  <title>{`${t(s.name)} · ${clock(s.start)}–${clock(s.end)}${s.lines.length ? `\n${s.lines.join("\n")}` : ""}`}</title>
                </g>
              ))}
              {lanes.map((l) => (
                <g key={l.part.name} className={`song-lane lane-${l.part.name}`}>
                  <rect className="song-lane-bg" x={0} y={l.y} width={width} height={l.h} onDoubleClick={(e) => addAt(e, l)} />
                  {l.part.notes.map((n) => (
                    <rect
                      key={n.id}
                      className={`song-note ${selected.has(n.id) ? "selected" : ""}`}
                      x={x(n.start)}
                      y={noteY(l, n.pitch)}
                      width={l.drums ? 3 : Math.max(2, x(n.end) - x(n.start))}
                      height={noteH(l)}
                      rx={1}
                      opacity={0.45 + n.velocity / 230}
                      onMouseDown={(e) => onNoteDown(e, l, n)}
                    >
                      <title>{l.drums ? t(n.pitch === 36 ? "Kick" : n.pitch === 38 ? "Snare" : "Hi-hat") : `${clock(n.start)} · ${noteName(n.pitch)}`}</title>
                    </rect>
                  ))}
                </g>
              ))}
              <line className="song-playhead" x1={x(pos)} x2={x(pos)} y1={0} y2={height} />
            </svg>
          </div>
        </div>
        <p className="small muted">
          {t("Right-click a section to have the music model change it. Click notes to select them (Shift for more), drag to move them, drag a note's end to lengthen it, double-click to add one, Delete to remove. The notes are read from the song: editing them doesn't change its sound, and they play on a simple built-in instrument.")}
        </p>
        {menu && (
          <div className="context-menu" style={{ left: menu.x, top: menu.y }} role="menu" onClick={(e) => e.stopPropagation()}>
            <div className="small muted pad-s">{t(menu.s.name)} · {clock(menu.s.start)}–{clock(menu.s.end)}</div>
            <button role="menuitem" onClick={() => { setAsking({ s: menu.s, kind: "edit" }); setText(""); setMenu(null); }}>{t("Edit with model…")}</button>
            <button role="menuitem" onClick={() => { remake(menu.s, ""); setMenu(null); }}>{t("Make it again")}</button>
            <button role="menuitem" onClick={() => { setAsking({ s: menu.s, kind: "add" }); setText(""); setWords(""); setMenu(null); }}>{t("Add a section after it…")}</button>
            <button role="menuitem" className="danger" disabled={busy} onClick={() => { const s = menu.s; setMenu(null); remove(s); }}>{t("Delete this section")}</button>
          </div>
        )}
        {asking && (
          <div className="song-view-ask form">
            {asking.kind === "edit" ? (
              <>
                <strong className="small">{t("Change the {name} ({from} to {to})", { name: t(asking.s.name), from: clock(asking.s.start), to: clock(asking.s.end) })}</strong>
                <textarea className="input" rows={2} autoFocus value={text} onChange={(e) => setText(e.target.value)} placeholder={t("E.g. bigger and louder with strings, or a guitar solo instead")} />
                <div className="row">
                  <button className="btn small" onClick={() => setAsking(null)}>{t("Cancel")}</button>
                  <button className="btn primary small" disabled={!text.trim()} onClick={() => { remake(asking.s, text); setAsking(null); }}>{t("Change it")}</button>
                </div>
              </>
            ) : (
              <>
                <strong className="small">{t("Add a section after the {name}", { name: t(asking.s.name) })}</strong>
                <textarea className="input" rows={2} autoFocus value={text} onChange={(e) => setText(e.target.value)} placeholder={t("What's in it, e.g. a piano break, or a big guitar solo")} />
                <textarea className="input" rows={2} value={words} onChange={(e) => setWords(e.target.value)} placeholder={t("Words to sing in it (optional, a line each)")} />
                <label className="small row">
                  {t("Seconds")} <input type="number" className="input narrow" min={4} max={60} value={addSecs} onChange={(e) => setAddSecs(Math.max(4, Math.min(60, +e.target.value || 10)))} />
                </label>
                <div className="row">
                  <button className="btn small" onClick={() => setAsking(null)}>{t("Cancel")}</button>
                  <button className="btn primary small" disabled={busy} onClick={() => { add(asking.s, text, words, addSecs); setAsking(null); }}>{t("Add it")}</button>
                </div>
              </>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

const NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
function noteName(p: number) {
  return `${NAMES[p % 12]}${Math.floor(p / 12) - 1}`;
}
