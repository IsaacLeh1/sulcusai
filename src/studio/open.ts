// SPDX-License-Identifier: AGPL-3.0-only
// Opening a song in the song studio.
import { api, type MediaItem } from "../api";
import { projectFor, type Project } from "./model";

/** A song's project: the one it has, else a new one around it (kept at once). */
export async function projectForSong(song: MediaItem): Promise<Project> {
  const all = await api.studioProjects();
  const have = all.find((x) => x.song === song.id);
  if (have) return have;
  const bpm = song.music?.bpm ?? 120;
  const beats = Number((song.music?.time_signature ?? "4").split("/")[0]) || 4;
  const made = projectFor(song, bpm, beats);
  await api.studioProjectSave(made);
  return made;
}
