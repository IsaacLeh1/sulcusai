import { describe, expect, it } from "vitest";
import type { Message } from "./api";
import { lowTokenSavings } from "./lowTokens";

const m = (inp: number, out: number, low?: boolean) =>
  ({ id: String(Math.random()), role: "assistant", content: "x", meta: { usage: { in: inp, out, thinking: null, cache_read: null, source: "provider" }, ...(low ? { low_tokens: true } : {}) } }) as unknown as Message;

describe("lowTokenSavings", () => {
  it("is null without low token replies", () => {
    expect(lowTokenSavings([m(100, 100)])).toBeNull();
  });
  it("compares per step against normal replies", () => {
    const s = lowTokenSavings([m(800, 200), m(300, 100, true)])!;
    expect(s.low).toBe(400);
    expect(s.normal).toBe(1000);
    expect(s.saved).toBeCloseTo(0.6);
  });
  it("has no share with nothing to compare", () => {
    expect(lowTokenSavings([m(10, 10, true)])!.saved).toBeNull();
  });
});
