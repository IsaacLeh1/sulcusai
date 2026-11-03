// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { busyOf, runningText } from "./Quiet";

describe("pausing with work running", () => {
  it("reads what's running from the core's answer", () => {
    expect(busyOf("busy:a reply, a download")).toBe("a reply, a download");
    expect(busyOf("Something else went wrong")).toBeNull();
  });

  it("keeps each part of the list, known or not", () => {
    expect(runningText("a reply, a download")).toBe("a reply, a download");
    expect(runningText("a reply, something new")).toBe("a reply, something new");
  });
});
