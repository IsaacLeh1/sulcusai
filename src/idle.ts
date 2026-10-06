// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef } from "react";

const EVENTS = ["mousemove", "mousedown", "keydown", "wheel", "touchstart"] as const;

/** Calls `onIdle` once after `minutes` without input (0 = off). */
export function useIdleLock(minutes: number, onIdle: () => void) {
  const last = useRef(Date.now());
  const cb = useRef(onIdle);
  cb.current = onIdle;

  useEffect(() => {
    if (!minutes) return;
    last.current = Date.now();
    const touch = () => {
      last.current = Date.now();
    };
    EVENTS.forEach((e) => window.addEventListener(e, touch, { passive: true }));
    const timer = window.setInterval(() => {
      if (Date.now() - last.current >= minutes * 60_000) {
        last.current = Date.now();
        cb.current();
      }
    }, 10_000);
    return () => {
      EVENTS.forEach((e) => window.removeEventListener(e, touch));
      window.clearInterval(timer);
    };
  }, [minutes]);
}
