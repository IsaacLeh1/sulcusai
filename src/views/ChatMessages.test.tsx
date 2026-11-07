// SPDX-License-Identifier: AGPL-3.0-only
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { MessageView, renderItems } from "./ChatMessages";
import type { Message } from "../api";

describe("Chat message timestamps", () => {
  const baseUserMsg: Message = {
    id: "u1",
    chat_id: "c1",
    role: "user",
    content: "Explain quantum computing",
    thinking: null,
    created_at: 1730900000000,
  };

  const baseAssistantMsg: Message = {
    id: "a1",
    chat_id: "c1",
    role: "assistant",
    content: "Quantum computing leverages qubits and superposition.",
    thinking: null,
    created_at: 1730900005000,
  };

  it("renders timestamp and read receipt on user message", () => {
    const markup = renderToStaticMarkup(<MessageView m={baseUserMsg} receipt="read" />);
    expect(markup).toContain("msg-time");
    expect(markup).toContain("Read");
    expect(markup).toContain("✓✓");
  });

  it("renders reply timestamp and duration on assistant message", () => {
    const markup = renderToStaticMarkup(<MessageView m={baseAssistantMsg} durationMs={5000} />);
    expect(markup).toContain("msg-meta-row");
    expect(markup).toContain("msg-time assistant");
    expect(markup).toContain("5.0s");
  });

  it("calculates turn duration in renderItems", () => {
    const items = renderItems(
      [baseUserMsg, baseAssistantMsg],
      new Set(),
      false,
      new Set(),
      () => {},
      {},
      () => {},
    );
    expect(items.length).toBeGreaterThanOrEqual(2);
    const markup = renderToStaticMarkup(<>{items}</>);
    expect(markup).toContain("5.0s");
  });
});
