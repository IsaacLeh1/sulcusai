// SPDX-License-Identifier: AGPL-3.0-only
// Models page › Speed on this PC: the installed chat models' measured speed
// next to the estimated speed of models that could be installed, with a
// speed test for each installed one (or all of them).
import { useEffect, useRef, useState } from "react";
import { api, errorText, on, type ModelCard, type SpeedStatus } from "../api";
import { speedLabel } from "../format";
import { speedOf } from "../ranking";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

/** Available models shown for comparison, besides every installed one. */
const COMPARE = 6;

export function SpeedChart({ models, onRefresh, toast }: { models: ModelCard[]; onRefresh: () => Promise<void>; toast: PushToast }) {
  // The core keeps the test's status, so leaving the page and coming back
  // (or opening it mid-test) shows where it is.
  const [status, setStatus] = useState<SpeedStatus | null>(null);
  const refresh = useRef(onRefresh);
  refresh.current = onRefresh;
  useEffect(() => {
    api.speedTestStatus().then(setStatus).catch(() => {});
    const sub = on("models:speed", (p) => {
      setStatus({ running: p.running, all: p.all, model_id: p.model_id, done: p.done, total: p.total, errors: p.errors });
      // A model just measured: its bar shows the measured speed now, not at the end.
      if (p.tps !== undefined || p.finished) refresh.current();
    });
    return () => {
      sub.then((u) => u());
    };
  }, []);
  const running = !!status?.running;
  const failed = status?.errors ?? {};

  const installed = models.filter((m) => m.installed);
  const others = models
    .filter((m) => !m.installed && speedOf(m) > 0)
    .sort((a, b) => speedOf(b) - speedOf(a))
    .slice(0, COMPARE);
  const rows = [...installed, ...others].sort((a, b) => speedOf(b) - speedOf(a) || Number(!!b.installed) - Number(!!a.installed));
  const max = Math.max(1, ...rows.map(speedOf));

  const testOne = async (m: ModelCard) => {
    setStatus((s) => ({ running: true, all: false, model_id: m.id, done: 0, total: 1, errors: s?.errors ?? {} }));
    try {
      const sp = await api.measureModelSpeed(m.id);
      toast(t("{model}: writes {speed} tokens/sec on this PC.", { model: m.name, speed: sp.tps.toFixed(1) }), "success");
    } catch (e) {
      toast(errorText(e), "error");
      api.speedTestStatus().then(setStatus).catch(() => {});
    }
  };
  const testAll = async () => {
    setStatus({ running: true, all: true, model_id: null, done: 0, total: installed.length, errors: {} });
    try {
      const n = await api.measureAllSpeeds();
      toast(t("Tested {n} of {total} models.", { n, total: installed.length }), n === installed.length ? "success" : "info");
    } catch (e) {
      toast(errorText(e), "error");
      api.speedTestStatus().then(setStatus).catch(() => {});
    }
  };

  if (installed.length === 0) return null;
  const busyName = status?.running && status.model_id ? models.find((m) => m.id === status.model_id)?.name ?? null : null;
  const pct = status?.running && status.all && status.total > 0 ? Math.max(4, (status.done / status.total) * 100) : null;

  return (
    <section className="card speed-card">
      <div className="row between">
        <div>
          <h2>{t("Speed on this PC")}</h2>
          <p className="muted small">{t("How fast each model writes, measured with the same settings your chats use. Models you could install are shown with their estimated speed.")}</p>
        </div>
        <button className="btn" onClick={testAll} disabled={running}>
          {running && status?.all ? t("Testing {done} of {total}…", { done: Math.min(status.done + 1, status.total), total: status.total }) : t("Test all installed")}
        </button>
      </div>
      {running && (
        <div className="speed-progress" role="status">
          <span className="small">
            {status?.all
              ? busyName
                ? t("Testing {name} · {done} of {total} done", { name: busyName, done: status.done, total: status.total })
                : t("Getting ready… {done} of {total} done", { done: status.done, total: status.total })
              : t("Loading and testing {name}…", { name: busyName ?? t("the model") })}
          </span>
          <div className={`progress ${pct === null ? "indeterminate" : ""}`}>
            <span style={{ width: pct === null ? undefined : `${pct}%` }} />
          </div>
        </div>
      )}
      <div className="speed-rows">
        {rows.map((m) => {
          const measured = m.installed?.tps ?? null;
          const value = speedOf(m);
          const isInstalled = !!m.installed;
          const busy = running && status?.model_id === m.id;
          return (
            <div key={m.id} className={`speed-row ${isInstalled ? "mine" : "other"}`}>
              <div className="speed-name ellipsis" title={m.name}>
                {m.name}
                {!isInstalled && <span className="muted small"> · {t("not installed")}</span>}
              </div>
              <div className="speed-bar" aria-hidden>
                <span className={measured !== null ? "measured" : "estimated"} style={{ width: `${Math.max(2, (value / max) * 100)}%` }} />
              </div>
              <div className="speed-value small">
                {value > 0 ? (
                  <>
                    <strong>{value.toFixed(value < 10 ? 1 : 0)}</strong> {t("tok/s")}
                    <span className="muted"> · {measured !== null ? speedLabel(value) : t("estimated")}</span>
                    {m.installed?.prompt_tps ? <span className="muted block">{t("reads {n} tok/s", { n: Math.round(m.installed.prompt_tps) })}</span> : null}
                  </>
                ) : (
                  <span className="muted">{t("Not measured yet")}</span>
                )}
                {failed[m.id] && <span className="warn-text block" title={failed[m.id]}>{t("The test didn't finish")}</span>}
              </div>
              <div className="speed-action">
                {isInstalled && (
                  <button className="btn ghost small" onClick={() => testOne(m)} disabled={running}>
                    {busy ? t("Testing…") : measured !== null ? t("Test again") : t("Test speed")}
                  </button>
                )}
              </div>
            </div>
          );
        })}
      </div>
      <p className="muted small speed-legend">
        <span className="legend-swatch measured" /> {t("Measured on this PC")} <span className="legend-swatch estimated" /> {t("Estimated from its size and this PC's hardware")}
      </p>
    </section>
  );
}
