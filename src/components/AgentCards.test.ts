// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { parseDiffStats } from "./AgentCards";

describe("parseDiffStats", () => {
  it("extracts additions and removals from unified diff format", () => {
    const diff = [
      "--- a/file.txt",
      "+++ b/file.txt",
      "@@ -1,4 +1,5 @@",
      " unchanged",
      "-removed line 1",
      "-removed line 2",
      "+added line 1",
      "+added line 2",
      "+added line 3",
      " unchanged end",
    ].join("\n");
    const stats = parseDiffStats(diff, undefined, "diff");
    expect(stats).toEqual({ added: 3, removed: 2 });
  });

  it("handles diffs with only additions", () => {
    const diff = ["--- a/file.txt", "+++ b/file.txt", "@@ -0,0 +1,2 @@", "+line 1", "+line 2"].join("\n");
    const stats = parseDiffStats(diff, undefined, "diff");
    expect(stats).toEqual({ added: 2, removed: 0 });
  });

  it("falls back to edit_file call arguments when detail is missing", () => {
    const call = {
      id: "call_1",
      name: "edit_file",
      arguments: JSON.stringify({
        path: "test.txt",
        old_text: "line 1\nline 2",
        new_text: "line 1\nline 2 modified\nline 3 added",
      }),
    };
    const stats = parseDiffStats(null, call);
    expect(stats).toEqual({ added: 3, removed: 2 });
  });

  it("falls back to write_file call arguments when detail is missing", () => {
    const call = {
      id: "call_2",
      name: "write_file",
      arguments: JSON.stringify({
        path: "new_file.txt",
        content: "alpha\nbeta\ngamma",
      }),
    };
    const stats = parseDiffStats(null, call);
    expect(stats).toEqual({ added: 3, removed: 0 });
  });

  it("returns null for non-diff text and unrelated tools", () => {
    expect(parseDiffStats("Just regular output text", undefined, "text")).toBeNull();
    const readCall = {
      id: "call_3",
      name: "read_file",
      arguments: JSON.stringify({ path: "readme.md" }),
    };
    expect(parseDiffStats(null, readCall)).toBeNull();
  });
});
