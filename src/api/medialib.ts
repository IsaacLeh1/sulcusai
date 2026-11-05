// SPDX-License-Identifier: AGPL-3.0-only
// The media library and reading: subtitles, feeds, e-books and the screen-history settings.
import { invoke } from "@tauri-apps/api/core";

export interface Cue {
  start: number;
  end: number;
  text: string;
}

export interface FeedItem {
  title: string;
  link: string;
  published: string;
  summary: string;
  audio: string | null;
}

export interface Feed {
  title: string;
  items: FeedItem[];
}

export interface Book {
  title: string;
  author: string;
  sections: { title: string; text: string }[];
}

export interface TimelineSettings {
  enabled: boolean;
  every_secs: number;
  keep_days: number;
  skip_apps: string[];
}

export interface Segment {
  start: number;
  end: number;
  speaker: string | null;
  text: string;
}

export interface Chapter {
  start: number;
  end: number;
  title: string;
}

export interface IndexSummary {
  id: string;
  path: string;
  title: string;
  seconds: number;
  added: number;
  chapters: number;
  speakers: [string, number][];
}

export interface IndexEntry {
  id: string;
  path: string;
  title: string;
  seconds: number;
  added: number;
  segments: Segment[];
  chapters: Chapter[];
}

export interface IndexHit {
  media: string;
  start: number;
  speaker: string | null;
  text: string;
  score: number;
}

export interface SavedFeed {
  url: string;
  title: string;
  podcast: boolean;
}

export interface Shot {
  at: number;
  exe: string;
  title: string;
}

export interface Photo {
  path: string;
  name: string;
  taken: number;
  size: number;
}

export const medialibApi = {
  indexAdd: (path: string) => invoke<IndexSummary>("media_index_add", { path }),
  indexList: () => invoke<IndexSummary[]>("media_index_list"),
  indexGet: (id: string) => invoke<IndexEntry>("media_index_get", { id }),
  indexRemove: (id: string) => invoke<void>("media_index_remove", { id }),
  indexSearch: (query: string) => invoke<IndexHit[]>("media_index_search", { query }),
  renameSpeaker: (id: string, from: string, to: string) => invoke<void>("media_index_rename_speaker", { id, from, to }),
  saveCaptions: (id: string, path: string) => invoke<void>("media_index_save_captions", { id, path }),
  feeds: () => invoke<SavedFeed[]>("media_feeds"),
  addFeed: (url: string) => invoke<SavedFeed[]>("media_feed_add", { url }),
  removeFeed: (url: string) => invoke<SavedFeed[]>("media_feed_remove", { url }),
  fetchFeed: (url: string) => invoke<Feed>("media_feed_fetch", { url }),
  timelineDay: (dayStart: number, pin?: string) => invoke<Shot[]>("media_timeline_day", { pin, dayStart }),
  timelineSearch: (query: string, pin?: string) => invoke<Shot[]>("media_timeline_search", { pin, query }),
  timelineImage: (at: number, pin?: string) => invoke<string>("media_timeline_image", { pin, at }),
  timelineClear: (pin?: string) => invoke<void>("media_timeline_clear", { pin }),
  liveCaptionsStart: (translate: boolean) => invoke<void>("media_live_captions_start", { translate }),
  liveCaptionsStop: () => invoke<void>("media_live_captions_stop"),
  liveCaptionsSave: (path: string) => invoke<void>("media_live_captions_save", { path }),
  photoFolders: () => invoke<string[]>("media_photo_folders"),
  setPhotoFolders: (folders: string[]) => invoke<void>("media_set_photo_folders", { folders }),
  photos: () => invoke<Photo[]>("media_photos"),
  photoThumb: (path: string, size?: number) => invoke<string>("media_photo_thumb", { path, size }),
  screenshot: () => invoke<string>("media_screenshot"),
  playFile: (path: string) => invoke<string>("media_play_file", { path }),
  findSubtitles: (path: string) => invoke<string[]>("media_find_subtitles", { path }),

  subtitles: (path: string) => invoke<Cue[]>("media_subtitles", { path }),
  parseFeed: (xml: string) => invoke<Feed>("media_feed_parse", { xml }),
  openBook: (path: string) => invoke<Book>("media_ebook_open", { path }),
  timeline: () => invoke<{ settings: TimelineSettings; locked: boolean }>("media_timeline_view"),
  setTimeline: (settings: TimelineSettings, pin?: string) => invoke<void>("media_timeline_set", { settings, pin }),
  setTimelinePin: (newPin: string, pin?: string) => invoke<void>("media_timeline_set_pin", { pin, newPin }),
};

/** The cue showing at a time, for the player's subtitle line. */
export function cueAt(cues: Cue[], t: number): Cue | undefined {
  return cues.find((c) => c.start <= t && t < c.end);
}

/** Where a reader is in a book, as a whole percent. */
export function readProgress(book: Book, section: number, within: number): number {
  const total = book.sections.reduce((n, s) => n + s.text.length, 0);
  if (total === 0) return 0;
  const before = book.sections.slice(0, section).reduce((n, s) => n + s.text.length, 0);
  const here = (book.sections[section]?.text.length ?? 0) * Math.min(Math.max(within, 0), 1);
  return Math.round(((before + here) / total) * 100);
}

/** "1:05:09" or "4:07", for times in the player and transcripts. */
export function clock(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${sec}` : `${m}:${sec}`;
}

/** Photos grouped by month ("2026-11"), newest month first, keeping order. */
export function byMonth(photos: Photo[]): [string, Photo[]][] {
  const groups = new Map<string, Photo[]>();
  for (const p of photos) {
    const d = new Date(p.taken * 1000);
    const key = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`;
    const list = groups.get(key);
    if (list) list.push(p);
    else groups.set(key, [p]);
  }
  return [...groups.entries()];
}

/** Local midnight of the day holding `ms`, in seconds. */
export function dayStart(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return Math.floor(d.getTime() / 1000);
}
