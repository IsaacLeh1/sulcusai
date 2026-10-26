// SPDX-License-Identifier: AGPL-3.0-only
import { api, type Chat } from "../api";
import { useWaitingApprovals } from "../approvals";
import { t } from "../i18n";
import { Modal } from "./Modal";

/** The assistant needs the web while it's off: a popup, wherever the person
 * is in the app. A yes turns web on for that chat and the reply carries on;
 * a no (or closing it) lets the reply finish without the web. */
export function WebPermission({ chats, onOpenChat, only }: { chats: Chat[]; onOpenChat: (id: string) => void; /** Only this chat's (a chat in a window of its own). */ only?: string }) {
  const [asks, setAsks] = useWaitingApprovals(chats, (p) => p.tool === "request_web" && (!only || p.chat_id === only));
  const p = asks[0];
  if (!p) return null;
  const chat = chats.find((c) => c.id === p.chat_id);
  const answer = async (d: "allow" | "deny") => {
    setAsks((all) => all.filter((a) => a.call_id !== p.call_id));
    await api.answerApproval(p.call_id, d).catch(() => {});
  };
  return (
    <Modal title={t("Use the web?")} onClose={() => answer("deny")}>
      <p>
        {chat ? t("To answer in “{title}”, the assistant wants to look this up online:", { title: chat.title }) : t("The assistant wants to look this up online:")}
      </p>
      {p.preview.detail && <p className="web-ask-reason">{p.preview.detail}</p>}
      <p className="muted small">{t("This turns on web access for that chat only; your other chats stay as they are. Searches and pages go out from this PC.")}</p>
      <div className="modal-actions">
        {chat && (
          <button className="btn ghost" onClick={() => onOpenChat(p.chat_id)}>
            {t("Show the chat")}
          </button>
        )}
        <span className="spacer" />
        <button className="btn" onClick={() => answer("deny")}>{t("Not now")}</button>
        <button className="btn primary" onClick={() => answer("allow")} autoFocus>
          {t("Allow web for this chat")}
        </button>
      </div>
    </Modal>
  );
}
