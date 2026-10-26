// SPDX-License-Identifier: AGPL-3.0-only
// Check this PC: tries each feature in turn and ticks it off, with what
// it's doing, how long each took and the time so far, so it's clear what
// works here and why something doesn't.
import { useEffect, useState } from "react";
import { api, on, type PcCheckReport, type PcCheckStep } from "../api";
import { clock } from "../format";
import { t } from "../i18n";
import { Busy } from "./Busy";

const MARK: Record<PcCheckStep["status"], string> = { waiting: "○", running: "", ok: "✓", warn: "!", failed: "✕", skipped: "–" };

export function PcCheck() {
  const [report, setReport] = useState<PcCheckReport | null>(null);
  useEffect(() => {
    api.pcCheckReport().then(setReport).catch(() => {});
    const sub = on("pccheck:step", setReport);
    return () => {
      sub.then((un) => un());
    };
  }, []);
  const start = () => api.pcCheckStart().then(setReport).catch(() => {});
  const steps = report?.steps ?? [];
  const finished = steps.filter((s) => !["waiting", "running"].includes(s.status)).length;
  const running = steps.find((s) => s.status === "running");
  const failed = steps.filter((s) => s.status === "failed");
  const warned = steps.filter((s) => s.status === "warn");

  return (
    <section className="card pc-check">
      <h2>{t("Check this PC")}</h2>
      <p className="small muted">
        {t("Tries each feature in turn (chat, pictures, songs, narration, voices, speech recognition, the documents library) and says which works and why one doesn't. Nothing new is downloaded, and anything it makes is deleted again. It takes a few minutes.")}
      </p>
      {report?.running ? (
        <Busy
          stage={running ? `${t(running.title)}${running.doing ? `: ${running.doing}` : ""}` : t("Starting")}
          fraction={steps.length ? finished / steps.length : null}
          since={report.started_at ?? undefined}
          onStop={() => api.pcCheckStop().catch(() => {})}
        />
      ) : (
        <div className="row">
          <button className="btn primary" onClick={start}>{report ? t("Check again") : t("Check this PC")}</button>
          {report && (
            <span className="small muted">
              {failed.length === 0 && warned.length === 0
                ? t("Everything it tried works ({time}).", { time: clock(report.took_ms / 1000) })
                : t("{failed} failed, {warn} to look at ({time}).", { failed: failed.length, warn: warned.length, time: clock(report.took_ms / 1000) })}
            </span>
          )}
        </div>
      )}
      {steps.length > 0 && (
        <ol className="pc-check-steps">
          {steps.map((s) => (
            <li key={s.id} className={`pc-check-step ${s.status}`}>
              <span className="pc-check-mark" aria-hidden>{s.status === "running" ? <span className="spinner" /> : MARK[s.status]}</span>
              <div className="grow">
                <div className="row">
                  <strong className="small">{t(s.title)}</strong>
                  <span className="spacer" />
                  {s.ms > 0 && <span className="small muted">{clock(s.ms / 1000)}</span>}
                </div>
                {s.status === "running" && s.doing && <span className="small muted">{s.doing}</span>}
                {s.detail && <span className={`small ${s.status === "failed" ? "warn-text" : "muted"}`}>{s.detail}</span>}
              </div>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}
