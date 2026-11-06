// SPDX-License-Identifier: AGPL-3.0-only
import { Fragment, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { useDismiss } from "../useDismiss";
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
  /** Its name in the arrange list. */
  label?: string;
  /** Folds before everything else (badges that are nice to see, not needed). */
  first?: boolean;
  /** Kept, but shrinks to a small form (an icon, a narrow box) when even the kept items don't fit. */
  compact?: boolean;
}

/** How wide a compact item is drawn when the bar is tight. */
export const COMPACT_WIDTH = 44;

/** When the always-kept items don't fit at full width, the compact ones
 *  shrink. Returns whether to, and the widths to plan the bar with. */
export function tighten(items: { id: string; keep?: boolean; compact?: boolean }[], widths: Record<string, number>, room: number, gap: number, moreWidth: number): { tight: boolean; widths: Record<string, number> } {
  const kept = items.filter((i) => i.keep);
  const need = kept.reduce((s, i) => s + (widths[i.id] ?? 0) + gap, 0) + (kept.length < items.length ? moreWidth + gap : 0);
  if (need <= room) return { tight: false, widths };
  const small = { ...widths };
  for (const i of kept) if (i.compact && (small[i.id] ?? 0) > COMPACT_WIDTH) small[i.id] = COMPACT_WIDTH;
  return { tight: true, widths: small };
}

/** The person's own arrangement: pinned items never fold; `order` puts the
 *  others in their order of importance (first folds last). */
export interface Arrangement {
  pinned: string[];
  order: string[];
}

const ARRANGE_KEY = "sulcusai.chatbar.arrangement";

export function loadArrangement(): Arrangement {
  try {
    const a = JSON.parse(localStorage.getItem(ARRANGE_KEY) ?? "null");
    if (a && Array.isArray(a.pinned) && Array.isArray(a.order)) return a;
  } catch {
    /* unreadable: the default arrangement */
  }
  return { pinned: [], order: [] };
}

function saveArrangement(a: Arrangement) {
  try {
    localStorage.setItem(ARRANGE_KEY, JSON.stringify(a));
  } catch {
    /* storage blocked: it applies until the window closes */
  }
}

/** Items in the person's order, pinned ones kept. Always-kept items hold
 *  their places; the others fill the remaining places on their own side of
 *  the bar in the person's order. */
