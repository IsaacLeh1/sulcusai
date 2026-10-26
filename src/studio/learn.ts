// SPDX-License-Identifier: AGPL-3.0-only
// MIDI Learn (Suno Studio's): right-click a slider, move a knob on a MIDI
// controller, and that knob moves the slider from then on (kept with the
// project). Right-click it again to forget.
import { createContext, type MouseEvent } from "react";
import { updateTrack, type Project } from "./model";

/** A slider that can follow a MIDI control: what it changes, its name, and its range. */
export interface LearnTarget {
  /** "master", "track:<id>:volume", "track:<id>:pan", "fx:<track>:<effect>:<setting>" or "synth:<track>:<path>". */
  id: string;
  label: string;
  min: number;
  max: number;
  log?: boolean;
  step?: number;
}

export interface Learn {
  /** The slider waiting for a MIDI control to be moved. */
  learning: string | null;
  /** Sliders following a MIDI control. */
  bound: Set<string>;
  /** Starts learning a slider, stops, or forgets the control it follows. */
  toggle: (t: LearnTarget) => void;
}

export const LearnContext = createContext<Learn>({ learning: null, bound: new Set(), toggle: () => {} });

/** What makes a slider learnable: a right-click to learn or forget, and a mark while learning or bound. */
export function learnable(l: Learn, t: LearnTarget) {
  return {
    onContextMenu: (e: MouseEvent) => {
      e.preventDefault();
      l.toggle(t);
    },
    "data-midi": l.learning === t.id ? "learning" : l.bound.has(t.id) ? "bound" : undefined,
  };
}

/** A synth patch's setting at `path` ("filter.cutoff", "lfos.0.rate") set to `v`, in place. */
export function setPath(obj: Record<string, unknown>, path: string, v: number) {
  const keys = path.split(".");
  let at: Record<string, unknown> = obj;
  for (const k of keys.slice(0, -1)) {
    const next = at[k];
    if (!next || typeof next !== "object") return;
    at = next as Record<string, unknown>;
  }
  const last = keys[keys.length - 1];
  if (typeof at[last] === "number") at[last] = v;
}

/** A learnt MIDI control's value put on its slider (`target` as `LearnTarget` names it). */
export function applyBinding(cur: Project, target: string, v: number): Project {
  const [kind, trackId, ...rest] = target.split(":");
  const tr = cur.tracks.find((x) => x.id === trackId);
  if (!tr) return cur;
  if (kind === "track" && (rest[0] === "volume" || rest[0] === "pan")) return updateTrack(cur, trackId, { [rest[0]]: v });
  if (kind === "fx") return updateTrack(cur, trackId, { effects: tr.effects.map((e) => (e.id === rest[0] ? { ...e, params: { ...e.params, [rest[1]]: v } } : e)) });
  if (kind === "synth" && tr.synth) {
    const synth = structuredClone(tr.synth);
    setPath(synth as unknown as Record<string, unknown>, rest.join(":"), v);
    return updateTrack(cur, trackId, { synth });
  }
  return cur;
}
