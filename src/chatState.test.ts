// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { PendingApproval } from "./api";
import { drawn, held, onDelta, onStart, onStatus, QUIET, startStreaming, withApproval, withoutApproval } from "./chatState";

describe("the reply's text", () => {
  it("adds deltas in order, held and then drawn", () => {
    let s = startStreaming(null, "m1", null);
    let h = held(null, "m1", "Hel", "");
    h = held(h, "m1", "lo", "hmm");
    s = drawn(s, h)!;
    s = drawn(s, held(null, "m1", "!", ""))!;
    expect(s).toEqual({ id: "m1", content: "Hello!", thinking: "hmm" });
  });

  it("starts a new bubble for a new message, and keeps one tried again", () => {
    const first = { id: "m1", content: "partial", thinking: "" };
    expect(startStreaming(first, "m2", null)).toEqual({ id: "m2", content: "", thinking: "" });
    // The same step again: what it wrote stays, with what was waiting to be drawn.
    expect(startStreaming(first, "m1", { id: "m1", content: " more", thinking: "" })).toEqual({ id: "m1", content: "partial more", thinking: "" });
    // Text for another message isn't drawn into this one.
    expect(drawn(first, { id: "m2", content: "x", thinking: "" })).toBe(first);
  });
});

describe("where the reply is", () => {
  it("shows a busy provider's retry, then clears it on the next delta", () => {
    let v = onStatus(QUIET, { chat_id: "c", status: "retrying", provider: "Anthropic", seconds: 4, attempt: 1, of: 4 }, 1_000, "the PC");
    expect(v.retrying).toEqual({ provider: "Anthropic", at: 5_000, attempt: 1, of: 4 });
    v = onDelta(v, { content: "Hi" });
    expect(v.retrying).toBeNull();
    expect(v.status).toBe("writing");
    // Thinking only: still writing nothing, the status stays.
    expect(onDelta({ ...QUIET, status: "thinking" }, { content: null }).status).toBe("thinking");
  });

  it("waits in line, cools down, and starts clean", () => {
    let v = onStatus(QUIET, { chat_id: "c", status: "queued", place: 2, detail: "Qwen" }, 0, "the PC");
    expect(v.queued).toEqual({ place: 2, model: "Qwen" });
    v = onStatus(v, { chat_id: "c", status: "cooling" }, 0, "the PC");
    expect([v.cooling, v.status, v.queued]).toEqual(["the PC", "working", null]);
    v = onStart({ ...v, lookingUp: "Mario", readingPictures: true });
    expect(v).toEqual({ ...QUIET, status: "thinking" });
    v = onStatus({ ...v, compacting: true }, { chat_id: "c", status: "idle" }, 0, "the PC");
    expect(v.compacting).toBe(false);
  });
});

describe("approval cards", () => {
  const card = (call: string): PendingApproval => ({ chat_id: "c", call_id: call, tool: "write_file", risk: "write", preview: { title: call, kind: "text", detail: null, note: null } });

  it("opens each once, and closes the right one when two are waiting", () => {
    let all = withApproval([], card("a"));
    all = withApproval(all, card("b"));
    all = withApproval(all, card("a"));
    expect(all.map((p) => p.call_id)).toEqual(["b", "a"]);
    all = withoutApproval(all, "b");
    expect(all.map((p) => p.call_id)).toEqual(["a"]);
  });
});
