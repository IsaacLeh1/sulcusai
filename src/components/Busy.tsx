// SPDX-License-Identifier: AGPL-3.0-only
// Work in progress, shown so nothing looks frozen: a spinner, what it's
// doing now, how far when that's known (a bar), the time so far (the
// ticking clock shows it's alive even with Windows' animations off), about
// how long is left, and where it runs.
import { useEffect, useRef, useState } from "react";
import { api, type WhereItRuns } from "../api";
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

/** Where the chat model runs while `on` (work done with it: a review, a
 * study set, a podcast's script): checked every few seconds. */
export function useModelPlace(on: boolean): string | null {
  const [runs, setRuns] = useState<WhereItRuns | null>(null);
  useEffect(() => {
    if (!on) return;
    let alive = true;
    const load = () => api.gpuMenu().then((m) => alive && setRuns(m.runs)).catch(() => {});
    load();
    const timer = window.setInterval(load, 5000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [on]);
  if (!on || !runs?.place) return null;
  if (runs.place === "card") return runs.device ? t("on the graphics card ({name})", { name: runs.device }) : t("on the graphics card");
  if (runs.place === "partly") return t("partly on the graphics card, the rest on the processor and memory");
  return t("on the processor and memory");
}

/** About how long is left: given, or worked out from how far it got. */
function left(fraction: number | null | undefined, since: number | undefined, now: number, eta: number | null | undefined): number | null {
  if (eta != null && eta >= 0) return eta;
  if (fraction == null || !since || fraction < 0.05 || fraction >= 1) return null;
  const spent = (now - since) / 1000;
  return spent < 3 ? null : spent / fraction - spent;
}

export function Busy({ stage, fraction, since, hint, onStop, eta, where }: {
  /** What it's doing now ("Getting the voice model (once, 2.3 GB)"). */
  stage: string;
  /** How far, 0 to 1, when that's known. */
  fraction?: number | null;
  /** When it started (ms): the time so far is shown. */
  since?: number;
  /** A line under it ("The first time takes longer: it downloads…"). */
  hint?: string;
  onStop?: () => void;
  /** Seconds left, when the work knows better than its fraction. */
  eta?: number | null;
  /** Where it runs ("on the graphics card (RTX 5070)"). */
  where?: string | null;
}) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!since) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [since]);
  const known = fraction != null && fraction >= 0 && fraction <= 1;
  const remaining = left(fraction, since, now, eta);
  return (
    <div className="busy" role="status" aria-live="polite">
      <div className="busy-line">
        <span className="spinner" aria-hidden />
        <span className="busy-stage">{stage}{known ? ` · ${Math.round(fraction! * 100)}%` : ""}</span>
        {since ? (
          <span className="busy-time">
            {t("{time} so far", { time: clock(Math.max(0, now - since) / 1000) })}
            {remaining != null ? ` · ${t("about {time} left", { time: clock(Math.max(1, remaining)) })}` : ""}
          </span>
        ) : null}
        {onStop && <button className="btn ghost small" onClick={onStop}>{t("Stop")}</button>}
      </div>
      <div className={`progress ${known ? "" : "indeterminate"}`}><span style={known ? { width: `${Math.round(fraction! * 100)}%` } : undefined} /></div>
      {where && <span className="small muted">{t("Running {where}", { where })}</span>}
      {hint && <span className="small muted">{hint}</span>}
    </div>
  );
}
