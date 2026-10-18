// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { on, type Chat, type PendingApproval } from "../api";
import { t } from "../i18n";
import { ApprovalCard } from "./AgentCards";
import { Modal } from "./Modal";

/** A chat that isn't on screen needs an OK (one another chat asked to do
 * something, or one still working after the person moved on): it asks here,
 * wherever they are, so it never waits unseen. The web, folder and API key
 * requests have their own popups. */
export function ElsewhereApprovals({ chats, openChatId, onOpenChat }: { chats: Chat[]; openChatId: string | null; onOpenChat: (id: string) => void }) {
  const [asks, setAsks] = useState<PendingApproval[]>([]);
  useEffect(() => {
    const subs = [
      on("agent:approval", (p) => {
        if (p.tool !== "request_web" && p.tool !== "request_folder" && p.tool !== "request_api_key") setAsks((all) => [...all.filter((a) => a.call_id !== p.call_id), p]);
      }),
      on("agent:approval_done", (p) => setAsks((all) => all.filter((a) => a.call_id !== p.call_id))),
    ];
    return () => {
      subs.forEach((s) => s.then((un) => un()));
    };
  }, []);
  // The open chat shows its own requests in the conversation.
  const p = asks.find((a) => a.chat_id !== openChatId);
  if (!p) return null;
  const chat = chats.find((c) => c.id === p.chat_id);
  const done = () => setAsks((all) => all.filter((a) => a.call_id !== p.call_id));
  return (
    <Modal title={chat ? t("“{title}” needs your OK", { title: chat.title }) : t("Another chat needs your OK")} onClose={done}>
      <p className="muted small">{t("That chat is working on something (perhaps for another chat) and wants to do this:")}</p>
      <ApprovalCard p={p} onAnswered={done} />
      <div className="modal-actions">
        <button
          className="btn ghost"
          onClick={() => {
            done();
            onOpenChat(p.chat_id);
          }}
        >
          {t("Show the chat")}
        </button>
      </div>
    </Modal>
  );
}
