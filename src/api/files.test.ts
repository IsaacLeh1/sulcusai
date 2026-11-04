import { describe, expect, it } from "vitest";
import { snippetParts } from "./files";

describe("snippetParts", () => {
  it("marks the bracketed words", () => {
    expect(snippetParts("the [county] assessor [refund]")).toEqual([
      { text: "the ", hit: false },
      { text: "county", hit: true },
      { text: " assessor ", hit: false },
      { text: "refund", hit: true },
    ]);
    expect(snippetParts("")).toEqual([]);
  });
});
