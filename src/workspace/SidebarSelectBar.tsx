// SPDX-License-Identifier: AGPL-3.0-only
import { setDraft } from "../drafts";
import { api, errorText, type Chat, type FolderView, type KeepLocks } from "../api";
import { t } from "../i18n";
import type { PushToast } from "../components/Toasts";
import type { View } from "../App";

/** The bar over the chat list while chats are picked together: folder, talk together, share, lock, hide, delete. */
export function SidebarSelectBar(p: {
  selected: string[];
  setSelected: (ids: string[]) => void;
  visible: Chat[];
  chats: Chat[];
  folders: FolderView[];
  setFolders: (f: FolderView[]) => void;
  locks: KeepLocks;
  setLocks: (l: KeepLocks) => void;
  peopleOn: boolean;
  setFolderFor: (id: string) => void;
  setSharingMany: (on: boolean) => void;
  setDeletingMany: (on: boolean) => void;
  hideChat: (key: string, hide: boolean) => void;
  refreshChats: () => Promise<void>;
  setActiveChat: (id: string | null) => void;
  setView: (v: View) => void;
  toast: PushToast;
}) {
  const { selected, setSelected, folders, locks, toast } = p;
  return (
    <div className="select-bar" role="toolbar" aria-label={t("Selected chats")}>
      <div className="row">
        <strong className="small">{t("{n} selected", { n: selected.length })}</strong>
        <span className="spacer" />
        <button className="link small" onClick={() => setSelected(p.visible.map((c) => c.id))}>{t("All")}</button>
        <button className="link small" onClick={() => setSelected([])}>{t("Done")}</button>
      </div>
      <div className="select-actions">
        <select
          value=""
          aria-label={t("Add to a folder")}
          onChange={async (e) => {
            const to = e.target.value;
            if (!to) return;
            if (to === "new") {
              p.setFolderFor(selected[0]);
              return;
            }
            try {
              let f = folders;
              for (const id of selected) f = await api.moveToFolder(id, to === "out" ? null : to);
              p.setFolders(f);
              toast(t("Moved {n} chats.", { n: selected.length }), "success");
            } catch (err) {
              toast(errorText(err), "error");
            }
          }}
        >
          <option value="">{t("Add to a folder…")}</option>
          {folders.map((f) => (
            <option key={f.id} value={f.id}>{f.name}</option>
          ))}
          <option value="out">{t("Out of their folders")}</option>
          <option value="new">{t("New folder…")}</option>
        </select>
        <button
          className="btn small"
          title={t("A new chat that reads these chats and asks them what it needs, so they work together")}
          onClick={async () => {
            try {
              const names = selected.map((id) => `“${p.chats.find((c) => c.id === id)?.title ?? id}”`).join(", ");
              const chat = await api.createChatIn(null, false);
              setDraft(chat.id, t("Work with my chats {list}: read what each has so far and ask them what you need, then ", { list: names }));
              await p.refreshChats();
              p.setActiveChat(chat.id);
              p.setView("chat");
              setSelected([]);
            } catch (err) {
              toast(errorText(err), "error");
            }
          }}
        >
          {t("Talk together")}
        </button>
        {p.peopleOn && <button className="btn small" onClick={() => p.setSharingMany(true)}>{t("Share…")}</button>}
        <button
          className="btn small"
          onClick={async () => {
            try {
              let l = locks;
              for (const id of selected) l = await api.setKept("chat", id, true);
              p.setLocks(l);
              toast(t("Locked {n} chats: the cleanup never removes them.", { n: selected.length }), "success");
            } catch (err) {
              toast(errorText(err), "error");
            }
          }}
        >
          {t("Lock")}
        </button>
        <button className="btn small" onClick={() => (selected.forEach((id) => p.hideChat(id, true)), setSelected([]))}>{t("Hide")}</button>
        <button className="btn small danger" onClick={() => p.setDeletingMany(true)}>{t("Delete…")}</button>
      </div>
    </div>
  );
}
