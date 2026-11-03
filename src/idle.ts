// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef } from "react";

const EVENTS = ["mousemove", "mousedown", "keydown", "wheel", "touchstart"] as const;

/** Calls `onIdle` once after `minutes` without input (0 = off). One timer,
 * set for when the time would run out and moved on only when it fires, so
 * input doesn't reset it each time and nothing wakes after it has fired
 * until there's input again. */
export function useIdleLock(minutes: number, onIdle: () => void) {
  const cb = useRef(onIdle);
  cb.current = onIdle;

  useEffect(() => {
    if (!minutes) return;
    const span = minutes * 60_000;
    let last = Date.now();
    let timer: number | undefined;
    const arm = (ms: number) => {
      timer = window.setTimeout(check, ms);
    };
    const check = () => {
      timer = undefined;
      const left = span - (Date.now() - last);
      if (left <= 0) cb.current();
      else arm(left);
    };
    const touch = () => {
      last = Date.now();
      if (timer === undefined) arm(span);
    };
    EVENTS.forEach((e) => window.addEventListener(e, touch, { passive: true }));
    arm(span);
    return () => {
      EVENTS.forEach((e) => window.removeEventListener(e, touch));
      window.clearTimeout(timer);
    };
  }, [minutes]);
}
