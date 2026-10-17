// SPDX-License-Identifier: AGPL-3.0-only
// Chat folders in the sidebar: sort chats into folders, lock a folder so the
// automatic cleanup keeps its chats, and share a whole folder with people.
import { useEffect, useRef, useState, type ReactNode } from "react";
import { api, errorText, type Chat, type FolderView } from "../api";
import { t } from "../i18n";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

/** The folders, each with its chats (when open). Chats can be dragged onto a folder. */
export function FolderSection({
  folders,
  chats,
  renderChat,
  onChanged,
  peopleOn,
  toast,
}: {
  folders: FolderView[];
  /** The chats shown in the sidebar (folders show theirs from these). */
  chats: Chat[];
  renderChat: (c: Chat, inFolder: boolean) => ReactNode;
  onChanged: (f: FolderView[]) => void;
  peopleOn: boolean;
  toast: PushToast;
}) {
  const [naming, setNaming] = useState<{ folder: FolderView | null } | null>(null);
  const [menu, setMenu] = useState<{ folder: FolderView; x: number; y: number } | null>(null);
  const [sharing, setSharing] = useState<FolderView | null>(null);
  const [deleting, setDeleting] = useState<FolderView | null>(null);
  const [over, setOver] = useState<string | null>(null);
  const act = (p: Promise<FolderView[]>) => p.then(onChanged).catch((e) => toast(errorText(e), "error"));

  return (
    <div className="side-section folders">
      <div className="side-head">
        <span>{t("Folders")}</span>
        <button className="icon-btn" title={t("New folder")} aria-label={t("New folder")} onClick={() => setNaming({ folder: null })}>
          +
        </button>
      </div>
      {folders.map((f) => {
        const inside = chats.filter((c) => f.chats.includes(c.id));
        return (
          <div key={f.id} className={`folder ${over === f.id ? "drop" : ""}`}>
            <button
              className="chat-item folder-head"
              aria-expanded={!f.collapsed}
              onClick={() => act(api.setFolderFlags(f.id, { collapsed: !f.collapsed }))}
              onContextMenu={(e) => {
                e.preventDefault();
                setMenu({ folder: f, x: e.clientX, y: e.clientY });
              }}
              onDragOver={(e) => {
                if (e.dataTransfer.types.includes("application/x-sulcus-chat")) {
                  e.preventDefault();
                  setOver(f.id);
                }
              }}
              onDragLeave={() => setOver(null)}
              onDrop={(e) => {
                setOver(null);
                const id = e.dataTransfer.getData("application/x-sulcus-chat");
                if (id) act(api.moveToFolder(id, f.id));
              }}
              title={t("Right-click for more")}
            >
              <span aria-hidden className="folder-caret">{f.collapsed ? "▸" : "▾"}</span>
              <span aria-hidden>📁</span>
              <span className="ellipsis">{f.name}</span>
              {f.locked && <span title={t("Locked: the automatic cleanup keeps these chats")}>🔒</span>}
              {f.shared_with.length > 0 && <span title={t("Shared with {names}", { names: f.shared_with.map((p) => p.name).join(", ") })}>👥</span>}
              <span className="muted small folder-count">{inside.length}</span>
            </button>
            {!f.collapsed && (
              <div className="folder-chats" role="list">
                {inside.length === 0 ? <p className="muted small pad folder-empty">{t("Drag chats here, or right-click a chat → Move to folder.")}</p> : inside.map((c) => renderChat(c, true))}
              </div>
            )}
          </div>
        );
      })}
      {menu && (
        <FloatingMenu x={menu.x} y={menu.y} onClose={() => setMenu(null)}>
          <button role="menuitem" onClick={() => (setNaming({ folder: menu.folder }), setMenu(null))}>
            ✏ {t("Rename")}
          </button>
          <button role="menuitem" onClick={() => (act(api.setFolderFlags(menu.folder.id, { locked: !menu.folder.locked })), setMenu(null))}>
            {menu.folder.locked ? `🔓 ${t("Unlock (the cleanup may remove its chats)")}` : `🔒 ${t("Lock (the cleanup keeps its chats)")}`}
          </button>
          <button role="menuitem" onClick={() => (setSharing(menu.folder), setMenu(null))}>
            👥 {t("Share folder…")}
          </button>
          <button role="menuitem" className="danger" onClick={() => (setDeleting(menu.folder), setMenu(null))}>
            🗑 {t("Delete folder")}
          </button>
        </FloatingMenu>
      )}
      {naming && (
        <NameModal
          title={naming.folder ? t("Rename folder") : t("New folder")}
          initial={naming.folder?.name ?? ""}
          onClose={() => setNaming(null)}
          onSave={(name) => {
            setNaming(null);
            act(naming.folder ? api.renameFolder(naming.folder.id, name) : api.createFolder(name));
          }}
        />
      )}
      {deleting && (
        <Modal title={t("Delete this folder?")} onClose={() => setDeleting(null)}>
          <p>{t("The folder goes; its {n} chats stay, back in the chat list.", { n: deleting.chats.length })}</p>
          {deleting.shared_with.length > 0 && <p className="small muted">{t("Its chats stop being shared with {names}.", { names: deleting.shared_with.map((p) => p.name).join(", ") })}</p>}
          <div className="modal-actions">
            <button className="btn" onClick={() => setDeleting(null)}>{t("Cancel")}</button>
            <button className="btn danger-fill" onClick={() => (act(api.deleteFolder(deleting.id)), setDeleting(null))}>{t("Delete folder")}</button>
          </div>
        </Modal>
      )}
      {sharing && <FolderShareModal folder={sharing} peopleOn={peopleOn} onClose={() => setSharing(null)} onDone={() => api.foldersView().then(onChanged).catch(() => {})} toast={toast} />}
    </div>
  );
}

