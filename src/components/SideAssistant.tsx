// SPDX-License-Identifier: AGPL-3.0-only
import { useState, type KeyboardEvent } from "react";
import { api, errorText } from "../api";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

/** Ask about the page you're on: the question starts a chat in its own window, so the page stays as it is. */
export function SideAssistant({ page, toast }: { page: string; toast: PushToast }) {
  const [open, setOpen] = useState(false);
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);

  const ask = async () => {
    const q = text.trim();
    if (!q || sending) return;
    setSending(true);
    try {
      const chat = await api.createChatIn(null, false);
      await api.openChatWindow(chat.id);
      // The page goes with the question, so "this" and "here" mean something;
      // changes still go through the chat's own approval.
      await api.send(chat.id, t("I'm on the {page} page of SulcusAI.", { page }) + " " + q);
      setText("");
      setOpen(false);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setSending(false);
    }
  };
  const onKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      ask();
    } else if (e.key === "Escape") setOpen(false);
  };

  return (
    <div className="side-assistant">
      {open && (
        <div className="side-assistant-pop" role="dialog" aria-label={t("Ask the assistant")}>
          <textarea
            autoFocus
            rows={3}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={onKey}
            placeholder={t("Ask about this page, or what to do here")}
          />
          <div className="side-assistant-row">
            <span className="muted small">{t("Opens in a window of its own. It asks before changing anything.")}</span>
            <button className="btn primary small" onClick={ask} disabled={sending || !text.trim()}>
              {sending ? t("Sending…") : t("Ask")}
            </button>
          </div>
        </div>
      )}
      <button className="btn side-assistant-btn" onClick={() => setOpen(!open)} aria-expanded={open}>
        {t("Ask")}
      </button>
    </div>
  );
}
