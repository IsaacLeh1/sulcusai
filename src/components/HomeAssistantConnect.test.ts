// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { homeAssistantUrl } from "./HomeAssistantConnect";

describe("Home Assistant address", () => {
  it("points at its MCP server, over https unless typed otherwise", () => {
    expect(homeAssistantUrl("abc123.ui.nabu.casa")).toBe("https://abc123.ui.nabu.casa/api/mcp");
    expect(homeAssistantUrl(" https://ha.example.com/ ")).toBe("https://ha.example.com/api/mcp");
    expect(homeAssistantUrl("https://ha.example.com/api/mcp")).toBe("https://ha.example.com/api/mcp");
    expect(homeAssistantUrl("http://localhost:8123")).toBe("http://localhost:8123/api/mcp");
  });
});
