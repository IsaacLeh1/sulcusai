// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { fraction, looksShared, secondsLeft, sizeText } from "./sharing";

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


describe("remote helpers", () => {
  it("counts down ten seconds", () => {
    expect(secondsLeft(1000, 1000)).toBe(10);
    expect(secondsLeft(1000, 10_500)).toBe(1);
    expect(secondsLeft(1000, 11_000)).toBe(0);
  });
  it("keeps pointer fractions inside the picture", () => {
    const box = { left: 10, top: 20, width: 100, height: 50 };
    expect(fraction(60, 45, box)).toEqual({ x: 0.5, y: 0.5 });
    expect(fraction(0, 999, box)).toEqual({ x: 0, y: 1 });
  });
});
