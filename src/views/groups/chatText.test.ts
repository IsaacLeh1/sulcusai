// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { blocks, inline, linkKind, linksIn, slash } from "./chatText";
import { searchEmoji, shortcodes } from "./emoji";

describe("chat markdown", () => {
  it("reads bold, italic, code, spoilers and links", () => {
    expect(inline("**hi** *there* `x` ||secret|| https://example.com/a.")).toEqual([
      { k: "bold", c: [{ k: "text", v: "hi" }] },
      { k: "text", v: " " },
      { k: "italic", c: [{ k: "text", v: "there" }] },
      { k: "text", v: " " },
      { k: "code", v: "x" },
      { k: "text", v: " " },
      { k: "spoiler", c: [{ k: "text", v: "secret" }] },
      { k: "text", v: " " },
      { k: "link", href: "https://example.com/a", v: "https://example.com/a" },
      { k: "text", v: "." },
    ]);
  });
  it("leaves snake_case and escapes alone", () => {
    expect(inline("snake_case_name")).toEqual([{ k: "text", v: "snake_case_name" }]);
    expect(inline("\\*not\\*")).toEqual([{ k: "text", v: "*not*" }]);
  });
  it("splits code blocks, quotes and lists", () => {
    const b = blocks("> quoted\n- item\n```js\nlet a = 1;\n```");
    expect(b.map((x) => x.k)).toEqual(["quote", "list", "code"]);
    expect(b[2]).toEqual({ k: "code", lang: "js", v: "let a = 1;" });
  });
  it("finds links once each", () => {
    expect(linksIn("see https://a.com and [b](https://b.com) and https://a.com")).toEqual(["https://a.com", "https://b.com"]);
  });
  it("tells media links apart", () => {
    expect(linkKind("https://x.com/a.mp4").k).toBe("video");
    expect(linkKind("https://x.com/a.PNG").k).toBe("image");
    expect(linkKind("https://youtu.be/dQw4w9WgXcQ")).toEqual({ k: "youtube", id: "dQw4w9WgXcQ" });
    expect(linkKind("https://youtu.be/dQw4w9WgXcQ/")).toEqual({ k: "youtube", id: "dQw4w9WgXcQ" });
    expect(linkKind("https://youtu.be/dQw4w9WgXcQ?si=abcdef12345")).toEqual({ k: "youtube", id: "dQw4w9WgXcQ" });
    expect(linkKind("https://www.youtube.com/watch?v=dQw4w9WgXcQ")).toEqual({ k: "youtube", id: "dQw4w9WgXcQ" });
    expect(linkKind("https://www.youtube.com/shorts/dQw4w9WgXcQ")).toEqual({ k: "youtube", id: "dQw4w9WgXcQ" });
    expect(linkKind("https://example.com/").k).toBe("page");
  });
  it("runs slash commands", () => {
    expect(slash("/spoiler boo")).toEqual({ text: "||boo||" });
    expect(slash("/gif cats")).toEqual({ gif: "cats" });
    expect(slash("hello")).toEqual({ text: "hello" });
    expect("error" in (slash("/nope") ?? {})).toBe(true);
    const shrug = slash("/shrug");
    expect(shrug && "text" in shrug && inline(shrug.text)).toEqual([{ k: "text", v: "¯\\_(ツ)_/¯" }]);
  });
});

describe("emoji", () => {
  it("finds by words and shortcodes", () => {
    expect(searchEmoji("thumbs up")).toContain("👍");
    expect(shortcodes("nice :fire: :nothing_here:")).toBe("nice 🔥 :nothing_here:");
  });
});