/** A small menu at the pointer; closes on a click elsewhere or Escape. */
export function FloatingMenu({ x, y, onClose, children }: { x: number; y: number; onClose: () => void; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const close = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const esc = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", esc);
    window.addEventListener("blur", onClose);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", esc);
      window.removeEventListener("blur", onClose);
    };
  }, [onClose]);
  return (
    <div ref={ref} className="context-menu" role="menu" style={{ left: Math.min(x, window.innerWidth - 260), top: Math.min(y, window.innerHeight - 260) }}>
      {children}
    </div>
  );
}

/** Asks for a folder's name. */
export function NameModal({ title, initial, onClose, onSave }: { title: string; initial: string; onClose: () => void; onSave: (name: string) => void }) {
  const [name, setName] = useState(initial);
  return (
    <Modal title={title} onClose={onClose}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (name.trim()) onSave(name.trim());
        }}
      >
        <input className="input" autoFocus value={name} maxLength={60} onChange={(e) => setName(e.target.value)} placeholder={t("Folder name")} aria-label={t("Folder name")} />
        <div className="modal-actions">
          <button type="button" className="btn" onClick={onClose}>{t("Cancel")}</button>
          <button type="submit" className="btn primary" disabled={!name.trim()}>{t("Save")}</button>
        </div>
      </form>
    </Modal>
  );
}

/** Shares a whole folder with people linked in People: every chat in it,
 * and chats moved in later. */
function FolderShareModal({ folder, peopleOn, onClose, onDone, toast }: { folder: FolderView; peopleOn: boolean; onClose: () => void; onDone: () => void; toast: PushToast }) {
  const [choices, setChoices] = useState<{ id: string; name: string; shared: boolean }[] | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    if (!peopleOn) return;
    api
      .folderSharing(folder.id)
      .then((c) => {
        setChoices(c);
        setPicked(new Set(c.filter((x) => x.shared).map((x) => x.id)));
      })
      .catch((e) => toast(errorText(e), "error"));
  }, [folder.id, peopleOn]); // eslint-disable-line react-hooks/exhaustive-deps
  return (
    <Modal title={t("Share “{name}”", { name: folder.name })} onClose={onClose}>
      {!peopleOn ? (
        <p>{t("Sharing uses People. Turn it on in Features, link with someone, then share the folder.")}</p>
      ) : !choices ? (
        <p className="muted">{t("Loading…")}</p>
      ) : choices.length === 0 ? (
        <p>{t("You haven't linked with anyone yet. Open People to link with someone, then share the folder.")}</p>
      ) : (
        <>
          <p className="small muted">
            {t("Each person sees every chat in this folder ({n} now), and chats you move in later, live while you're both connected. Their own model answers when they write in one.", { n: folder.chats.length })}
          </p>
          {choices.map((c) => (
            <label key={c.id} className="check">
              <input
                type="checkbox"
                checked={picked.has(c.id)}
                onChange={(e) => {
                  const next = new Set(picked);
                  if (e.target.checked) next.add(c.id);
                  else next.delete(c.id);
                  setPicked(next);
                }}
              />
              <span>{c.name}</span>
            </label>
          ))}
        </>
      )}
      <div className="modal-actions">
        <button className="btn" onClick={onClose}>{t("Cancel")}</button>
        {peopleOn && choices && choices.length > 0 && (
          <button
            className="btn primary"
            disabled={saving}
            onClick={async () => {
              setSaving(true);
              try {
                await api.shareFolder(folder.id, [...picked]);
                toast(picked.size ? t("The folder is shared.") : t("The folder isn't shared any more."), "success");
                onDone();
                onClose();
              } catch (e) {
                toast(errorText(e), "error");
              } finally {
                setSaving(false);
              }
            }}
          >
            {t("Save")}
          </button>
        )}
      </div>
    </Modal>
  );
}
