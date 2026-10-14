// SPDX-License-Identifier: AGPL-3.0-only
// Models page › Speed on this PC: the installed chat models' measured speed
// next to the estimated speed of models that could be installed, with a
// speed test for each installed one (or all of them).
import { useEffect, useState } from "react";
import { api, errorText, on, type ModelCard } from "../api";
import { speedLabel } from "../format";
import { speedOf } from "../ranking";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

/** Available models shown for comparison, besides every installed one. */
const COMPARE = 6;

export function SpeedChart({ models, onRefresh, toast }: { models: ModelCard[]; onRefresh: () => Promise<void>; toast: PushToast }) {
  const [testing, setTesting] = useState<{ id: string | null; done: number; total: number; all: boolean } | null>(null);
  const [failed, setFailed] = useState<Record<string, string>>({});

  useEffect(() => {
    const sub = on("models:speed", (p) => {
      if (p.finished) return;
      setTesting((now) => ({ id: p.done < p.total ? p.model_id ?? null : null, done: p.done, total: p.total, all: now?.all ?? true }));
      if (p.error && p.model_id) setFailed((f) => ({ ...f, [p.model_id!]: p.error! }));
    });
    return () => {
      sub.then((u) => u());
    };
  }, []);

  const installed = models.filter((m) => m.installed);
  const others = models
    .filter((m) => !m.installed && speedOf(m) > 0)
    .sort((a, b) => speedOf(b) - speedOf(a))
    .slice(0, COMPARE);
  const rows = [...installed, ...others].sort((a, b) => speedOf(b) - speedOf(a) || Number(!!b.installed) - Number(!!a.installed));
  const max = Math.max(1, ...rows.map(speedOf));

  const testOne = async (m: ModelCard) => {
    setTesting({ id: m.id, done: 0, total: 1, all: false });
    setFailed(({ [m.id]: _, ...rest }) => rest);
    try {
      const sp = await api.measureModelSpeed(m.id);
      toast(t("{model}: writes {speed} tokens/sec on this PC.", { model: m.name, speed: sp.tps.toFixed(1) }), "success");
      await onRefresh();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setTesting(null);
    }
  };
  const testAll = async () => {
    setTesting({ id: null, done: 0, total: installed.length, all: true });
    setFailed({});
    try {
      const n = await api.measureAllSpeeds();
      await onRefresh();
      toast(t("Tested {n} of {total} models.", { n, total: installed.length }), n === installed.length ? "success" : "info");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setTesting(null);
    }
  };

  if (installed.length === 0) return null;
  const busyName = testing?.id ? models.find((m) => m.id === testing.id)?.name : null;

  return (
    <section className="card speed-card">
      <div className="row between">
        <div>
          <h2>{t("Speed on this PC")}</h2>
          <p className="muted small">{t("How fast each model writes, measured with the same settings your chats use. Models you could install are shown with their estimated speed.")}</p>
        </div>
        <button className="btn" onClick={testAll} disabled={!!testing}>
          {testing?.all ? t("Testing {done} of {total}…", { done: Math.min(testing.done + 1, testing.total), total: testing.total }) : t("Test all installed")}
        </button>
      </div>
      {testing && busyName && <p className="small muted">{t("Loading and testing {name}…", { name: busyName })}</p>}
      <div className="speed-rows">
        {rows.map((m) => {
          const measured = m.installed?.tps ?? null;
          const value = speedOf(m);
          const isInstalled = !!m.installed;
          const busy = testing?.id === m.id;
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
                  <button className="btn ghost small" onClick={() => testOne(m)} disabled={!!testing}>
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
