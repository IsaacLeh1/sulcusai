// SPDX-License-Identifier: AGPL-3.0-only
// The ⚙ menu at the bottom of the sidebar: the pages you visit now and then
// (settings, models, features, activity), so the top stays about your work.
import { useEffect, useRef, useState } from "react";
import { t, tx } from "../i18n";

export type MenuView = "settings" | "models" | "features" | "activity";

const ITEMS: { id: MenuView; icon: string; label: string }[] = [
  { id: "settings", icon: "⚙", label: tx("Settings") },
  { id: "models", icon: "🧩", label: tx("Models") },
  { id: "features", icon: "✨", label: tx("Features") },
  { id: "activity", icon: "📜", label: tx("Activity") },
];

export function SideMenu({ view, installing, onOpen }: { view: string; installing: boolean; onOpen: (v: MenuView) => void }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const current = ITEMS.find((i) => i.id === view);

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
          {ITEMS.map((i) => (
            <button
              key={i.id}
              role="menuitem"
              className={view === i.id ? "active" : ""}
              onClick={() => {
                setOpen(false);
                onOpen(i.id);
              }}
            >
              <span aria-hidden className="side-menu-icon">{i.icon}</span>
              {t(i.label)}
              {i.id === "models" && installing && <span className="dot" aria-label={t("Installing")} />}
            </button>
          ))}
        </div>
      )}
      <button className={`side-menu-btn ${current ? "active" : ""}`} onClick={() => setOpen((o) => !o)} aria-haspopup="menu" aria-expanded={open} title={t("Settings, models, features and activity")}>
        <span aria-hidden>⚙</span>
        <span>{current ? t(current.label) : t("Settings & more")}</span>
        {installing && <span className="dot" aria-label={t("Installing a model")} />}
      </button>
    </div>
  );
}
