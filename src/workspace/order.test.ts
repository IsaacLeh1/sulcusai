// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { arrange, half, placed } from "./order";

describe("order", () => {
  it("keeps the usual order when nothing was dragged", () => {
    const items = [{ id: "a" }, { id: "b" }];
    expect(arrange(items, [])).toBe(items);
  });
  it("puts new items first and placed ones in the dragged order", () => {
    const items = [{ id: "a" }, { id: "b" }, { id: "c" }];
    expect(arrange(items, ["b", "a"]).map((x) => x.id)).toEqual(["c", "b", "a"]);
  });
  it("places an id before or after a target", () => {
    expect(placed(["a", "b", "c"], "c", "a", "before")).toEqual(["c", "a", "b"]);
    expect(placed(["a", "b", "c"], "a", "b", "after")).toEqual(["b", "a", "c"]);
    expect(placed(["a", "b"], "a", "zzz", "after")).toEqual(["a", "b"]);
  });
  it("tells the half of an item a drag is over", () => {
    const el = { getBoundingClientRect: () => ({ top: 0, height: 10 }) } as unknown as Element;
    expect(half({ clientY: 2, currentTarget: el })).toBe("before");
    expect(half({ clientY: 8, currentTarget: el })).toBe("after");
  });
});
