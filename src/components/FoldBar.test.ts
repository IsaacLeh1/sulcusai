import { describe, expect, it } from "vitest";
import { fold } from "./FoldBar";
import { chatTotal, replyUsage } from "../tokenUsage";
import type { Message } from "../api";

// The chat bar's controls at their real label widths (measured at 100%
// scale in English; longer languages only make the folding start sooner).
const bar = [
  { id: "folder", w: 90 },
  { id: "attach", w: 34, keep: true },
  { id: "web", w: 78, keep: true },
  { id: "model", w: 220, keep: true },
  { id: "cloud", w: 52, keep: true },
  { id: "cost", w: 70, keep: true },
  { id: "manage", w: 120 },
  { id: "side", w: 104 },
  { id: "google", w: 96 },
  { id: "tokens", w: 150 },
  { id: "mic", w: 34 },
  { id: "meter", w: 60, keep: true },
  { id: "send", w: 64, keep: true },
];
const widths = Object.fromEntries(bar.map((i) => [i.id, i.w]));
const GAP = 8;
const MORE = 64;

function used(shown: Set<string>) {
  const folded = bar.some((i) => !shown.has(i.id));
  return bar.filter((i) => shown.has(i.id)).reduce((s, i) => s + i.w + GAP, 0) + (folded ? MORE + GAP : 0);
}

describe("chat bar folding", () => {
  for (const room of [360, 480, 640, 800, 1024, 1280, 1600, 1920]) {
    it(`never wraps or clips at ${room}px`, () => {
      const shown = fold(bar, widths, room, GAP, MORE);
      // Clipping or wrapping happens exactly when the shown controls are wider than the bar.
      const keptWidth = bar.filter((i) => i.keep).reduce((s, i) => s + i.w + GAP, 0) + MORE + GAP;
      if (keptWidth <= room) expect(used(shown)).toBeLessThanOrEqual(room);
      for (const i of bar.filter((b) => b.keep)) expect(shown.has(i.id)).toBe(true);
    });
  }

  it("shows everything with no More menu when it all fits", () => {
    expect(fold(bar, widths, 5000, GAP, MORE).size).toBe(bar.length);
  });

  it("folds from the end of the list first", () => {
    const shown = fold(bar, widths, 900, GAP, MORE);
    expect(shown.has("folder")).toBe(true);
    expect(shown.has("mic")).toBe(false);
  });

  it("the bar's style keeps it on one line", async () => {
    // Read from disk: stylesheet imports come back empty under vitest, and the app's types carry no node modules.
    const fs = (await import(/* @vite-ignore */ "node:" + "fs")) as { readFileSync(p: string, enc: string): string };
    const css = fs.readFileSync("src/styles.css", "utf8");
    expect(css).toMatch(/\.fold-bar \{[^}]*flex-wrap: nowrap/);
    expect(css).toMatch(/\.fold-bar > \.fold-item \{[^}]*flex: none/);
  });
});

const msg = (id: string, role: Message["role"], content: string, usage?: object): Message =>
  ({ id, chat_id: "c", role, content, thinking: null, created_at: 0, meta: usage ? { usage } : undefined }) as Message;

describe("token figures", () => {
  const chat = [
    msg("u1", "user", "find it"),
    msg("a1", "assistant", "", { in: 100, out: 10, thinking: null, cache_read: null, source: "provider" }),
    msg("t1", "tool", "result"),
    msg("a2", "assistant", "found", { in: 150, out: 20, thinking: 5, cache_read: null, source: "provider" }),
    msg("u2", "user", "thanks"),
    msg("a3", "assistant", "ok", { in: 200, out: 3, thinking: null, cache_read: null, source: "estimate", model: "Small" }),
  ];

  it("sums a reply's tool steps into one figure on its last message", () => {
    const r = replyUsage(chat);
    expect(r.get("a1")).toBeUndefined();
    expect(r.get("a2")).toMatchObject({ in: 250, out: 30, thinking: 5, steps: 2, source: "provider" });
    expect(r.get("a3")).toMatchObject({ in: 200, out: 3, steps: 1, source: "estimate" });
  });

  it("totals the chat", () => {
    expect(chatTotal(chat)).toMatchObject({ in: 450, out: 33, estimate: true });
    expect(chatTotal(chat)?.byModel).toEqual([
      ["", { in: 250, out: 30, estimate: false }],
      ["Small", { in: 200, out: 3, estimate: true }],
    ]);
    expect(chatTotal([msg("u", "user", "hi")])).toBeNull();
  });
});
