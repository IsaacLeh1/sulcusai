// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { addTake, audible, audioClip, compGroup, compTake, deleteTime, duplicateTime, end, heal, midiClip, moveClips, newTrack, pasteClips, quantizeWarp, recordedClips, removeTake, snap, soundAt, sourceOf, splitClip, startIn, stretchClip, timeOfSound, trimClip, updateClip, valueAt, type Project } from "./model";


function project(): Project {
  const a = newTrack("Song", "audio", 0);
  a.clips = [audioClip("song1", "Song", 0, 100)];
  const b = newTrack("Keys", "midi", 1);
  b.clips = [midiClip("Keys", 10, 20, [{ pitch: 60, start: 1, end: 3, velocity: 90 }, { pitch: 64, start: 12, end: 15, velocity: 90 }])];
  return { id: "p", name: "P", bpm: 120, beats: 4, tracks: [a, b], loop: null, created_at: 0, updated_at: 0 };
}

describe("studio project", () => {
  it("splits a clip where its source goes on, notes and all", () => {
    const p = project();
    const id = p.tracks[0].clips[0].id;
    const q = splitClip(p, id, 40);
    const [a, b] = q.tracks[0].clips;
    expect([a.start, a.length, b.start, b.offset, b.length]).toEqual([0, 40, 40, 40, 60]);
    expect(sourceOf(b)).toBe("song1");
    const m = splitClip(q, q.tracks[1].clips[0].id, 20);
    const [l, r] = m.tracks[1].clips;
    expect(l.notes?.map((n) => n.pitch)).toEqual([60]);
    expect(r.notes).toEqual([{ pitch: 64, start: 2, end: 5, velocity: 90 }]);
    expect(splitClip(p, id, 0)).toBe(p);
  });

  it("trims an edge without passing the source's start or end", () => {
    const p = project();
    const id = p.tracks[0].clips[0].id;
    const shorter = trimClip(p, id, "end", 150, 100);
    expect(shorter.tracks[0].clips[0].length).toBe(100);
    const later = trimClip(p, id, "start", 25, 100).tracks[0].clips[0];
    expect([later.start, later.offset, later.length]).toEqual([25, 25, 75]);
    const back = trimClip({ ...p, tracks: [{ ...p.tracks[0], clips: [later] }] }, id, "start", 5, 100).tracks[0].clips[0];
    expect([back.start, back.offset]).toEqual([5, 5]);
  });

  it("moves clips, to another track too", () => {
    const p = project();
    const id = p.tracks[1].clips[0].id;
    const moved = moveClips(p, new Set([id]), -15, p.tracks[0].id);
    expect(moved.tracks[1].clips.length).toBe(0);
    expect(moved.tracks[0].clips.find((c) => c.id === id)?.start).toBe(0);
  });

  it("takes time out of every track and closes the gap", () => {
    const p = project();
    const q = deleteTime(p, 20, 30);
    expect(q.tracks[0].clips.map((c) => [c.start, c.offset, c.length])).toEqual([[0, 0, 20], [20, 30, 70]]);
    expect(q.tracks[1].clips.map((c) => [c.start, c.length])).toEqual([[10, 10]]);
    const d = duplicateTime(p, 0, 10);
    expect(Math.max(...d.tracks[0].clips.map(end))).toBe(110);
  });

  it("pastes clips keeping their spacing", () => {
    const p = project();
    const clips = p.tracks.map((t) => ({ trackId: t.id, clip: t.clips[0] }));
    const q = pasteClips(p, clips, 200);
    expect(q.tracks[0].clips[1].start).toBe(200);
    expect(q.tracks[1].clips[1].start).toBe(210);
  });

  it("keeps takes and plays the chosen one", () => {
    const p = project();
    const id = p.tracks[0].clips[0].id;
    const q = addTake(p, id, "song2", "Take 2");
    expect(sourceOf(q.tracks[0].clips[0])).toBe("song2");
    expect(q.tracks[0].clips[0].takes.length).toBe(2);
  });

  it("snaps to the beat and reads automation between points", () => {
    const p = project();
    expect(snap(p, 1.13, 1)).toBe(1);
    expect(snap(p, 1.13, 0.25)).toBeCloseTo(1.125);
    expect(valueAt([{ t: 0, v: 0 }, { t: 10, v: -10 }], 5, 0)).toBe(-5);
    expect(valueAt(undefined, 5, 3)).toBe(3);
  });

  it("hears soloed tracks only, else the unmuted", () => {
    const p = project();
    expect(audible({ ...p, tracks: [{ ...p.tracks[0], mute: true }, p.tracks[1]] }).size).toBe(1);
    expect([...audible({ ...p, tracks: [p.tracks[0], { ...p.tracks[1], solo: true }] })]).toEqual([p.tracks[1].id]);
  });
});

