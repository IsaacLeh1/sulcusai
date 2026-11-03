// SPDX-License-Identifier: AGPL-3.0-only
// A gear that opens the choice of model for one feature (Mail, Calendar, the
// Assistant): installed models that can do the job, the ones that can't and
// why, advice and a download when none can, and a short test of the work.
// (ModelPicker.tsx is the status bar's default-model menu.)
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { errorText, on } from "../api";
import { bytes } from "../format";
import { t } from "../i18n";
import { GearIcon } from "./Icons";
import { Modal } from "./Modal";

export type ModelFeature = "mail" | "calendar" | "assistant";

export interface FitVerdict {
  passed: number;
  total: number;
  ok: boolean;
}

export interface FitCandidate {
  model_id: string;
  name: string;
  params_b: number;
  /** Why the model can't do the job; empty when it can. */
  refused: string[];
  tested: FitVerdict | null;
}

export interface FitAdvice {
  model_id: string;
  name: string;
  params_b: number;
  quant: string;
  size: number;
  est_tps: number;
}

export interface FitPickerView {
  feature: ModelFeature;
  minimum_params_b: number;
  minimum_ctx: number;
  current: string | null;
  installed: FitCandidate[];
  advice: FitAdvice[];
}

export interface FitReport {
  verdict: FitVerdict;
  results: { id: string; passed: boolean; problem: string | null }[];
}

const featureName = (f: ModelFeature) => ({ mail: t("Mail"), calendar: t("Calendar"), assistant: t("the Assistant") })[f];

const fitApi = {
  view: (feature: ModelFeature) => invoke<FitPickerView>("model_fit_view", { feature }),
  choose: (feature: ModelFeature, modelId: string | null) => invoke<void>("set_feature_model", { feature, modelId }),
  test: (feature: ModelFeature, modelId: string) => invoke<FitReport>("model_fit_test", { feature, modelId }),
  download: (modelId: string, quant: string) => invoke<void>("install_model", { modelId, quant, force: false }),
};

export function verdictText(v: FitVerdict): string {
  return v.ok ? t("Passed the test ({passed} of {total})", { passed: v.passed, total: v.total }) : t("Failed the test ({passed} of {total})", { passed: v.passed, total: v.total });
}

