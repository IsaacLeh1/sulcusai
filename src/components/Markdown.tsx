// SPDX-License-Identifier: AGPL-3.0-only
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

/** Model output as Markdown. Raw HTML is never rendered, and links can't navigate the app window. */
export function Markdown({ text }: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          a: ({ children, href }) => (
            <span className="md-link" title={href}>
              {children}
            </span>
          ),
          img: ({ alt }) => <span className="muted">[image: {alt}]</span>,
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}
