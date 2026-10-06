// SPDX-License-Identifier: AGPL-3.0-only
import type { Placement } from "./api";

const GIB = 1024 ** 3;
const MIB = 1024 ** 2;

export function bytes(n: number): string {
  if (n >= GIB) return `${(n / GIB).toFixed(n >= 10 * GIB ? 0 : 1)} GB`;
  if (n >= MIB) return `${Math.round(n / MIB)} MB`;
  return `${Math.max(0, Math.round(n / 1024))} KB`;
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
      return "Runs on your graphics card";
    case "split":
      return "Shares graphics card and memory";
    case "cpu":
      return "Runs on your processor";
    default:
      return "Can't run on this PC";
  }
}

/** Plain-language speed for a tokens-per-second figure. */
export function speedLabel(tps: number): string {
  if (tps >= 40) return "Very fast";
  if (tps >= 15) return "Fast";
  if (tps >= 8) return "Comfortable";
  return "Slow";
}
