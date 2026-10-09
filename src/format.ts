// SPDX-License-Identifier: AGPL-3.0-only
import type { Placement } from "./api";
import { t } from "./i18n";

const GIB = 1024 ** 3;
const MIB = 1024 ** 2;

export function bytes(n: number): string {
  if (n >= GIB) return t("{n} GB", { n: (n / GIB).toFixed(n >= 10 * GIB ? 0 : 1) });
  if (n >= MIB) return t("{n} MB", { n: Math.round(n / MIB) });
  return t("{n} KB", { n: Math.max(0, Math.round(n / 1024)) });
}

export function tokens(n: number): string {
  return n.toLocaleString("en-US");
}

export function percent(part: number, whole: number): number {
  if (whole <= 0) return 0;
  return Math.min(100, Math.max(0, (part / whole) * 100));
}

export function placementLabel(p: Placement | null): string {
  switch (p) {
    case "gpu":
      return t("Runs on your graphics card");
    case "split":
      return t("Shares graphics card and memory");
    case "cpu":
      return t("Runs on your processor");
    default:
      return t("Can't run on this PC");
  }
}

/** Plain-language speed for a tokens-per-second figure. */
export function speedLabel(tps: number): string {
  if (tps >= 40) return t("Very fast");
  if (tps >= 15) return t("Fast");
  if (tps >= 8) return t("Comfortable");
  return t("Slow");
}

/** "4:05" or "1:02:09" for a number of seconds. */
export function clock(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/** Puts dictated text at the cursor, with a space where words would touch. */
export function insertDictation(value: string, start: number, end: number, text: string): { value: string; cursor: number } {
  const before = value.slice(0, start);
  const after = value.slice(end);
  const lead = before.length > 0 && !/\s$/.test(before) ? " " : "";
  const trail = after.length > 0 && !/^\s/.test(after) ? " " : "";
  const piece = lead + text.trim() + trail;
  return { value: before + piece + after, cursor: before.length + piece.length - trail.length };
}

/** How long a meeting ran, in words: "45 min", "1 h 5 min". */
export function duration(ms: number): string {
  const min = Math.max(1, Math.round(ms / 60000));
  if (min < 60) return t("{min} min", { min });
  const h = Math.floor(min / 60);
  return min % 60 ? t("{h} h {min} min", { h, min: min % 60 }) : t("{h} h", { h });
}
