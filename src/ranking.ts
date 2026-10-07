// SPDX-License-Identifier: AGPL-3.0-only
// Ordering and filtering for the Models page.
import type { ModelCard, Ratings } from "./api";

export type Area = Exclude<keyof Ratings, "overall">;
export type Filter = "all" | Area | "fastest";

export const FILTERS: { id: Filter; label: string; hint: string }[] = [
  { id: "all", label: "All", hint: "Every model this PC can run, ranked by overall capability." },
  { id: "coding", label: "Coding", hint: "Best at writing, explaining and fixing code." },
  { id: "writing", label: "Writing", hint: "Best at drafting, rewriting and tone." },
  { id: "research", label: "Research", hint: "Best at reasoning through sources, long documents and web results." },
  { id: "agents", label: "Agents", hint: "Best at using tools: files, commands, notes, web search." },
  { id: "languages", label: "Languages", hint: "Best outside English: translation and chats in other languages." },
  { id: "fastest", label: "Fastest", hint: "Quickest replies on this PC." },
];

/** Strong enough in an area to appear on its tab. */
export const AREA_MIN = 6;

/** Tokens per second on this PC: measured once installed, else estimated. */
export function speedOf(m: ModelCard): number {
  if (m.installed?.tps) return m.installed.tps;
  const q = m.installed?.quant ?? m.fit.recommended;
  return m.fit.variants.find((v) => v.quant === q)?.est_tps ?? 0;
}

export function arrange(models: ModelCard[], filter: Filter, mostFirst: boolean): ModelCard[] {
  if (filter === "fastest") return [...models].sort((a, b) => speedOf(b) - speedOf(a));
  const score = (m: ModelCard) => (filter === "all" ? m.ratings.overall : m.ratings[filter] * 100 + m.ratings.overall);
  const shown = filter === "all" ? models : models.filter((m) => m.ratings[filter] >= AREA_MIN);
  return [...shown].sort((a, b) => (mostFirst ? score(b) - score(a) : score(a) - score(b)));
}
