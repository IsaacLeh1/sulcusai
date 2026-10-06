// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { ModelCard } from "./api";
import { starterModel } from "./starter";

function card(id: string, params_b: number, tps: number, tags: string[] = [], installed = false): ModelCard {
  return {
    id,
    name: id,
    publisher: "",
    source: "",
    description: "",
    license: { name: "MIT", url: "", commercial: true },
    tags,
    params_b,
    default_ctx: 8192,
    variants: [],
    fit: { ctx: 8192, recommended: "Q4", variants: [{ quant: "Q4", placement: "gpu", needed_bytes: 0, est_tps: tps, gpu_layers: 0, disk_ok: true }] },
    installed: installed ? { model_id: id, quant: "Q4", path: "", size: 0, installed_at: 0, tps: null } : null,
  };
}

describe("starterModel", () => {
  it("prefers the catalog's recommended pick", () => {
    expect(starterModel([card("big", 14, 40), card("rec", 4, 60, ["recommended"])])?.id).toBe("rec");
  });

  it("otherwise picks the largest model that runs comfortably", () => {
    expect(starterModel([card("small", 2, 90), card("mid", 8, 30), card("huge", 14, 6)])?.id).toBe("mid");
  });

  it("falls back to the smallest model on a slow PC", () => {
    expect(starterModel([card("a", 4, 6), card("b", 2, 9)])?.id).toBe("b");
  });

  it("skips installed models and handles an empty list", () => {
    expect(starterModel([card("rec", 4, 60, ["recommended"], true), card("x", 2, 50)])?.id).toBe("x");
    expect(starterModel([])).toBeNull();
  });
});
