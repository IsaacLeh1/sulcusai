// SPDX-License-Identifier: AGPL-3.0-only
import { isValidElement, memo, useEffect, useRef, useState, type ReactNode } from "react";
import ReactMarkdown, { type Components, type Options } from "react-markdown";
import { api } from "../api";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import { t, useLanguage } from "../i18n";
import { mathify } from "../math";
import { CopyButton } from "./CopyButton";

const REMARK: Options["remarkPlugins"] = [remarkGfm, [remarkMath, { singleDollarTextMath: false }]];

// KaTeX and the code colours are most of the window's startup code, and
// most replies need neither, so they load the first time one does.
type Plugin = NonNullable<Options["rehypePlugins"]>[number];
const plugins: { math?: Plugin; code?: Plugin } = {};
const loading: { math?: Promise<void>; code?: Promise<void> } = {};
const needsMath = (md: string) => md.includes("$$");
// Code is coloured only when the reply names its language (guessing it
// was often wrong, and slow on a reply that's still coming).
const needsCode = (md: string) => /```[\w+#.-]/.test(md);

function load(kind: "math" | "code"): Promise<void> {
  loading[kind] ??=
    kind === "math"
      ? Promise.all([import("rehype-katex"), import("katex/dist/katex.min.css")]).then(([m]) => {
          plugins.math = [m.default, { throwOnError: false, strict: "ignore" }];
        })
      : import("rehype-highlight").then((m) => {
          plugins.code = [m.default, { detect: false }];
        });
  return loading[kind];
}

/** Loads what these texts need to show fully, for rendering them all at once (a saved page). */
export async function markdownReady(texts: string[]): Promise<void> {
  const md = texts.map(mathify);
  await Promise.all([md.some(needsMath) && load("math"), md.some(needsCode) && load("code")]);
}

/** The text inside a parsed element (a code block's code). */
function textOf(node: unknown): string {
  const n = node as { type?: string; value?: string; children?: unknown[] } | undefined;
  if (!n) return "";
  if (n.type === "text") return n.value ?? "";
  return (n.children ?? []).map(textOf).join("");
}

// A preview runs on its own: scripts yes, but nothing from the network and
// nothing of the app's (no same origin), so a page a model wrote can't send
// anything anywhere or reach SulcusAI.
const SANDBOX_CSP = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:";

function previewDoc(code: string, lang: string): string {
  const csp = `<meta http-equiv="Content-Security-Policy" content="${SANDBOX_CSP}">`;
  if (lang === "svg" || lang === "xml") {
    return `<!doctype html><html><head>${csp}<style>html,body{margin:0;height:100%;background:#fff}body>svg{display:block;width:100%;height:100%}</style></head><body>${code}</body></html>`;
  }
  // The policy goes first, before anything the page itself has.
  return /<head[^>]*>/i.test(code) ? code.replace(/<head[^>]*>/i, (h) => `${h}${csp}`) : `${csp}${code}`;
}

/** A code block with its language and a button that copies it; HTML and SVG can be seen as they'd show. */
function CodeBlock({ children, code }: { children?: ReactNode; code: string }) {
  const pre = useRef<HTMLPreElement>(null);
  const cls = isValidElement<{ className?: string }>(children) ? children.props.className ?? "" : "";
  const lang = /language-([\w+#.-]+)/.exec(cls)?.[1]?.toLowerCase();
  const previewable = lang === "html" || lang === "svg" || (lang === "xml" && /^\s*<svg[\s>]/i.test(code));
  const [preview, setPreview] = useState(false);
  const [tall, setTall] = useState(false);
  return (
    <div className="code-block">
      <div className="code-head">
        <span>{lang ?? ""}</span>
        <span className="code-actions">
          {previewable && (
            <button className="link small" onClick={() => setPreview((p) => !p)} aria-pressed={preview}>
              {preview ? t("Code") : t("Preview")}
            </button>
          )}
          {preview && (
            <button className="link small" onClick={() => setTall((x) => !x)}>
              {tall ? t("Smaller") : t("Bigger")}
            </button>
          )}
          <CopyButton get={() => code.replace(/\n$/, "")} label={t("Copy the code")} />
        </span>
      </div>
      {preview && lang ? (
        <iframe className={`code-preview ${tall ? "tall" : ""}`} sandbox="allow-scripts allow-modals" srcDoc={previewDoc(code, lang)} title={t("Preview of the code")} />
      ) : (
        <pre ref={pre}>{children}</pre>
      )}
    </div>
  );
}

const COMPONENTS: Components = {
  pre: ({ children, node }) => <CodeBlock code={textOf(node)}>{children}</CodeBlock>,
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
  const md = mathify(text);
  const math = needsMath(md);
  const code = needsCode(md);
  const [, loaded] = useState(0);
  useEffect(() => {
    const waiting = [math && !plugins.math && load("math"), code && !plugins.code && load("code")].filter(Boolean);
    if (waiting.length) Promise.all(waiting).then(() => loaded((n) => n + 1), () => {});
  }, [math, code]);
  const rehype = [math && plugins.math, code && plugins.code].filter((p): p is Plugin => !!p);
  return (
    <div className="md">
      <ReactMarkdown remarkPlugins={REMARK} rehypePlugins={rehype} components={COMPONENTS}>
        {md}
      </ReactMarkdown>
    </div>
  );
});
