// SPDX-License-Identifier: AGPL-3.0-only
// Forgiving search for short lists like the Settings page: typos, word
// order, partial words and other words for the same thing still find it.

/** Lowercase words, without punctuation or accents. */
export function words(text: string): string[] {
  return text
    .toLowerCase()
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .split(/[^a-z0-9]+/)
    .filter(Boolean);
}

/** Edits (insert, delete, change, swap two letters) between two words, or
 * more than `max` as `max + 1`. */
export function distance(a: string, b: string, max = 2): number {
  if (Math.abs(a.length - b.length) > max) return max + 1;
  const prev2 = new Array(b.length + 1).fill(0);
  let prev = Array.from({ length: b.length + 1 }, (_, j) => j);
  for (let i = 1; i <= a.length; i++) {
    const cur = [i];
    let best = i;
    for (let j = 1; j <= b.length; j++) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1;
      let d = Math.min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + cost);
      if (i > 1 && j > 1 && a[i - 1] === b[j - 2] && a[i - 2] === b[j - 1]) d = Math.min(d, prev2[j - 2] + 1);
      cur.push(d);
      best = Math.min(best, d);
    }
    if (best > max) return max + 1;
    prev2.splice(0, prev2.length, ...prev);
    prev = cur;
  }
  return prev[b.length];
}

/** How well one typed word matches a word in the text: 1 exact, less for a
 * start of the word, inside it, or a typo; 0 for no match. */
export function wordScore(typed: string, word: string): number {
  if (typed === word) return 1;
  if (typed.length >= 2 && word.startsWith(typed)) return 0.9;
  if (typed.length >= 3 && word.includes(typed)) return 0.7;
  if (typed.length >= 4) {
    // A typo in what's typed so far ("notifcations", "pasword").
    const allowed = typed.length >= 7 ? 2 : 1;
    if (distance(typed, word, allowed) <= allowed) return 0.75;
    if (word.length > typed.length && distance(typed, word.slice(0, typed.length), allowed) <= allowed) return 0.6;
  }
  return 0;
}

export interface Match<T> {
  item: T;
  score: number;
  /** Typed words that matched only with a typo, and what they matched. */
  corrected: [string, string][];
}

/**
 * Ranks `items` against `query`. Each typed word counts by its best match
 * in the item's text, or in the other words given for it (`synonyms` maps a
 * word people type to words the settings use). Items missing a typed word
 * entirely score lower but can still show.
 */
export function rank<T>(items: T[], query: string, text: (item: T) => string, synonyms: Record<string, string[]> = {}): Match<T>[] {
  const typed = words(query);
  if (typed.length === 0) return [];
  const out: Match<T>[] = [];
  for (const item of items) {
    const ws = words(text(item));
    let total = 0;
    const corrected: [string, string][] = [];
    for (const q of typed) {
      let best = 0;
      let via = "";
      for (const w of ws) {
        const s = wordScore(q, w);
        if (s > best) {
          best = s;
          via = w;
        }
      }
      // Another word for it: "password" finds the lock settings.
      for (const [key, alts] of Object.entries(synonyms)) {
        const k = wordScore(q, key);
        if (k < 0.75) continue;
        for (const alt of alts) {
          for (const a of words(alt)) {
            if (ws.includes(a) && k * 0.9 > best) {
              best = k * 0.9;
              via = k < 0.9 ? key : "";
            }
          }
        }
      }
      if (best > 0 && best < 0.9 && via && via !== q && wordScore(q, via) === 0.75) corrected.push([q, via]);
      total += best;
    }
    let score = total / typed.length;
    // The words in order, as typed.
    if (typed.length > 1 && ws.join(" ").includes(typed.join(" "))) score += 0.2;
    if (score >= 0.45) out.push({ item, score, corrected });
  }
  return out.sort((a, b) => b.score - a.score);
}
