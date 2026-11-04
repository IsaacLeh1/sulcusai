// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { api, errorText, type Chat, type FolderView, type KeepLocks, type Project, type SharedHere, type SidebarLayout } from "../api";
import { t } from "../i18n";
import { FolderSection } from "../components/ChatFolders";
import { ChatSearchResults } from "../components/ChatSearch";
import { IncognitoIcon } from "../components/IncognitoIcon";
import { BookIcon, GlobeIcon, LockIcon, PeopleIcon, ShieldIcon } from "../components/Icons";
import type { PushToast } from "../components/Toasts";
import type { View } from "../App";
import { half, placed, type DropAt } from "./order";

export interface SidebarChatsProps {
  view: View;
  setView: (v: View) => void;
  activeChat: string | null;
  openChat: (id: string) => void;
  chatQuery: string;
  setChatQuery: (q: string) => void;
  showHidden: boolean;
  setShowHidden: (f: (v: boolean) => boolean) => void;
  /** Every chat that shows in the list (hidden ones too). */
  listed: Chat[];
  /** The ones not hidden, in the order dragged into. */
  visible: Chat[];
  hiddenOwn: Chat[];
  hiddenShared: SharedHere[];
  sharedVisible: SharedHere[];
  sharedHere: SharedHere[];
  sharedByMe: Map<string, string[]>;
  sharedKey: (s: SharedHere) => string;
  openShared: SharedHere | null;
  setOpenShared: (s: SharedHere) => void;
  folders: FolderView[];
  setFolders: (f: FolderView[]) => void;
  foldered: Set<string>;
  locks: KeepLocks;
  projects: Project[];
  running: string[];
  selected: string[];
  setSelected: (f: (s: string[]) => string[]) => void;
  layout: SidebarLayout;
  saveLayout: (next: SidebarLayout) => void;
  setChatMenu: (m: { id: string; x: number; y: number }) => void;
  setSharedMenu: (m: { s: SharedHere; x: number; y: number }) => void;
  refreshChats: () => Promise<void>;
  peopleOn: boolean;
  toast: PushToast;
}

