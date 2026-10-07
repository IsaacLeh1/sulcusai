// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { ModelCard } from "./api";
import { arrange } from "./ranking";

function m(id: string, overall: number, coding: number, tps: number, measured: number | null = null): ModelCard {
  return {
    id,
    name: id,
    publisher: "",
    source: "",
    description: "",
    license: { name: "MIT", url: "", commercial: true },
    tags: [],
    params_b: 1,
    default_ctx: 8192,
    tools: true,
    variants: [],
    ratings: { overall, coding, writing: 5, research: 5, agents: 5, languages: 5 },
    fit: { ctx: 8192, recommended: "Q4", variants: [{ quant: "Q4", placement: "gpu", needed_bytes: 0, est_tps: tps, gpu_layers: 0, disk_ok: true }] },
    installed: measured === null ? null : { model_id: id, quant: "Q4", path: "", size: 0, installed_at: 0, tps: measured },
  };
}

const all = [m("big", 70, 8, 20), m("tiny", 20, 3, 90), m("coder", 40, 7, 50, 60)];
const ids = (ms: ModelCard[]) => ms.map((x) => x.id);

describe("arrange", () => {
  it("ranks from least to most capable, or the other way", () => {
    expect(ids(arrange(all, "all", false))).toEqual(["tiny", "coder", "big"]);
    expect(ids(arrange(all, "all", true))).toEqual(["big", "coder", "tiny"]);
  });
  it("shows only models strong in an area, best first when asked", () => {
    expect(ids(arrange(all, "coding", true))).toEqual(["big", "coder"]);
  });
  it("orders by speed on this PC, measured over estimated", () => {
    expect(ids(arrange(all, "fastest", false))).toEqual(["tiny", "coder", "big"]);
    const slowMeasured = [m("a", 1, 1, 100, 10), m("b", 1, 1, 30)];
    expect(ids(arrange(slowMeasured, "fastest", false))).toEqual(["b", "a"]);
  });
});
