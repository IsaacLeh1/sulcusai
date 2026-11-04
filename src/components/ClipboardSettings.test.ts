// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { preview } from "./ClipboardSettings";

describe("clipboard history list", () => {
  it("shows a copy on one short line", () => {
    expect(preview("a\n  b\tc")).toBe("a b c");
    expect(preview("x".repeat(200), 10)).toBe("xxxxxxxxx…");
  });
});
