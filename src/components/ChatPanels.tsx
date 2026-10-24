// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type Chat, type Memory } from "../api";
import { t } from "../i18n";
import { MemoryList } from "./MemoryList";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

/** A chat's memories: what was saved from it (by the assistant, or added
 * here), to read, edit, delete or add to. */
export function ChatMemories({ chat, onClose, toast }: { chat: Chat; onClose: () => void; toast: PushToast }) {
  const [list, setList] = useState<Memory[] | null>(null);
  const load = () =>
    api
      .memories()
      .then((all) => setList(all.filter((m) => m.source_chat === chat.id)))
      .catch((e) => toast(errorText(e), "error"));
  useEffect(() => {
    load();
  }, [chat.id]); // eslint-disable-line react-hooks/exhaustive-deps
  return (
    <Modal title={t("Memories from this chat")} onClose={onClose}>
      <p className="muted small">{t("What the assistant remembered from this chat, and what you added here. Other chats recall them when they matter. Everything it remembers is on the Memory page.")}</p>
      {list && <MemoryList memories={list} add={{ projectId: chat.project_id, chatId: chat.id }} empty={t("Nothing remembered from this chat yet.")} toast={toast} onChanged={load} />}
      <div className="modal-actions">
        <button className="btn" onClick={onClose}>{t("Close")}</button>
      </div>
    </Modal>
  );
}

/** Chats linked with this one to work together: each one's assistant knows
 * the others, reads what they've done and sends them a question or a piece
 * of work (`message_chat`), whose answer comes back. */
export function LinkedChats({ chat, onOpenChat, onClose, toast }: { chat: Chat; onOpenChat: (id: string) => void; onClose: () => void; toast: PushToast }) {
  const [linked, setLinked] = useState<Chat[]>([]);
  const [all, setAll] = useState<Chat[]>([]);
  const [pick, setPick] = useState("");
  const load = () => {
    api.linkedChats(chat.id).then(setLinked).catch((e) => toast(errorText(e), "error"));
    api.chats().then(setAll).catch(() => {});
  };
  useEffect(load, [chat.id]); // eslint-disable-line react-hooks/exhaustive-deps
  const run = (f: () => Promise<unknown>) =>
    f()
      .then(load)
      .catch((e) => toast(errorText(e), "error"));
  const choices = all.filter((c) => c.id !== chat.id && !c.incognito && !linked.some((l) => l.id === c.id));
  return (
    <Modal title={t("Linked chats")} onClose={onClose}>
      <p className="muted small">{t("Linked chats work together: each one's assistant knows the others, can read what they've done, and can send them a question or a piece of work. The answer comes back to the chat that asked.")}</p>
      {linked.length === 0 ? (
        <p className="muted small">{t("No chats linked yet.")}</p>
      ) : (
        <ul className="linked-list">
          {linked.map((c) => (
            <li key={c.id}>
              <button
                className="link"
                onClick={() => {
                  onClose();
                  onOpenChat(c.id);
                }}
              >
                {c.title}
              </button>
              <button className="link danger" onClick={() => run(() => api.unlinkChats(chat.id, c.id))}>{t("Unlink")}</button>
            </li>
          ))}
        </ul>
      )}
      <div className="row wrap">
        <select className="input" value={pick} onChange={(e) => setPick(e.target.value)} aria-label={t("Chat to link")}>
          <option value="">{t("Pick a chat to link…")}</option>
          {choices.map((c) => (
            <option key={c.id} value={c.id}>{c.title}</option>
          ))}
        </select>
        <button
          className="btn primary"
          disabled={!pick}
          onClick={() =>
            run(async () => {
              await api.linkChats(chat.id, pick);
              setPick("");
            })
          }
        >
          {t("Link")}
        </button>
      </div>
      <div className="modal-actions">
        <button className="btn" onClick={onClose}>{t("Close")}</button>
      </div>
    </Modal>
  );
}
