// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { t } from "../i18n";

/** A small "i" beside a setting: what it does, shown on hover, focus or a
 * click (and closed by Escape or a click elsewhere). */
export function InfoTip({ text, label }: { text: string; label?: string }) {
  const [open, setOpen] = useState(false);
  const [pinned, setPinned] = useState(false);
  // Which way the note opens: toward the side with room on screen.
  const [side, setSide] = useState<"" | "left" | "right">("");
  const id = useId();
  const box = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (!open || !box.current) return;
    const r = box.current.getBoundingClientRect();
    setSide(r.right + 170 > window.innerWidth ? "right" : r.left < 170 ? "left" : "");
  }, [open]);
  useEffect(() => {
    if (!pinned) return;
    const away = (e: MouseEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) {
        setPinned(false);
        setOpen(false);
      }
    };
    const esc = (e: KeyboardEvent) => e.key === "Escape" && (setPinned(false), setOpen(false));
    document.addEventListener("mousedown", away);
    document.addEventListener("keydown", esc);
    return () => {
      document.removeEventListener("mousedown", away);
      document.removeEventListener("keydown", esc);
    };
  }, [pinned]);
  return (
    <span className="info-tip" ref={box} onMouseEnter={() => setOpen(true)} onMouseLeave={() => !pinned && setOpen(false)}>
      <button
        type="button"
        className="info-tip-btn"
        aria-label={label ? t("About {what}", { what: label }) : t("More about this")}
        aria-describedby={open ? id : undefined}
        aria-expanded={open}
        onClick={(e) => {
          e.preventDefault();
          e.stopPropagation();
          setPinned((p) => !p);
          setOpen((o) => !pinned || !o);
        }}
        onFocus={() => setOpen(true)}
        onBlur={() => !pinned && setOpen(false)}
      >
        i
      </button>
      {open && (
        <span id={id} role="tooltip" className={`info-tip-pop ${side}`}>
          {text}
        </span>
      )}
    </span>
  );
}

/** A group of settings that folds away: its title, a short line of what's
 * picked when it's closed, and the settings when it's open. */
export function Fold({ title, summary, info, open: startOpen = false, children }: { title: string; summary?: string; info?: string; open?: boolean; children: ReactNode }) {
  const [open, setOpen] = useState(startOpen);
  return (
    <section className={`fold ${open ? "open" : ""}`}>
      <button type="button" className="fold-head" onClick={() => setOpen((o) => !o)} aria-expanded={open}>
        <span className="fold-chevron" aria-hidden>›</span>
        <strong>{title}</strong>
        {!open && summary && <span className="small muted ellipsis fold-summary">{summary}</span>}
      </button>
      {info && <InfoTip text={info} label={title} />}
      {open && <div className="fold-body">{children}</div>}
    </section>
  );
}
