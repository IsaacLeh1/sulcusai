// SPDX-License-Identifier: AGPL-3.0-only
import type { Project, SharedHere, SidebarLayout } from "../api";
import { t } from "../i18n";
import { FloatingMenu } from "../components/ChatFolders";
import type { View } from "../App";

/** Right-click menu on a page link: window, fold into Collapse, reset the order. */
export function NavMenu({ menu, onClose, phone, layout, saveLayout, studioWindow }: {
  menu: { id: string; x: number; y: number };
  onClose: () => void;
  phone: boolean;
  layout: SidebarLayout;
  saveLayout: (next: SidebarLayout) => void;
  studioWindow: () => void;
}) {
  const tucked = layout.collapsed ?? [];
  return (
    <FloatingMenu x={menu.x} y={menu.y} onClose={onClose}>
      {menu.id === "studio" && !phone && (
        <button role="menuitem" onClick={() => (onClose(), studioWindow())}>
          {t("Open in a new window")}
        </button>
      )}
      {tucked.includes(menu.id) ? (
        <button role="menuitem" onClick={() => (onClose(), saveLayout({ ...layout, collapsed: tucked.filter((x) => x !== menu.id) }))}>
          {t("Take out of Collapse")}
        </button>
      ) : (
        <button role="menuitem" onClick={() => (onClose(), saveLayout({ ...layout, collapsed: [...tucked, menu.id] }))}>
          {t("Add to Collapse")}
        </button>
      )}
      {layout.nav.length > 0 && (
        <button role="menuitem" onClick={() => (onClose(), saveLayout({ ...layout, nav: [] }))}>
          {t("Put the menu back in its usual order")}
        </button>
      )}
    </FloatingMenu>
  );
}

/** Right-click menu on a project: open, new chat in it, rename, delete. */
export function ProjectMenu({ menu, onClose, setActiveProject, setView, newChat, onRename, onDelete }: {
  menu: { p: Project; x: number; y: number };
  onClose: () => void;
  setActiveProject: (id: string | null) => void;
  setView: (v: View) => void;
  newChat: (projectId: string) => void;
  onRename: (p: Project) => void;
  onDelete: (p: Project) => void;
}) {
  return (
    <FloatingMenu x={menu.x} y={menu.y} onClose={onClose}>
      <button
        role="menuitem"
        onClick={() => {
          setActiveProject(menu.p.id);
          setView("project");
          onClose();
        }}
      >
        {t("Open")}
      </button>
      <button
        role="menuitem"
        onClick={() => {
          newChat(menu.p.id);
          onClose();
        }}
      >
        ＋ {t("New chat in this project")}
      </button>
      <button role="menuitem" onClick={() => (onRename(menu.p), onClose())}>
        {t("Rename…")}
      </button>
      <hr />
      <button role="menuitem" className="danger" onClick={() => (onDelete(menu.p), onClose())}>
        {t("Delete project…")}
      </button>
    </FloatingMenu>
  );
}

/** Right-click menu on a chat someone shares: open it, or hide it from the list and show it again. */
export function SharedMenu({ menu, onClose, sharedKey, hidden, hideChat, openShared, setView }: {
  menu: { s: SharedHere; x: number; y: number };
  onClose: () => void;
  sharedKey: (s: SharedHere) => string;
  hidden: string[];
  hideChat: (key: string, hide: boolean) => void;
  openShared: (s: SharedHere) => void;
  setView: (v: View) => void;
}) {
  const s = menu.s;
  const key = sharedKey(s);
  return (
    <FloatingMenu x={menu.x} y={menu.y} onClose={onClose}>
      <span className="menu-label">{t("Shared by {name}", { name: s.contact_name })}</span>
      <button
        role="menuitem"
        onClick={() => {
          onClose();
          openShared(s);
          setView("shared");
        }}
      >
        {t("Open")}
      </button>
      <button
        role="menuitem"
        onClick={() => {
          onClose();
          hideChat(key, !hidden.includes(key));
        }}
      >
        {hidden.includes(key) ? t("Show in the list again") : t("Hide from the list")}
      </button>
    </FloatingMenu>
  );
}
