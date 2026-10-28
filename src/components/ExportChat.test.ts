// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { Chat, Message } from "../api";
import { chatPage } from "./ExportChat";
import { markdownReady } from "./Markdown";

// The page takes its language from the window's document.
(globalThis as { document?: unknown }).document ??= { documentElement: { lang: "en" } };

describe("a chat saved as a page", () => {
  it("has its math and code rendered, not left as text", async () => {
    const chat = { id: "c", title: "Circles", created_at: 0 } as Chat;
    const reply = "The area is $$A = \\pi r^2$$\n\n```python\nprint(3.14)\n```";
    const messages = [
      { id: "u", role: "user", content: "How big is a circle?", created_at: 0 },
      { id: "a", role: "assistant", content: reply, created_at: 0 },
    ] as Message[];
    await markdownReady(messages.map((m) => m.content));
    const page = chatPage(chat, messages);
    expect(page).toContain('class="katex');
    expect(page).toContain("hljs-");
    expect(page).toContain("katex.min.css");
    expect(page).toContain("How big is a circle?");
  });
});
