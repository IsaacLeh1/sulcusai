// SPDX-License-Identifier: AGPL-3.0-only
import ReactMarkdown from "react-markdown";
import { api } from "../api";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import rehypeKatex from "rehype-katex";
import "katex/dist/katex.min.css";
import { t } from "../i18n";
import { mathify } from "../math";

/** Model output as Markdown, with math formulas typeset. Raw HTML is never rendered, and
 *  links never navigate the app window: web links open in the user's main browser,
 *  anything else is just text. */
export function Markdown({ text }: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown
        remarkPlugins={[remarkGfm, [remarkMath, { singleDollarTextMath: false }]]}
        rehypePlugins={[[rehypeKatex, { throwOnError: false, strict: "ignore" }]]}
        components={{
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
        }}
      >
        {mathify(text)}
      </ReactMarkdown>
    </div>
  );
}
