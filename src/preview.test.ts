// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { Message } from "./api";
import { latestPreview } from "./components/PreviewPanel";

const tool = (id: string, meta: Message["meta"]): Message => ({ id, chat_id: "c", role: "tool", content: "", created_at: 0, meta }) as Message;

describe("site preview", () => {
  it("shows the newest site this chat previewed", () => {
    const messages = [
      tool("1", { title: "Previewed old", status: "ok", kind: "text", preview_url: "http://127.0.0.1:5001/index.html", preview_site: "a", preview_root: "C:/s/old" }),
      tool("2", { title: "Wrote x", status: "ok", kind: "text", file: "C:/s/new/index.html" }),
      tool("3", { title: "Previewed new", status: "ok", kind: "text", preview_url: "http://127.0.0.1:5002/index.html", preview_site: "b", preview_root: "C:/s/new", preview_start: "index.html" }),
      tool("4", { title: "Couldn't preview port 9", status: "error", kind: "text", preview_url: "http://localhost:9/" }),
    ];
    expect(latestPreview(messages)).toEqual({ url: "http://127.0.0.1:5002/index.html", site: "b", root: "C:/s/new", start: "index.html", port: undefined });
    expect(latestPreview(messages.slice(1, 2))).toBeNull();
  });
});