/** The chat search box and the chat list: folders, Shared, the rest, and Hidden chats. */
export function SidebarChats(p: SidebarChatsProps) {
  const { view, activeChat, selected, sharedByMe, folders, visible, listed, sharedHere, toast } = p;
  const [chatOver, setChatOver] = useState<DropAt | null>(null);
  /** One chat in the sidebar (main list or a folder); it can be dragged onto a folder. */
  const chatItem = (c: Chat, inFolder: boolean) => (
    <button
      key={c.id}
      role="listitem"
      className={`chat-item ${inFolder ? "in-folder" : ""} ${view === "chat" && c.id === activeChat ? "active" : ""} ${chatOver?.id === c.id ? `drop-${chatOver.at}` : ""} ${selected.includes(c.id) ? "selected" : ""}`}
      aria-pressed={selected.length > 0 ? selected.includes(c.id) : undefined}
      onClick={(e) => {
        if (e.ctrlKey || e.metaKey || selected.length > 0) {
          p.setSelected((s) => (s.includes(c.id) ? s.filter((x) => x !== c.id) : [...s, c.id]));
          return;
        }
        p.openChat(c.id);
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        p.setChatMenu({ id: c.id, x: e.clientX, y: e.clientY });
      }}
      draggable
      onDragStart={(e) => {
        e.dataTransfer.setData("application/x-sulcus-chat", c.id);
        e.dataTransfer.effectAllowed = "move";
      }}
      onDragOver={(e) => {
        if (!e.dataTransfer.types.includes("application/x-sulcus-chat")) return;
        e.preventDefault();
        e.stopPropagation();
        setChatOver({ id: c.id, at: half(e) });
      }}
      onDragLeave={() => setChatOver(null)}
      onDrop={(e) => {
        const moved = e.dataTransfer.getData("application/x-sulcus-chat");
        setChatOver(null);
        if (!moved || moved === c.id) return;
        e.preventDefault();
        e.stopPropagation();
        p.saveLayout({ ...p.layout, chats: placed(visible.map((x) => x.id), moved, c.id, half(e)) });
        // Dropped among a folder's chats: into that folder; among the others: out of its folder.
        const into = folders.find((f) => f.chats.includes(c.id)) ?? null;
        const from = folders.find((f) => f.chats.includes(moved)) ?? null;
        if (into?.id !== from?.id) api.moveToFolder(moved, into?.id ?? null).then(p.setFolders).catch((err) => toast(errorText(err), "error"));
      }}
      title={c.shut ? t("Protected chat") : c.title}
    >
      {selected.length > 0 && <span className={`select-box ${selected.includes(c.id) ? "on" : ""}`} aria-hidden="true" />}
      {c.incognito && <IncognitoIcon title={t("Incognito")} />}
      {c.protected && <ShieldIcon title={t("Protected with your PIN")} />}
      <span className="ellipsis">{c.shut ? t("Protected chat") : c.title}</span>
      {p.locks.chats.includes(c.id) && <LockIcon className="chat-kept" title={t("Locked: the automatic cleanup never removes it")} />}
      {c.project_id && <BookIcon className="chat-project" title={p.projects.find((x) => x.id === c.project_id)?.name} />}
      {c.web && <GlobeIcon className="globe-mini" title={t("Web on for this chat")} />}
      {sharedByMe.has(c.id) && <PeopleIcon className="chat-project" title={t("Shared with {names}", { names: sharedByMe.get(c.id)!.join(", ") })} />}
      {p.running.includes(c.id) && <span className="dot" title={t("Working")} aria-label={t("Working")} />}
    </button>
  );
  /** A chat someone shares with this user (People): right-click to hide it from the list, or show it again. */
  const sharedItem = (s: SharedHere) => (
    <button
      key={p.sharedKey(s)}
      role="listitem"
      className={`chat-item shared ${view === "shared" && p.openShared?.chat.id === s.chat.id && p.openShared.contact_id === s.contact_id ? "active" : ""} ${s.online ? "" : "offline"}`}
      onClick={() => {
        p.setOpenShared(s);
        p.setView("shared");
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        p.setSharedMenu({ s, x: e.clientX, y: e.clientY });
      }}
      title={s.online ? t("{name} shares this chat with you", { name: s.contact_name }) : t("{name} shares this chat with you (not connected right now)", { name: s.contact_name })}
    >
      <span className="ellipsis">{s.chat.title}</span>
      <span className="shared-from"><PeopleIcon /> {s.contact_name}</span>
      {s.chat.busy && <span className="dot" title={t("Working")} aria-label={t("Working")} />}
    </button>
  );
  const hiddenCount = p.hiddenOwn.length + p.hiddenShared.length;
  return (
    <>
      {listed.length > 0 && (
        <div className="chat-search">
          <input
            className="input search"
            type="search"
            value={p.chatQuery}
            onChange={(e) => p.setChatQuery(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && p.setChatQuery("")}
            placeholder={t("Search chats")}
            aria-label={t("Search chats")}
          />
        </div>
      )}
      {p.chatQuery.trim() ? (
        <ChatSearchResults query={p.chatQuery.trim()} onOpen={p.openChat} />
      ) : (<>
        {(folders.length > 0 || visible.length > 0) && (
          <FolderSection folders={folders} chats={visible} renderChat={chatItem} onChanged={(f) => (p.setFolders(f), p.refreshChats())} peopleOn={p.peopleOn} toast={toast} />
        )}
        {/* Shared: chats people share with this user, and this user's shared chats. */}
        {(p.sharedVisible.length > 0 || visible.some((c) => sharedByMe.has(c.id) && !p.foldered.has(c.id))) && (
          <div className="side-section">
            <div className="side-head">
              <span>{t("Shared")}</span>
            </div>
            <div className="chat-list" role="list" aria-label={t("Shared")}>
              {p.sharedVisible.map((s) => sharedItem(s))}
              {visible.filter((c) => sharedByMe.has(c.id) && !p.foldered.has(c.id)).map((c) => chatItem(c, false))}
            </div>
          </div>
        )}
        <div className="chat-list" role="list">
          {listed.length === 0 && sharedHere.length === 0 && <p className="muted small pad">{t("Your chats will appear here.")}</p>}
          {visible.filter((c) => !p.foldered.has(c.id) && !sharedByMe.has(c.id)).map((c) => chatItem(c, false))}
          {hiddenCount > 0 && (
            <button className="hidden-toggle small muted" onClick={() => p.setShowHidden((v) => !v)} aria-expanded={p.showHidden}>
              {p.showHidden ? "▾" : "▸"} {t("Hidden chats ({n})", { n: hiddenCount })}
            </button>
          )}
          {p.showHidden && (
            <div className="hidden-chats" role="list" aria-label={t("Hidden chats")}>
              {p.hiddenShared.map((s) => sharedItem(s))}
              {p.hiddenOwn.map((c) => chatItem(c, false))}
              <p className="muted small pad">{t("Right-click a chat to show it in the list again.")}</p>
            </div>
          )}
        </div>
      </>)}
    </>
  );
}
