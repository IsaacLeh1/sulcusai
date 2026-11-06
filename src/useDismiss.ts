// SPDX-License-Identifier: AGPL-3.0-only
// The one way popups, menus and pickers close: a press outside them, or Esc.
import { useEffect, useRef, type RefObject } from "react";

/**
 * While `open`, calls `close` on a press outside `ref` (and outside any
 * element matching `keep`, such as the button that opens it) or on Esc.
 */
export function useDismiss(open: boolean, close: () => void, ref: RefObject<Element | null>, keep?: string) {
  const latest = useRef(close);
  latest.current = close;
  useEffect(() => {
    if (!open) return;
    const press = (e: PointerEvent) => {
      const target = e.target as Element | null;
      if (!target || ref.current?.contains(target)) return;
      if (keep && target.closest?.(keep)) return;
      latest.current();
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") latest.current();
    };
    // Added on the next tick, so the press that opened it doesn't close it.
    const id = window.setTimeout(() => {
      document.addEventListener("pointerdown", press, true);
      document.addEventListener("keydown", key);
    });
    return () => {
      window.clearTimeout(id);
      document.removeEventListener("pointerdown", press, true);
      document.removeEventListener("keydown", key);
    };
  }, [open, ref, keep]);
}
