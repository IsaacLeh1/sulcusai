// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

import { fixRequest } from "./ScheduledView";

describe("fixing a failed task", () => {
  it("tells the assistant what failed, when it runs and what it does", () => {
    const text = fixRequest({ id: "1", name: "Morning brief", prompt: "Sum up my mail", cadence: { kind: "on_start" }, model_id: null, project_id: null, allow_changes: false, actions: [], enabled: true, last_run: 1, next_run: null, last_chat: null, last_error: "The model isn't installed.", when: "When SulcusAI starts" });
    expect(text).toContain("Morning brief");
    expect(text).toContain("The model isn't installed.");
    expect(text).toContain("When SulcusAI starts");
    expect(text).toContain("Sum up my mail");
  });
});