export function arrange<T extends { id: string; keep?: boolean; push?: boolean }>(items: T[], a: Arrangement): T[] {
  const pushAt = items.findIndex((i) => i.push);
  const side = (i: number) => (pushAt >= 0 && i >= pushAt ? 1 : 0);
  const rank = (id: string, i: number) => {
    const at = a.order.indexOf(id);
    return at < 0 ? a.order.length + i : at;
  };
  const out = [...items];
  for (const s of [0, 1]) {
    const slots = items.map((item, i) => ({ item, i })).filter(({ item, i }) => !item.keep && side(i) === s);
    const sorted = [...slots].sort((x, y) => rank(x.item.id, x.i) - rank(y.item.id, y.i));
    slots.forEach((slot, k) => {
      // The slot keeps its side's start mark, whoever now fills it.
      out[slot.i] = { ...sorted[k].item, push: items[slot.i].push };
    });
  }
  return out.map((item) => (a.pinned.includes(item.id) ? { ...item, keep: true } : item));
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
export function FoldBar({ items: given, className = "" }: { items: FoldItem[]; className?: string }) {
  const [arrangement, setArrangement] = useState(loadArrangement);
  const [arranging, setArranging] = useState(false);
  const items = arrange(given, arrangement);
  const bar = useRef<HTMLDivElement>(null);
  const more = useRef<HTMLButtonElement>(null);
  const widths = useRef<Record<string, number>>({});
  const [room, setRoom] = useState(Infinity);
  const [open, setOpen] = useState(false);
  const moreWrap = useRef<HTMLSpanElement>(null);
  const [, remeasured] = useState(0);
  const moreW = more.current?.offsetWidth || 64;
  const plan = tighten(items, widths.current, room, GAP, moreW);
  // Badges marked `first` go to the end of the folding order, so they fold before the rest.
  const ordered = [...items.filter((i) => !i.first), ...items.filter((i) => i.first)];
  // Shrinking is the last step: once the kept controls are shrunk, every other control stays in More.
  const shown = plan.tight ? new Set(items.filter((i) => i.keep).map((i) => i.id)) : fold(ordered, plan.widths, room, GAP, moreW);
  const folded = items.filter((i) => !shown.has(i.id));
  const ids = items.map((i) => i.id).join(",");

  useLayoutEffect(() => {
    const el = bar.current;
    if (!el) return;
    const measure = () => {
      let changed = false;
      el.querySelectorAll<HTMLElement>(":scope > [data-fold]").forEach((n) => {
        const id = n.dataset.fold!;
        // A shrunken item keeps its full width on record, to know when it fits again.
        if (plan.tight && n.classList.contains("compact")) return;
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
  }, [ids, shown.size, plan.tight]);

  useLayoutEffect(() => {
    if (folded.length === 0) setOpen(false);
  }, [folded.length]);

  const groups = [...new Set(folded.map((i) => i.group))];
  const change = (a: Arrangement) => {
    setArrangement(a);
    saveArrangement(a);
  };
  const movable = items.filter((i) => !given.find((g) => g.id === i.id)?.keep);
  const move = (id: string, by: number) => {
    const ids = movable.map((i) => i.id);
    const at = ids.indexOf(id);
    const to = at + by;
    if (at < 0 || to < 0 || to >= ids.length) return;
    [ids[at], ids[to]] = [ids[to], ids[at]];
    change({ ...arrangement, order: ids });
  };
  const pin = (id: string, on: boolean) => change({ ...arrangement, pinned: on ? [...arrangement.pinned, id] : arrangement.pinned.filter((p) => p !== id) });
  const arrangePanel = arranging && (
    <div className="fold-menu" role="dialog" aria-label={t("Arrange the chat bar")} onKeyDown={(e) => e.key === "Escape" && setArranging(false)}>
      <div className="fold-group">{t("Arrange the chat bar")}</div>
      <span className="muted small">{t("Pinned controls always stay on the bar. Higher ones fold into More last.")}</span>
      {movable.map((i, k) => {
        const name = i.label ?? i.id;
        return (
          <div key={i.id} className="fold-row row small">
            <label className="row">
              <input type="checkbox" checked={arrangement.pinned.includes(i.id)} onChange={(e) => pin(i.id, e.target.checked)} />
              {t("Pin {name}", { name })}
            </label>
            <span className="spacer" />
            <button className="btn small" disabled={k === 0} onClick={() => move(i.id, -1)} aria-label={t("Move {name} up", { name })}>{t("Up")}</button>
            <button className="btn small" disabled={k === movable.length - 1} onClick={() => move(i.id, 1)} aria-label={t("Move {name} down", { name })}>{t("Down")}</button>
          </div>
        );
      })}
      <div className="row">
        <button className="btn small" onClick={() => change({ pinned: [], order: [] })}>{t("Back to the default")}</button>
        <span className="spacer" />
        <button className="btn small" onClick={() => setArranging(false)}>{t("Done")}</button>
      </div>
    </div>
  );
  useDismiss(open, () => setOpen(false), moreWrap);
  const firstPush = items.find((i) => i.push && shown.has(i.id))?.id;
  const isMoreAtStartOfRight = folded.length > 0 && !!firstPush;
  const isMoreAtEndOfBar = folded.length > 0 && !firstPush;
  const moreMenu = folded.length > 0 && (
    <span key="__more" ref={moreWrap} className="fold-more-wrap push-right">
      <button ref={more} className="globe" aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen((o) => !o)} title={t("More controls for this chat")}>
        {t("More")}
      </button>
      {open && (
        <div
          className="fold-menu"
          role="dialog"
          aria-label={t("More controls for this chat")}
          onKeyDown={(e) => {
            if (e.key !== "Escape") return;
            setOpen(false);
            more.current?.focus();
          }}
        >
          {groups.map((g) => (
            <Fragment key={g}>
              <div className="fold-group" role="heading" aria-level={3}>{g}</div>
              {folded
                .filter((i) => i.group === g)
                .map((i) => (
                  <div key={i.id} className="fold-row">
                    {i.node}
                  </div>
                ))}
            </Fragment>
          ))}
          <div className="fold-row">
            <button className="btn small" onClick={() => { setOpen(false); setArranging(true); }}>{t("Arrange the bar…")}</button>
          </div>
        </div>
      )}
    </span>
  );
  return (
    <div
      ref={bar}
      className={`composer-bar fold-bar ${plan.tight ? "tight " : ""}${className}`}
      onContextMenu={(e) => {
        e.preventDefault();
        setArranging(true);
      }}
      title={t("Right-click (or Shift+F10) to arrange the bar")}
    >
      {items.map((i) => {
        if (!shown.has(i.id)) return null;
        // When the More button initiates the right-aligned cluster, subsequent items stay grouped.
        const needsPush = isMoreAtStartOfRight ? false : i.id === firstPush;
        return (
          <Fragment key={i.id}>
            {i.id === firstPush && moreMenu}
            <span data-fold={i.id} className={`fold-item${needsPush ? " push-right" : ""}${i.compact ? " compact" : ""}`}>
              {i.node}
            </span>
          </Fragment>
        );
      })}
      {isMoreAtEndOfBar && moreMenu}
      {arrangePanel && <span className="fold-more-wrap">{arrangePanel}</span>}
    </div>
  );
}
