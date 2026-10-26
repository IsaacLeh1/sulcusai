// SPDX-License-Identifier: AGPL-3.0-only
// Gallery files in the window: the core hands over decrypted bytes, which
// become blob: URLs (cached, so scrolling the gallery doesn't refetch).
import { useEffect, useState } from "react";
import { api, type MediaItem, type MediaOp } from "./api";
import { t, tx } from "./i18n";

// Newest last: one used again moves to the end.
const cache = new Map<string, Promise<string>>();
// How many views show each one now; those are never let go.
const inUse = new Map<string, number>();
// Full files (whole videos and songs) are let go beyond the newest few;
// previews are small, so many more stay.
const KEEP = { f: 8, t: 400 } as const;

function url(key: string, load: () => Promise<ArrayBuffer>, mime: string): Promise<string> {
  let p = cache.get(key);
  if (p) {
    cache.delete(key);
    cache.set(key, p);
    return p;
  }
  const made = load().then((buf) => URL.createObjectURL(new Blob([buf], { type: mime })));
  made.catch(() => cache.get(key) === made && cache.delete(key));
  cache.set(key, made);
  trim(key.slice(0, 1) as keyof typeof KEEP);
  return made;
}

function drop(key: string) {
  cache.get(key)?.then((u) => URL.revokeObjectURL(u)).catch(() => {});
  cache.delete(key);
}

/** Lets go of the oldest of a kind (full files "f", previews "t") that nothing shows, past its limit. */
function trim(kind: keyof typeof KEEP) {
  const keys = [...cache.keys()].filter((k) => k.startsWith(`${kind}:`));
  let extra = keys.length - KEEP[kind];
  for (const k of keys) {
    if (extra <= 0) break;
    if (inUse.get(k)) continue;
    drop(k);
    extra--;
  }
}

/** Which cached file a view of this item shows. */
function keyOf(item: Pick<MediaItem, "id" | "kind"> & { poster?: boolean }, thumb: boolean): string {
  return thumb && (item.kind === "image" || item.poster) ? `t:${item.id}` : `f:${item.id}`;
}

// Bumped when an item's preview is replaced (a song's new cover art), so
// views showing it load the new one.
const thumbGen = new Map<string, number>();

/** Lets go of an item's cached preview; views of it load it again. */
export function forgetThumb(id: string) {
  drop(`t:${id}`);
  thumbGen.set(id, (thumbGen.get(id) ?? 0) + 1);
}

/** Keeps a cached file while it's used; the returned function lets it go. */
function hold(key: string): () => void {
  inUse.set(key, (inUse.get(key) ?? 0) + 1);
  return () => {
    const n = (inUse.get(key) ?? 1) - 1;
    if (n > 0) inUse.set(key, n);
    else inUse.delete(key);
  };
}

/** The full file, as a blob: URL. */
export function fileUrl(item: Pick<MediaItem, "id" | "mime">): Promise<string> {
  return url(`f:${item.id}`, () => api.mediaFile(item.id), item.mime);
}

/** A small preview (pictures, and videos with a still); other kinds fall back to the file itself. */
export function thumbUrl(item: Pick<MediaItem, "id" | "mime" | "kind"> & { poster?: boolean }): Promise<string> {
  if (keyOf(item, true).startsWith("f:")) return fileUrl(item);
  // Thumbnails are JPEG or PNG; the browser sniffs either from the bytes.
  return url(`t:${item.id}`, () => api.mediaThumb(item.id), "image/jpeg");
}

/** Videos made before stills were saved get one, taken here (the window
    decodes WebM), one at a time. */
let stills: Promise<unknown> = Promise.resolve();
export function makeStill(item: MediaItem): Promise<string | null> {
  const capture = async (src: string): Promise<string | null> => {
    const video = document.createElement("video");
    video.muted = true;
    video.preload = "auto";
    video.src = src;
    await new Promise<void>((ok, fail) => {
      video.onloadeddata = () => ok();
      video.onerror = () => fail(new Error("can't read the clip"));
    });
    video.currentTime = Math.min(0.5, (video.duration || 1) / 3);
    await new Promise<void>((ok) => (video.onseeked = () => ok()));
    const canvas = document.createElement("canvas");
    const scale = Math.min(1, 384 / Math.max(video.videoWidth, video.videoHeight));
    canvas.width = Math.round(video.videoWidth * scale);
    canvas.height = Math.round(video.videoHeight * scale);
    canvas.getContext("2d")?.drawImage(video, 0, 0, canvas.width, canvas.height);
    const dataUrl = canvas.toDataURL("image/jpeg", 0.85);
    await api.mediaSetPoster(item.id, dataUrl.split(",")[1] ?? "");
    return dataUrl;
  };
  // The clip is held meanwhile, so the cache doesn't let it go mid-read.
  const run = async (): Promise<string | null> => {
    const release = hold(keyOf(item, false));
    try {
      return await capture(await fileUrl(item));
    } finally {
      release();
    }
  };
  const next = stills.then(run, run).catch(() => null);
  stills = next;
  return next;
}

/** Forgets a deleted item's URLs. */
export function forget(id: string) {
  drop(`f:${id}`);
  drop(`t:${id}`);
}

