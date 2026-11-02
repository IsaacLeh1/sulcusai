// SPDX-License-Identifier: AGPL-3.0-only
import { api, errorText, type Chat, type FolderView, type KeepLocks, type SidebarLayout } from "../api";
import { t } from "../i18n";
import { FloatingMenu } from "./ChatFolders";
import type { PushToast } from "./Toasts";

export interface ChatMenuProps {
  menu: { id: string; x: number; y: number };
  onClose: () => void;
  chats: Chat[];
  folders: FolderView[];
  setFolders: (f: FolderView[]) => void;
  locks: KeepLocks;
  setLocks: (l: KeepLocks) => void;
  layout: SidebarLayout;
  saveLayout: (l: SidebarLayout) => void;
  hidden: string[];
  hideChat: (key: string, hide: boolean) => void;
  sharedByMe: Map<string, string[]>;
  peopleOn: boolean;
  phone: boolean;
  setSelected: (f: (s: string[]) => string[]) => void;
  setFolderFor: (id: string) => void;
  setSharing: (id: string) => void;
  chatWindow: (id: string) => void;
  setNotesFor: (id: string) => void;
  setConnecting: (id: string) => void;
  setConfirmDelete: (c: Chat | null) => void;
  onProtect: (id: string) => void;
  onUnprotect: (id: string) => void;
  toast: PushToast;
}

/** A chat's right-click menu in the sidebar. */
export function ChatMenu(p: ChatMenuProps) {
  const { menu, onClose, chats, folders, setFolders, locks, setLocks, layout, saveLayout, hidden, hideChat, sharedByMe, peopleOn, phone, setSelected, setFolderFor, setSharing, chatWindow, setNotesFor, setConnecting, setConfirmDelete, toast } = p;
  return (
    <FloatingMenu x={menu.x} y={menu.y} onClose={onClose}>
      {(() => {
        const id = menu.id;
        const kept = locks.chats.includes(id);
        const current = folders.find((f) => f.chats.includes(id));
        const chat = chats.find((c) => c.id === id);
        const done = onClose;
        const move = (folderId: string | null) => {
          done();
          api.moveToFolder(id, folderId).then(setFolders).catch((e) => toast(errorText(e), "error"));
        };
        return (
          <>
            <button role="menuitem" onClick={() => (done(), setSelected((s) => (s.includes(id) ? s : [...s, id])))}>
              {t("Select")} <span className="muted small">{t("(Ctrl+click)")}</span>
            </button>
            <hr />
            <span className="menu-label">{t("Move to folder")}</span>
            {folders.filter((f) => f.id !== current?.id).map((f) => (
              <button key={f.id} role="menuitem" onClick={() => move(f.id)}>
                {f.name}
              </button>
            ))}
            <button role="menuitem" onClick={() => (setFolderFor(id), done())}>
              ＋ {t("New folder…")}
            </button>
            {current && (
              <button role="menuitem" onClick={() => move(null)}>
                ↩ {t("Out of “{name}”", { name: current.name })}
              </button>
            )}
            <hr />
            {peopleOn && !chats.find((c) => c.id === id)?.incognito && (
              <button role="menuitem" onClick={() => (done(), setSharing(id))}>
                {sharedByMe.has(id) ? t("Shared with {names}…", { names: sharedByMe.get(id)!.join(", ") }) : t("Share…")}
              </button>
            )}
            {!chats.find((c) => c.id === id)?.incognito && (
              <>
                {!phone && (
                  <button role="menuitem" onClick={() => (done(), chatWindow(id))}>
                    {t("Open in a new window")}
                  </button>
                )}
                <button role="menuitem" onClick={() => (done(), setNotesFor(id))}>
                  {t("Notes…")}
                </button>
                <button role="menuitem" onClick={() => (done(), setConnecting(id))}>
                  {t("Connect session…")}
                </button>
              </>
            )}
            {layout.chats.length > 0 && (
              <button role="menuitem" onClick={() => (done(), saveLayout({ ...layout, chats: [] }))}>
                {t("Sort chats by newest again")}
              </button>
            )}
            <button
              role="menuitem"
              onClick={() => {
                done();
                hideChat(id, !hidden.includes(id));
              }}
            >
              {hidden.includes(id) ? t("Show in the list again") : t("Hide from the list")}
            </button>
            <button
              role="menuitem"
              onClick={() => {
                done();
                api.setKept("chat", id, !kept).then(setLocks).catch((e) => toast(errorText(e), "error"));
              }}
            >
              {kept ? t("Unlock (the cleanup may remove it)") : t("Lock (the cleanup never removes it)")}
            </button>
            {chat && !chat.incognito && (
              <button role="menuitem" onClick={() => (done(), chat.protected ? p.onUnprotect(id) : p.onProtect(id))}>
                {chat.protected ? t("Remove PIN protection…") : t("Protect with your PIN")}
              </button>
            )}
            <button
              role="menuitem"
              className="danger"
              onClick={() => {
                setConfirmDelete(chats.find((c) => c.id === id) ?? null);
                done();
              }}
            >
              {t("Delete chat")}
            </button>
          </>
        );
      })()}
    </FloatingMenu>
  );
}
