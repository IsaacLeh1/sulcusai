// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { errorText } from "../api";
import { t } from "../i18n";
import { Markdown } from "./Markdown";
import { Modal } from "./Modal";

/** A quick aside about a chat: answered from its recent messages, never added to it. */
export function SideChat({ chatId, onClose }: { chatId: string; onClose: () => void }) {
  const [question, setQuestion] = useState("");
  const [asked, setAsked] = useState<{ q: string; a: string | null; error?: string }[]>([]);
  const busy = asked.some((x) => x.a === null && !x.error);

  const ask = async () => {
    const q = question.trim();
    if (!q || busy) return;
    setQuestion("");
    setAsked((all) => [...all, { q, a: null }]);
    try {
      const a = await invoke<string>("side_ask", { chatId, question: q });
      setAsked((all) => all.map((x) => (x.q === q && x.a === null ? { ...x, a } : x)));
    } catch (e) {
      setAsked((all) => all.map((x) => (x.q === q && x.a === null ? { ...x, error: errorText(e) } : x)));
    }
  };

  return (
    <Modal title={t("Side question")} onClose={onClose}>
      <p className="small muted">{t("Ask about this chat without adding to it or stopping it. Answered on this PC; nothing here is kept.")}</p>
      <div className="side-chat">
        {asked.map((x, i) => (
          <div key={i} className="side-chat-turn">
            <p className="small"><strong>{x.q}</strong></p>
            {x.a === null && !x.error && <p className="muted small">{t("Thinking…")}</p>}
            {x.a && <Markdown text={x.a} />}
            {x.error && <p className="error-text small">{x.error}</p>}
          </div>
        ))}
      </div>
      <form
        className="row"
        onSubmit={(e) => {
          e.preventDefault();
          ask();
        }}
      >
        <input className="input" style={{ flex: 1 }} autoFocus value={question} onChange={(e) => setQuestion(e.target.value)} placeholder={t("What does that mean?")} aria-label={t("Side question")} />
        <button className="btn primary" type="submit" disabled={busy || !question.trim()}>{t("Ask")}</button>
      </form>
    </Modal>
  );
}
