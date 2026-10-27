// SPDX-License-Identifier: AGPL-3.0-only
import { isValidElement, memo, useRef, type ReactNode } from "react";
import ReactMarkdown, { type Components, type Options } from "react-markdown";
import { api } from "../api";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import rehypeKatex from "rehype-katex";
import rehypeHighlight from "rehype-highlight";
import "katex/dist/katex.min.css";
import { t, useLanguage } from "../i18n";
import { mathify } from "../math";
import { CopyButton } from "./CopyButton";

const REMARK: Options["remarkPlugins"] = [remarkGfm, [remarkMath, { singleDollarTextMath: false }]];
// Code is coloured only when the reply names its language (guessing it
// was often wrong, and slow on a reply that's still coming).
const REHYPE: Options["rehypePlugins"] = [[rehypeKatex, { throwOnError: false, strict: "ignore" }], [rehypeHighlight, { detect: false }]];

/** A code block with its language and a button that copies it. */
function CodeBlock({ children }: { children?: ReactNode }) {
  const pre = useRef<HTMLPreElement>(null);
  const cls = isValidElement<{ className?: string }>(children) ? children.props.className ?? "" : "";
  const lang = /language-([\w+#.-]+)/.exec(cls)?.[1];
  return (
    <div className="code-block">
      <div className="code-head">
        <span>{lang ?? ""}</span>
        <CopyButton get={() => pre.current?.innerText.replace(/\n$/, "") ?? ""} label={t("Copy the code")} />
      </div>
      <pre ref={pre}>{children}</pre>
    </div>
  );
}

const COMPONENTS: Components = {
  pre: ({ children }) => <CodeBlock>{children}</CodeBlock>,
  a: ({ children, href }) =>
    href && /^https?:\/\//i.test(href) ? (
      <a
        className="md-link"
        href={href}
        title={href}
        onClick={(e) => {
          e.preventDefault();
          api.openWeb(href);
        }}
      >
        {children}
      </a>
    ) : href && /^doc:[0-9a-f]+$/i.test(href) ? (
      // A source in the documents library: opens the file.
      <a
        className="md-link doc-link"
        href={href}
        title={t("Open this document")}
        onClick={(e) => {
          e.preventDefault();
          api.docsOpen(href.slice(4)).catch(() => {});
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
