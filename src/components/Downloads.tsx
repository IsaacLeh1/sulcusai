// SPDX-License-Identifier: AGPL-3.0-only
// Models page › Downloads: the model being downloaded (speed, time left,
// pause, cancel) and paused ones (resume, remove). Pausing keeps what's
// downloaded; resuming carries on from there.
import { useEffect, useRef, useState } from "react";
import { api, errorText, on, type InstallProgress, type ModelCard, type PausedDownload } from "../api";
import { bytes, percent } from "../format";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";
import { TESTING, TestingNote, useTestStart } from "./Testing";

const PHASE: Record<string, string> = {
  engine: "Setting up the engine",
  verify: "Checking the downloaded file",
  download: "Downloading",
  loading: "Loading the model to test it",
  benchmark: "Testing speed on this PC",
  vision: "Downloading the picture reader",
};

/** Bytes a second, from the last few progress reports. */
function useRate(id: string, p: InstallProgress): number | null {
  const samples = useRef<{ at: number; received: number }[]>([]);
  const [rate, setRate] = useState<number | null>(null);
  useEffect(() => {
    samples.current = [];
    setRate(null);
  }, [id, p.phase]);
  useEffect(() => {
    const now = Date.now();
    const s = samples.current;
    s.push({ at: now, received: p.received });
    while (s.length > 2 && now - s[0].at > 8000) s.shift();
    const first = s[0];
    if (s.length > 1 && now - first.at > 900) setRate(Math.max(0, (p.received - first.received) / ((now - first.at) / 1000)));
  }, [p.received]);
  return rate;
}

function duration(seconds: number): string {
  if (seconds < 60) return t("{n} sec left", { n: Math.max(1, Math.round(seconds)) });
  if (seconds < 3600) return t("{n} min left", { n: Math.round(seconds / 60) });
  return t("{h} h {m} min left", { h: Math.floor(seconds / 3600), m: Math.round((seconds % 3600) / 60) });
}

function Active({ id, p, name }: { id: string; p: InstallProgress; name: string }) {
  const rate = useRate(id, p);
  const pct = p.total > 0 ? percent(p.received, p.total) : null;
  const pausable = p.phase === "download" || p.phase === "vision" || p.phase === "engine";
  const left = rate && rate > 0 && p.total > p.received ? (p.total - p.received) / rate : null;
  const testing = TESTING.has(p.phase);
  const since = useTestStart(testing);
  return (
    <div className="dl-row">
      <div className="dl-what">
        <strong className="ellipsis">{name}</strong>
        <span className="muted small">
          {t(PHASE[p.phase] ?? "Working")}
          {pct !== null && ` · ${t("{received} of {total}", { received: bytes(p.received), total: bytes(p.total) })}`}
          {rate !== null && !testing && ` · ${bytes(rate)}/s`}
          {left !== null && ` · ${duration(left)}`}
        </span>
        <div className={`progress ${pct === null ? "indeterminate" : ""}`}>
          <span style={{ width: pct === null ? undefined : `${pct}%` }} />
        </div>
        {testing && <TestingNote since={since} />}
      </div>
      <div className="dl-actions">
        {pausable && <button className="btn small" onClick={() => api.pauseInstall(id)}>{t("Pause")}</button>}
        {!testing && <button className="btn ghost small" onClick={() => api.cancelInstall(id)}>{t("Cancel")}</button>}
      </div>
    </div>
  );
}

export function Downloads({ progress, models, onRefresh, toast }: { progress: Record<string, InstallProgress>; models: ModelCard[]; onRefresh: () => Promise<void>; toast: PushToast }) {
  const [paused, setPaused] = useState<PausedDownload[]>([]);
  useEffect(() => {
    const load = () => api.downloadsView().then((v) => setPaused(v.paused)).catch(() => {});
    load();
    const subs = [on("downloads:changed", load), on("install:finished", load)];
    return () => {
      subs.forEach((s) => s.then((un) => un()));
    };
  }, []);
  const active = Object.entries(progress);
  const busy = active.length > 0;
  const nameOf = (id: string, fallback?: string) => models.find((m) => m.id === id)?.name ?? fallback ?? id;
  if (active.length === 0 && paused.length === 0) return null;
  return (
    <section className="card downloads">
      <h2>{t("Downloads")}</h2>
      {active.map(([id, p]) => (
        <Active key={id} id={id} p={p} name={nameOf(id)} />
      ))}
      {paused.map((d) => (
        <div key={d.model_id} className="dl-row">
          <div className="dl-what">
            <strong className="ellipsis">{nameOf(d.model_id, d.name)}</strong>
            <span className="muted small">
              {t("Paused")}
              {d.total > 0 && ` · ${t("{received} of {total}", { received: bytes(d.received), total: bytes(d.total) })}`}
            </span>
            <div className="progress paused">
              <span style={{ width: d.total > 0 ? `${percent(d.received, d.total)}%` : "0%" }} />
            </div>
          </div>
          <div className="dl-actions">
            <button
              className="btn small"
              disabled={busy}
              title={busy ? t("One install at a time") : undefined}
              onClick={async () => {
                try {
                  await api.resumeInstall(d.model_id);
                  await onRefresh();
                } catch (e) {
                  toast(errorText(e), "error");
                }
              }}
            >
              {t("Resume")}
            </button>
            <button className="btn ghost small" onClick={() => api.cancelInstall(d.model_id)} title={t("Deletes what was downloaded")}>
              {t("Remove")}
            </button>
          </div>
        </div>
      ))}
    </section>
  );
}

/** Status bar: the download in progress; opens the Models page. */
export function DownloadsPill({ progress, models, onOpen }: { progress: Record<string, InstallProgress>; models: ModelCard[]; onOpen: () => void }) {
  const first = Object.entries(progress)[0];
  if (!first) return null;
  const [id, p] = first;
  const name = models.find((m) => m.id === id)?.name ?? id;
  const pct = p.total > 0 ? percent(p.received, p.total) : null;
  return (
    <button className="status-item link-btn" onClick={onOpen} title={t("Downloads")}>
      ⬇ {name}
      {pct !== null ? ` · ${Math.round(pct)}%` : "…"}
    </button>
  );
}
