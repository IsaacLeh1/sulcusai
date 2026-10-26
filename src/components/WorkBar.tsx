// SPDX-License-Identifier: AGPL-3.0-only
// The status bar's work: what SulcusAI is busy with, with a progress bar.
// Downloads come first (models of every kind, the AI engine), then the
// pictures, videos, songs and training being made; "No work" when there's
// nothing. Clicking it opens Models (downloads) or the Studio (the rest).
import { useEffect, useRef, useState } from "react";
import { api, on, type InstallProgress, type MediaJob } from "../api";
import { percent } from "../format";
import { t, tx } from "../i18n";
import { JOB_LABEL } from "../media";

// What a download is doing.
const PHASE: Record<InstallProgress["phase"], string> = {
  download: tx("Downloading {name}"),
  verify: tx("Checking {name}"),
  engine: tx("Setting up the engine for {name}"),
  loading: tx("Testing {name}"),
  benchmark: tx("Testing {name}"),
  vision: tx("Downloading the picture reader for {name}"),
  optimizing: tx("Optimizing {name}"),
  waiting: tx("{name} is waiting its turn"),
};

/** One piece of work: what it is, how far along (0 to 1, or null when that isn't known), and where it's shown. */
interface Work {
  key: string;
  label: string;
  fraction: number | null;
  page: "models" | "studio";
}

/** Model names by id, looked up once (chat, picture/video/music and speech models). */
function useNames(ids: string[]) {
  const [names, setNames] = useState<Record<string, string>>({});
  const asked = useRef(false);
  const missing = ids.some((id) => !names[id]);
  useEffect(() => {
    if (!missing || asked.current) return;
    asked.current = true;
    Promise.all([
      api.catalog().then((c) => c.models.map((m) => [m.id, m.name] as const)).catch(() => []),
      api.mediaView(true).then((v) => v.models.map((m) => [m.id, m.name] as const)).catch(() => []),
      api.speechView().then((v) => v.models.map((m) => [m.id, m.name] as const)).catch(() => []),
    ]).then((lists) => {
      setNames(Object.fromEntries(lists.flat()));
      // A model added later is looked up again when it shows.
      setTimeout(() => (asked.current = false), 30_000);
    });
  }, [missing]);
  return names;
}

export function WorkBar({ onOpen }: { onOpen: (page: "models" | "studio") => void }) {
  const [downloads, setDownloads] = useState<Record<string, InstallProgress>>({});
  const [engine, setEngine] = useState<{ received: number; total: number } | null>(null);
  const [jobs, setJobs] = useState<MediaJob[]>([]);
  useEffect(() => {
    api.mediaJobs().then(setJobs).catch(() => {});
    const subs = [
      on("install:progress", (p) => setDownloads((all) => ({ ...all, [p.model_id]: p }))),
      on("install:finished", (f) => setDownloads(({ [f.model_id]: _, ...rest }) => rest)),
      on("engine:download", (p) => setEngine(p.done ? null : { received: p.received ?? 0, total: p.total ?? 0 })),
      on("media:progress", (j) => setJobs((all) => (all.some((x) => x.id === j.id) ? all.map((x) => (x.id === j.id ? j : x)) : [...all, j]))),
      on("media:done", (d) => setJobs((all) => all.filter((x) => x.id !== d.job_id))),
    ];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, []);
  const names = useNames(Object.keys(downloads));

  const work: Work[] = [];
  if (engine) work.push({ key: "engine", label: t("Getting the AI engine"), fraction: engine.total > 0 ? engine.received / engine.total : null, page: "models" });
  for (const [id, p] of Object.entries(downloads)) {
    const name = names[id] ?? id;
    work.push({ key: id, label: t(PHASE[p.phase] ?? "Downloading {name}", { name }), fraction: p.total > 0 && p.phase !== "loading" && p.phase !== "benchmark" ? p.received / p.total : null, page: "models" });
  }
  for (const j of jobs) {
    const what = t(JOB_LABEL[j.op] ?? "Working");
    work.push({ key: j.id, label: j.status === "queued" ? t("{what} (waiting)", { what }) : j.stage ? `${what} · ${j.stage}` : what, fraction: j.status === "queued" ? 0 : j.fraction, page: "studio" });
  }

  if (!work.length) {
    return (
      <span className="status-item work-bar idle" title={t("Nothing is downloading or being made")}>
        {t("No work")}
      </span>
    );
  }
  const first = work[0];
  const pct = first.fraction === null ? null : Math.round(percent(first.fraction, 1));
  return (
    <button className="status-item link-btn work-bar" onClick={() => onOpen(first.page)} title={work.map((w) => (w.fraction === null ? w.label : `${w.label} · ${Math.round(percent(w.fraction, 1))}%`)).join("\n")}>
      <span className="ellipsis work-label">{first.label}</span>
      <span className={`progress work-progress ${pct === null ? "indeterminate" : ""}`} role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct ?? undefined}>
        <span style={{ width: pct === null ? undefined : `${pct}%` }} />
      </span>
      {pct !== null && <span className="mono">{pct}%</span>}
      {work.length > 1 && <span className="muted">{t("+{n} more", { n: work.length - 1 })}</span>}
    </button>
  );
}
