// SPDX-License-Identifier: AGPL-3.0-only
// Share a chat with people linked in People: they can watch it live, read-only.
import { useEffect, useState } from "react";
import { api, errorText, on, type SharingChoice } from "../api";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";
import { t } from "../i18n";

/** Who a chat is shared with, and the people it could be (null while loading). */
function useSharing(chatId: string): [SharingChoice[] | null, (l: SharingChoice[]) => void] {
  const [list, setList] = useState<SharingChoice[] | null>(null);
  useEffect(() => {
    const load = () => api.peopleChatSharing(chatId).then(setList).catch(() => setList([]));
    load();
    const sub = on("people:changed", load);
    return () => {
      sub.then((un) => un());
    };
  }, [chatId]);
  return [list, setList];
}

/** The chat header's Share button (when anyone is linked in People). */
export function ShareChat({ chatId, toast }: { chatId: string; toast: PushToast }) {
  const [list] = useSharing(chatId);
  const [open, setOpen] = useState(false);
  if (!list || list.length === 0) return null;
  const shared = list.filter((p) => p.shared);
  return (
    <>
      <button
        className={`btn ghost small ${shared.length ? "shared-on" : ""}`}
        title={shared.length ? t("Shared with {names}", { names: shared.map((p) => p.name).join(", ") }) : t("Let people you're linked with watch this chat")}
        onClick={() => setOpen(true)}
      >
        {shared.length ? t("Shared · {n}", { n: shared.length }) : t("Share")}
      </button>
      {open && <ShareChatDialog chatId={chatId} onClose={() => setOpen(false)} toast={toast} />}
    </>
  );
}

/** Picking who may watch a chat: from its Share button, or a right-click on it in the sidebar. */
export function ShareChatDialog({ chatId, onClose, toast }: { chatId: string; onClose: () => void; toast: PushToast }) {
  const [list, setList] = useSharing(chatId);
  const [picked, setPicked] = useState<Set<string> | null>(null);
  const chosen = picked ?? new Set((list ?? []).filter((p) => p.shared).map((p) => p.id));

  const save = async () => {
    try {
      setList(await api.peopleShareChat(chatId, [...chosen]));
      onClose();
      toast(chosen.size ? t("Shared. They can watch this chat while you're both connected.") : t("This chat isn't shared anymore."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <Modal title={t("Share this chat")} onClose={onClose}>
      <p className="muted small">
        {t("They can open it in People and watch it live, read-only, while you're both connected: your messages, the replies as they're written, and the steps the assistant takes (not what its tools found). Nothing is kept on their PC.")}
      </p>
      {list && list.length === 0 && <p className="small">{t("Link with someone in People first: they get a code from you, or you from them.")}</p>}
      <div className="share-list">
        {(list ?? []).map((p) => (
          <label key={p.id} className="check">
            <input
              type="checkbox"
              checked={chosen.has(p.id)}
              onChange={(e) => {
                const next = new Set(chosen);
                if (e.target.checked) next.add(p.id);
                else next.delete(p.id);
                setPicked(next);
              }}
            />
            <span>
              {p.name} <span className="muted small">{p.online ? t("connected") : t("not connected")}</span>
            </span>
          </label>
        ))}
      </div>
      <div className="adv-actions">
        <button className="btn primary" onClick={save} disabled={!list || list.length === 0}>
          {t("Save")}
        </button>
        <button className="btn ghost" onClick={onClose}>
          {t("Cancel")}
        </button>
      </div>
    </Modal>
  );
}

/** Sharing several chats at once (picked in the sidebar): the people picked
 * are added to each one's sharing; who already watches one keeps watching. */
export function ShareChatsDialog({ chatIds, onClose, toast }: { chatIds: string[]; onClose: () => void; toast: PushToast }) {
  const [list] = useSharing(chatIds[0] ?? "");
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [saving, setSaving] = useState(false);
  const save = async () => {
    setSaving(true);
    try {
      for (const id of chatIds) {
        const now = await api.peopleChatSharing(id);
        const keep = now.filter((p) => p.shared).map((p) => p.id);
        await api.peopleShareChat(id, [...new Set([...keep, ...chosen])]);
      }
      onClose();
      toast(t("Shared {n} chats. They can watch them while you're both connected.", { n: chatIds.length }), "success");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setSaving(false);
    }
  };
  return (
    <Modal title={t("Share {n} chats", { n: chatIds.length })} onClose={onClose}>
      <p className="muted small">{t("The people you pick can open these chats in People and watch them live, read-only, while you're both connected. Nothing is kept on their PC.")}</p>
      {list && list.length === 0 && <p className="small">{t("Link with someone in People first: they get a code from you, or you from them.")}</p>}
      <div className="share-list">
        {(list ?? []).map((p) => (
          <label key={p.id} className="check">
            <input
              type="checkbox"
              checked={chosen.has(p.id)}
              onChange={(e) => {
                const next = new Set(chosen);
                if (e.target.checked) next.add(p.id);
                else next.delete(p.id);
                setChosen(next);
              }}
            />
            <span>
              {p.name} <span className="muted small">{p.online ? t("connected") : t("not connected")}</span>
            </span>
          </label>
        ))}
      </div>
      <div className="adv-actions">
        <button className="btn primary" onClick={save} disabled={saving || chosen.size === 0}>
          {t("Share them")}
        </button>
        <button className="btn ghost" onClick={onClose}>
          {t("Cancel")}
        </button>
      </div>
    </Modal>
  );
}
