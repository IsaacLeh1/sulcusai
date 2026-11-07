// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useMemo, useState } from "react";
import { t } from "../i18n";
import { rank } from "../fuzzy";

export interface Command {
  id: string;
  label: string;
  /** Where it comes from: "Chat", "Page", "Setting", "Model"... */
  group: string;
  hint?: string;
  run: () => void;
}

/** Ctrl+K: one box to go anywhere and search everything. */
export function CommandBar({ commands, onClose, search }: { commands: Command[]; onClose: () => void; search?: (q: string) => Promise<Command[]> }) {
  const [q, setQ] = useState("");
  const [sel, setSel] = useState(0);
  const [found, setFound] = useState<Command[]>([]);
  useEffect(() => {
    if (!search || q.trim().length < 2) return setFound([]);
    let live = true;
    const id = setTimeout(() => search(q).then((r) => live && setFound(r)).catch(() => {}), 150);
    return () => {
      live = false;
      clearTimeout(id);
    };
  }, [q, search]);
  const shown = useMemo(() => {
    const all = [...commands, ...found];
    if (!q.trim()) return all.slice(0, 30);
    return rank(all, q, (c) => `${c.label} ${c.group}`)
      .slice(0, 40)
      .map((m) => m.item);
  }, [commands, found, q]);
  const pick = (c?: Command) => {
    if (!c) return;
    onClose();
    c.run();
  };
  return (
    <div className="modal-backdrop ui-cmd-back" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="ui-cmd" role="dialog" aria-label={t("Command bar")}>
        <input
          autoFocus
          placeholder={t("Go to, search chats, groups, files, settings, models…")}
          value={q}
          onChange={(e) => (setQ(e.target.value), setSel(0))}
          onKeyDown={(e) => {
            if (e.key === "Escape") onClose();
            else if (e.key === "ArrowDown") (e.preventDefault(), setSel((s) => Math.min(shown.length - 1, s + 1)));
            else if (e.key === "ArrowUp") (e.preventDefault(), setSel((s) => Math.max(0, s - 1)));
            else if (e.key === "Enter") (e.preventDefault(), pick(shown[sel]));
          }}
        />
        <div className="ui-cmd-list" role="listbox">
          {shown.map((c, i) => (
            <button key={`${c.group}:${c.id}`} role="option" aria-selected={i === sel} className={i === sel ? "sel" : ""} onMouseEnter={() => setSel(i)} onClick={() => pick(c)}>
              <span className="ellipsis">{c.label}</span>
              <span className="muted small">{c.hint ?? c.group}</span>
            </button>
          ))}
          {shown.length === 0 && <p className="muted small">{t("Nothing matches")}</p>}
        </div>
      </div>
    </div>
  );
}