/** The picker's content, for a view the core has worked out. */
export function FeatureModelPanel({ view, testing, report, reportFor, downloading, onChoose, onTest, onDownload }: {
  view: FitPickerView;
  /** The model being tested now. */
  testing: string | null;
  report: FitReport | null;
  reportFor: string | null;
  downloading: string[];
  onChoose: (modelId: string | null) => void;
  onTest: (modelId: string) => void;
  onDownload: (a: FitAdvice) => void;
}) {
  const allowed = view.installed.filter((m) => m.refused.length === 0);
  const name = featureName(view.feature);
  return (
    <div className="feature-model">
      <p className="small muted">
        {t("The model {feature} uses must have at least {size}B parameters, call tools and keep {ctx}K of the conversation in mind.", { feature: name, size: view.minimum_params_b, ctx: Math.round(view.minimum_ctx / 1024) })}
      </p>
      <div role="radiogroup" aria-label={t("Model for {feature}", { feature: name })}>
        <div className="row">
          <button role="radio" aria-checked={view.current === null} className={`btn small grow${view.current === null ? " primary" : ""}`} onClick={() => onChoose(null)}>
            {t("The default model")}
          </button>
        </div>
        {view.installed.map((m) => {
          const refused = m.refused.length > 0;
          return (
            <div key={m.model_id} className="feature-model-row">
              <div className="row">
                <button
                  role="radio"
                  aria-checked={view.current === m.model_id}
                  className={`btn small grow${view.current === m.model_id ? " primary" : ""}`}
                  disabled={refused}
                  onClick={() => onChoose(m.model_id)}
                >
                  {m.name}
                  {m.params_b > 0 && <span className="muted small"> {m.params_b}B</span>}
                </button>
                {!refused && (
                  <button className="btn small" disabled={testing !== null} onClick={() => onTest(m.model_id)}>
                    {testing === m.model_id ? t("Testing…") : t("Test it")}
                  </button>
                )}
              </div>
              {refused && m.refused.map((why) => <p key={why} className="small muted feature-model-why">{why}</p>)}
              {m.tested && <p className={`small feature-model-test ${m.tested.ok ? "ok" : "warn"}`}>{verdictText(m.tested)}</p>}
              {report && reportFor === m.model_id && (
                <ul className="small feature-model-results">
                  {report.results.map((r) => (
                    <li key={r.id}>{r.passed ? t("Answered well") : r.problem}</li>
                  ))}
                </ul>
              )}
            </div>
          );
        })}
      </div>
      {allowed.length === 0 && (
        <div className="feature-model-advice">
          <p className="small">{view.installed.length === 0 ? t("No model is installed yet.") : t("None of your installed models can do this job.")} {view.advice.length > 0 ? t("These can, and fit this PC's graphics card:") : t("No model in the catalog both does this job and fits this PC.")}</p>
          {view.advice.map((a) => {
            const busy = downloading.includes(a.model_id);
            return (
              <div key={a.model_id} className="row">
                <span className="grow">
                  {a.name} <span className="muted small">{t("{size}B, {download} download", { size: a.params_b, download: bytes(a.size) })}</span>
                </span>
                <button className="btn small primary" disabled={busy} onClick={() => onDownload(a)}>
                  {busy ? t("Downloading…") : t("Download")}
                </button>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

export function FeatureModelPicker({ feature }: { feature: ModelFeature }) {
  const [open, setOpen] = useState(false);
  const [view, setView] = useState<FitPickerView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [testing, setTesting] = useState<string | null>(null);
  const [report, setReport] = useState<{ modelId: string; report: FitReport } | null>(null);
  const [downloading, setDownloading] = useState<string[]>([]);

  const load = useCallback(() => fitApi.view(feature).then(setView).catch((e) => setError(errorText(e))), [feature]);

  useEffect(() => {
    if (!open) return;
    void load();
    // A finished download is a model to choose from.
    const done = on("install:finished", (e) => {
      setDownloading((d) => d.filter((id) => id !== e.model_id));
      void load();
    });
    return () => {
      void done.then((off) => off());
    };
  }, [open, load]);

  const run = (job: Promise<unknown>) => job.then(load).catch((e) => setError(errorText(e)));

  return (
    <>
      <button className="btn ghost small" aria-label={t("Choose the model for {feature}", { feature: featureName(feature) })} title={t("Choose the model for {feature}", { feature: featureName(feature) })} onClick={() => setOpen(true)}>
        <GearIcon />
      </button>
      {open && (
        <Modal title={t("Model for {feature}", { feature: featureName(feature) })} onClose={() => setOpen(false)}>
          {error && <p className="small error" role="alert">{error}</p>}
          {view ? (
            <FeatureModelPanel
              view={view}
              testing={testing}
              report={report?.report ?? null}
              reportFor={report?.modelId ?? null}
              downloading={downloading}
              onChoose={(id) => {
                setError(null);
                void run(fitApi.choose(feature, id));
              }}
              onTest={(id) => {
                setError(null);
                setTesting(id);
                fitApi
                  .test(feature, id)
                  .then((r) => setReport({ modelId: id, report: r }))
                  .then(load)
                  .catch((e) => setError(errorText(e)))
                  .finally(() => setTesting(null));
              }}
              onDownload={(a) => {
                setError(null);
                setDownloading((d) => [...d, a.model_id]);
                fitApi.download(a.model_id, a.quant).catch((e) => {
                  setDownloading((d) => d.filter((id) => id !== a.model_id));
                  setError(errorText(e));
                });
              }}
            />
          ) : (
            !error && <p className="small muted">{t("Looking at your models…")}</p>
          )}
          <div className="modal-actions">
            <button className="btn" onClick={() => setOpen(false)}>{t("Close")}</button>
          </div>
        </Modal>
      )}
    </>
  );
}
