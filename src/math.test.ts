// SPDX-License-Identifier: AGPL-3.0-only
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import rehypeKatex from "rehype-katex";
import remarkMath from "remark-math";
import { describe, expect, it } from "vitest";
import { mathify } from "./math";

const render = (text: string) =>
  renderToStaticMarkup(
    createElement(ReactMarkdown, { remarkPlugins: [[remarkMath, { singleDollarTextMath: false }]], rehypePlugins: [[rehypeKatex, { throwOnError: false }]] }, mathify(text)),
  );

describe("math", () => {
  it("reads formulas the way models write them", () => {
    expect(mathify("The area is \\(\\pi r^2\\).")).toBe("The area is $$\\pi r^2$$.");
    expect(mathify("So \\[\\int_0^3 x^2\\,dx = 9\\]")).toBe("So \n$$\n\\int_0^3 x^2\\,dx = 9\n$$\n");
    expect(mathify("where $x^2 + y^2 = z^2$ holds")).toBe("where $$x^2 + y^2 = z^2$$ holds");
    expect(mathify("$$E = mc^2$$")).toBe("$$\nE = mc^2\n$$");
    expect(mathify("let $n$ be the count")).toBe("let $$n$$ be the count");
  });

  it("leaves money and code alone", () => {
    for (const s of ["It costs $5 and $10 later.", "Between $1,200 and $1,500", "Save $20 today", "`echo $HOME $PATH`", "```sh\nexport A=$B^2\n```"]) {
      expect(mathify(s)).toBe(s);
    }
  });

  it("typesets inline and display math", () => {
    const html = render("The roots are \\(x = \\frac{-b \\pm \\sqrt{b^2-4ac}}{2a}\\).\n\n\\[\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}\\]");
    expect(html).toContain('class="katex"');
    expect(html).toContain("katex-display");
    expect(html).toContain("katex-html");
    expect(html).not.toContain("katex-error");
    // Money stays text.
    expect(render("It costs $5 and $10.")).not.toContain("katex");
  });
});
