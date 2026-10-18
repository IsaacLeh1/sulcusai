// SPDX-License-Identifier: AGPL-3.0-only
import { memo } from "react";
import ReactMarkdown, { type Components, type Options } from "react-markdown";
import { api } from "../api";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import rehypeKatex from "rehype-katex";
import "katex/dist/katex.min.css";
import { t, useLanguage } from "../i18n";
import { mathify } from "../math";

const REMARK: Options["remarkPlugins"] = [remarkGfm, [remarkMath, { singleDollarTextMath: false }]];
const REHYPE: Options["rehypePlugins"] = [[rehypeKatex, { throwOnError: false, strict: "ignore" }]];
const COMPONENTS: Components = {
  a: ({ children, href }) =>
    href && /^https?:\/\//i.test(href) ? (
      <a
        className="md-link"
        href={href}
        title={href}
        onClick={(e) => {
          e.preventDefault();
          api.openWeb(href).catch(() => {});
        }}
      >
        {children}
      </a>
    ) : (
      <span className="md-link" title={href}>
        {children}
      </span>
    ),
  img: ({ alt }) => <span className="muted">{t("[image: {alt}]", { alt: alt ?? "" })}</span>,
};

/** Model output as Markdown, with math formulas typeset. Raw HTML is never rendered, and
 *  links never navigate the app window: web links open in the user's main browser,
 *  anything else is just text. Parsed again only when its text changes (a streaming
 *  reply re-renders the chat on every word) or the language does. */
export const Markdown = memo(function Markdown({ text }: { text: string }) {
  useLanguage();
  return (
    <div className="md">
      <ReactMarkdown remarkPlugins={REMARK} rehypePlugins={REHYPE} components={COMPONENTS}>
        {mathify(text)}
      </ReactMarkdown>
    </div>
  );
});
