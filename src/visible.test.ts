// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { everyWhileVisible } from "./visible";

function fakeDoc() {
  const listeners = new Set<() => void>();
  return {
    hidden: false,
    addEventListener: (_: string, f: () => void) => listeners.add(f),
    removeEventListener: (_: string, f: () => void) => listeners.delete(f),
    set(hidden: boolean) {
      this.hidden = hidden;
      listeners.forEach((f) => f());
    },
    listeners,
  };
}

describe("everyWhileVisible", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("ticks while shown, stops while hidden, and catches up on showing", () => {
    const doc = fakeDoc();
    const fn = vi.fn();
    const stop = everyWhileVisible(fn, 1000, doc as unknown as Document);
    vi.advanceTimersByTime(3000);
    expect(fn).toHaveBeenCalledTimes(3);
    doc.set(true);
    vi.advanceTimersByTime(60_000);
    expect(fn).toHaveBeenCalledTimes(3);
    doc.set(false);
    expect(fn).toHaveBeenCalledTimes(4);
    vi.advanceTimersByTime(1000);
    expect(fn).toHaveBeenCalledTimes(5);
    stop();
    vi.advanceTimersByTime(10_000);
    expect(fn).toHaveBeenCalledTimes(5);
    expect(doc.listeners.size).toBe(0);
  });

  it("starts with no timer when the window is already hidden", () => {
    const doc = fakeDoc();
    doc.hidden = true;
    const fn = vi.fn();
    everyWhileVisible(fn, 1000, doc as unknown as Document);
    vi.advanceTimersByTime(10_000);
    expect(fn).not.toHaveBeenCalled();
  });
});
