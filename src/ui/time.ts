// SPDX-License-Identifier: AGPL-3.0-only
import { t } from "../i18n";

/** "just now", "5 min ago", "yesterday", then a date: what people read faster than a timestamp. */
export function relativeTime(when: number | string | Date, now = Date.now()): string {
  const ms = typeof when === "number" ? (when < 1e12 ? when * 1000 : when) : new Date(when).getTime();
  if (!Number.isFinite(ms)) return "";
  const s = Math.round((now - ms) / 1000);
  if (s < 45) return t("just now");
  const m = Math.round(s / 60);
  if (m < 60) return t("{n} min ago", { n: m });
  const h = Math.round(m / 60);
  if (h < 24) return t("{n} h ago", { n: h });
  const d = Math.round(h / 24);
  if (d === 1) return t("yesterday");
  if (d < 7) return t("{n} days ago", { n: d });
  return new Date(ms).toLocaleDateString();
}

/** Time left from progress so far: "about 3 min left". */
export function timeLeft(startedAt: number, fraction: number, now = Date.now()): string | null {
  if (fraction <= 0.02 || fraction >= 1) return null;
  const left = ((now - startedAt) / fraction) * (1 - fraction) / 1000;
  if (left < 60) return t("less than a minute left");
  if (left < 3600) return t("about {n} min left", { n: Math.round(left / 60) });
  return t("about {n} h left", { n: Math.round(left / 3600) });
}
