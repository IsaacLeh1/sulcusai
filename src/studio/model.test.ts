// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { addTake, audible, audioClip, deleteTime, duplicateTime, end, midiClip, moveClips, newTrack, pasteClips, snap, sourceOf, splitClip, trimClip, valueAt, type Project } from "./model";


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
