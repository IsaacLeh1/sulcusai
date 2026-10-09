// SPDX-License-Identifier: AGPL-3.0-only
import type { ModelCard } from "./api";

const COMFORTABLE_TPS = 15;

/** The model to suggest first on this PC. */
export function starterModel(models: ModelCard[]): ModelCard | null {
  const available = models.filter((m) => !m.installed);
  // One that's already on this PC needs no download.
  const onDisk = available.find((m) => m.on_disk);
  if (onDisk) return onDisk;
  const recommended = available.find((m) => m.tags.includes("recommended"));
  if (recommended) return recommended;
  const speedOf = (m: ModelCard) => m.fit.variants.find((v) => v.quant === m.fit.recommended)?.est_tps ?? 0;
  // Otherwise the most capable model that still runs comfortably…
  const comfortable = available.filter((m) => speedOf(m) >= COMFORTABLE_TPS).sort((a, b) => b.params_b - a.params_b);
  if (comfortable[0]) return comfortable[0];
  // …or, on a slow PC, the smallest one.
  return [...available].sort((a, b) => a.params_b - b.params_b)[0] ?? null;
}
