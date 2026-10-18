// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it, vi } from "vitest";

const read: string[] = [];
vi.mock("./api", () => ({
  api: {
    mediaFile: async (id: string) => (read.push(id), new ArrayBuffer(4)),
    mediaThumb: async (id: string) => (read.push(`thumb ${id}`), new ArrayBuffer(4)),
  },
}));

const { fileUrl } = await import("./media");
const item = (id: string) => ({ id, mime: "video/webm" });

describe("gallery file cache", () => {
  it("keeps the newest few full files and lets the oldest go", async () => {
    const revoked: string[] = [];
    const revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation((u) => void revoked.push(u));
    const first = await fileUrl(item("a"));
    for (const id of ["b", "c", "d", "e", "f", "g", "h"]) await fileUrl(item(id));
    // Eight kept: "a" again comes from the cache (and is now the newest).
    expect(await fileUrl(item("a"))).toBe(first);
    expect(read.filter((r) => r === "a")).toHaveLength(1);
    // A ninth lets the oldest unused one ("b") go.
    await fileUrl(item("i"));
    await Promise.resolve();
    expect(revoked).toHaveLength(1);
    await fileUrl(item("b"));
    expect(read.filter((r) => r === "b")).toHaveLength(2);
    expect(await fileUrl(item("a"))).toBe(first);
    revoke.mockRestore();
  });
});