export function useMediaUrl(item: Pick<MediaItem, "id" | "mime" | "kind"> | null, thumb = false): string | null {
  const [u, setU] = useState<string | null>(null);
  useEffect(() => {
    if (!item) {
      setU(null);
      return;
    }
    let live = true;
    // Held while it's shown, so it isn't let go from under the view.
    const release = hold(keyOf(item, thumb));
    (thumb ? thumbUrl(item) : fileUrl(item)).then((v) => live && setU(v)).catch(() => live && setU(null));
    return () => {
      live = false;
      release();
    };
  }, [item?.id, thumb, item && thumb ? thumbGen.get(item.id) ?? 0 : 0]); // eslint-disable-line react-hooks/exhaustive-deps
  return u;
}

/** "About 40 s", "About 3 min". */
export function waitLabel(secs: number | null | undefined): string {
  if (secs == null || !isFinite(secs)) return "";
  const s = Math.max(1, Math.round(secs));
  if (s < 60) return t("about {s} s", { s });
  const m = Math.round(s / 60);
  return t("about {m} min", { m });
}

export const OP_LABEL: Record<string, string> = {
  generate: tx("Made"),
  edit: tx("Edited"),
  fill: tx("Filled in"),
  extend: tx("Extended"),
  restyle: tx("Restyled"),
  upscale: tx("Upscaled"),
  remove_background: tx("Cut out"),
  video: tx("Video"),
  music: tx("Song"),
  sound: tx("Sound"),
  narrate: tx("Narration"),
  podcast: tx("Podcast"),
  dialogue: tx("Reading with several voices"),
  explainer: tx("Explainer video"),
  compose: tx("Composed"),
  wallpaper: tx("Wallpaper"),
  import: tx("Imported"),
  screenshot: tx("Screenshot"),
  trim: tx("Trimmed"),
  stem: tx("A part of a song"),
  dry: tx("Effects removed"),
  song_video: tx("Song video"),
  paste: tx("Pasted"),
  attach: tx("Attached"),
};

export const JOB_LABEL: Record<MediaOp | string, string> = {
  generate: tx("Making a picture"),
  edit: tx("Editing a picture"),
  fill: tx("Filling in"),
  extend: tx("Extending a picture"),
  restyle: tx("Restyling"),
  upscale: tx("Upscaling"),
  remove_background: tx("Removing the background"),
  video: tx("Making a video"),
  music: tx("Making a song"),
  sound: tx("Making a sound"),
  narrate: tx("Reading aloud"),
  podcast: tx("Making a podcast"),
  dialogue: tx("Reading with several voices"),
  explainer: tx("Making an explainer video"),
  tune: tx("Teaching the prompt writer"),
  coach: tx("Coaching the prompt writer"),
  listen: tx("Listening to a song"),
  remake: tx("Making the unclear parts again"),
  map: tx("Seeing how a song is built"),
  revoice: tx("Singing a song in a recorded voice"),
  cover_art: tx("Making cover art"),
  stems: tx("Taking a song apart"),
  dry: tx("Removing reverb and echo"),
  studio_part: tx("Making a part for the song studio"),
  song_video: tx("Making a song video"),
  train: tx("Training a custom model on your songs"),
  trainer: tx("Getting the trainer"),
  video_edit: tx("Editing a video"),
  video_sound: tx("Making sound for a video"),
  dub: tx("Dubbing a video"),
};

/** 16-bit PCM WAV from an audio buffer's samples, between two times. */
export function encodeWav(buf: AudioBuffer, start: number, end: number): Uint8Array {
  const rate = buf.sampleRate;
  const from = Math.max(0, Math.floor(start * rate));
  const to = Math.min(buf.length, Math.ceil(end * rate));
  const frames = Math.max(0, to - from);
  const ch = Math.min(2, buf.numberOfChannels);
  const data = new DataView(new ArrayBuffer(44 + frames * ch * 2));
  const str = (o: number, s: string) => [...s].forEach((c, i) => data.setUint8(o + i, c.charCodeAt(0)));
  str(0, "RIFF");
  data.setUint32(4, 36 + frames * ch * 2, true);
  str(8, "WAVE");
  str(12, "fmt ");
  data.setUint32(16, 16, true);
  data.setUint16(20, 1, true);
  data.setUint16(22, ch, true);
  data.setUint32(24, rate, true);
  data.setUint32(28, rate * ch * 2, true);
  data.setUint16(32, ch * 2, true);
  data.setUint16(34, 16, true);
  str(36, "data");
  data.setUint32(40, frames * ch * 2, true);
  const chans = Array.from({ length: ch }, (_, c) => buf.getChannelData(c));
  // A few milliseconds of fade at each end avoids clicks.
  const fade = Math.min(Math.floor(rate * 0.008), Math.floor(frames / 2));
  let o = 44;
  for (let i = 0; i < frames; i++) {
    const g = i < fade ? i / fade : i >= frames - fade ? (frames - i) / fade : 1;
    for (let c = 0; c < ch; c++) {
      const v = Math.max(-1, Math.min(1, chans[c][from + i] * g));
      data.setInt16(o, v < 0 ? v * 0x8000 : v * 0x7fff, true);
      o += 2;
    }
  }
  return new Uint8Array(data.buffer);
}
