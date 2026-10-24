// SPDX-License-Identifier: AGPL-3.0-only
// The gear menu at the bottom of the sidebar: the pages you visit now and then
// (settings, models, features, activity), so the top stays about your work.
import { useEffect, useRef, useState } from "react";
import { t, tx } from "../i18n";
import { GearIcon } from "./Icons";
import { ThemePicker } from "./ThemePicker";

export type MenuView = "settings" | "models" | "features" | "activity" | "connectors" | "people";

const ITEMS: { id: MenuView; label: string }[] = [
  { id: "settings", label: tx("Settings") },
  { id: "models", label: tx("Models") },
  { id: "features", label: tx("Features") },
  { id: "activity", label: tx("Activity") },
];

/** `extra`: feature pages that are on (Connectors, People), after the rest,
 * with a count of what's unread. */
export function SideMenu({ view, installing, extra = [], onOpen }: { view: string; installing: boolean; extra?: { id: MenuView; label: string; unread?: number }[]; onOpen: (v: MenuView) => void }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const items: { id: MenuView; label: string; unread?: number }[] = [...ITEMS, ...extra];
  const current = items.find((i) => i.id === view);
  const unread = extra.reduce((n, e) => n + (e.unread ?? 0), 0);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: globalThis.KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", esc);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", esc);
    };
  }, [open]);

  return (
    <div className="side-foot" ref={ref}>
      {open && (
        <div className="side-menu" role="menu">
          {items.map((i) => (
            <button
              key={i.id}
              role="menuitem"
              className={view === i.id ? "active" : ""}
              onClick={() => {
                setOpen(false);
                onOpen(i.id);
              }}
            >
              {t(i.label)}
              {i.id === "models" && installing && <span className="dot" aria-label={t("Installing")} />}
              {!!i.unread && (
                <span className="badge push-right" aria-label={t("{n} unread", { n: i.unread })}>
                  {i.unread}
                </span>
              )}
            </button>
          ))}
          <div className="side-menu-theme">
            <span className="muted small">{t("Appearance")}</span>
            <ThemePicker compact />
          </div>
        </div>
      )}
      <button className={`side-menu-btn ${current ? "active" : ""}`} onClick={() => setOpen((o) => !o)} aria-haspopup="menu" aria-expanded={open} title={t("Settings, models, features and activity")}>
        <GearIcon />
        <span>{current ? t(current.label) : t("Settings & more")}</span>
        {installing && <span className="dot" aria-label={t("Installing a model")} />}
        {unread > 0 && <span className="badge push-right" aria-label={t("{n} unread", { n: unread })}>{unread}</span>}
      </button>
    </div>
  );
}
