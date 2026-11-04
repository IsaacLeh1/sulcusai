// SPDX-License-Identifier: AGPL-3.0-only

/** Items in the order the user dragged them into; ones not placed yet
 * (new chats, a page just turned on) come first, in their usual order. */
export function arrange<T extends { id: string }>(items: T[], order: string[]): T[] {
  if (order.length === 0) return items;
  const at = new Map(order.map((id, i) => [id, i]));
  const fresh = items.filter((x) => !at.has(x.id));
  const kept = items.filter((x) => at.has(x.id)).sort((a, b) => at.get(a.id)! - at.get(b.id)!);
  return [...fresh, ...kept];
}

/** The ids with `moved` put just before or after `target`. */
export function placed(ids: string[], moved: string, target: string, at: "before" | "after"): string[] {
  const rest = ids.filter((id) => id !== moved);
  const i = rest.indexOf(target);
  if (i < 0) return ids;
  rest.splice(at === "before" ? i : i + 1, 0, moved);
  return rest;
}

/** Whether a drag is over the top or bottom half of an item. */
export function half(e: { clientY: number; currentTarget: Element }): "before" | "after" {
  const r = e.currentTarget.getBoundingClientRect();
  return e.clientY < r.top + r.height / 2 ? "before" : "after";
}

/** Where a dragged item would land: before or after this one. */
export interface DropAt {
  id: string;
  at: "before" | "after";
}
