// SPDX-License-Identifier: AGPL-3.0-only
import { renderToStaticMarkup } from "react-dom/server";
import { save } from "@tauri-apps/plugin-dialog";
import { api, errorText, type Chat, type Message } from "../api";
import { t } from "../i18n";
import { Markdown } from "./Markdown";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

// A saved chat reads on its own: light or dark with the reader's system,
// code coloured, nothing loaded from elsewhere (math fonts aside).
const PAGE_CSS = `
:root { color-scheme: light dark; --text: #17151f; --muted: #625f72; --bg: #fff; --code: #f1f0f6; --bubble: #eeebfd; --border: #e2e0ea;
  --kw: #6a3fd8; --str: #12704a; --com: #85829a; --num: #a85400; --fn: #1d5fbf; --attr: #a6346a; }
@media (prefers-color-scheme: dark) { :root { --text: #ecebf3; --muted: #a09db0; --bg: #121119; --code: #22212c; --bubble: #29244a; --border: #2b2938;
  --kw: #b7a8ff; --str: #7fd8a8; --com: #807d90; --num: #f0b45a; --fn: #8fb8ff; --attr: #f28fb4; } }
body { margin: 0; background: var(--bg); color: var(--text); font: 15px/1.6 "Segoe UI", system-ui, sans-serif; }
main { max-width: 760px; margin: 0 auto; padding: 32px 20px 64px; }
h1 { font-size: 1.6em; margin: 0 0 4px; }
.when { color: var(--muted); font-size: 0.9em; margin: 0 0 28px; }
.who { color: var(--muted); font-size: 0.8em; font-weight: 600; text-transform: uppercase; letter-spacing: 0.04em; margin: 22px 0 6px; }
.you { background: var(--bubble); padding: 10px 14px; border-radius: 12px; white-space: pre-wrap; overflow-wrap: anywhere; }
pre { background: var(--code); padding: 12px; border-radius: 0 0 8px 8px; overflow-x: auto; font-size: 13px; margin: 0; }
code { font-family: "Cascadia Code", Consolas, monospace; font-size: 0.92em; }
:not(pre) > code { background: var(--code); padding: 1px 5px; border-radius: 4px; }
.code-block { margin: 0 0 10px; }
.code-head { background: var(--code); border-radius: 8px 8px 0 0; padding: 4px 12px 0; font-size: 12px; color: var(--muted); min-height: 8px; }
.code-head button { display: none; }
table { border-collapse: collapse; } th, td { border: 1px solid var(--border); padding: 5px 10px; }
blockquote { margin: 0; padding-left: 12px; border-left: 3px solid var(--border); color: var(--muted); }
a { color: var(--fn); }
.hljs-keyword, .hljs-built_in, .hljs-section, .hljs-name, .hljs-selector-tag { color: var(--kw); }
.hljs-string, .hljs-regexp, .hljs-symbol, .hljs-bullet, .hljs-addition { color: var(--str); }
.hljs-comment, .hljs-quote, .hljs-meta { color: var(--com); font-style: italic; }
.hljs-number, .hljs-literal, .hljs-deletion { color: var(--num); }
.hljs-title, .hljs-type { color: var(--fn); }
.hljs-attr, .hljs-attribute, .hljs-property, .hljs-params, .hljs-tag { color: var(--attr); }
@media print { body { background: #fff; color: #000; } main { padding: 0; } }
`;

const when = (ms: number) => new Date(ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });

/** The chat as a page of its own. */
export function chatPage(chat: Chat, messages: Message[]): string {
  const body = renderToStaticMarkup(
    <main>
      <h1>{chat.title}</h1>
      <p className="when">{t("Started {when}", { when: when(chat.created_at) })}</p>
      {messages
        .filter((m) => (m.role === "user" || m.role === "assistant") && m.content.trim() && !m.meta?.compacted)
        .map((m) => (
          <section key={m.id}>
            <div className="who">{m.role === "user" ? t("You") + " · " + when(m.created_at) : t("Assistant")}</div>
            {m.role === "user" ? <div className="you">{m.content}</div> : <Markdown text={m.content} />}
          </section>
        ))}
    </main>,
  );
  const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  const math = body.includes('class="katex') ? '<link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/katex@0.16.22/dist/katex.min.css">' : "";
  return `<!doctype html>\n<html lang="${document.documentElement.lang || "en"}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>${esc(chat.title)}</title><style>${PAGE_CSS}</style>${math}</head><body>${body}</body></html>\n`;
}

/** File names can't have some characters. */
const fileName = (title: string) => title.replace(/[<>:"/\\|?*\u0000-\u001f]/g, "-").trim().slice(0, 80) || t("Chat");

/** Saves a chat as Markdown or a web page, or prints it (to paper or a PDF). */
export function ExportChat({ chat, messages, onClose, toast }: { chat: Chat; messages: Message[]; onClose: () => void; toast: PushToast }) {
  const saveAs = async (ext: "md" | "html") => {
    const path = await save({
      defaultPath: `${fileName(chat.title)}.${ext}`,
      filters: [ext === "md" ? { name: t("Markdown"), extensions: ["md"] } : { name: t("Web page"), extensions: ["html"] }],
    });
    if (!path) return;
    try {
      await api.exportChat(chat.id, path, ext === "html" ? chatPage(chat, messages) : undefined);
      toast(t("Saved the chat."), "success");
      onClose();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  // Printing the page by itself, out of sight: the print window can save it as a PDF.
  const print = () => {
    const frame = document.createElement("iframe");
    frame.style.cssText = "position:fixed;width:0;height:0;border:0;right:0;bottom:0";
    frame.srcdoc = chatPage(chat, messages);
    frame.onload = () => {
      frame.contentWindow?.focus();
      frame.contentWindow?.print();
      setTimeout(() => frame.remove(), 60_000);
    };
    document.body.appendChild(frame);
    onClose();
  };
  return (
    <Modal title={t("Save this chat")} onClose={onClose}>
      <p className="muted small">{t("Your messages and the replies, as they read now. Tool steps and earlier versions aren't included.")}</p>
      <div className="export-choices">
        <button className="btn" onClick={() => saveAs("md")}>
          <strong>{t("Markdown")}</strong>
          <span className="small muted block">{t("Plain text with formatting marks, for notes apps and editors")}</span>
        </button>
        <button className="btn" onClick={() => saveAs("html")}>
          <strong>{t("Web page")}</strong>
          <span className="small muted block">{t("Opens in any browser, with code coloured and math laid out")}</span>
        </button>
        <button className="btn" onClick={print}>
          <strong>{t("Print or save as PDF")}</strong>
          <span className="small muted block">{t("Choose Microsoft Print to PDF or Save as PDF as the printer")}</span>
        </button>
      </div>
      <div className="modal-actions">
        <button className="btn" onClick={onClose}>{t("Cancel")}</button>
      </div>
    </Modal>
  );
}
