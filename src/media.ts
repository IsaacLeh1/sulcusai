// SPDX-License-Identifier: AGPL-3.0-only
// Gallery files in the window: the core hands over decrypted bytes, which
// become blob: URLs (cached, so scrolling the gallery doesn't refetch).
import { useEffect, useState } from "react";
import { api, type MediaItem, type MediaOp } from "./api";

const cache = new Map<string, Promise<string>>();

function url(key: string, load: () => Promise<ArrayBuffer>, mime: string): Promise<string> {
  let p = cache.get(key);
  if (!p) {
    p = load().then((buf) => URL.createObjectURL(new Blob([buf], { type: mime })));
    p.catch(() => cache.delete(key));
    cache.set(key, p);
  }
  return p;
}

/** The full file, as a blob: URL. */
export function fileUrl(item: Pick<MediaItem, "id" | "mime">): Promise<string> {
  return url(`f:${item.id}`, () => api.mediaFile(item.id), item.mime);
}

/** A small preview (pictures); other kinds fall back to the file itself. */
export function thumbUrl(item: Pick<MediaItem, "id" | "mime" | "kind">): Promise<string> {
  if (item.kind !== "image") return fileUrl(item);
  // Thumbnails are JPEG or PNG; the browser sniffs either from the bytes.
  return url(`t:${item.id}`, () => api.mediaThumb(item.id), "image/jpeg");
}

/** Forgets a deleted item's URLs. */
export function forget(id: string) {
  for (const k of [`f:${id}`, `t:${id}`]) {
    cache.get(k)?.then((u) => URL.revokeObjectURL(u)).catch(() => {});
    cache.delete(k);
  }
}

export function useMediaUrl(item: Pick<MediaItem, "id" | "mime" | "kind"> | null, thumb = false): string | null {
  const [u, setU] = useState<string | null>(null);
  useEffect(() => {
    if (!item) {
      setU(null);
      return;
    }
    let live = true;
    (thumb ? thumbUrl(item) : fileUrl(item)).then((v) => live && setU(v)).catch(() => live && setU(null));
    return () => {
      live = false;
    };
  }, [item?.id, thumb]); // eslint-disable-line react-hooks/exhaustive-deps
  return u;
}

/** "About 40 s", "About 3 min". */
export function waitLabel(secs: number | null | undefined): string {
  if (secs == null || !isFinite(secs)) return "";
  const s = Math.max(1, Math.round(secs));
  if (s < 60) return `about ${s} s`;
  const m = Math.round(s / 60);
  return `about ${m} min`;
}

export const OP_LABEL: Record<string, string> = {
  generate: "Made",
  edit: "Edited",
  fill: "Filled in",
  extend: "Extended",
  restyle: "Restyled",
  upscale: "Upscaled",
  remove_background: "Cut out",
  video: "Video",
  music: "Song",
  sound: "Sound",
  narrate: "Narration",
  import: "Imported",
  screenshot: "Screenshot",
  trim: "Trimmed",
  paste: "Pasted",
  attach: "Attached",
};

export const JOB_LABEL: Record<MediaOp | string, string> = {
  generate: "Making a picture",
  edit: "Editing a picture",
  fill: "Filling in",
  extend: "Extending a picture",
  restyle: "Restyling",
  upscale: "Upscaling",
  remove_background: "Removing the background",
  video: "Making a video",
  music: "Making a song",
  sound: "Making a sound",
  narrate: "Reading aloud",
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
