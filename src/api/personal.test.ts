// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { byDay, type PersonalRecord } from "./personal";

const r = (id: string, at: number): PersonalRecord => ({ id, kind: "journal", at, data: {}, updated_at: at });

describe("byDay", () => {
  it("groups by local day, newest first", () => {
    const a = new Date(2026, 10, 3, 9).getTime();
    const b = new Date(2026, 10, 3, 20).getTime();
    const c = new Date(2026, 10, 4, 8).getTime();
    const g = byDay([r("a", a), r("c", c), r("b", b)]);
    expect(g.map(([d]) => d)).toEqual(["2026-11-04", "2026-11-03"]);
    expect(g[1][1].map((x) => x.id)).toEqual(["b", "a"]);
  });
});

import { parseIngredients, weekIndex } from "./personal";

describe("parseIngredients", () => {
  it("reads amount, unit and name", () => {
    expect(parseIngredients("2 eggs\n300 g flour\n1,5 l milk\nsalt")).toEqual([
      { qty: 2, unit: "", name: "eggs" },
      { qty: 300, unit: "g", name: "flour" },
      { qty: 1.5, unit: "l", name: "milk" },
      { qty: 1, unit: "", name: "salt" },
    ]);
  });
  it("turns the week on Monday", () => {
    const sun = Date.UTC(2026, 10, 8, 12);
    const mon = Date.UTC(2026, 10, 9, 12);
    expect(weekIndex(mon)).toBe(weekIndex(sun) + 1);
  });
});
