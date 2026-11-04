// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { looksShared, sizeText } from "./sharing";

describe("sharing helpers", () => {
  it("spots links and .sulcus files", () => {
    expect(looksShared(" sulcus://share/abc ")).toBe(true);
    expect(looksShared('{"v":1,"link":{},"manifest":{}}')).toBe(true);
    expect(looksShared("https://example.com")).toBe(false);
  });
  it("writes sizes", () => {
    expect(sizeText(500)).toBe("500 B");
    expect(sizeText(1536)).toBe("1.5 KB");
    expect(sizeText(3 * 1024 ** 3)).toBe("3.0 GB");
  });
});
