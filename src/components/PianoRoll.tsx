// SPDX-License-Identifier: AGPL-3.0-only
// The song studio's piano roll: a MIDI clip's notes to add (double-click),
// move and lengthen (drag, its right end), delete, transpose (arrows,
// Shift for an octave), quantize (Shift+Q) and set the loudness of.
import { useEffect, useRef, useState } from "react";
import { t } from "../i18n";
import type { Note } from "../studio/model";

const ROW = 11;
const NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const black = (p: number) => [1, 3, 6, 8, 10].includes(p % 12);

export function PianoRoll({ notes, length, grid, onChange, onTap }: { notes: Note[]; length: number; grid: number; onChange: (n: Note[]) => void; onTap: (pitch: number) => void }) {
  const [sel, setSel] = useState<Set<number>>(new Set());
  const [pxs, setPxs] = useState(80);
  const drag = useRef<{ kind: "move" | "end"; x: number; y: number; base: Note[]; ids: Set<number> } | null>(null);
  const box = useRef<HTMLDivElement>(null);
  const pitches = notes.map((n) => n.pitch);
  const hi = Math.min(127, Math.max(84, ...pitches) + 4);
  const lo = Math.max(0, Math.min(48, ...pitches) - 4);
  const rows = hi - lo + 1;
  const width = Math.max(400, length * pxs);
  const y = (p: number) => (hi - p) * ROW;
  const step = Math.max(0.01, grid);
  const q = (s: number) => Math.max(0, Math.round(s / step) * step);

  useEffect(() => {
    const move = (e: MouseEvent) => {
      const d = drag.current;
      if (!d) return;
      const dt = (e.clientX - d.x) / pxs;
      const dp = -Math.round((e.clientY - d.y) / ROW);
      onChange(
        d.base.map((n, i) => {
          if (!d.ids.has(i)) return n;
          if (d.kind === "end") return { ...n, end: Math.max(n.start + step / 2, q(n.end + dt)) };
          const start = Math.max(0, q(n.start + dt));
          return { ...n, start, end: start + (n.end - n.start), pitch: Math.max(0, Math.min(127, n.pitch + dp)) };
        }),
      );
    };
    const up = () => (drag.current = null);
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
  });

  const key = (e: React.KeyboardEvent) => {
    if (!sel.size) return;
    const on = (f: (n: Note) => Note) => onChange(notes.map((n, i) => (sel.has(i) ? f(n) : n)));
    if (e.key === "Delete" || e.key === "Backspace") {
      e.preventDefault();
      onChange(notes.filter((_, i) => !sel.has(i)));
      setSel(new Set());
    } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
      e.preventDefault();
      const by = (e.key === "ArrowUp" ? 1 : -1) * (e.shiftKey ? 12 : 1);
      on((n) => ({ ...n, pitch: Math.max(0, Math.min(127, n.pitch + by)) }));
    } else if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      e.preventDefault();
      const by = (e.key === "ArrowRight" ? 1 : -1) * step;
      on((n) => (e.shiftKey ? { ...n, end: Math.max(n.start + step / 2, n.end + by) } : { ...n, start: Math.max(0, n.start + by), end: Math.max(step / 2, n.end + by) }));
    } else if (e.key.toLowerCase() === "q" && e.shiftKey) {
      e.preventDefault();
      on((n) => {
        const start = q(n.start);
        return { ...n, start, end: Math.max(start + step / 2, q(n.end)) };
      });
    } else if (e.key.toLowerCase() === "a" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      setSel(new Set(notes.map((_, i) => i)));
    }
  };

  const velocity = sel.size ? Math.round(notes.filter((_, i) => sel.has(i)).reduce((s, n) => s + n.velocity, 0) / sel.size) : null;

  return (
    <div className="piano-roll" tabIndex={0} onKeyDown={key} ref={box}>
      <div className="row wrap small">
        <span className="muted">{t("Double-click to add a note; drag to move, its right end to lengthen; Delete, arrows to move, Shift+Q to line up on the grid.")}</span>
        <span className="spacer" />
        {velocity !== null && (
          <label className="row">
            {t("Loudness")}
            <input type="range" min={1} max={127} value={velocity} onChange={(e) => onChange(notes.map((n, i) => (sel.has(i) ? { ...n, velocity: +e.target.value } : n)))} />
          </label>
        )}
        <button className="btn small" onClick={() => setPxs((z) => Math.max(20, z / 1.4))} aria-label={t("Zoom out")}>−</button>
        <button className="btn small" onClick={() => setPxs((z) => Math.min(600, z * 1.4))} aria-label={t("Zoom in")}>+</button>
      </div>
      <div className="piano-roll-body">
        <div className="piano-keys" style={{ height: rows * ROW }}>
          {Array.from({ length: rows }, (_, i) => hi - i).map((p) => (
            <button key={p} className={`piano-key ${black(p) ? "black" : ""}`} style={{ top: y(p), height: ROW }} onMouseDown={() => onTap(p)} tabIndex={-1}>
              {p % 12 === 0 ? `${NAMES[0]}${Math.floor(p / 12) - 1}` : ""}
            </button>
          ))}
        </div>
        <div className="piano-scroll">
          <svg
            width={width}
            height={rows * ROW}
            className="piano-grid"
            onMouseDown={(e) => {
              if (e.target === e.currentTarget) setSel(new Set());
            }}
            onDoubleClick={(e) => {
              const r = e.currentTarget.getBoundingClientRect();
              // In the grid cell clicked.
              const start = Math.max(0, Math.floor((e.clientX - r.left) / pxs / step) * step);
              const pitch = hi - Math.floor((e.clientY - r.top) / ROW);
              const n: Note = { pitch, start, end: start + Math.max(step, 0.25), velocity: 96 };
              onChange([...notes, n]);
              setSel(new Set([notes.length]));
              onTap(pitch);
            }}
          >
            {Array.from({ length: rows }, (_, i) => hi - i).map((p) => (
              <rect key={p} x={0} y={y(p)} width={width} height={ROW} className={black(p) ? "pr-row black" : "pr-row"} />
            ))}
            {Array.from({ length: Math.ceil(length / step) + 1 }, (_, i) => (
              <line key={i} x1={i * step * pxs} x2={i * step * pxs} y1={0} y2={rows * ROW} className="pr-line" />
            ))}
            <line x1={length * pxs} x2={length * pxs} y1={0} y2={rows * ROW} className="pr-end" />
            {notes.map((n, i) => (
              <rect
                key={i}
                x={n.start * pxs}
                y={y(n.pitch) + 1}
                width={Math.max(3, (n.end - n.start) * pxs)}
                height={ROW - 2}
                rx={2}
                className={`pr-note ${sel.has(i) ? "selected" : ""}`}
                style={{ opacity: 0.45 + (0.55 * n.velocity) / 127 }}
                onMouseDown={(e) => {
                  e.stopPropagation();
                  const r = (e.currentTarget.ownerSVGElement as SVGSVGElement).getBoundingClientRect();
                  const atEnd = e.clientX - r.left > n.end * pxs - 5;
                  const ids = e.shiftKey ? new Set(sel).add(i) : sel.has(i) ? sel : new Set([i]);
                  setSel(ids);
                  drag.current = { kind: atEnd ? "end" : "move", x: e.clientX, y: e.clientY, base: notes, ids };
                  if (!atEnd) onTap(n.pitch);
                  box.current?.focus();
                }}
              />
            ))}
          </svg>
        </div>
      </div>
    </div>
  );
}
