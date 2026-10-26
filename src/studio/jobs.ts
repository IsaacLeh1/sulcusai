// SPDX-License-Identifier: AGPL-3.0-only
// What the Studio asked the music model for (a new part, a section made
// again, notes made real, stems), and what to do with the result when it
// comes: applied to the open project, or to the kept one when the Studio was
// closed meanwhile.
import { api, on, type MediaItem } from "../api";
import { addTake, addTrack, audioClip, findClip, newTrack, updateClip, type Project } from "./model";

export type Pending =
  /** Takes of a new part, as a new track from `at`. */
  | { kind: "part"; project: string; name: string; at: number; length: number }
  /** A stretch of a clip made again: a take of it. */
  | { kind: "replace"; project: string; clip: string; label: string }
  /** A clip's song made longer: a take of it, the clip as long as the new song. */
  | { kind: "longer"; project: string; clip: string; label: string }
  /** Notes made real, or a song's parts: new tracks from `at`. */
  | { kind: "tracks"; project: string; at: number; offset: number; length: number; mute?: string };

const pending = new Map<string, Pending>();
let openProject: { id: string; apply: (f: (p: Project) => Project) => void } | null = null;

/** Remembers what a job is for. */
export function expect(jobId: string, what: Pending) {
  pending.set(jobId, what);
}

/** How many jobs a project is waiting for. */
export function waitingFor(projectId: string) {
  return [...pending.values()].filter((p) => p.project === projectId).length;
}

/** The open project (results go straight into it), or none. */
export function setOpen(o: typeof openProject) {
  openProject = o;
}

function applyResult(p: Project, what: Pending, items: MediaItem[]): Project {
  switch (what.kind) {
    case "part": {
      if (!items.length) return p;
      const t = newTrack(what.name, "audio", p.tracks.length);
      // A part is as long as the mix it was made in: it plays from the
      // same moment, for the stretch it was asked for.
      const first = items[0];
      const base = audioClip(first.id, what.name, what.at, Math.max(1, Math.min(what.length, first.seconds - what.at)));
      const clip = { ...base, offset: what.at, takes: items.map((it, i) => ({ id: `${base.take}-${i}`, source: it.id, label: it.title || `Take ${i + 1}` })), take: `${base.take}-0` };
      t.clips = [clip];
      return addTrack(p, t);
    }
    case "replace": {
      if (!items.length || !findClip(p, what.clip)) return p;
      return addTake(p, what.clip, items[0].id, what.label);
    }
    case "longer": {
      const found = findClip(p, what.clip);
      if (!items.length || !found) return p;
      const c = found.clip;
      return updateClip(addTake(p, what.clip, items[0].id, what.label), what.clip, { length: Math.max(c.length, (items[0].seconds - c.offset) / c.speed) });
    }
    case "tracks": {
      let out = p;
      for (const it of items) {
        const t = newTrack(it.title?.match(/\(([^)]+)\)$/)?.[1] ?? it.title ?? "Track", "audio", out.tracks.length);
        t.clips = [{ ...audioClip(it.id, t.name, what.at, Math.min(what.length, it.seconds - what.offset)), offset: what.offset }];
        out = addTrack(out, t);
      }
      if (what.mute) out = { ...out, tracks: out.tracks.map((t) => (t.id === what.mute ? { ...t, mute: true } : t)) };
      return out;
    }
  }
}

// Results as they come, whether or not the Studio is open.
on("media:done", (d) => {
  const what = pending.get(d.job_id);
  if (!what) return;
  pending.delete(d.job_id);
  if (!d.ok || !d.items?.length) return;
  const items = d.items;
  if (openProject?.id === what.project) {
    openProject.apply((p) => applyResult(p, what, items));
    return;
  }
  api
    .studioProjects()
    .then((all) => {
      const p = all.find((x) => x.id === what.project);
      if (p) return api.studioProjectSave({ ...applyResult(p, what, items), updated_at: Date.now() });
    })
    .catch(() => {});
});
