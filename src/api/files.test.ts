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

import { parseSteps } from "./files";

describe("parseSteps", () => {
  it("reads one step per line", () => {
    expect(parseSteps("Total of Cost by Room\nrows where Item contains lamp\nSummary of Cost\nnonsense")).toEqual([
      { step: "group_total", column: "Cost", by: "Room" },
      { step: "filter", column: "Item", contains: "lamp" },
      { step: "summary", column: "Cost" },
    ]);
  });
});
