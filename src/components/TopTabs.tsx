// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { t } from "../i18n";
import type { View } from "../App";
import { TOP_TABS, useUiPrefs, type TopTab } from "../ui/prefs";

export const TAB_LABEL: Record<TopTab, string> = {
  home: "Home",
  chat: "Chat",
  create: "Create",
  social: "Social",
  networks: "Networks",
  library: "Library",
  settings: "Settings",
};

/** Which top tab a page lives under: the rarer tools sit inside one of the six. */
export function tabOf(view: View): TopTab {
  switch (view) {
    case "home":
      return "home";
    case "studio":
    case "writing":
    case "translate":
    case "study":
      return "create";
    case "people":
    case "shared":
    case "meetings":
      return "social";
    case "networks":
      return "networks";
    case "library":
    case "notes":
    case "documents":
    case "memory":
    case "personal":
      return "library";
    case "settings":
    case "models":
    case "features":
    case "connectors":
    case "activity":
    case "pc":
    case "compute":
      return "settings";
    default:
      return "chat";
  }
}

const OPENS: Record<TopTab, View> = {
  home: "home",
  chat: "chat",
  create: "studio",
  social: "people",
  networks: "networks",
  library: "library",
  settings: "settings",
};

export interface OpenTab {
  id: string;
  title: string;
}

/** The six top tabs, then the chats and channels left open, reorderable, middle-click to close. */
export function TopTabs({ view, setView, open, active, onOpen, onClose, onReorder, unread }: { view: View; setView: (v: View) => void; open: OpenTab[]; active: string | null; onOpen: (id: string) => void; onClose: (id: string) => void; onReorder: (ids: string[]) => void; unread?: Partial<Record<TopTab, number>> }) {
  const prefs = useUiPrefs();
  const here = tabOf(view);
  const [dragging, setDragging] = useState<string | null>(null);
  const tabs = prefs.tabs.filter((x): x is TopTab => (TOP_TABS as readonly string[]).includes(x) && !prefs.hiddenTabs.includes(x));
  return (
    <div className="ui-tabs" role="tablist" aria-label={t("Sections")}>
      {tabs.map((x) => (
        <button key={x} role="tab" aria-selected={here === x} className={here === x ? "active" : ""} onClick={() => setView(OPENS[x])}>
          {t(TAB_LABEL[x])}
          {!!unread?.[x] && !prefs.calm && <span className="ui-badge">{unread[x]}</span>}
        </button>
      ))}
      {open.length > 0 && <span className="ui-tabs-sep" aria-hidden="true" />}
      {open.map((c) => (
        <span
          key={c.id}
          className={`ui-open-tab${view === "chat" && active === c.id ? " active" : ""}`}
          draggable
          onDragStart={() => setDragging(c.id)}
          onDragOver={(e) => e.preventDefault()}
          onDrop={() => {
            if (!dragging || dragging === c.id) return;
            const ids = open.map((o) => o.id).filter((id) => id !== dragging);
            ids.splice(ids.indexOf(c.id), 0, dragging);
            onReorder(ids);
            setDragging(null);
          }}
          onAuxClick={(e) => e.button === 1 && (e.preventDefault(), onClose(c.id))}
        >
          <button className="ellipsis" title={c.title} onClick={() => onOpen(c.id)}>
            {c.title || t("New chat")}
          </button>
          <button className="icon-btn" aria-label={t("Close tab")} onClick={() => onClose(c.id)}>
            ×
          </button>
        </span>
      ))}
    </div>
  );
}
