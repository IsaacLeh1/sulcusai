// SPDX-License-Identifier: AGPL-3.0-only
import type { Chat } from "../api";
import { useWaitingApprovals } from "../approvals";
import { t } from "../i18n";
import { ApprovalCard } from "./AgentCards";
import { Modal } from "./Modal";

/** A chat that isn't on screen needs an OK (one another chat asked to do
 * something, or one still working after the person moved on): it asks here,
 * wherever they are, so it never waits unseen. The web, folder and API key
 * requests have their own popups. */
export function ElsewhereApprovals({ chats, openChatId, onOpenChat }: { chats: Chat[]; openChatId: string | null; onOpenChat: (id: string) => void }) {
  const [asks, setAsks] = useWaitingApprovals(chats, (p) => p.tool !== "request_web" && p.tool !== "request_folder" && p.tool !== "request_api_key" && p.preview.kind !== "privacy");
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
