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

export const medialibApi = {
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
