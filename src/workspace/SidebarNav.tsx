// SPDX-License-Identifier: AGPL-3.0-only
import { useState, type ReactNode } from "react";
import type { FeatureId, SidebarLayout } from "../api";
import { t } from "../i18n";
import { PcIcon } from "../components/Icons";
import type { View } from "../App";
import { arrange, half, placed, type DropAt } from "./order";

export interface SidebarNavProps {
  view: View;
  setView: (v: View) => void;
  phone: boolean;
  creates: boolean;
  computeOn: boolean;
  liveMeeting: string | null;
  has: (f: FeatureId) => boolean;
  layout: SidebarLayout;
  saveLayout: (next: SidebarLayout) => void;
  studioWindow: () => void;
  onMenu: (m: { id: string; x: number; y: number }) => void;
}

/** The page links under the new-chat button: draggable, reorderable with Alt+arrows, foldable into "Collapse". */
export function SidebarNav({ view, setView, phone, creates, computeOn, liveMeeting, has, layout, saveLayout, studioWindow, onMenu }: SidebarNavProps) {
  // Where a dragged menu item would land.
  const [navOver, setNavOver] = useState<DropAt | null>(null);
  const tucked = layout.collapsed ?? [];
  const all: { id: string; show: boolean | string | null; label: ReactNode }[] = arrange(
    [
      { id: "pc", show: phone, label: <><PcIcon /> {t("PC")}</> },
      { id: "studio", show: creates, label: t("Studio") },
      { id: "meetings", show: has("meetings") || liveMeeting, label: <>{t("Meetings")}{liveMeeting && <span className="dot rec" aria-label={t("Recording")} />}</> },
      { id: "notes", show: has("notes"), label: t("Notes") },
      { id: "documents", show: has("documents"), label: t("Documents") },
      { id: "study", show: has("documents"), label: t("Study") },
      { id: "review", show: has("files"), label: t("Review code") },
      { id: "compute", show: computeOn, label: t("Compute PC") },
      { id: "tasks", show: has("tasks"), label: t("Tasks") },
      { id: "mail", show: has("email"), label: t("Mail") },
      { id: "calendar", show: has("calendar"), label: t("Calendar") },
      { id: "translate", show: has("translate"), label: t("Translate") },
      { id: "memory", show: has("memory"), label: t("Memory") },
      { id: "scheduled", show: has("scheduled"), label: t("Scheduled") },
    ].filter((n) => n.show),
    layout.nav,
  );
  const navButton = (n: (typeof all)[number], i: number, shown: typeof all) => (
    <button
      key={n.id}
      className={`${view === n.id ? "active" : ""} ${navOver?.id === n.id ? `drop-${navOver.at}` : ""}`}
      onClick={(e) => (e.shiftKey && n.id === "studio" && !phone ? studioWindow() : setView(n.id as View))}
      draggable
      title={n.id === "studio" && !phone ? t("Shift+click: the Studio in a window of its own. Drag to put the menu in the order you like.") : t("Drag to put the menu in the order you like (or Alt+Up / Alt+Down)")}
      onDragStart={(e) => {
        e.dataTransfer.setData("application/x-sulcus-nav", n.id);
        e.dataTransfer.effectAllowed = "move";
      }}
      onDragOver={(e) => {
        if (!e.dataTransfer.types.includes("application/x-sulcus-nav")) return;
        e.preventDefault();
        setNavOver({ id: n.id, at: half(e) });
      }}
      onDragLeave={() => setNavOver(null)}
      onDrop={(e) => {
        const moved = e.dataTransfer.getData("application/x-sulcus-nav");
        setNavOver(null);
        if (moved) saveLayout({ ...layout, nav: placed(shown.map((x) => x.id), moved, n.id, half(e)) });
      }}
      onKeyDown={(e) => {
        if (!e.altKey || (e.key !== "ArrowUp" && e.key !== "ArrowDown")) return;
        e.preventDefault();
        const to = shown[i + (e.key === "ArrowUp" ? -1 : 1)];
        if (to) saveLayout({ ...layout, nav: placed(shown.map((x) => x.id), n.id, to.id, e.key === "ArrowUp" ? "before" : "after") });
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        onMenu({ id: n.id, x: e.clientX, y: e.clientY });
      }}
    >
      {n.label}
    </button>
  );
  const main = all.filter((n) => !tucked.includes(n.id));
  const folded = all.filter((n) => tucked.includes(n.id));
  // The page open now shows even when its group is folded.
  const open = !!layout.collapse_open || folded.some((n) => n.id === view);
  return (
    <nav className="nav" aria-label={t("Main")}>
      {main.map((n, i) => navButton(n, i, main))}
      {folded.length > 0 && (
        <>
          <button className="nav-fold small muted" aria-expanded={open} onClick={() => saveLayout({ ...layout, collapse_open: !layout.collapse_open })}>
            {open ? "▾" : "▸"} {t("Collapse ({n})", { n: folded.length })}
          </button>
          {open && <div className="nav-folded">{folded.map((n, i) => navButton(n, i, folded))}</div>}
        </>
      )}
    </nav>
  );
}
