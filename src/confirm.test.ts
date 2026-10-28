// SPDX-License-Identifier: AGPL-3.0-only
import { expect, it } from "vitest";

// The phone runs in a plain browser, where window.confirm waits as usual.
const sources = import.meta.glob<string>(["./**/*.{ts,tsx}", "!./phone/**", "!./**/*.test.ts", "!./confirm.ts"], { query: "?raw", import: "default", eager: true });

it("the window asks with confirmed(), never a bare window.confirm", () => {
  expect(Object.keys(sources).length).toBeGreaterThan(50);
  const bare = Object.entries(sources).filter(([, text]) => /window\.confirm\(/.test(text)).map(([path]) => path);
  expect(bare).toEqual([]);
});
