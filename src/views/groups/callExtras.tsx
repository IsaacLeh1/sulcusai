// SPDX-License-Identifier: AGPL-3.0-only
// Small pieces of a call kept apart from GroupCall: notes between call
// windows, drawing on a shared screen, and sharing part of a screen.
import { useRef, useState } from "react";
import { t } from "../../i18n";

/** A note passed between call windows over the "signal" stream. */
export type Signal =
  | { t: "rec"; on: boolean }
  | { t: "changed"; on: boolean }
  | { t: "draw"; key: string; pts: number[]; color: string }
  | { t: "clear"; key: string }
  | { t: "quality"; to: string; kind: string; low: boolean };

export function encodeSignal(s: Signal): Uint8Array {
  return new TextEncoder().encode(JSON.stringify(s));
}

export function decodeSignal(data: Uint8Array): Signal | null {
  try {
    const s = JSON.parse(new TextDecoder().decode(data));
    return s && typeof s.t === "string" ? (s as Signal) : null;
  } catch {
    return null;
  }
}

/** One colour per person, so strokes say who drew them. */
export function colorFor(id: string): string {
  let h = 0;
  for (const c of id) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return `hsl(${h % 360} 85% 55%)`;
}

export type Stroke = { pts: number[]; color: string; at: number };

/**
 * Strokes over a shared screen, in 0..1 of the picture so every size lines up.
 * Drawing is on while `drawing`; finished strokes go to `onStroke`.
 */
export function DrawLayer({ strokes, drawing, color, onStroke }: { strokes: Stroke[]; drawing: boolean; color: string; onStroke: (pts: number[]) => void }) {
  const [live, setLive] = useState<number[] | null>(null);
  const box = useRef<SVGSVGElement>(null);
  const at = (e: React.PointerEvent) => {
    const r = box.current!.getBoundingClientRect();
    return [(e.clientX - r.left) / r.width, (e.clientY - r.top) / r.height];
  };
  const line = (pts: number[]) => {
    const out: string[] = [];
    for (let i = 0; i + 1 < pts.length; i += 2) out.push(`${(pts[i] * 1000).toFixed(1)},${(pts[i + 1] * 1000).toFixed(1)}`);
    return out.join(" ");
  };
  return (
    <svg
      ref={box}
      className={drawing ? "call-draw on" : "call-draw"}
      viewBox="0 0 1000 1000"
      preserveAspectRatio="none"
      onPointerDown={(e) => {
        if (!drawing) return;
        e.stopPropagation();
        (e.currentTarget as Element).setPointerCapture(e.pointerId);
        setLive(at(e));
      }}
      onPointerMove={(e) => live && setLive([...live, ...at(e)])}
      onPointerUp={() => {
        if (live && live.length >= 4) onStroke(live);
        setLive(null);
      }}
      onClick={(e) => drawing && e.stopPropagation()}
    >
      {strokes.map((s, i) => (
        <polyline key={i} points={line(s.pts)} stroke={s.color} />
      ))}
      {live && <polyline points={line(live)} stroke={color} />}
    </svg>
  );
}

export type Region = { x: number; y: number; w: number; h: number };

/** Drag over your own shared screen to pick the part to share. */
export function RegionPicker({ onPick, onCancel }: { onPick: (r: Region) => void; onCancel: () => void }) {
  const [from, setFrom] = useState<[number, number] | null>(null);
  const [to, setTo] = useState<[number, number] | null>(null);
  const box = useRef<HTMLDivElement>(null);
  const at = (e: React.PointerEvent): [number, number] => {
    const r = box.current!.getBoundingClientRect();
    return [Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)), Math.max(0, Math.min(1, (e.clientY - r.top) / r.height))];
  };
  const rect = from && to ? { x: Math.min(from[0], to[0]), y: Math.min(from[1], to[1]), w: Math.abs(to[0] - from[0]), h: Math.abs(to[1] - from[1]) } : null;
  return (
    <div
      ref={box}
      className="call-region"
      onClick={(e) => e.stopPropagation()}
      onPointerDown={(e) => {
        (e.currentTarget as Element).setPointerCapture(e.pointerId);
        setFrom(at(e));
        setTo(at(e));
      }}
      onPointerMove={(e) => from && setTo(at(e))}
      onPointerUp={() => {
        if (rect && rect.w > 0.05 && rect.h > 0.05) onPick(rect);
        else setFrom(null);
      }}
      onKeyDown={(e) => e.key === "Escape" && onCancel()}
      tabIndex={0}
    >
      <span className="call-region-hint">{t("Drag over the part to share. Esc to cancel.")}</span>
      {rect && <div className="call-region-box" style={{ left: `${rect.x * 100}%`, top: `${rect.y * 100}%`, width: `${rect.w * 100}%`, height: `${rect.h * 100}%` }} />}
    </div>
  );
}

/**
 * Part of a shared screen as its own stream. The whole capture keeps running
 * underneath (the system picker has no region choice); only the part is sent.
 */
export function cropStream(src: MediaStream, r: Region, fps = 30): { stream: MediaStream; stop: () => void } {
  const video = document.createElement("video");
  video.muted = true;
  video.srcObject = src;
  video.play().catch(() => {});
  const canvas = document.createElement("canvas");
  const g = canvas.getContext("2d")!;
  // A timer, not animation frames: those stop while the window is hidden.
  const id = window.setInterval(() => {
    const vw = video.videoWidth;
    const vh = video.videoHeight;
    if (!vw || !vh) return;
    const w = Math.max(2, Math.round(vw * r.w));
    const h = Math.max(2, Math.round(vh * r.h));
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    g.drawImage(video, vw * r.x, vh * r.y, w, h, 0, 0, w, h);
  }, 1000 / fps);
  const stream = canvas.captureStream(fps);
  return {
    stream,
    stop: () => {
      window.clearInterval(id);
      stream.getTracks().forEach((tr) => tr.stop());
      video.srcObject = null;
    },
  };
}

/** How one person's stream is arriving, from piece timings alone. */
export type Arrival = { times: number[]; last: number };

export function quality(a: Arrival | undefined, now: number, sliceMs: number): { level: "good" | "fair" | "poor" | "none"; why: string; late: number } {
  if (!a || a.times.length < 2) return { level: "none", why: t("Not sending sound"), late: 0 };
  const age = now - a.last;
  let late = 0;
  for (let i = 1; i < a.times.length; i++) if (a.times[i] - a.times[i - 1] > sliceMs * 2.5) late++;
  const share = late / (a.times.length - 1);
  if (age > 3000) return { level: "poor", why: t("Nothing heard for {n} s", { n: Math.round(age / 1000) }), late: share };
  if (share > 0.2) return { level: "poor", why: t("Many pieces arrive late; their connection or the host's is struggling"), late: share };
  if (share > 0.06) return { level: "fair", why: t("Some pieces arrive late"), late: share };
  return { level: "good", why: t("Steady"), late: share };
}
