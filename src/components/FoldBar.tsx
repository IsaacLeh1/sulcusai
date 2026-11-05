// SPDX-License-Identifier: AGPL-3.0-only
import { Fragment, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { t } from "../i18n";

export interface FoldItem {
  id: string;
  /** Heading it goes under in the More menu. */
  group: string;
  node: ReactNode;
  /** Never folded: what the person must always see (send, model, cloud and cost badges, context). */
  keep?: boolean;
  /** Starts the right-hand side of the bar. */
  push?: boolean;
}

/** Which items stay on the bar. Folding starts from the last foldable item,
 *  so the order given is the order of importance. Widths not yet measured
 *  count as 0 so a new control gets one try on the bar before it folds. */
export function fold(items: { id: string; keep?: boolean }[], widths: Record<string, number>, room: number, gap: number, moreWidth: number): Set<string> {
  const w = (id: string) => (widths[id] ?? 0) + gap;
  const kept = items.filter((i) => i.keep);
  const loose = items.filter((i) => !i.keep);
  const shown = new Set(kept.map((i) => i.id));
  let used = kept.reduce((s, i) => s + w(i.id), 0);
  const all = loose.reduce((s, i) => s + w(i.id), 0);
  if (used + all <= room) {
    loose.forEach((i) => shown.add(i.id));
    return shown;
  }
  used += moreWidth + gap;
  for (const i of loose) {
    if (used + w(i.id) > room) break;
    used += w(i.id);
    shown.add(i.id);
  }
  return shown;
}

const GAP = 8;

/** The chat bar: one line, never wrapped or clipped. What doesn't fit moves
 *  into one More menu, grouped, instead of squeezing the labels. */
export function FoldBar({ items, className = "" }: { items: FoldItem[]; className?: string }) {
  const bar = useRef<HTMLDivElement>(null);
  const more = useRef<HTMLButtonElement>(null);
  const widths = useRef<Record<string, number>>({});
  const [room, setRoom] = useState(Infinity);
  const [open, setOpen] = useState(false);
  const [, remeasured] = useState(0);
  const shown = fold(items, widths.current, room, GAP, more.current?.offsetWidth || 64);
  const folded = items.filter((i) => !shown.has(i.id));
  const ids = items.map((i) => i.id).join(",");

  useLayoutEffect(() => {
    const el = bar.current;
    if (!el) return;
    const measure = () => {
      let changed = false;
      el.querySelectorAll<HTMLElement>(":scope > [data-fold]").forEach((n) => {
        const id = n.dataset.fold!;
        if (n.offsetWidth > 0 && widths.current[id] !== n.offsetWidth) {
          widths.current[id] = n.offsetWidth;
          changed = true;
        }
      });
      setRoom(el.clientWidth);
      if (changed) remeasured((n) => n + 1);
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [ids, shown.size]);

  useLayoutEffect(() => {
    if (folded.length === 0) setOpen(false);
  }, [folded.length]);

  const groups = [...new Set(folded.map((i) => i.group))];
  const firstPush = items.find((i) => i.push && shown.has(i.id))?.id;
  const moreMenu = folded.length > 0 && (
    <span key="__more" className="fold-more-wrap">
      <button ref={more} className="globe" aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen((o) => !o)} title={t("More controls for this chat")}>
        {t("More")}
      </button>
      {open && (
        <div className="fold-menu" role="menu" onKeyDown={(e) => e.key === "Escape" && setOpen(false)}>
          {groups.map((g) => (
            <Fragment key={g}>
              <div className="fold-group">{g}</div>
              {folded
                .filter((i) => i.group === g)
                .map((i) => (
                  <div key={i.id} className="fold-row">
                    {i.node}
                  </div>
                ))}
            </Fragment>
          ))}
        </div>
      )}
    </span>
  );
  return (
    <div ref={bar} className={`composer-bar fold-bar ${className}`}>
      {items.map((i) =>
        shown.has(i.id) ? (
          <Fragment key={i.id}>
            {i.id === firstPush && moreMenu}
            <span data-fold={i.id} className={`fold-item${i.push ? " push-right" : ""}`}>
              {i.node}
            </span>
          </Fragment>
        ) : null,
      )}
      {!firstPush && moreMenu}
    </div>
  );
}
