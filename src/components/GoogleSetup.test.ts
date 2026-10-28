// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { CloudModel } from "../api";
import { aiModeUrl, firstGemini } from "./GoogleSetup";

const model = (id: string) => ({ id, name: id, ctx: 0, max_output: 0, vision: true, price_in: null, price_out: null }) as CloudModel;

describe("Google in the model list", () => {
  it("starts with a plain Flash model", () => {
    const listed = ["gemini-2.5-pro", "gemini-2.5-flash-lite", "gemini-2.5-flash-image", "gemini-3-flash-preview", "gemini-2.5-flash", "text-embedding-004"].map(model);
    expect(firstGemini(listed)?.id).toBe("gemini-2.5-flash");
    expect(firstGemini(["gemini-2.5-pro"].map(model))?.id).toBe("gemini-2.5-pro");
    expect(firstGemini([])).toBeUndefined();
  });

  it("asks Google's AI Mode with the message as written", () => {
    expect(aiModeUrl("  what's 2+2 & why?  ")).toBe("https://www.google.com/search?udm=50&q=what's%202%2B2%20%26%20why%3F");
  });
});