describe("takes, comping and warp markers", () => {
  /** A song clip with a second take ("song2"), as a part's takes or a stretch made again come. */
  function twoTakes(): { p: Project; id: string } {
    const p = project();
    const id = p.tracks[0].clips[0].id;
    return { p: addTake(p, id, "song2", "Take 2"), id };
  }

  it("comps a stretch from another take, and heals back to one clip", () => {
    const { p, id } = twoTakes();
    const t1 = p.tracks[0].clips[0].takes[0].id;
    const t2 = p.tracks[0].clips[0].takes[1].id;
    // Take 2 is playing; take 1 for 20 to 30 s.
    const q = compTake(p, id, t1, 20, 30);
    const clips = [...q.tracks[0].clips].sort((a, b) => a.start - b.start);
    expect(clips.map((c) => [c.start, c.length, c.offset, sourceOf(c)])).toEqual([[0, 20, 0, "song2"], [20, 10, 20, "song1"], [30, 70, 30, "song2"]]);
    expect(compGroup(q, clips[1].id)?.clips.length).toBe(3);
    // Take 2 all through again: one clip, as it was.
    const r = compTake(q, clips[1].id, t2, 0, 100);
    expect(r.tracks[0].clips.map((c) => [c.start, c.length, c.offset, c.take])).toEqual([[0, 100, 0, t2]]);
  });

  it("gives a new take to every piece of a comp, played where it was asked for", () => {
    const { p, id } = twoTakes();
    const t1 = p.tracks[0].clips[0].takes[0].id;
    const q = compTake(p, id, t1, 40, 60);
    const mid = q.tracks[0].clips.find((c) => c.start === 40)!;
    const r = addTake(q, mid.id, "song3", "Made again");
    const sorted = [...r.tracks[0].clips].sort((a, b) => a.start - b.start);
    expect(sorted.map((c) => c.takes.length)).toEqual([3, 3, 3]);
    expect(sorted.map((c) => sourceOf(c))).toEqual(["song2", "song3", "song2"]);
    // Taken off again: the pieces on it go back to the first take.
    const gone = removeTake(r, mid.id, sorted[1].take!);
    expect([...gone.tracks[0].clips].sort((a, b) => a.start - b.start).map((c) => sourceOf(c))).toEqual(["song2", "song1", "song2"]);
  });

  it("doesn't join clips that don't play on from each other", () => {
    const p = project();
    const id = p.tracks[0].clips[0].id;
    const q = splitClip(p, id, 50);
    const [a, b] = q.tracks[0].clips;
    expect(heal(q, q.tracks[0].id).tracks[0].clips.length).toBe(1);
    expect(heal(updateClip(q, b.id, { offset: 55 }), q.tracks[0].id).tracks[0].clips.length).toBe(2);
    expect(heal(updateClip(q, a.id, { fadeOut: 1 }), q.tracks[0].id).tracks[0].clips.length).toBe(2);
  });

  it("maps time to sound through warp markers, both ways", () => {
    const c = { ...audioClip("s", "S", 0, 20), warp: [{ t: 4, s: 2 }, { t: 8, s: 8 }] };
    expect(soundAt(c, 2)).toBeCloseTo(1);
    expect(soundAt(c, 6)).toBeCloseTo(5);
    expect(soundAt(c, 12)).toBeCloseTo(12);
    for (const s of [0.5, 2, 5, 9, 15]) expect(soundAt(c, timeOfSound(c, s))).toBeCloseTo(s);
    expect(soundAt({ ...c, warpOff: true }, 6)).toBeCloseTo(6);
  });

  it("splits a warped clip where its sound goes on", () => {
    const p = project();
    const id = p.tracks[0].clips[0].id;
    const w = updateClip(p, id, { warp: [{ t: 10, s: 5 }, { t: 30, s: 30 }] });
    const q = splitClip(w, id, 20);
    const [a, b] = q.tracks[0].clips;
    expect(a.warp).toEqual([{ t: 10, s: 5 }, { t: 20, s: 17.5 }]);
    for (const t of [3, 15]) expect(soundAt(a, t)).toBeCloseTo(soundAt(w.tracks[0].clips[0], t));
    expect(b.offset).toBeCloseTo(17.5);
    expect(b.warp).toEqual([{ t: 10, s: 30 }]);
    // Healed back, it plays as before.
    const h = heal(q, q.tracks[0].id).tracks[0].clips[0];
    for (const t of [3, 15, 25, 50]) expect(soundAt(h, t)).toBeCloseTo(soundAt(w.tracks[0].clips[0], t));
  });

  it("stretches a clip to a length and lines markers up with the grid", () => {
    const p = project();
    const id = p.tracks[0].clips[0].id;
    const s = stretchClip(updateClip(p, id, { length: 10, warp: [{ t: 5, s: 5 }] }), id, 20).tracks[0].clips[0];
    expect([s.length, s.speed, s.keepPitch, s.warp?.[0].t]).toEqual([20, 0.5, true, 10]);
    // At 120 bpm a beat is half a second: a hit at 1.2 s goes to 1 s.
    const q = quantizeWarp(updateClip(p, id, { warp: [{ t: 1.2, s: 1.2 }, { t: 2.9, s: 2.9 }] }), id, 1).tracks[0].clips[0];
    expect(q.warp).toEqual([{ t: 1, s: 1.2 }, { t: 3, s: 2.9 }]);
  });

  it("makes each pass round a loop a take of one recording", () => {
    // Recording from 2 s, the music heard 0.3 s in, round a loop from 4 to 8 s, 15.3 s recorded.
    const clips = recordedClips("rec", "Recording", 15.3, 2, 0.3, [4, 8], (n) => `Pass ${n}`);
    expect(clips.length).toBe(2);
    const [before, loop] = clips;
    expect([before.start, before.length, before.offset]).toEqual([2, 2, 0.3]);
    expect([loop.start, loop.length]).toEqual([4, 4]);
    expect(loop.takes.map((k) => k.shift)).toEqual([2.3, 6.3, 10.3]);
    expect(loop.take).toBe(loop.takes[2].id);
    expect(startIn(loop)).toBeCloseTo(10.3);
    // Played straight through: one clip, the lead-in left out.
    const one = recordedClips("rec", "Recording", 10, 2, 0.4, null, (n) => `Pass ${n}`);
    expect(one.map((c) => [c.start, c.offset, c.length])).toEqual([[2, 0.4, 9.6]]);
    // Begun inside the loop: the first pass is a take too, silent before it began.
    const inside = recordedClips("rec", "Recording", 9, 6, 0.2, [4, 8], (n) => `Pass ${n}`);
    expect(inside.length).toBe(1);
    expect(inside[0].takes.map((k) => k.shift)).toEqual([-1.8, 2.2, 6.2]);
    expect(inside[0].take).toBe(inside[0].takes[1].id);
  });

  it("keeps a take's place in its recording when trimming", () => {
    const [loop] = recordedClips("rec", "Recording", 20, 4, 0, [4, 8], (n) => `Pass ${n}`);
    const p = project();
    const q = { ...p, tracks: [{ ...p.tracks[0], clips: [loop] }] };
    const t = trimClip(q, loop.id, "start", 5, 20).tracks[0].clips[0];
    expect([t.start, t.offset, startIn(t)]).toEqual([5, 1, 1 + (loop.takes.find((k) => k.id === loop.take)!.shift ?? 0)]);
  });
});
