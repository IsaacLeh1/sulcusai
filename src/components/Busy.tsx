// SPDX-License-Identifier: AGPL-3.0-only
// Work in progress, shown so nothing looks frozen: a spinner, what it's
// doing now, how far when that's known (a bar), and the time so far (the
// ticking clock shows it's alive even with Windows' animations off).
import { useEffect, useRef, useState } from "react";
import { clock } from "../format";
import { t } from "../i18n";

/** When work started: resets each time `on` turns on. */
export function useSince(on: boolean): number {
  const start = useRef(Date.now());
  const was = useRef(false);
  if (on && !was.current) start.current = Date.now();
  was.current = on;
  return start.current;
}

export function Busy({ stage, fraction, since, hint, onStop }: {
  /** What it's doing now ("Getting the voice model (once, 2.3 GB)"). */
  stage: string;
  /** How far, 0 to 1, when that's known. */
  fraction?: number | null;
  /** When it started (ms): the time so far is shown. */
  since?: number;
  /** A line under it ("The first time takes longer: it downloads…"). */
  hint?: string;
  onStop?: () => void;
}) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!since) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [since]);
  const known = fraction != null && fraction >= 0 && fraction <= 1;
  return (
    <div className="busy" role="status" aria-live="polite">
      <div className="busy-line">
        <span className="spinner" aria-hidden />
        <span className="busy-stage">{stage}{known ? ` · ${Math.round(fraction! * 100)}%` : ""}</span>
        {since ? <span className="busy-time">{t("{time} so far", { time: clock(Math.max(0, now - since) / 1000) })}</span> : null}
        {onStop && <button className="btn ghost small" onClick={onStop}>{t("Stop")}</button>}
      </div>
      <div className={`progress ${known ? "" : "indeterminate"}`}><span style={known ? { width: `${Math.round(fraction! * 100)}%` } : undefined} /></div>
      {hint && <span className="small muted">{hint}</span>}
    </div>
  );
}
