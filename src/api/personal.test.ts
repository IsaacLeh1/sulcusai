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
